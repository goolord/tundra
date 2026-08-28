use lofty::file::TaggedFileExt;
use lofty::ogg::tag::VorbisComments;
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag};
use std::path::Path;

use crate::types::is_audio;

use super::fields::{ManualTagEdits, TagFields};
use super::hints::artist_hint_from_path;

pub(crate) fn push_field(value: &mut String, source: Option<impl AsRef<str>>) {
    if value.is_empty()
        && let Some(text) = source
            .as_ref()
            .map(AsRef::as_ref)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    {
        *value = text.to_owned();
    }
}

pub fn file_mtime_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

pub(crate) fn push_instrument_field(fields: &mut TagFields, tag: &Tag) {
    if let Some(value) = explicit_instrument_from_tag(tag) {
        push_field(&mut fields.explicit_instrument, Some(&value));
        push_field(&mut fields.instrument, Some(&value));
    }

    if fields.instrument.is_empty() {
        push_field(&mut fields.instrument, tag.get_string(ItemKey::ContentGroup));
        push_field(&mut fields.instrument, tag.get_string(ItemKey::Description));
    }
}

const LEGACY_TUNDRA_AUTO_TAG_COMMENT: &str = "Automatically tagged by Tundra";
/// Bump when auto-tag field layout or semantics change so older tags can upgrade.
pub const TUNDRA_TAG_VERSION: u32 = 1;
// Canonical tag keys per container. RIFF INFO has no instrument chunk, so WAV
// uses IKEY (keywords), the only free-form field taggers reliably surface.
pub(crate) const WAV_INSTRUMENT_KEY: &str = "IKEY";
pub(crate) const WAV_ARTIST_KEY: &str = "IART";
pub(crate) const WAV_COMMENT_KEY: &str = "ICMT";
pub(crate) const WAV_TITLE_KEY: &str = "INAM";
pub(crate) const WAV_GENRE_KEY: &str = "IGNR";
// Vorbis comments (FLAC, OGG) and ID3v2 user text (MP3) both name the field
// INSTRUMENT, which is what Mp3tag and similar taggers display.
pub(crate) const VORBIS_INSTRUMENT_KEY: &str = "INSTRUMENT";
pub(crate) const VORBIS_ARTIST_KEY: &str = "ARTIST";
pub(crate) const VORBIS_COMMENT_KEY: &str = "COMMENT";
const ID3_INSTRUMENT_KEY: &str = "INSTRUMENT";

pub(crate) fn instrument_from_marked_comment(comment: &str) -> Option<String> {
    for line in comment.lines() {
        let line = line.trim();
        for prefix in ["INSTRUMENT:", "INSTRUMENT="] {
            if let Some(rest) = line.strip_prefix(prefix) {
                let trimmed = rest.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

fn tundra_comment_marker() -> String {
    format!("Tundra v{TUNDRA_TAG_VERSION}")
}

/// Version recorded in a Tundra marker comment, if any.
pub(crate) fn parse_tundra_comment_version(comment: &str) -> Option<u32> {
    for line in comment.lines() {
        let line = line.trim();
        if line.eq_ignore_ascii_case("Tundra") {
            return Some(0);
        }
        if line.eq_ignore_ascii_case(LEGACY_TUNDRA_AUTO_TAG_COMMENT) {
            return Some(0);
        }
        let lower = line.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("tundra v") {
            if let Ok(version) = rest.trim().parse::<u32>() {
                return Some(version);
            }
        }
    }
    None
}

pub(crate) fn file_tundra_tag_version(path: &Path, comment: &str, native_instrument: &str) -> Option<u32> {
    parse_tundra_comment_version(comment).or_else(|| {
        native_instrument
            .trim()
            .is_empty()
            .then(|| crate::tag_store::tag_version(path))
            .flatten()
    })
}

#[cfg(test)]
pub fn tundra_tag_is_current(path: &Path, comment: &str) -> bool {
    file_tundra_tag_version(path, comment, "") == Some(TUNDRA_TAG_VERSION)
}

fn tundra_owns_tags(comment: &str) -> bool {
    parse_tundra_comment_version(comment).is_some()
        || instrument_from_marked_comment(comment).is_some()
}

/// Tundra may replace tags it wrote; sidecar alone does not own native tags.
pub fn tundra_tagged_file(path: &Path, comment: &str, native_instrument: &str) -> bool {
    tundra_owns_tags(comment)
        || (native_instrument.trim().is_empty() && crate::tag_store::instrument(path).is_some())
}

/// Containers Tundra tags natively. Each maps the instrument label to the one
/// key third-party taggers read back, so a write is always round-trippable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Container {
    Wav,
    Flac,
    Ogg,
    Mp3,
    Aiff,
}

impl Container {
    pub(crate) fn of(path: &Path) -> Option<Self> {
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            if let Some(container) = Self::from_extension(ext) {
                return Some(container);
            }
        }
        Self::from_staged_name(path).or_else(|| Self::sniff(path))
    }

    fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "wav" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "ogg" => Some(Self::Ogg),
            "mp3" => Some(Self::Mp3),
            "aiff" | "aif" => Some(Self::Aiff),
            _ => None,
        }
    }

    fn from_staged_name(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?;
        for marker in [".tundra-tag-", ".tundra-replace-"] {
            if let Some(original) = name.split(marker).next() {
                if let Some(container) = Path::new(original)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(Self::from_extension)
                {
                    return Some(container);
                }
            }
        }
        None
    }

    fn sniff(path: &Path) -> Option<Self> {
        use std::io::Read;

        let mut header = [0u8; 12];
        let mut file = std::fs::File::open(path).ok()?;
        file.read_exact(&mut header).ok()?;
        if header.starts_with(b"RIFF") && header[8..12] == *b"WAVE" {
            return Some(Self::Wav);
        }
        if header.starts_with(b"fLaC") {
            return Some(Self::Flac);
        }
        if header.starts_with(b"OggS") {
            return Some(Self::Ogg);
        }
        if header.starts_with(b"ID3") || header.starts_with(&[0xFF, 0xFB]) {
            return Some(Self::Mp3);
        }
        if header.starts_with(b"FORM") && header.get(8..12) == Some(b"AIFF") {
            return Some(Self::Aiff);
        }
        None
    }
}

/// The fields the auto-tagger reads and writes. `None` means "absent" on read
/// and "leave alone" on write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NativeTags {
    pub instrument: Option<String>,
    pub artist: Option<String>,
    pub comment: Option<String>,
}

impl NativeTags {
    pub(crate) fn is_empty(&self) -> bool {
        self.instrument.is_none() && self.artist.is_none() && self.comment.is_none()
    }
}

/// Skip audio properties and cover art on tag-only reads.
pub(crate) fn tag_parse_options() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false)
}

/// Read cover art back in before write so saves do not strip it.
pub(crate) fn write_parse_options() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new().read_properties(false)
}

/// Generic tag view for fields lofty maps consistently (title, album, genre, bpm, key).
fn probe_tags(path: &Path) -> Option<lofty::file::TaggedFile> {
    lofty::probe::Probe::open(path)
        .ok()?
        .options(tag_parse_options())
        .read()
        .ok()
}

pub(crate) fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// `RiffInfoList::get` compares fourccs case-sensitively even though `insert`
/// does not, so a list written with non-canonical case needs a second look.
fn riff_get(info: &lofty::iff::wav::RiffInfoList, key: &str) -> Option<String> {
    if let Some(value) = non_empty(info.get(key)) {
        return Some(value);
    }
    info.clone()
        .into_iter()
        .find(|(found, _)| found.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| non_empty(Some(&value)))
}

fn apply_aiff_text_tags(text: &mut lofty::iff::aiff::AiffTextChunks, tags: &NativeTags) {
    use lofty::tag::Accessor;
    if let Some(artist) = &tags.artist {
        text.set_artist(artist.clone());
    }
    if let Some(comment) = &tags.comment {
        text.set_comment(comment.clone());
    }
}

fn vorbis_native_tags(vorbis: Option<&VorbisComments>) -> NativeTags {
    NativeTags {
        instrument: vorbis.and_then(|tag| non_empty(tag.get(VORBIS_INSTRUMENT_KEY))),
        artist: vorbis.and_then(|tag| non_empty(tag.get(VORBIS_ARTIST_KEY))),
        comment: vorbis.and_then(|tag| non_empty(tag.get(VORBIS_COMMENT_KEY))),
    }
}

pub(crate) fn apply_vorbis_tags(vorbis: &mut VorbisComments, tags: &NativeTags) {
    for (key, value) in [
        (VORBIS_INSTRUMENT_KEY, &tags.instrument),
        (VORBIS_ARTIST_KEY, &tags.artist),
        (VORBIS_COMMENT_KEY, &tags.comment),
    ] {
        if let Some(value) = value {
            let _removed: Vec<_> = vorbis.remove(key).collect();
            vorbis.insert(key.to_string(), value.clone());
        }
    }
}

/// Canonical native keys plus generic tags from one parse.
pub(crate) struct FileTags {
    native: NativeTags,
    generic: Vec<Tag>,
}

fn collect(tags: [Option<Tag>; 2]) -> Vec<Tag> {
    tags.into_iter().flatten().collect()
}

fn read_container_tags(path: &Path) -> Option<FileTags> {
    use lofty::file::AudioFile;
    use lofty::tag::Accessor;

    let container = Container::of(path)?;
    let mut file = std::fs::File::open(path).ok()?;
    let options = tag_parse_options();

    Some(match container {
        Container::Wav => {
            let mut wav = lofty::iff::wav::WavFile::read_from(&mut file, options).ok()?;
            let info = wav.remove_riff_info();
            let id3 = wav.remove_id3v2();
            FileTags {
                native: NativeTags {
                    instrument: info.as_ref().and_then(|list| riff_get(list, WAV_INSTRUMENT_KEY)),
                    artist: info.as_ref().and_then(|list| riff_get(list, WAV_ARTIST_KEY)),
                    comment: info.as_ref().and_then(|list| riff_get(list, WAV_COMMENT_KEY)),
                },
                generic: collect([info.map(Tag::from), id3.map(Tag::from)]),
            }
        }
        Container::Flac => {
            let mut flac = lofty::flac::FlacFile::read_from(&mut file, options).ok()?;
            let vorbis = flac.remove_vorbis_comments();
            let id3 = flac.remove_id3v2();
            FileTags {
                native: vorbis_native_tags(vorbis.as_ref()),
                generic: collect([vorbis.map(Tag::from), id3.map(Tag::from)]),
            }
        }
        Container::Ogg => {
            let mut ogg = lofty::ogg::VorbisFile::read_from(&mut file, options).ok()?;
            let vorbis = std::mem::take(ogg.vorbis_comments_mut());
            FileTags {
                native: vorbis_native_tags(Some(&vorbis)),
                generic: collect([Some(Tag::from(vorbis)), None]),
            }
        }
        Container::Mp3 => {
            let mut mp3 = lofty::mpeg::MpegFile::read_from(&mut file, options).ok()?;
            let id3v2 = mp3.remove_id3v2();
            let other = mp3
                .remove_id3v1()
                .map(Tag::from)
                .or_else(|| mp3.remove_ape().map(Tag::from));
            FileTags {
                native: NativeTags {
                    instrument: id3v2
                        .as_ref()
                        .and_then(|tag| non_empty(tag.get_user_text(ID3_INSTRUMENT_KEY))),
                    artist: id3v2.as_ref().and_then(|tag| non_empty(tag.artist().as_deref())),
                    comment: id3v2
                        .as_ref()
                        .and_then(|tag| non_empty(tag.comment().as_deref())),
                },
                generic: collect([id3v2.map(Tag::from), other]),
            }
        }
        Container::Aiff => {
            use lofty::tag::Accessor;

            let mut aiff = lofty::iff::aiff::AiffFile::read_from(&mut file, options).ok()?;
            let text = aiff.remove_text_chunks();
            let id3 = aiff.remove_id3v2();
            let empty_text = lofty::iff::aiff::AiffTextChunks::default();
            let text_ref = text.as_ref().unwrap_or(&empty_text);
            FileTags {
                native: NativeTags {
                    instrument: id3
                        .as_ref()
                        .and_then(|tag| non_empty(tag.get_user_text(ID3_INSTRUMENT_KEY)))
                        .or_else(|| {
                            text_ref.annotations.as_ref().and_then(|lines| {
                                lines
                                    .iter()
                                    .find_map(|line| instrument_from_marked_comment(line))
                            })
                        })
                        .or_else(|| {
                            text_ref
                                .comment()
                                .and_then(|comment| instrument_from_marked_comment(&comment))
                        }),
                    artist: non_empty(text_ref.author.as_deref()).or_else(|| {
                        id3.as_ref()
                            .and_then(|tag| non_empty(tag.artist().as_deref()))
                    }),
                    comment: text_ref
                        .comment()
                        .map(|comment| comment.to_string())
                        .or_else(|| id3.as_ref().and_then(|tag| non_empty(tag.comment().as_deref()))),
                },
                generic: collect([text.map(Tag::from), id3.map(Tag::from)]),
            }
        }
    })
}

/// Fallback when extension and container disagree; instrument may live in the sidecar.
pub(crate) fn read_file_tags(path: &Path) -> Option<FileTags> {
    if let Some(tags) = read_container_tags(path) {
        return Some(tags);
    }
    let tagged = probe_tags(path)?;
    Some(FileTags {
        native: NativeTags::default(),
        generic: tagged.tags().to_vec(),
    })
}

/// Canonical instrument/artist/comment keys for the container.
pub(crate) fn read_native_tags(path: &Path) -> Option<NativeTags> {
    read_container_tags(path).map(|tags| tags.native)
}

/// Writes the populated fields of `tags` to the container's canonical keys.
pub(crate) fn write_native_tags(path: &Path, tags: &NativeTags) -> Result<(), String> {
    if tags.is_empty() {
        return Ok(());
    }
    stage_and_replace(path, |staged| apply_native_tags_staged(staged, tags))
}

fn apply_id3_native_tags(id3: &mut lofty::id3::v2::Id3v2Tag, tags: &NativeTags) {
    use lofty::tag::Accessor;
    if let Some(instrument) = &tags.instrument {
        id3.insert_user_text(ID3_INSTRUMENT_KEY.to_string(), instrument.clone());
    }
    if let Some(artist) = &tags.artist {
        id3.set_artist(artist.clone());
    }
    if let Some(comment) = &tags.comment {
        id3.set_comment(comment.clone());
    }
}

pub(crate) fn apply_native_tags_staged(staged: &Path, tags: &NativeTags) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;

    let container = Container::of(staged)
        .ok_or_else(|| format!("Unsupported file type: {}", staged.display()))?;
    let options = write_parse_options();
    let read_error =
        |err: lofty::error::FileParseError| crate::path_util::path_io_error("read", staged, err);
    let write_error = |err: lofty::error::FileEncodingError| {
        crate::path_util::path_io_error("write tags to", staged, err)
    };
    let open = || crate::path_util::open_file(staged);

    match container {
        Container::Wav => write_wav_info_preserving_chunks(staged, Some(tags), None),
        Container::Flac => {
            let mut flac = {
                let mut file = open()?;
                lofty::flac::FlacFile::read_from(&mut file, options).map_err(read_error)?
            };
            let mut vorbis = flac.remove_vorbis_comments().unwrap_or_default();
            apply_vorbis_tags(&mut vorbis, tags);
            flac.set_vorbis_comments(vorbis);
            flac.save_to_path(staged, WriteOptions::default())
                .map_err(write_error)
        }
        Container::Ogg => {
            let mut ogg = {
                let mut file = open()?;
                lofty::ogg::VorbisFile::read_from(&mut file, options).map_err(read_error)?
            };
            apply_vorbis_tags(ogg.vorbis_comments_mut(), tags);
            ogg.save_to_path(staged, WriteOptions::default())
                .map_err(write_error)
        }
        Container::Mp3 => {
            let mut mp3 = {
                let mut file = open()?;
                lofty::mpeg::MpegFile::read_from(&mut file, options).map_err(read_error)?
            };
            let mut id3 = mp3.remove_id3v2().unwrap_or_default();
            apply_id3_native_tags(&mut id3, tags);
            mp3.set_id3v2(id3);
            mp3.save_to_path(staged, WriteOptions::default())
                .map_err(write_error)
        }
        Container::Aiff => {
            let mut aiff = {
                let mut file = open()?;
                lofty::iff::aiff::AiffFile::read_from(&mut file, options).map_err(read_error)?
            };
            let mut text = aiff.remove_text_chunks().unwrap_or_default();
            apply_aiff_text_tags(&mut text, tags);
            aiff.set_text_chunks(text);
            let mut id3 = aiff.remove_id3v2().unwrap_or_default();
            apply_id3_native_tags(&mut id3, tags);
            aiff.set_id3v2(id3);
            aiff.save_to_path(staged, WriteOptions::default())
                .map_err(write_error)
        }
    }
}

/// Rewrite only the LIST INFO chunk so `smpl` / `cue ` / `inst` / ACID survive.
pub(crate) fn write_wav_info_preserving_chunks(
    path: &Path,
    native: Option<&NativeTags>,
    generic: Option<&ManualTagEdits>,
) -> Result<(), String> {
    if native.is_none() && generic.is_none() {
        return Ok(());
    }
    let bytes = std::fs::read(path)
        .map_err(|err| crate::path_util::path_io_error("read", path, err))?;
    let mut chunks = parse_riff_wave_chunks(&bytes)?;
    upsert_wav_list_info_chunk(&mut chunks, native, generic);
    let encoded = encode_riff_wave(&chunks);
    std::fs::write(path, encoded)
        .map_err(|err| crate::path_util::path_io_error("write tags to", path, err))
}

pub(crate) fn parse_riff_wave_chunks(bytes: &[u8]) -> Result<Vec<([u8; 4], Vec<u8>)>, String> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Not a RIFF WAVE file".into());
    }
    let mut offset = 12usize;
    let mut chunks = Vec::new();
    while offset + 8 <= bytes.len() {
        let id: [u8; 4] = bytes[offset..offset + 4]
            .try_into()
            .map_err(|_| "Invalid WAV chunk id".to_string())?;
        let size = u32::from_le_bytes(
            bytes[offset + 4..offset + 8]
                .try_into()
                .map_err(|_| "Invalid WAV chunk size".to_string())?,
        ) as usize;
        offset += 8;
        if offset + size > bytes.len() {
            return Err("Truncated WAV chunk".into());
        }
        chunks.push((id, bytes[offset..offset + size].to_vec()));
        offset += size;
        if size % 2 == 1 {
            offset += 1;
        }
    }
    Ok(chunks)
}

fn upsert_wav_list_info_chunk(
    chunks: &mut Vec<([u8; 4], Vec<u8>)>,
    native: Option<&NativeTags>,
    generic: Option<&ManualTagEdits>,
) {
    let mut fields = match chunks.iter().find(|(id, data)| {
        id == b"LIST" && data.len() >= 4 && &data[..4] == b"INFO"
    }) {
        Some((_, data)) => parse_info_fields(&data[4..]),
        None => Vec::new(),
    };
    if let Some(tags) = native {
        for (key, value) in [
            (WAV_INSTRUMENT_KEY, &tags.instrument),
            (WAV_ARTIST_KEY, &tags.artist),
            (WAV_COMMENT_KEY, &tags.comment),
        ] {
            if let Some(value) = value {
                upsert_info_field(&mut fields, key, value);
            }
        }
    }
    if let Some(edits) = generic {
        upsert_or_remove_info_field(&mut fields, WAV_TITLE_KEY, &edits.title);
        upsert_or_remove_info_field(&mut fields, WAV_GENRE_KEY, &edits.genre);
    }
    if fields.is_empty() {
        chunks.retain(|(id, data)| !(id == b"LIST" && data.len() >= 4 && &data[..4] == b"INFO"));
        return;
    }
    let encoded = encode_info_fields(&fields);
    if let Some(index) = chunks
        .iter()
        .position(|(id, data)| id == b"LIST" && data.len() >= 4 && &data[..4] == b"INFO")
    {
        chunks[index] = (*b"LIST", encoded);
    } else {
        chunks.push((*b"LIST", encoded));
    }
}

fn parse_info_fields(bytes: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
    let mut offset = 0usize;
    let mut fields = Vec::new();
    while offset + 8 <= bytes.len() {
        let Ok(id) = bytes[offset..offset + 4].try_into() else {
            break;
        };
        let Ok(size_bytes) = bytes[offset + 4..offset + 8].try_into() else {
            break;
        };
        let size = u32::from_le_bytes(size_bytes) as usize;
        offset += 8;
        if offset + size > bytes.len() {
            break;
        }
        fields.push((id, bytes[offset..offset + size].to_vec()));
        offset += size;
        if size % 2 == 1 {
            offset += 1;
        }
    }
    fields
}

fn upsert_info_field(fields: &mut Vec<([u8; 4], Vec<u8>)>, key: &str, value: &str) {
    let mut id = [b' '; 4];
    for (index, byte) in key.as_bytes().iter().take(4).enumerate() {
        id[index] = *byte;
    }
    let mut data = value.as_bytes().to_vec();
    data.push(0);
    if let Some(existing) = fields.iter_mut().find(|(found, _)| found == &id) {
        existing.1 = data;
    } else {
        fields.push((id, data));
    }
}

fn info_field_id(key: &str) -> [u8; 4] {
    let mut id = [b' '; 4];
    for (index, byte) in key.as_bytes().iter().take(4).enumerate() {
        id[index] = *byte;
    }
    id
}

fn remove_info_field(fields: &mut Vec<([u8; 4], Vec<u8>)>, key: &str) {
    let id = info_field_id(key);
    fields.retain(|(found, _)| found != &id);
}

fn upsert_or_remove_info_field(fields: &mut Vec<([u8; 4], Vec<u8>)>, key: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        remove_info_field(fields, key);
    } else {
        upsert_info_field(fields, key, trimmed);
    }
}

fn encode_info_fields(fields: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::from(*b"INFO");
    for (id, data) in fields {
        body.extend_from_slice(id);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
    }
    body
}

pub(crate) fn encode_riff_wave(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::from(*b"WAVE");
    for (id, data) in chunks {
        body.extend_from_slice(id);
        body.extend_from_slice(&(data.len() as u32).to_le_bytes());
        body.extend_from_slice(data);
        if data.len() % 2 == 1 {
            body.push(0);
        }
    }
    let mut bytes = Vec::from(*b"RIFF");
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend(body);
    bytes
}

/// Edits a copy, then swaps it in, so a failed write never truncates the original.
pub(crate) fn stage_and_replace(
    path: &Path,
    edit: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let original_perms = std::fs::metadata(path).ok().map(|meta| meta.permissions());
    let restore_perms = || {
        if let Some(perms) = original_perms.clone() {
            if path.exists() {
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    };
    let tmp = crate::path_util::unique_sidecar(path, "tag");
    if let Err(err) = std::fs::copy(path, &tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Failed to stage {}: {err}", path.display()));
    }
    if let Err(err) = crate::path_util::ensure_writable(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "Failed to prepare tagged file {}: {err}",
            path.display()
        ));
    }

    if let Err(err) = edit(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = crate::path_util::sync_file(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Failed to sync tagged file {}: {err}", tmp.display()));
    }
    if let Err(err) = crate::path_util::ensure_writable(path) {
        let _ = std::fs::remove_file(&tmp);
        restore_perms();
        return Err(format!(
            "Cannot write tags to read-only file {}: {err}",
            path.display()
        ));
    }
    if let Err(err) = crate::path_util::replace_file(&tmp, path) {
        if path.exists() {
            restore_perms();
            let _ = std::fs::remove_file(&tmp);
        } else {
            let _ = std::fs::rename(&tmp, path);
            restore_perms();
        }
        return Err(format!("Failed to replace {}: {err}", path.display()));
    }

    let _ = crate::path_util::sync_parent_dir(path);
    restore_perms();
    Ok(())
}

/// Tundra's marker comment, preserving a comment the user already wrote.
pub(crate) fn tundra_comment(existing: Option<&str>) -> String {
    let marker = tundra_comment_marker();
    let Some(existing) = existing.map(str::trim).filter(|text| !text.is_empty()) else {
        return marker;
    };
    if tundra_owns_tags(existing) {
        marker
    } else {
        existing.to_string()
    }
}

fn explicit_instrument_from_tag(tag: &Tag) -> Option<String> {
    for item in tag.items() {
        let description = item.description();
        if description.eq_ignore_ascii_case("instrument")
            || description.eq_ignore_ascii_case("instrumentname")
            || description.eq_ignore_ascii_case("instrument type")
        {
            if let ItemValue::Text(text) = item.value() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    if let Some(value) = tag.comment().and_then(|text| instrument_from_marked_comment(&text))
    {
        return Some(value);
    }
    None
}

/// Instrument from native key, legacy placements, then sidecar. Pass `tags` to skip re-parse.
pub(crate) fn durable_instrument(path: &Path, native: &NativeTags, tags: Option<&[Tag]>) -> Option<String> {
    if let Some(instrument) = &native.instrument {
        return Some(instrument.clone());
    }
    let legacy = match tags {
        Some(tags) => tags.iter().find_map(explicit_instrument_from_tag),
        None => read_file_tags(path)
            .and_then(|tags| tags.generic.iter().find_map(explicit_instrument_from_tag)),
    };
    legacy.or_else(|| crate::tag_store::instrument(path))
}

pub(crate) fn apply_path_artist_hint(fields: &mut TagFields, path: &Path) {
    push_field(&mut fields.artist, artist_hint_from_path(path));
}

fn overlay_nonempty(dest: &mut String, source: &str) {
    let trimmed = source.trim();
    if !trimmed.is_empty() {
        *dest = trimmed.to_string();
    }
}

pub(crate) fn overlay_sidecar_manual_fields(
    fields: &mut TagFields,
    sidecar: &crate::tag_store::SidecarManualFields,
) {
    overlay_nonempty(&mut fields.title, &sidecar.title);
    overlay_nonempty(&mut fields.artist, &sidecar.artist);
    if !sidecar.artist.trim().is_empty() {
        fields.file_artist = fields.artist.clone();
    }
    overlay_nonempty(&mut fields.bpm, &sidecar.bpm);
    overlay_nonempty(&mut fields.key, &sidecar.key);
    overlay_nonempty(&mut fields.genre, &sidecar.genre);
    overlay_nonempty(&mut fields.comment, &sidecar.comment);
    if !sidecar.comment.trim().is_empty() {
        fields.file_comment = fields.comment.clone();
    }
    if !sidecar.instrument.trim().is_empty() && fields.instrument.trim().is_empty() {
        fields.instrument = sidecar.instrument.trim().to_string();
        fields.explicit_instrument = fields.instrument.clone();
    }
}

/// Returns `None` when nothing at all could be read, so callers do not cache a
/// transient failure as "this file has no tags".
pub fn read_tag_fields(path: &Path) -> Option<TagFields> {
    if !is_audio(path) {
        return None;
    }

    let file_tags = read_file_tags(path);
    let sidecar_instrument = crate::tag_store::instrument(path);
    let sidecar_manual = crate::tag_store::manual_fields(path);
    if file_tags.is_none() && sidecar_instrument.is_none() && sidecar_manual.is_none() {
        return None;
    }
    let (native, tags) = file_tags
        .map(|tags| (tags.native, tags.generic))
        .unwrap_or_default();
    let tags = tags.as_slice();

    let mut fields = TagFields::default();
    for tag in tags {
        push_field(&mut fields.title, tag.title());
        push_field(&mut fields.artist, tag.artist());
        push_field(&mut fields.album, tag.album());
        push_field(&mut fields.genre, tag.genre());
        push_field(&mut fields.comment, tag.comment());
        push_field(
            &mut fields.album_artist,
            tag.get_string(ItemKey::AlbumArtist),
        );
        push_field(&mut fields.composer, tag.get_string(ItemKey::Composer));
        push_field(&mut fields.label, tag.get_string(ItemKey::Label));
        push_field(&mut fields.title, tag.get_string(ItemKey::TrackTitle));
        push_field(&mut fields.artist, tag.get_string(ItemKey::TrackArtist));
        push_field(&mut fields.bpm, tag.get_string(ItemKey::Bpm));
        push_field(&mut fields.bpm, tag.get_string(ItemKey::IntegerBpm));
        push_field(&mut fields.key, tag.get_string(ItemKey::InitialKey));
    }

    if let Some(instrument) = durable_instrument(path, &native, Some(tags)) {
        fields.explicit_instrument = instrument.clone();
        fields.instrument = instrument;
    } else {
        for tag in tags {
            push_instrument_field(&mut fields, tag);
        }
    }
    fields.file_comment = native.comment.clone().unwrap_or_default();
    if !fields.file_comment.is_empty() {
        fields.comment = fields.file_comment.clone();
    }
    fields.file_artist = native.artist.clone().unwrap_or_default();
    if !fields.file_artist.is_empty() {
        fields.artist = fields.file_artist.clone();
    }
    apply_path_artist_hint(&mut fields, path);

    if let Some(sidecar) = sidecar_manual {
        overlay_sidecar_manual_fields(&mut fields, &sidecar);
    }

    Some(fields)
}

pub fn instrument_tag(path: &Path) -> Option<String> {
    if !is_audio(path) {
        return None;
    }

    let native = read_native_tags(path).unwrap_or_default();
    durable_instrument(path, &native, None)
}
