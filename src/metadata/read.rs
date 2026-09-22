use lofty::file::{AudioFile, TaggedFileExt};
use lofty::id3::v2::Id3v2Tag;
use lofty::ogg::tag::VorbisComments;
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag};
use std::path::Path;

use super::fields::{ManualTagEdits, TagFields};
use super::hints::artist_hint_from_path;

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
pub(crate) const ID3_INSTRUMENT_KEY: &str = "INSTRUMENT";
/// Instrument, artist, and comment keys, in `NativeTags` order.
pub(crate) const WAV_NATIVE_KEYS: [&str; 3] = [WAV_INSTRUMENT_KEY, WAV_ARTIST_KEY, WAV_COMMENT_KEY];
pub(crate) const VORBIS_NATIVE_KEYS: [&str; 3] = [VORBIS_INSTRUMENT_KEY, VORBIS_ARTIST_KEY, VORBIS_COMMENT_KEY];

/// Sets `value` from `source` unless it already holds something.
fn push_field(value: &mut String, source: Option<impl AsRef<str>>) {
    if value.is_empty()
        && let Some(text) = source
            .as_ref()
            .map(|text| text.as_ref().trim())
            .filter(|text| !text.is_empty())
    {
        *value = text.to_owned();
    }
}

pub(crate) fn non_empty(value: Option<&str>) -> Option<String> {
    value.map(str::trim).filter(|text| !text.is_empty()).map(str::to_owned)
}

fn marked_instrument_line(line: &str) -> Option<&str> {
    let line = line.trim();
    ["INSTRUMENT:", "INSTRUMENT="]
        .iter()
        .find_map(|prefix| line.strip_prefix(prefix))
        .map(str::trim)
        .filter(|rest| !rest.is_empty())
}

pub(crate) fn instrument_from_marked_comment(comment: &str) -> Option<String> {
    comment.lines().find_map(marked_instrument_line).map(str::to_string)
}

fn marker_line_version(line: &str) -> Option<u32> {
    let line = line.trim();
    if line.eq_ignore_ascii_case("Tundra") || line.eq_ignore_ascii_case(LEGACY_TUNDRA_AUTO_TAG_COMMENT) {
        return Some(0);
    }
    line.to_ascii_lowercase()
        .strip_prefix("tundra v")
        .and_then(|rest| rest.trim().parse().ok())
}

/// Version recorded in a Tundra marker comment, if any.
pub(crate) fn parse_tundra_comment_version(comment: &str) -> Option<u32> {
    comment.lines().find_map(marker_line_version)
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

/// Tundra may replace tags it wrote; sidecar alone does not own native tags.
pub(crate) fn tundra_tagged_file(path: &Path, comment: &str, native_instrument: &str) -> bool {
    parse_tundra_comment_version(comment).is_some()
        || instrument_from_marked_comment(comment).is_some()
        || (native_instrument.trim().is_empty() && crate::tag_store::tundra_instrument(path).is_some())
}

/// The comment to write: lines Tundra wrote earlier are replaced by the current
/// marker, and a comment with no Tundra lines is left exactly as the user wrote
/// it (Tundra does not claim files whose comment it does not own).
pub(crate) fn tundra_comment(existing: Option<&str>) -> String {
    let existing = existing.unwrap_or_default().trim();
    let is_tundra_line = |line: &&str| marker_line_version(line).is_some() || marked_instrument_line(line).is_some();
    if !existing.lines().any(|line| is_tundra_line(&line)) && !existing.is_empty() {
        return existing.to_string();
    }
    existing
        .lines()
        .filter(|line| !is_tundra_line(line) && !line.trim().is_empty())
        .map(str::to_string)
        .chain([format!("Tundra v{TUNDRA_TAG_VERSION}")])
        .collect::<Vec<_>>()
        .join("\n")
}

/// File extensions Tundra lists and plays; one per `Container`, plus `aif`.
pub const AUDIO_EXTENSIONS: &[&str] = &["flac", "wav", "mp3", "ogg", "aiff", "aif"];

/// Whether `path` has a supported audio extension. Does not touch the disk.
pub fn is_audio(path: &Path) -> bool {
    Container::from_extension(path).is_some()
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
    /// Extension first, content second.
    pub(crate) fn of(path: &Path) -> Option<Self> {
        Self::from_extension(path).or_else(|| Self::sniff(path))
    }

    /// Content first, extension second: a write must use the parser that matches
    /// the bytes, whatever the file is called.
    pub(crate) fn detect(path: &Path) -> Option<Self> {
        Self::sniff(path).or_else(|| Self::from_extension(path))
    }

    fn from_extension(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        [
            ("wav", Self::Wav),
            ("flac", Self::Flac),
            ("ogg", Self::Ogg),
            ("mp3", Self::Mp3),
            ("aiff", Self::Aiff),
            ("aif", Self::Aiff),
        ]
        .into_iter()
        .find_map(|(known, container)| known.eq_ignore_ascii_case(ext).then_some(container))
    }

    fn sniff(path: &Path) -> Option<Self> {
        use std::io::Read;

        let mut header = [0u8; 12];
        std::fs::File::open(path).ok()?.read_exact(&mut header).ok()?;
        // An MPEG audio frame sync with a non-reserved layer.
        let frame_sync = header[0] == 0xFF && header[1] & 0xE0 == 0xE0 && header[1] & 0x06 != 0;
        match (&header[..4], &header[8..]) {
            (b"RIFF", b"WAVE") => Some(Self::Wav),
            (b"FORM", b"AIFF") => Some(Self::Aiff),
            (b"fLaC", _) => Some(Self::Flac),
            (b"OggS", _) => Some(Self::Ogg),
            _ if header.starts_with(b"ID3") || frame_sync => Some(Self::Mp3),
            _ => None,
        }
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

    fn from_keys(keys: [&str; 3], get: impl Fn(&str) -> Option<String>) -> Self {
        let [instrument, artist, comment] = keys.map(get);
        Self {
            instrument,
            artist,
            comment,
        }
    }

    fn vorbis(vorbis: Option<&VorbisComments>) -> Self {
        Self::from_keys(VORBIS_NATIVE_KEYS, |key| vorbis.and_then(|tag| non_empty(tag.get(key))))
    }

    fn id3(id3: Option<&Id3v2Tag>) -> Self {
        Self {
            instrument: id3.and_then(|tag| non_empty(tag.get_user_text(ID3_INSTRUMENT_KEY))),
            artist: id3.and_then(|tag| non_empty(tag.artist().as_deref())),
            comment: id3.and_then(|tag| non_empty(tag.comment().as_deref())),
        }
    }
}

/// Skip audio properties and cover art on tag-only reads.
fn tag_parse_options() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false)
}

/// Read cover art back in before write so saves do not strip it.
pub(crate) fn write_parse_options() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new().read_properties(false)
}

/// `RiffInfoList::get` compares fourccs case-sensitively even though `insert`
/// does not, so a list written with non-canonical case needs a second look.
fn riff_get(info: &lofty::iff::wav::RiffInfoList, key: &str) -> Option<String> {
    non_empty(info.get(key)).or_else(|| {
        info.into_iter()
            .find(|(found, _)| found.eq_ignore_ascii_case(key))
            .and_then(|(_, value)| non_empty(Some(value)))
    })
}

/// Canonical native keys plus generic tags from one parse.
pub(crate) struct FileTags {
    pub native: NativeTags,
    pub generic: Vec<Tag>,
}

pub(crate) fn read_container_tags_as(path: &Path, container: Container) -> Option<FileTags> {
    let mut file = std::fs::File::open(path).ok()?;
    let options = tag_parse_options();
    let tags = |tags: [Option<Tag>; 2]| tags.into_iter().flatten().collect();

    Some(match container {
        Container::Wav => {
            let mut wav = lofty::iff::wav::WavFile::read_from(&mut file, options).ok()?;
            let info = wav.remove_riff_info();
            let native = NativeTags::from_keys(WAV_NATIVE_KEYS, |key| {
                info.as_ref().and_then(|list| riff_get(list, key))
            });
            let generic = tags([info.map(Tag::from), wav.remove_id3v2().map(Tag::from)]);
            FileTags { native, generic }
        }
        Container::Flac => {
            let mut flac = lofty::flac::FlacFile::read_from(&mut file, options).ok()?;
            let vorbis = flac.remove_vorbis_comments();
            FileTags {
                native: NativeTags::vorbis(vorbis.as_ref()),
                generic: tags([vorbis.map(Tag::from), flac.remove_id3v2().map(Tag::from)]),
            }
        }
        Container::Ogg => {
            let mut ogg = lofty::ogg::VorbisFile::read_from(&mut file, options).ok()?;
            let vorbis = std::mem::take(ogg.vorbis_comments_mut());
            FileTags {
                native: NativeTags::vorbis(Some(&vorbis)),
                generic: vec![Tag::from(vorbis)],
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
                native: NativeTags::id3(id3v2.as_ref()),
                generic: tags([id3v2.map(Tag::from), other]),
            }
        }
        Container::Aiff => {
            let mut aiff = lofty::iff::aiff::AiffFile::read_from(&mut file, options).ok()?;
            let text = aiff.remove_text_chunks();
            let id3 = aiff.remove_id3v2();
            let from_id3 = NativeTags::id3(id3.as_ref());
            let comment = text
                .as_ref()
                .and_then(|text| text.comment())
                .map(|comment| comment.to_string());
            let annotations = text.as_ref().and_then(|text| text.annotations.as_ref());
            let native = NativeTags {
                instrument: from_id3.instrument.or_else(|| {
                    let lines = annotations.into_iter().flatten().map(String::as_str);
                    lines.chain(comment.as_deref()).find_map(instrument_from_marked_comment)
                }),
                artist: text
                    .as_ref()
                    .and_then(|text| non_empty(text.author.as_deref()))
                    .or(from_id3.artist),
                comment: comment.or(from_id3.comment),
            };
            FileTags {
                native,
                generic: tags([text.map(Tag::from), id3.map(Tag::from)]),
            }
        }
    })
}

fn read_container_tags(path: &Path) -> Option<FileTags> {
    read_container_tags_as(path, Container::of(path)?)
}

/// Falls back to a generic probe when extension and container disagree
/// (the instrument may then live in the sidecar).
pub(crate) fn read_file_tags(path: &Path) -> Option<FileTags> {
    if let Some(tags) = read_container_tags(path) {
        return Some(tags);
    }
    let tagged = lofty::probe::Probe::open(path)
        .ok()?
        .options(tag_parse_options())
        .read()
        .ok()?;
    Some(FileTags {
        native: NativeTags::default(),
        generic: tagged.tags().to_vec(),
    })
}

/// Canonical instrument/artist/comment keys for the container.
pub(crate) fn read_native_tags(path: &Path) -> Option<NativeTags> {
    read_container_tags(path).map(|tags| tags.native)
}

fn explicit_instrument_from_tag(tag: &Tag) -> Option<String> {
    tag.items()
        .filter(|item| {
            let description = item.description();
            ["instrument", "instrumentname", "instrument type"]
                .iter()
                .any(|name| description.eq_ignore_ascii_case(name))
        })
        .find_map(|item| match item.value() {
            ItemValue::Text(text) => non_empty(Some(text)),
            _ => None,
        })
        .or_else(|| tag.comment().and_then(|text| instrument_from_marked_comment(&text)))
}

/// Instrument from native key, legacy placements, then sidecar. Pass `tags` to skip re-parse.
pub(crate) fn durable_instrument(path: &Path, native: &NativeTags, tags: Option<&[Tag]>) -> Option<String> {
    if let Some(instrument) = &native.instrument {
        return Some(instrument.clone());
    }
    let legacy = match tags {
        Some(tags) => tags.iter().find_map(explicit_instrument_from_tag),
        None => read_file_tags(path).and_then(|tags| tags.generic.iter().find_map(explicit_instrument_from_tag)),
    };
    legacy.or_else(|| crate::tag_store::instrument(path))
}

/// Values saved in the sidecar win over the file's.
fn overlay_sidecar_manual_fields(fields: &mut TagFields, sidecar: &ManualTagEdits) {
    let artist = sidecar.artist.trim();
    let comment = sidecar.comment.trim();
    for (dest, value) in [
        (&mut fields.title, sidecar.title.trim()),
        (&mut fields.artist, artist),
        (&mut fields.file_artist, artist),
        (&mut fields.bpm, sidecar.bpm.trim()),
        (&mut fields.key, sidecar.key.trim()),
        (&mut fields.genre, sidecar.genre.trim()),
        (&mut fields.comment, comment),
        (&mut fields.file_comment, comment),
    ] {
        if !value.is_empty() {
            *dest = value.to_string();
        }
    }
    let instrument = sidecar.instrument.trim();
    if !instrument.is_empty() && fields.instrument.trim().is_empty() {
        fields.instrument = instrument.to_string();
        fields.explicit_instrument = fields.instrument.clone();
    }
}

/// Fields lofty maps consistently across tag types, first non-empty value wins.
pub(crate) fn generic_tag_fields(tags: &[Tag]) -> TagFields {
    let mut fields = TagFields::default();
    for tag in tags {
        push_field(&mut fields.title, tag.title());
        push_field(&mut fields.artist, tag.artist());
        push_field(&mut fields.album, tag.album());
        push_field(&mut fields.genre, tag.genre());
        push_field(&mut fields.comment, tag.comment());
        for (value, key) in [
            (&mut fields.album_artist, ItemKey::AlbumArtist),
            (&mut fields.composer, ItemKey::Composer),
            (&mut fields.label, ItemKey::Label),
            (&mut fields.title, ItemKey::TrackTitle),
            (&mut fields.artist, ItemKey::TrackArtist),
            (&mut fields.bpm, ItemKey::Bpm),
            (&mut fields.key, ItemKey::InitialKey),
        ] {
            push_field(value, tag.get_string(key));
        }
        push_field(&mut fields.bpm, tag.get_string(ItemKey::IntegerBpm));
    }
    fields
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
    let (native, tags) = file_tags.map(|tags| (tags.native, tags.generic)).unwrap_or_default();

    let mut fields = generic_tag_fields(&tags);
    match durable_instrument(path, &native, Some(&tags)) {
        Some(instrument) => {
            fields.explicit_instrument = instrument.clone();
            fields.instrument = instrument;
        }
        // Loose placements older taggers used; searchable, but not the file's own instrument.
        None => {
            for tag in &tags {
                push_field(&mut fields.instrument, tag.get_string(ItemKey::ContentGroup));
                push_field(&mut fields.instrument, tag.get_string(ItemKey::Description));
            }
        }
    }
    fields.file_comment = native.comment.unwrap_or_default();
    if !fields.file_comment.is_empty() {
        fields.comment = fields.file_comment.clone();
    }
    fields.file_artist = native.artist.unwrap_or_default();
    if !fields.file_artist.is_empty() {
        fields.artist = fields.file_artist.clone();
    }
    push_field(&mut fields.artist, artist_hint_from_path(path));
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
