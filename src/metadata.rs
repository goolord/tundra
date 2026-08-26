use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use lofty::file::TaggedFileExt;
use lofty::ogg::tag::VorbisComments;
use lofty::tag::{Accessor, ItemKey, ItemValue, Tag};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::types::is_audio;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TagField {
    Title,
    Artist,
    Album,
    Genre,
    Comment,
    AlbumArtist,
    Composer,
    Label,
    Bpm,
    Key,
    Instrument,
}

impl TagField {
    pub const ALL: [TagField; 11] = [
        TagField::Bpm,
        TagField::Key,
        TagField::Instrument,
        TagField::Title,
        TagField::Artist,
        TagField::Genre,
        TagField::Comment,
        TagField::Album,
        TagField::AlbumArtist,
        TagField::Composer,
        TagField::Label,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TagField::Title => "title",
            TagField::Artist => "artist",
            TagField::Album => "album",
            TagField::Genre => "genre",
            TagField::Comment => "comment",
            TagField::AlbumArtist => "albumartist",
            TagField::Composer => "composer",
            TagField::Label => "label",
            TagField::Bpm => "bpm",
            TagField::Key => "key",
            TagField::Instrument => "instrument",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TagField::Title => "Title",
            TagField::Artist => "Artist",
            TagField::Album => "Album",
            TagField::Genre => "Genre",
            TagField::Comment => "Comment",
            TagField::AlbumArtist => "Album artist",
            TagField::Composer => "Composer",
            TagField::Label => "Label",
            TagField::Bpm => "BPM",
            TagField::Key => "Key",
            TagField::Instrument => "Instrument",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "title" | "track" | "name" => Some(TagField::Title),
            "artist" | "trackartist" => Some(TagField::Artist),
            "album" => Some(TagField::Album),
            "genre" => Some(TagField::Genre),
            "comment" => Some(TagField::Comment),
            "albumartist" | "album_artist" => Some(TagField::AlbumArtist),
            "composer" => Some(TagField::Composer),
            "label" => Some(TagField::Label),
            "bpm" | "tempo" => Some(TagField::Bpm),
            "key" | "initialkey" | "initial_key" => Some(TagField::Key),
            "instrument" | "inst" => Some(TagField::Instrument),
            _ => None,
        }
    }

    fn matches_query(self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        self.match_score(needle) >= 700
    }

    fn match_score(self, needle: &str) -> i32 {
        if needle.is_empty() {
            return 0;
        }
        let key = self.as_str();
        let label = self.label().to_ascii_lowercase();
        if key == needle {
            1_000
        } else if label == needle {
            900
        } else if key.starts_with(needle) {
            800
        } else if label.starts_with(needle) {
            700
        } else if key.contains(needle) {
            400
        } else if label.contains(needle) {
            300
        } else {
            0
        }
    }
}

pub fn tag_field_match_score(field: TagField, input: &str) -> i32 {
    field.match_score(&input.trim().to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagFilter {
    pub field: TagField,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagFields {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub comment: String,
    pub album_artist: String,
    pub composer: String,
    pub label: String,
    pub bpm: String,
    pub key: String,
    pub instrument: String,
    #[serde(default)]
    pub explicit_instrument: String,
    /// Artist under the container's canonical key, with no path hint applied.
    #[serde(default)]
    pub file_artist: String,
    /// Comment under the container's canonical key. Kept separate from
    /// `comment` so auto-tag status is judged from the same values whether it
    /// is computed from a cached entry or read fresh from disk.
    #[serde(default)]
    pub file_comment: String,
}

impl TagFields {
    pub fn field_value(&self, field: TagField) -> &str {
        match field {
            TagField::Instrument if !self.explicit_instrument.is_empty() => {
                &self.explicit_instrument
            }
            TagField::Title => &self.title,
            TagField::Artist => &self.artist,
            TagField::Album => &self.album,
            TagField::Genre => &self.genre,
            TagField::Comment => &self.comment,
            TagField::AlbumArtist => &self.album_artist,
            TagField::Composer => &self.composer,
            TagField::Label => &self.label,
            TagField::Bpm => &self.bpm,
            TagField::Key => &self.key,
            TagField::Instrument => &self.instrument,
        }
    }
}

/// Compact tags for the waveform toolbar (instrument, bpm, key, genre).
pub fn control_bar_tags(fields: &TagFields) -> Vec<(TagField, String)> {
    let mut tags = Vec::new();
    let instrument = fields.field_value(TagField::Instrument);
    if !instrument.is_empty() {
        tags.push((TagField::Instrument, instrument.to_string()));
    }
    if !fields.bpm.is_empty() {
        tags.push((TagField::Bpm, fields.bpm.clone()));
    }
    if !fields.key.is_empty() {
        tags.push((TagField::Key, fields.key.clone()));
    }
    if !fields.genre.is_empty() {
        tags.push((TagField::Genre, fields.genre.clone()));
    }
    tags
}

/// Compact tag line for tests and text-only surfaces.
pub fn format_control_bar_tags(fields: &TagFields) -> Option<String> {
    let tags = control_bar_tags(fields);
    if tags.is_empty() {
        None
    } else {
        Some(
            tags.iter()
                .map(|(field, value)| format!("{}: {value}", field.label()))
                .collect::<Vec<_>>()
                .join(" · "),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagParseError {
    MissingSeparator,
    UnknownField,
    EmptyValue,
    UnclosedQuote,
}

pub fn tag_parse_message(err: TagParseError) -> &'static str {
    match err {
        TagParseError::MissingSeparator => {
            "Use field:value (example: title:My Song)."
        }
        TagParseError::UnknownField => {
            "Unknown tag field. Try bpm, key, instrument, title, genre…"
        }
        TagParseError::EmptyValue => "Tag value cannot be empty.",
        TagParseError::UnclosedQuote => "Closing quote missing in tag value.",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedMetadata {
    pub mtime_secs: u64,
    pub fields: TagFields,
}

/// Disk-backed caches loaded off the UI thread during startup.
#[derive(Debug, Clone, Default)]
pub struct PersistedCaches {
    pub dirs: HashMap<PathBuf, Vec<PathBuf>>,
    pub metadata: HashMap<PathBuf, CachedMetadata>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub paths: Vec<PathBuf>,
    pub new_metadata: HashMap<PathBuf, CachedMetadata>,
    pub cached_roots: HashMap<PathBuf, Vec<PathBuf>>,
}

pub struct MetadataLookup {
    cache: Arc<HashMap<PathBuf, CachedMetadata>>,
    new_entries: HashMap<PathBuf, CachedMetadata>,
}

impl MetadataLookup {
    pub fn new(cache: Arc<HashMap<PathBuf, CachedMetadata>>) -> Self {
        Self {
            cache,
            new_entries: HashMap::new(),
        }
    }

    pub fn into_new_entries(self) -> HashMap<PathBuf, CachedMetadata> {
        self.new_entries
    }

    fn lookup_cached(&self, path: &Path) -> Option<&CachedMetadata> {
        for key in crate::path_util::cache_lookup_keys(path) {
            if let Some(cached) = self.new_entries.get(&key).or_else(|| self.cache.get(&key)) {
                return Some(cached);
            }
        }
        None
    }

    fn store_fields(&mut self, path: &Path, mtime_secs: u64, fields: TagFields) -> TagFields {
        let fields_clone = fields.clone();
        self.new_entries.insert(
            crate::path_util::cache_key(path.to_path_buf()),
            CachedMetadata {
                mtime_secs,
                fields,
            },
        );
        fields_clone
    }

    pub fn tag_fields(&mut self, path: &Path) -> TagFields {
        self.tag_fields_for_search(path, true)
    }

    fn tag_fields_for_search(&mut self, path: &Path, allow_disk_read: bool) -> TagFields {
        let cached = self.lookup_cached(path).cloned();
        if !allow_disk_read && cached.is_none() {
            return TagFields::default();
        }

        if let Some(mtime_secs) = file_mtime_secs(path) {
            if let Some(cached) = &cached {
                if cached.mtime_secs == mtime_secs {
                    return cached.fields.clone();
                }
            }
            let Some(fields) = read_tag_fields(path) else {
                return TagFields::default();
            };
            return self.store_fields(path, mtime_secs, fields);
        }

        if !path.exists() {
            return TagFields::default();
        }

        if let Some(cached) = cached {
            return cached.fields;
        }

        let Some(fields) = read_tag_fields(path) else {
            return TagFields::default();
        };
        self.store_fields(path, 0, fields)
    }
}

fn push_field(value: &mut String, source: Option<impl AsRef<str>>) {
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

pub fn read_tag_fields_mtime(path: &Path) -> Option<u64> {
    file_mtime_secs(path)
}

fn file_mtime_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

fn push_instrument_field(fields: &mut TagFields, tag: &Tag) {
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
const WAV_INSTRUMENT_KEY: &str = "IKEY";
const WAV_ARTIST_KEY: &str = "IART";
const WAV_COMMENT_KEY: &str = "ICMT";
// Vorbis comments (FLAC, OGG) and ID3v2 user text (MP3) both name the field
// INSTRUMENT, which is what Mp3tag and similar taggers display.
const VORBIS_INSTRUMENT_KEY: &str = "INSTRUMENT";
const VORBIS_ARTIST_KEY: &str = "ARTIST";
const VORBIS_COMMENT_KEY: &str = "COMMENT";
const ID3_INSTRUMENT_KEY: &str = "INSTRUMENT";

fn instrument_from_marked_comment(comment: &str) -> Option<String> {
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
fn parse_tundra_comment_version(comment: &str) -> Option<u32> {
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

fn file_tundra_tag_version(path: &Path, comment: &str, native_instrument: &str) -> Option<u32> {
    parse_tundra_comment_version(comment).or_else(|| {
        native_instrument
            .trim()
            .is_empty()
            .then(|| crate::tag_store::tag_version(path))
            .flatten()
    })
}

pub fn tundra_tag_is_current(path: &Path, comment: &str) -> bool {
    file_tundra_tag_version(path, comment, "") == Some(TUNDRA_TAG_VERSION)
}

fn is_tundra_written_comment(comment: &str) -> bool {
    parse_tundra_comment_version(comment).is_some()
}

fn tundra_owns_tags(comment: &str) -> bool {
    is_tundra_written_comment(comment) || instrument_from_marked_comment(comment).is_some()
}

/// True when Tundra wrote (or owns) the on-file tags and may replace them.
/// A sidecar row is fallback-only: it does not own a file that already has a
/// native instrument from the user or another tagger.
pub fn tundra_tagged_file(path: &Path, comment: &str, native_instrument: &str) -> bool {
    tundra_owns_tags(comment)
        || (native_instrument.trim().is_empty() && crate::tag_store::instrument(path).is_some())
}

/// Containers Tundra tags natively. Each maps the instrument label to the one
/// key third-party taggers read back, so a write is always round-trippable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Wav,
    Flac,
    Ogg,
    Mp3,
}

impl Container {
    fn of(path: &Path) -> Option<Self> {
        match path
            .extension()
            .and_then(|ext| ext.to_str())?
            .to_ascii_lowercase()
            .as_str()
        {
            "wav" => Some(Self::Wav),
            "flac" => Some(Self::Flac),
            "ogg" => Some(Self::Ogg),
            "mp3" => Some(Self::Mp3),
            _ => None,
        }
    }
}

/// The fields the auto-tagger reads and writes. `None` means "absent" on read
/// and "leave alone" on write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NativeTags {
    instrument: Option<String>,
    artist: Option<String>,
    comment: Option<String>,
}

impl NativeTags {
    fn is_empty(&self) -> bool {
        self.instrument.is_none() && self.artist.is_none() && self.comment.is_none()
    }
}

/// Tag reads never need audio properties or embedded art, and decoding them
/// dominates the cost of a library scan, so both are switched off.
fn tag_parse_options() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false)
}

/// Writing re-emits only what was parsed, so artwork must be read back in or
/// saving would silently strip it.
fn write_parse_options() -> lofty::config::ParseOptions {
    lofty::config::ParseOptions::new().read_properties(false)
}

/// Generic multi-tag view of a file, used for the fields whose keys lofty
/// already maps consistently across formats (title, album, genre, bpm, key).
fn probe_tags(path: &Path) -> Option<lofty::file::TaggedFile> {
    lofty::probe::Probe::open(path)
        .ok()?
        .options(tag_parse_options())
        .read()
        .ok()
}

fn non_empty(value: Option<&str>) -> Option<String> {
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

fn vorbis_native_tags(vorbis: Option<&VorbisComments>) -> NativeTags {
    NativeTags {
        instrument: vorbis.and_then(|tag| non_empty(tag.get(VORBIS_INSTRUMENT_KEY))),
        artist: vorbis.and_then(|tag| non_empty(tag.get(VORBIS_ARTIST_KEY))),
        comment: vorbis.and_then(|tag| non_empty(tag.get(VORBIS_COMMENT_KEY))),
    }
}

fn apply_vorbis_tags(vorbis: &mut VorbisComments, tags: &NativeTags) {
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

/// Everything a tag read needs, from one parse: the canonical keys for this
/// container, plus a generic view of every tag it carries for the fields whose
/// names lofty already maps consistently across formats.
struct FileTags {
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
    })
}

/// When the extension misdescribes the container the canonical keys are out of
/// reach, since lofty's generic view does not carry them. Such a file still
/// yields its common fields here, and the auto-tagger routes its instrument to
/// the sidecar store, which is the intended home for containers Tundra cannot
/// tag natively.
fn read_file_tags(path: &Path) -> Option<FileTags> {
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
fn read_native_tags(path: &Path) -> Option<NativeTags> {
    read_container_tags(path).map(|tags| tags.native)
}

/// Writes the populated fields of `tags` to the container's canonical keys.
fn write_native_tags(path: &Path, tags: &NativeTags) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;
    use lofty::tag::Accessor;

    if tags.is_empty() {
        return Ok(());
    }
    let container = Container::of(path)
        .ok_or_else(|| format!("Unsupported file type: {}", path.display()))?;

    stage_and_replace(path, |staged| {
        let options = write_parse_options();
        let read_error =
            |err: lofty::error::FileParseError| format!("Failed to read {}: {err}", path.display());
        let write_error = |err: lofty::error::FileEncodingError| {
            format!("Failed to write tags to {}: {err}", path.display())
        };
        let open = || {
            std::fs::File::open(staged)
                .map_err(|err| format!("Failed to open {}: {err}", staged.display()))
        };

        match container {
            Container::Wav => write_wav_tags_preserving_chunks(staged, tags),
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
                if let Some(instrument) = &tags.instrument {
                    id3.insert_user_text(ID3_INSTRUMENT_KEY.to_string(), instrument.clone());
                }
                if let Some(artist) = &tags.artist {
                    id3.set_artist(artist.clone());
                }
                if let Some(comment) = &tags.comment {
                    id3.set_comment(comment.clone());
                }
                mp3.set_id3v2(id3);
                mp3.save_to_path(staged, WriteOptions::default())
                    .map_err(write_error)
            }
        }
    })
}

/// Rewrite only the LIST INFO chunk so `smpl` / `cue ` / `inst` / ACID survive.
fn write_wav_tags_preserving_chunks(path: &Path, tags: &NativeTags) -> Result<(), String> {
    let bytes = std::fs::read(path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
    let mut chunks = parse_riff_wave_chunks(&bytes)?;
    upsert_wav_info_chunk(&mut chunks, tags);
    let encoded = encode_riff_wave(&chunks);
    std::fs::write(path, encoded)
        .map_err(|err| format!("Failed to write tags to {}: {err}", path.display()))
}

fn parse_riff_wave_chunks(bytes: &[u8]) -> Result<Vec<([u8; 4], Vec<u8>)>, String> {
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

fn upsert_wav_info_chunk(chunks: &mut Vec<([u8; 4], Vec<u8>)>, tags: &NativeTags) {
    let mut fields = match chunks.iter().find(|(id, data)| {
        id == b"LIST" && data.len() >= 4 && &data[..4] == b"INFO"
    }) {
        Some((_, data)) => parse_info_fields(&data[4..]),
        None => Vec::new(),
    };
    for (key, value) in [
        (WAV_INSTRUMENT_KEY, &tags.instrument),
        (WAV_ARTIST_KEY, &tags.artist),
        (WAV_COMMENT_KEY, &tags.comment),
    ] {
        if let Some(value) = value {
            upsert_info_field(&mut fields, key, value);
        }
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

fn encode_riff_wave(chunks: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
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
fn stage_and_replace(
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
fn tundra_comment(existing: Option<&str>) -> String {
    let marker = tundra_comment_marker();
    let Some(existing) = existing.map(str::trim).filter(|text| !text.is_empty()) else {
        return marker;
    };
    if is_tundra_written_comment(existing) || instrument_from_marked_comment(existing).is_some() {
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

/// Instrument label that survives a round trip, in order of authority: the
/// container's canonical key, then placements older Tundra builds used, then the
/// sidecar store for files whose container refused the write.
/// `tags` lets callers that already parsed the file skip a second parse; pass
/// `None` outside the search hot path.
fn durable_instrument(path: &Path, native: &NativeTags, tags: Option<&[Tag]>) -> Option<String> {
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

/// Returns `None` when nothing at all could be read, so callers do not cache a
/// transient failure as "this file has no tags".
pub fn read_tag_fields(path: &Path) -> Option<TagFields> {
    if !is_audio(path) {
        return None;
    }

    let file_tags = read_file_tags(path);
    let sidecar = crate::tag_store::instrument(path);
    if file_tags.is_none() && sidecar.is_none() {
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

    Some(fields)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoTagFieldStatus {
    pub needs_instrument: bool,
    pub needs_artist: bool,
    pub needs_comment: bool,
    /// Instrument is present but was written by Tundra, so classify/apply may
    /// replace it. User-owned instrument tags stay read-only.
    pub can_retag_instrument: bool,
}

impl AutoTagFieldStatus {
    pub fn needs_any(self) -> bool {
        self.needs_instrument || self.needs_artist || self.needs_comment
    }

    pub fn allows_instrument_work(self) -> bool {
        self.needs_instrument || self.can_retag_instrument
    }

    pub fn is_complete(self) -> bool {
        !self.needs_any() && !self.can_retag_instrument
    }

    pub fn from_parts(
        path: &Path,
        explicit_instrument: &str,
        native_instrument: &str,
        file_artist: &str,
        comment: &str,
        native_writable: bool,
    ) -> Self {
        let tundra_tagged = tundra_tagged_file(path, comment, native_instrument);
        let has_instrument = !explicit_instrument.trim().is_empty();
        let current_tag =
            file_tundra_tag_version(path, comment, native_instrument) == Some(TUNDRA_TAG_VERSION);
        Self {
            needs_instrument: !has_instrument,
            can_retag_instrument: tundra_tagged && has_instrument && !current_tag,
            needs_artist: native_writable
                && file_artist.trim().is_empty()
                && artist_hint_from_path(path).is_some(),
            needs_comment: native_writable && needs_auto_tag_comment(comment),
        }
    }
}

pub fn auto_tag_already_complete_message() -> &'static str {
    "This file already has the tags Tundra would add."
}

pub fn auto_tag_field_status(path: &Path) -> Option<AutoTagFieldStatus> {
    if !is_audio(path) {
        return None;
    }
    let native = read_native_tags(path);
    let native_writable = native.is_some();
    let native = native.unwrap_or_default();
    Some(AutoTagFieldStatus::from_parts(
        path,
        durable_instrument(path, &native, None).unwrap_or_default().as_str(),
        native.instrument.as_deref().unwrap_or_default(),
        native.artist.as_deref().unwrap_or_default(),
        native.comment.as_deref().unwrap_or_default(),
        native_writable,
    ))
}

pub fn auto_tag_field_status_from_fields(path: &Path, fields: &TagFields) -> AutoTagFieldStatus {
    let native = read_native_tags(path);
    AutoTagFieldStatus::from_parts(
        path,
        &fields.explicit_instrument,
        native
            .as_ref()
            .and_then(|tags| tags.instrument.as_deref())
            .unwrap_or(""),
        &fields.file_artist,
        &fields.file_comment,
        native.is_some(),
    )
}

fn needs_auto_tag_comment(comment: &str) -> bool {
    let comment = comment.trim();
    if comment.is_empty() {
        return true;
    }
    match parse_tundra_comment_version(comment) {
        Some(version) => version != TUNDRA_TAG_VERSION,
        None => instrument_from_marked_comment(comment).is_some(),
    }
}

fn apply_path_artist_hint(fields: &mut TagFields, path: &Path) {
    push_field(&mut fields.artist, artist_hint_from_path(path));
}

fn parse_tag_value(raw: &str) -> Result<String, TagParseError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(TagParseError::EmptyValue);
    }

    if raw.starts_with('"') {
        let rest = &raw[1..];
        let end = rest.find('"').ok_or(TagParseError::UnclosedQuote)?;
        let value = rest[..end].trim();
        if value.is_empty() {
            return Err(TagParseError::EmptyValue);
        }
        return Ok(value.to_owned());
    }

    Ok(raw.to_owned())
}

pub fn parse_tag_filter(input: &str) -> Result<TagFilter, TagParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(TagParseError::EmptyValue);
    }
    let Some((key, value)) = input.split_once(':') else {
        return Err(TagParseError::MissingSeparator);
    };
    let Some(field) = TagField::parse(key) else {
        return Err(TagParseError::UnknownField);
    };
    let value = parse_tag_value(value)?;
    Ok(TagFilter { field, value })
}

pub fn tag_field_suggestions(input: &str) -> Vec<TagField> {
    if input.contains(':') {
        return Vec::new();
    }
    let needle = input.trim().to_ascii_lowercase();
    TagField::ALL
        .into_iter()
        .filter(|field| field.matches_query(&needle))
        .collect()
}

pub fn tag_field_best_match(input: &str) -> Option<TagField> {
    if input.contains(':') {
        return None;
    }
    let needle = input.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    TagField::ALL
        .iter()
        .filter(|field| field.matches_query(&needle))
        .max_by_key(|field| field.match_score(&needle))
        .copied()
}

pub fn index_paths(
    paths: &[PathBuf],
    cache: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> HashMap<PathBuf, CachedMetadata> {
    let mut lookup = MetadataLookup::new(cache);
    for path in paths {
        if is_audio(path) {
            lookup.tag_fields(path);
        }
    }
    lookup.into_new_entries()
}

pub const FILE_SEARCH_MIN_QUERY_LEN: usize = 2;
pub const TAG_SEARCH_DEBOUNCE_MS: u64 = 200;
const FILE_SEARCH_DEBOUNCE_MS: u64 = 200;
const FILE_SEARCH_DEBOUNCE_MS_SHORT: u64 = 450;
const CONTAINS_NAME_MATCH_SCORE: i64 = 400;
const FILE_SEARCH_MIN_FUZZY_SCORE: i32 = 70;
const FILE_SEARCH_CONFIDENT_RESULT_CAP: usize = 2_000;
const FILE_SEARCH_MAX_RESULTS: usize = 10_000;

pub fn file_search_debounce_ms(query_len: usize) -> u64 {
    if query_len <= FILE_SEARCH_MIN_QUERY_LEN {
        FILE_SEARCH_DEBOUNCE_MS_SHORT
    } else {
        FILE_SEARCH_DEBOUNCE_MS
    }
}

fn file_search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect()
}

fn text_contains(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    if case_sensitive {
        haystack.contains(needle)
    } else {
        haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }
}

fn term_field_score(
    matcher: &SkimMatcherV2,
    text: &str,
    term: &str,
    case_sensitive: bool,
) -> i64 {
    if text_contains(text, term, case_sensitive) {
        return CONTAINS_NAME_MATCH_SCORE + term.len() as i64;
    }
    matcher
        .fuzzy_match(text, term)
        .filter(|&score| score >= FILE_SEARCH_MIN_FUZZY_SCORE as i64)
        .unwrap_or(0) as i64
}

fn path_name_fields(path: &Path) -> Vec<String> {
    let mut name_fields = Vec::new();
    if let Some(name) = crate::path_util::file_name_lossy(path) {
        name_fields.push(name);
    }
    if let Some(stem) = crate::path_util::file_stem_lossy(path) {
        if name_fields.last().is_none_or(|last| last != &stem) {
            name_fields.push(stem);
        }
    }
    name_fields
}

fn path_has_substring_match(path: &Path, query: &str, case_sensitive: bool) -> bool {
    let terms = file_search_terms(query);
    if terms.is_empty() {
        return false;
    }
    let name_fields = path_name_fields(path);
    let path_string = path.to_string_lossy();
    terms.iter().all(|term| {
        name_fields
            .iter()
            .any(|field| text_contains(field, term, case_sensitive))
            || text_contains(&path_string, term, case_sensitive)
    })
}

fn is_confident_file_match(name_score: i64, path_score: i64) -> bool {
    name_score >= CONTAINS_NAME_MATCH_SCORE || path_score >= CONTAINS_NAME_MATCH_SCORE
}

fn path_match_scores(
    matcher: &SkimMatcherV2,
    path: &Path,
    query: &str,
    case_sensitive: bool,
) -> (i64, i64) {
    let terms = file_search_terms(query);
    if terms.is_empty() {
        return (0, 0);
    }

    let name_fields = path_name_fields(path);
    let path_string = path.to_string_lossy().into_owned();
    let mut name_score = 0i64;
    let mut path_score = 0i64;

    for term in &terms {
        let name_term_score = name_fields
            .iter()
            .map(|field| term_field_score(matcher, field, term, case_sensitive))
            .max()
            .unwrap_or(0);
        let path_term_score = if text_contains(&path_string, term, case_sensitive) {
            CONTAINS_NAME_MATCH_SCORE + term.len() as i64
        } else {
            0
        };
        let term_score = name_term_score.max(path_term_score);
        if term_score == 0 {
            return (0, 0);
        }
        name_score += name_term_score;
        path_score += path_term_score;
    }

    (name_score, path_score)
}

fn is_direct_name_match(name_score: i64) -> bool {
    name_score > 0
}

fn text_eq(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(b)
    }
}

fn text_starts_with(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    if case_sensitive {
        haystack.starts_with(needle)
    } else {
        haystack[..needle.len()].eq_ignore_ascii_case(needle)
    }
}

const FILE_SEARCH_BONUS: i64 = 1_000;
const DIRECT_FOLDER_BONUS: i64 = 2_000;
const EXACT_STEM_BONUS: i64 = 50_000;
const PREFIX_STEM_BONUS: i64 = 10_000;

fn search_sort_score(
    path: &Path,
    query: &str,
    name_score: i64,
    path_score: i64,
    is_dir: bool,
    case_sensitive: bool,
) -> i64 {
    if is_dir {
        if is_direct_name_match(name_score) {
            name_score + DIRECT_FOLDER_BONUS
        } else {
            path_score
        }
    } else {
        let base = if name_score > 0 { name_score } else { path_score };
        let mut score = base + FILE_SEARCH_BONUS;
        if let Some(stem) = crate::path_util::file_stem_lossy(path) {
            if text_eq(&stem, query, case_sensitive) {
                score += EXACT_STEM_BONUS;
            } else if text_starts_with(&stem, query, case_sensitive) {
                score += PREFIX_STEM_BONUS;
            }
        }
        score
    }
}

#[derive(Eq, PartialEq)]
struct SearchRank {
    score: i64,
    path: PathBuf,
}

impl Ord for SearchRank {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for SearchRank {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl SearchRank {
    fn for_file_search(
        path: PathBuf,
        query: &str,
        name_score: i64,
        path_score: i64,
        case_sensitive: bool,
    ) -> Self {
        let is_dir = !is_audio(&path);
        Self {
            score: search_sort_score(&path, query, name_score, path_score, is_dir, case_sensitive),
            path,
        }
    }

    fn for_tag_search(path: PathBuf, tag_score: i64) -> Self {
        Self { score: tag_score, path }
    }

    fn for_combined_search(
        path: PathBuf,
        query: &str,
        name_score: i64,
        path_score: i64,
        tag_score: i64,
        case_sensitive: bool,
    ) -> Self {
        let file_score =
            search_sort_score(&path, query, name_score, path_score, !is_audio(&path), case_sensitive);
        Self {
            score: file_score.saturating_add(tag_score.saturating_mul(100)),
            path,
        }
    }
}

fn file_search_matcher(case_sensitive: bool) -> SkimMatcherV2 {
    if case_sensitive {
        SkimMatcherV2::default().respect_case()
    } else {
        SkimMatcherV2::default().ignore_case()
    }
}

fn tag_search_matcher() -> SkimMatcherV2 {
    SkimMatcherV2::default().ignore_case()
}

const INSTRUMENT_ALIAS_GROUPS: &[&[&str]] = &[
    &[
        "kick", "bd", "bassdrum", "bass drum", "kick drum", "kickdrum", "808 kick", "808kick",
        "kik",
    ],
    &["snare", "sd", "sn", "snr"],
    &["rim", "rimshot", "rim shot", "side stick", "sidestick"],
    &[
        "hihat", "hi-hat", "hi hat", "hh", "hat", "hats", "open hat", "closed hat", "openhat",
        "closedhat", "op hat", "cl hat",
    ],
    &["clap", "handclap", "hand clap"],
    &["tom", "toms", "floor tom", "floortom", "rack tom", "racktom"],
    &[
        "perc", "percs", "percussion", "conga", "bongo", "bongos", "shaker", "tamb",
        "tambourine", "cowbell", "triangle", "woodblock", "cabasa",
    ],
    &["crash", "cymbal", "cymbals", "ride", "splash", "china"],
    &["bass", "sub", "subbass", "sub bass", "808 bass", "808bass", "reese", "wobble"],
    &["synth", "lead", "pad", "pluck", "stab", "arp", "arpeggio", "keys"],
    &[
        "fx", "sfx", "effect", "impact", "riser", "sweep", "noise", "atm", "atmosphere",
        "ambient", "transition", "downlifter", "uplifter",
    ],
    &["vocal", "vox", "voice", "acapella", "aca", "phrase", "adlib"],
    &["piano", "rhodes", "organ", "electric piano", "ep"],
    &["guitar", "gtr", "acoustic guitar", "acousticguitar"],
    &["loop", "loops", "top loop", "toploop", "drum loop", "drumloop"],
    &["oneshot", "one shot", "one-shot"],
    &["brass", "horn", "trumpet", "sax", "saxophone", "flute", "strings", "string"],
];

const INSTRUMENT_HINT_LABELS: &[&str] = &[
    "Kick",
    "Snare",
    "Rim",
    "Hi-Hat",
    "Clap",
    "Tom",
    "Percussion",
    "Cymbal",
    "Bass",
    "Synth",
    "FX",
    "Vocal",
    "Piano",
    "Guitar",
    "Loop",
    "One-shot",
    "Brass",
];

const _: () = assert!(INSTRUMENT_ALIAS_GROUPS.len() == INSTRUMENT_HINT_LABELS.len());

const MIN_INSTRUMENT_SUBSTRING_LEN: usize = 4;

fn instrument_alias_matches(needle: &str, alias_norm: &str) -> bool {
    if needle.is_empty() || alias_norm.is_empty() {
        return false;
    }
    if needle == alias_norm || is_simple_plural(needle, alias_norm) || is_simple_plural(alias_norm, needle)
    {
        return true;
    }
    // Short tokens only: "hat"/"hh". Prefix on longer names made `bass` hit
    // `bassdrum` (Kick) and `shot` hit `rimshot`.
    needle.len() < MIN_INSTRUMENT_SUBSTRING_LEN
        && alias_norm.len() < MIN_INSTRUMENT_SUBSTRING_LEN
        && (alias_norm.starts_with(needle) || needle.starts_with(alias_norm))
}

fn is_simple_plural(plural: &str, singular: &str) -> bool {
    plural.len() == singular.len() + 1 && plural.starts_with(singular) && plural.ends_with('s')
}

fn normalize_instrument_term(term: &str) -> String {
    term.to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn instrument_alias_groups_for(term: &str) -> Vec<&'static [&'static str]> {
    let needle = normalize_instrument_term(term);
    if needle.is_empty() {
        return Vec::new();
    }
    INSTRUMENT_ALIAS_GROUPS
        .iter()
        .copied()
        .filter(|group| {
            group.iter().any(|alias| {
                instrument_alias_matches(&needle, &normalize_instrument_term(alias))
            })
        })
        .collect()
}

fn instrument_search_terms(query: &str) -> Vec<String> {
    let mut terms = HashSet::new();
    terms.insert(normalize_instrument_term(query));
    for group in instrument_alias_groups_for(query) {
        for alias in group {
            terms.insert(normalize_instrument_term(alias));
        }
    }
    terms.into_iter().filter(|term| !term.is_empty()).collect()
}

pub fn instruments_related(left: &str, right: &str) -> bool {
    instrument_terms_related(left, right)
}

/// Folder and file-name tokens mapped to a canonical instrument label.
pub fn instrument_hint_from_path(path: &Path) -> Option<String> {
    let mut best: Option<(i32, &'static str)> = None;
    let mut consider = |score: i32, label: &'static str| {
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, label));
        }
    };

    if let Some(stem) = crate::path_util::file_stem_lossy(path) {
        for token in hint_name_tokens(&stem) {
            if let Some(label) = hint_label_for_term(&token) {
                consider(1_000 + token.len() as i32, label);
            }
        }
    }

    for (depth, ancestor) in path.ancestors().skip(1).enumerate() {
        let score_base = 500_i32.saturating_sub(depth as i32 * 50);
        if score_base == 0 {
            break;
        }
        let Some(name) = ancestor.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        for token in hint_name_tokens(name) {
            if let Some(label) = hint_label_for_term(&token) {
                consider(score_base + token.len() as i32, label);
            }
        }
    }

    best.map(|(_, label)| label.to_string())
}

/// Where an instrument hint came from, so the classifier can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintSource {
    Path,
    Tags,
}

impl HintSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Path => "Path hint",
            Self::Tags => "Tag hint",
        }
    }
}

/// Instrument suggested by tags the file already carries. Weaker evidence than
/// the path, so it is only consulted when the path names no instrument. This
/// catches files whose instrument sits in a non-canonical field: a grouping
/// written by an older build, a genre of "Kick", a title of "Kick 01".
fn instrument_hint_from_fields(fields: &TagFields) -> Option<String> {
    for value in [
        fields.instrument.as_str(),
        fields.title.as_str(),
        fields.genre.as_str(),
        fields.album.as_str(),
        fields.comment.as_str(),
    ] {
        if value.trim().is_empty() {
            continue;
        }
        for token in hint_name_tokens(value) {
            if let Some(label) = hint_label_for_term(&token) {
                return Some(label.to_string());
            }
        }
    }
    None
}

/// Everything known about a file before decoding its audio: filename, then
/// directory structure, then tags it already carries.
pub fn instrument_hint(path: &Path) -> Option<(String, HintSource)> {
    if let Some(hint) = instrument_hint_from_path(path) {
        return Some((hint, HintSource::Path));
    }
    let fields = read_tag_fields(path)?;
    instrument_hint_from_fields(&fields).map(|hint| (hint, HintSource::Tags))
}

const GENERIC_PATH_SEGMENTS: &[&str] = &[
    "samples",
    "sample",
    "packs",
    "pack",
    "library",
    "libraries",
    "lib",
    "audio",
    "music",
    "sound",
    "sounds",
    "assets",
    "content",
    "download",
    "downloads",
    "documents",
    "desktop",
    "users",
    "user",
    "home",
    "wav",
    "wavs",
    "aiff",
    "aif",
    "flac",
    "mp3",
    "splice",
    "loopcloud",
    "loopmasters",
    "producerloops",
    "one shots",
    "oneshots",
    "one-shots",
    "one_shots",
    "loops",
    "loop",
    "stems",
    "stem",
    "multis",
    "multitracks",
    "presets",
    "preset",
    "projects",
    "project",
    "favorites",
    "favourites",
    "recent",
    "temp",
    "tmp",
    "data",
    "media",
    "files",
    "file",
    "drive",
    "storage",
];

/// Best-effort artist label from parent folders (e.g. pack or label folder names).
pub fn artist_hint_from_path(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let mut candidates = Vec::new();

    for ancestor in parent.ancestors() {
        if should_stop_artist_walk(ancestor) {
            break;
        }
        let Some(name) = ancestor.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_generic_path_segment(name)
            || path_segment_is_instrument_category(name)
            || path_segment_is_pack_metadata(name)
            || path_segment_is_ephemeral_temp(name)
        {
            continue;
        }
        let formatted = format_path_segment_as_artist(name);
        if formatted.is_empty() {
            continue;
        }
        candidates.push(formatted);
    }

    match candidates.len() {
        0 => None,
        1 => Some(candidates[0].clone()),
        _ => Some(candidates[1].clone()),
    }
}

fn should_stop_artist_walk(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    std::env::temp_dir()
        .canonicalize()
        .ok()
        .and_then(|temp| path.canonicalize().ok().map(|canonical| canonical == temp))
        .unwrap_or(false)
}

fn path_segment_is_ephemeral_temp(name: &str) -> bool {
    name.split('_').any(|part| {
        part.len() >= 8 && part.chars().all(|ch| ch.is_ascii_digit())
    })
}

fn is_generic_path_segment(name: &str) -> bool {
    let norm = normalize_instrument_term(name);
    if norm.is_empty() {
        return true;
    }
    GENERIC_PATH_SEGMENTS
        .iter()
        .any(|segment| norm == normalize_instrument_term(segment))
}

fn path_segment_is_instrument_category(name: &str) -> bool {
    hint_name_tokens(name)
        .iter()
        .any(|token| hint_label_for_term(token).is_some())
}

fn path_segment_is_pack_metadata(name: &str) -> bool {
    let norm = normalize_instrument_term(name);
    norm.starts_with("vol") && norm.len() <= 8
        || norm.starts_with("volume")
        || (norm.starts_with("pt") && norm.len() <= 5)
        || norm.starts_with("part")
        || norm.starts_with("disc") && norm.len() <= 6
}

fn format_path_segment_as_artist(name: &str) -> String {
    let primary = name.split(" - ").next().unwrap_or(name).trim();
    primary
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn hint_name_tokens(name: &str) -> Vec<String> {
    let mut tokens = vec![normalize_instrument_term(name)];
    let mut current = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(normalize_instrument_term(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        tokens.push(normalize_instrument_term(&current));
    }
    tokens.retain(|token| !token.is_empty());
    tokens
}

fn hint_label_for_term(term: &str) -> Option<&'static str> {
    let needle = normalize_instrument_term(term);
    if needle.is_empty() {
        return None;
    }
    INSTRUMENT_ALIAS_GROUPS
        .iter()
        .zip(INSTRUMENT_HINT_LABELS.iter())
        .find(|(group, _)| {
            group.iter().any(|alias| {
                instrument_alias_matches(&needle, &normalize_instrument_term(alias))
            })
        })
        .map(|(_, label)| *label)
}

fn instrument_terms_related(query: &str, value: &str) -> bool {
    let query_groups = instrument_alias_groups_for(query);
    let value_groups = instrument_alias_groups_for(value);
    query_groups
        .iter()
        .any(|group| value_groups.iter().any(|other| std::ptr::eq(*group, *other)))
}

fn instrument_field_score(
    matcher: &SkimMatcherV2,
    value: &str,
    query: &str,
) -> Option<i64> {
    let terms = file_search_terms(query);
    if terms.is_empty() {
        return None;
    }
    let mut scores = Vec::with_capacity(terms.len());
    for term in &terms {
        scores.push(instrument_term_score(matcher, value, term)?);
    }
    Some(scores.into_iter().min().unwrap_or(0))
}

fn instrument_term_score(matcher: &SkimMatcherV2, value: &str, term: &str) -> Option<i64> {
    let direct = term_field_score(matcher, value, term, false);
    if direct > 0 {
        return Some(direct);
    }
    if instrument_terms_related(term, value) {
        return Some(80 + term.len() as i64);
    }
    instrument_search_terms(term)
        .iter()
        .filter_map(|alias| {
            let score = term_field_score(matcher, value, alias, false);
            if score > 0 {
                Some(score)
            } else if instrument_terms_related(alias, value) {
                Some(80 + alias.len() as i64)
            } else {
                None
            }
        })
        .max()
}

fn tag_value_score(matcher: &SkimMatcherV2, value: &str, query: &str) -> Option<i64> {
    let terms = file_search_terms(query);
    if terms.is_empty() {
        return None;
    }
    let mut scores = Vec::with_capacity(terms.len());
    for term in &terms {
        let score = term_field_score(matcher, value, term, false);
        if score <= 0 {
            return None;
        }
        scores.push(score);
    }
    Some(scores.into_iter().min().unwrap_or(0))
}

fn tag_field_score(
    matcher: &SkimMatcherV2,
    fields: &TagFields,
    filter: &TagFilter,
) -> Option<i64> {
    let value = fields.field_value(filter.field);
    if value.is_empty() {
        None
    } else if filter.field == TagField::Instrument {
        instrument_field_score(matcher, value, &filter.value)
    } else {
        tag_value_score(matcher, value, &filter.value)
    }
}

fn tag_match_score(
    matcher: &SkimMatcherV2,
    path: &Path,
    filters: &[TagFilter],
    lookup: &mut MetadataLookup,
    allow_disk_read: bool,
) -> i64 {
    if filters.is_empty() || !is_audio(path) {
        return 0;
    }
    let fields = lookup.tag_fields_for_search(path, allow_disk_read);
    let mut scores = Vec::with_capacity(filters.len());
    for filter in filters {
        let Some(score) = tag_field_score(matcher, &fields, filter) else {
            return 0;
        };
        scores.push(score);
    }
    scores.into_iter().min().unwrap_or(0)
}

fn collect_tag_matches(
    paths: &[PathBuf],
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
    allow_disk_read: bool,
) -> SearchResult {
    let tag_matcher = tag_search_matcher();
    let mut lookup = MetadataLookup::new(metadata);
    let mut matches = Vec::new();

    for path in paths.iter().filter(|path| is_audio(path)) {
        let tag_score = tag_match_score(
            &tag_matcher,
            path,
            tag_filters,
            &mut lookup,
            allow_disk_read,
        );
        if tag_score > 0 {
            matches.push(SearchRank::for_tag_search(path.clone(), tag_score));
        }
    }

    matches.sort();
    let paths = matches.into_iter().map(|entry| entry.path).collect();
    SearchResult {
        paths,
        new_metadata: lookup.into_new_entries(),
        cached_roots: HashMap::new(),
    }
}

pub fn tag_search_paths(
    paths: &[PathBuf],
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    collect_tag_matches(paths, tag_filters, metadata, true)
}

/// Tag-only search over the metadata index. Unindexed files are skipped so a
/// library-wide `instrument:` query does not re-parse every audio file.
pub fn tag_search_cached_paths(
    paths: &[PathBuf],
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    collect_tag_matches(paths, tag_filters, metadata, false)
}

pub fn search_paths(
    paths: &[PathBuf],
    file_query: &str,
    tag_filters: &[TagFilter],
    case_sensitive: bool,
    show_directories: bool,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    let file_active = file_query.len() >= FILE_SEARCH_MIN_QUERY_LEN;
    let tag_active = !tag_filters.is_empty();
    if tag_active && !file_active {
        return collect_tag_matches(paths, tag_filters, metadata, true);
    }

    collect_file_matches(
        paths,
        file_query,
        tag_filters,
        case_sensitive,
        show_directories,
        metadata,
    )
}

fn collect_file_matches(
    paths: &[PathBuf],
    file_query: &str,
    tag_filters: &[TagFilter],
    case_sensitive: bool,
    show_directories: bool,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    let file_matcher = file_search_matcher(case_sensitive);
    let tag_matcher = tag_search_matcher();
    let tag_active = !tag_filters.is_empty();
    let mut lookup = MetadataLookup::new(metadata);
    let mut matches = Vec::new();

    for path in paths {
        if !show_directories && !is_audio(path) {
            continue;
        }

        if matches.len() >= FILE_SEARCH_CONFIDENT_RESULT_CAP
            && !path_has_substring_match(path, file_query, case_sensitive)
        {
            continue;
        }

        let (name_score, path_score) =
            path_match_scores(&file_matcher, path, file_query, case_sensitive);

        if name_score == 0 && path_score == 0 {
            continue;
        }

        if matches.len() >= FILE_SEARCH_CONFIDENT_RESULT_CAP
            && !is_confident_file_match(name_score, path_score)
        {
            continue;
        }

        let tag_score = if tag_active {
            tag_match_score(&tag_matcher, path, tag_filters, &mut lookup, true)
        } else {
            0
        };

        if tag_active && tag_score == 0 {
            continue;
        }

        let rank = if tag_active {
            SearchRank::for_combined_search(
                path.clone(),
                file_query,
                name_score,
                path_score,
                tag_score,
                case_sensitive,
            )
        } else {
            SearchRank::for_file_search(
                path.clone(),
                file_query,
                name_score,
                path_score,
                case_sensitive,
            )
        };
        matches.push(rank);
    }

    matches.sort();
    if matches.len() > FILE_SEARCH_MAX_RESULTS {
        matches.truncate(FILE_SEARCH_MAX_RESULTS);
    }
    let paths = matches.into_iter().map(|entry| entry.path).collect();
    SearchResult {
        paths,
        new_metadata: lookup.into_new_entries(),
        cached_roots: HashMap::new(),
    }
}

pub fn instrument_tag(path: &Path) -> Option<String> {
    if !is_audio(path) {
        return None;
    }

    let native = read_native_tags(path).unwrap_or_default();
    durable_instrument(path, &native, None)
}

/// Returns `Ok(true)` when tags were written, `Ok(false)` when nothing was missing.
///
/// Native container tags are the primary store because they travel with the
/// file. When the container rejects the write the label is recorded in the
/// sidecar store instead, so search still finds the file.
pub fn write_auto_tags(path: &Path, instrument: &str) -> Result<bool, String> {
    if !is_audio(path) {
        return Err(format!("Not an audio file: {}", path.display()));
    }
    // An unreadable container has no tags to preserve and will refuse the
    // native write below, which routes the label to the sidecar store.
    let native = read_native_tags(path);
    let native_writable = native.is_some();
    let native = native.unwrap_or_default();
    let durable = durable_instrument(path, &native, None);
    let comment = native.comment.as_deref().unwrap_or_default();
    let status = AutoTagFieldStatus::from_parts(
        path,
        durable.as_deref().unwrap_or_default(),
        native.instrument.as_deref().unwrap_or_default(),
        native.artist.as_deref().unwrap_or_default(),
        comment,
        native_writable,
    );
    if status.is_complete() {
        return Ok(false);
    }

    let instrument = instrument.trim();
    if status.allows_instrument_work() && instrument.is_empty() {
        return Err("Instrument label cannot be empty".into());
    }

    let current = durable.as_deref().unwrap_or_default().trim();
    let instrument_changed = status.allows_instrument_work()
        && !instrument.is_empty()
        && !current.eq_ignore_ascii_case(instrument);
    let pending = NativeTags {
        instrument: instrument_changed.then(|| instrument.to_string()),
        artist: status
            .needs_artist
            .then(|| artist_hint_from_path(path))
            .flatten()
            .filter(|artist| !artist.is_empty()),
        comment: status
            .needs_comment
            .then(|| tundra_comment(native.comment.as_deref())),
    };

    if pending.is_empty() {
        return Ok(false);
    }

    match write_native_tags(path, &pending) {
        Ok(()) => Ok(true),
        Err(native_error) => {
            // Container refused the write (unsupported layout, unwritable file).
            // Record the label in the sidecar store so search still finds it.
            // Never stamp a sidecar over a native instrument: durable reads
            // prefer native, and a sidecar vN would skip retag forever.
            if let Some(instrument) = &pending.instrument {
                if native.instrument.as_ref().is_none_or(|value| value.trim().is_empty()) {
                    crate::tag_store::set_instrument(path, instrument, TUNDRA_TAG_VERSION)
                        .map_err(|store_error| format!("{native_error} (sidecar: {store_error})"))?;
                    return Ok(true);
                }
                return Err(native_error);
            }
            // The instrument is already covered, and the remaining fields have
            // nowhere to go in a container this broken. Report the file as done
            // rather than failing it again on every future scan.
            if durable.is_some() && !status.can_retag_instrument {
                return Ok(false);
            }
            Err(native_error)
        }
    }
}

pub fn refresh_cached_metadata(path: &Path) -> Option<CachedMetadata> {
    let mtime_secs = file_mtime_secs(path)?;
    let fields = read_tag_fields(path)?;
    Some(CachedMetadata {
        mtime_secs,
        fields,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_search_matches_snare_in_filename() {
        let matcher = file_search_matcher(false);
        let path = PathBuf::from(r"C:\Samples\Snare Drum 01.wav");
        let (name_score, path_score) = path_match_scores(&matcher, &path, "snare", false);
        assert!(name_score > 0 || path_score > 0, "expected snare to match filename");
    }

    #[test]
    fn file_search_requires_all_terms() {
        let matcher = file_search_matcher(false);
        let path = PathBuf::from(r"C:\Samples\Snare Drum 01.wav");
        let (both, _) = path_match_scores(&matcher, &path, "snare drum", false);
        let (snare_only, _) = path_match_scores(&matcher, &path, "snare", false);
        let (missing, _) = path_match_scores(&matcher, &path, "snare kick", false);
        assert!(both > 0);
        assert!(snare_only > 0);
        assert_eq!(missing, 0);
    }

    #[test]
    fn file_search_rejects_weak_fuzzy_matches() {
        let matcher = file_search_matcher(false);
        let path = PathBuf::from(r"C:\Samples\Synth Pad 01.wav");
        let (weak, _) = path_match_scores(&matcher, &path, "snare", false);
        assert_eq!(weak, 0, "unrelated fuzzy matches should be rejected");
    }

    #[test]
    fn file_search_confident_cap_skips_fuzzy_only_matches() {
        let paths = vec![
            PathBuf::from(r"C:\Samples\01 Snare.wav"),
            PathBuf::from(r"C:\Samples\Synth Pad 01.wav"),
        ];
        let confident: Vec<_> = (0..FILE_SEARCH_CONFIDENT_RESULT_CAP)
            .map(|index| PathBuf::from(format!(r"C:\Samples\snare-{index:04}.wav")))
            .collect();
        let mut all_paths = confident;
        all_paths.extend(paths);

        let result = collect_file_matches(
            &all_paths,
            "snare",
            &[],
            false,
            true,
            Arc::new(HashMap::new()),
        );

        assert!(
            result.paths.iter().any(|path| path.ends_with("01 Snare.wav")),
            "substring matches should remain"
        );
        assert!(
            !result
                .paths
                .iter()
                .any(|path| path.ends_with("Synth Pad 01.wav")),
            "fuzzy-only matches should be dropped once the confident cap is full"
        );
    }

    #[test]
    fn tag_only_search_matches_metadata_on_audio_files() {
        let dir = std::env::temp_dir().join("tundra_tag_search_test");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let mtime_secs = file_mtime_secs(&audio).expect("temp file mtime");
        let paths = vec![dir.join("nested"), audio.clone()];
        let mut cache = HashMap::new();
        cache.insert(
            audio.clone(),
            CachedMetadata {
                mtime_secs,
                fields: TagFields {
                    bpm: "120".into(),
                    ..TagFields::default()
                },
            },
        );
        let filters = vec![TagFilter {
            field: TagField::Bpm,
            value: "120".into(),
        }];
        let result = collect_tag_matches(&paths, &filters, Arc::new(cache), true);
        assert_eq!(result.paths, vec![audio.clone()]);
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn tag_match_requires_every_filter() {
        let dir = std::env::temp_dir().join("tundra_tag_match_all_test");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let mtime_secs = file_mtime_secs(&audio).expect("temp file mtime");
        let paths = vec![audio.clone()];
        let mut cache = HashMap::new();
        cache.insert(
            audio.clone(),
            CachedMetadata {
                mtime_secs,
                fields: TagFields {
                    bpm: "120".into(),
                    key: "Am".into(),
                    ..TagFields::default()
                },
            },
        );
        let filters = vec![
            TagFilter {
                field: TagField::Bpm,
                value: "120".into(),
            },
            TagFilter {
                field: TagField::Key,
                value: "Bm".into(),
            },
        ];
        let result = collect_tag_matches(&paths, &filters, Arc::new(cache), true);
        assert!(result.paths.is_empty(), "partial tag matches should be rejected");
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn instrument_filter_uses_explicit_instrument() {
        let fields = TagFields {
            explicit_instrument: "Snare Drum".into(),
            instrument: "Drums".into(),
            ..TagFields::default()
        };
        let matcher = tag_search_matcher();
        let filter = TagFilter {
            field: TagField::Instrument,
            value: "snare".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &filter).is_some(),
            "explicit instrument should satisfy instrument filter"
        );
    }

    #[test]
    fn format_control_bar_tags_shows_instrument_bpm_and_key() {
        let fields = TagFields {
            explicit_instrument: "Snare".into(),
            bpm: "120".into(),
            key: "Am".into(),
            ..TagFields::default()
        };
        assert_eq!(
            control_bar_tags(&fields),
            vec![
                (TagField::Instrument, "Snare".into()),
                (TagField::Bpm, "120".into()),
                (TagField::Key, "Am".into()),
            ]
        );
        let summary = format_control_bar_tags(&fields).expect("tag summary");
        assert!(summary.contains("Instrument: Snare"));
        assert!(summary.contains("BPM: 120"));
        assert!(summary.contains("Key: Am"));
    }

    #[test]
    fn tag_filter_fuzzy_matches_partial_values() {
        let matcher = tag_search_matcher();
        let fields = TagFields {
            bpm: "120.00".into(),
            ..TagFields::default()
        };
        let filter = TagFilter {
            field: TagField::Bpm,
            value: "120".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &filter).is_some(),
            "tag filters should fuzzy-match partial field values"
        );
    }

    #[test]
    fn tag_filter_fuzzy_matches_multi_word_values() {
        let matcher = tag_search_matcher();
        let fields = TagFields {
            comment: "dark snare loop".into(),
            ..TagFields::default()
        };
        let filter = TagFilter {
            field: TagField::Comment,
            value: "snare loop".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &filter).is_some(),
            "multi-word tag filters should require every term to match"
        );
        let miss = TagFilter {
            field: TagField::Comment,
            value: "snare kick".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &miss).is_none(),
            "multi-word tag filters should reject partial term matches"
        );
    }

    #[test]
    fn tundra_tagged_status_allows_retag_but_preserves_user_tags() {
        let dir = unique_temp_dir("tundra_retag_status");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        write_minimal_wav(&audio);
        assert!(write_auto_tags(&audio, "Snare").expect("tundra write"));

        let status = auto_tag_field_status(&audio).expect("status");
        assert!(!status.needs_instrument);
        assert!(!status.can_retag_instrument);
        assert!(tundra_tag_is_current(&audio, riff_info(&audio).get(WAV_COMMENT_KEY).unwrap_or("")));

        let user_dir = unique_temp_dir("tundra_user_tag");
        std::fs::create_dir_all(&user_dir).expect("temp dir");
        let user_audio = user_dir.join("snare.wav");
        write_minimal_wav(&user_audio);
        {
            use lofty::config::WriteOptions;
            use lofty::file::AudioFile;
            use lofty::iff::wav::RiffInfoList;

            let mut wav = {
                let mut file = std::fs::File::open(&user_audio).expect("open");
                lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse")
            };
            let mut info = RiffInfoList::new();
            info.insert(WAV_INSTRUMENT_KEY.to_string(), "Snare".to_string());
            info.insert(WAV_COMMENT_KEY.to_string(), "Recorded live".to_string());
            wav.set_riff_info(info);
            wav.save_to_path(&user_audio, WriteOptions::default())
                .expect("save user tags");
        }

        let user_status = auto_tag_field_status(&user_audio).expect("user status");
        assert!(!user_status.needs_instrument);
        assert!(!user_status.can_retag_instrument);
        assert!(
            !write_auto_tags(&user_audio, "Kick").expect("user tag write attempt"),
            "user-owned instrument tags must not be overwritten"
        );
        assert_eq!(instrument_tag(&user_audio).as_deref(), Some("Snare"));

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(user_dir);
    }

    #[test]
    fn tag_fields_ignore_cache_when_file_missing() {
        let path = PathBuf::from(r"C:\missing\tundra-kick.wav");
        let mut cache = HashMap::new();
        cache.insert(
            path.clone(),
            CachedMetadata {
                mtime_secs: 1,
                fields: TagFields {
                    bpm: "120".into(),
                    ..TagFields::default()
                },
            },
        );
        let mut lookup = MetadataLookup::new(Arc::new(cache));
        assert!(lookup.tag_fields(&path).bpm.is_empty());
    }

    #[test]
    fn metadata_lookup_matches_cache_key_variants() {
        let dir = unique_temp_dir("tundra_lookup_keys");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let mtime_secs = file_mtime_secs(&audio).expect("temp file mtime");
        let key = crate::path_util::cache_key(audio.clone());
        let mut cache = HashMap::new();
        cache.insert(
            key,
            CachedMetadata {
                mtime_secs,
                fields: TagFields {
                    explicit_instrument: "Snare".into(),
                    instrument: "Snare".into(),
                    ..TagFields::default()
                },
            },
        );
        let metadata = Arc::new(cache);
        let mut lookup = MetadataLookup::new(Arc::clone(&metadata));
        assert_eq!(lookup.tag_fields(&audio).explicit_instrument, "Snare");

        let hits = tag_search_cached_paths(
            std::slice::from_ref(&audio),
            &[TagFilter {
                field: TagField::Instrument,
                value: "snare".to_string(),
            }],
            metadata,
        );
        assert_eq!(hits.paths, vec![audio.clone()]);
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn cached_tag_search_skips_unindexed_files() {
        let dir = unique_temp_dir("tundra_cached_skip");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let hits = tag_search_cached_paths(
            std::slice::from_ref(&audio),
            &[TagFilter {
                field: TagField::Instrument,
                value: "snare".to_string(),
            }],
            Arc::new(HashMap::new()),
        );
        assert!(
            hits.paths.is_empty(),
            "tag-only search must not parse unindexed files"
        );
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn file_search_debounce_is_longer_for_two_chars() {
        assert_eq!(file_search_debounce_ms(2), FILE_SEARCH_DEBOUNCE_MS_SHORT);
        assert_eq!(file_search_debounce_ms(3), FILE_SEARCH_DEBOUNCE_MS);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    fn write_minimal_wav(path: &Path) {
        write_wav_chunks(
            path,
            vec![
                (*b"fmt ", wav_pcm_fmt_chunk()),
                (*b"data", vec![0; 512]),
            ],
        );
    }

    fn wav_pcm_fmt_chunk() -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&44100u32.to_le_bytes());
        fmt.extend_from_slice(&88200u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt
    }

    fn write_wav_chunks(path: &Path, chunks: Vec<([u8; 4], Vec<u8>)>) {
        std::fs::write(path, encode_riff_wave(&chunks)).unwrap();
    }

    fn wav_chunk<'a>(chunks: &'a [([u8; 4], Vec<u8>)], id: &[u8; 4]) -> Option<&'a [u8]> {
        chunks
            .iter()
            .find(|(found, _)| found == id)
            .map(|(_, data)| data.as_slice())
    }

    fn wav_list_chunk<'a>(
        chunks: &'a [([u8; 4], Vec<u8>)],
        form: &[u8; 4],
    ) -> Option<&'a [u8]> {
        chunks.iter().find_map(|(id, data)| {
            (id == b"LIST" && data.len() >= 4 && &data[..4] == form).then_some(data.as_slice())
        })
    }

    #[test]
    fn write_auto_tags_preserves_wav_non_info_chunks() {
        let dir = unique_temp_dir("tundra_wav_chunk_preserve");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        let smpl = vec![0x11; 60];
        let cue = vec![0x22; 28];
        let inst = vec![0x33; 8];
        let acid = vec![0x44; 24];
        let adtl = {
            let mut data = Vec::from(*b"adtl");
            data.extend_from_slice(b"labl");
            data.extend_from_slice(&8u32.to_le_bytes());
            data.extend_from_slice(&1u32.to_le_bytes());
            data.extend_from_slice(b"cue\0");
            data
        };
        write_wav_chunks(
            &audio,
            vec![
                (*b"fmt ", wav_pcm_fmt_chunk()),
                (*b"data", vec![0; 512]),
                (*b"smpl", smpl.clone()),
                (*b"cue ", cue.clone()),
                (*b"inst", inst.clone()),
                (*b"acid", acid.clone()),
                (*b"LIST", adtl.clone()),
            ],
        );

        write_auto_tags(&audio, "Kick").expect("tag wav");

        let bytes = std::fs::read(&audio).expect("read tagged wav");
        let chunks = parse_riff_wave_chunks(&bytes).expect("parse tagged wav");
        assert_eq!(wav_chunk(&chunks, b"smpl"), Some(smpl.as_slice()));
        assert_eq!(wav_chunk(&chunks, b"cue "), Some(cue.as_slice()));
        assert_eq!(wav_chunk(&chunks, b"inst"), Some(inst.as_slice()));
        assert_eq!(wav_chunk(&chunks, b"acid"), Some(acid.as_slice()));
        assert_eq!(wav_list_chunk(&chunks, b"adtl"), Some(adtl.as_slice()));
        assert!(
            wav_list_chunk(&chunks, b"INFO").is_some(),
            "LIST INFO must be written without replacing adtl"
        );
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_instrument_tag_sets_wav_riff_info_for_explorer() {
        use lofty::file::TaggedFileExt;
        use lofty::tag::TagType;

        let dir = unique_temp_dir("tundra_wav_instrument_tag");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);

        write_auto_tags(&audio, "Kick").expect("write instrument");

        let bytes = std::fs::read(&audio).expect("read tagged wav bytes");
        assert!(
            bytes.windows(4).any(|chunk| chunk == b"LIST" || chunk == b"id3 " || chunk == b"ID3"),
            "no tag chunks written, len={}, head={:02x?}",
            bytes.len(),
            &bytes[..bytes.len().min(80)]
        );

        let tagged = lofty::read_from_path(&audio).expect("re-read wav");
        let types: Vec<_> = tagged.tags().iter().map(|tag| tag.tag_type()).collect();
        let ascii: String = bytes
            .iter()
            .map(|b| if b.is_ascii_graphic() { *b as char } else { '.' })
            .collect();
        assert!(
            tagged.tag(TagType::RiffInfo).is_some(),
            "expected RIFF INFO, found tags {types:?}, instrument={:?}, ascii={ascii}",
            instrument_tag(&audio)
        );
        let riff = tagged
            .tag(TagType::RiffInfo)
            .expect("RIFF INFO tag for Windows Explorer");
        assert!(
            riff.genre().is_none() || riff.genre().is_some_and(|genre| genre.trim().is_empty()),
            "instrument must not be written to Genre, got {:?}",
            riff.genre()
        );
        let comment = riff.comment().expect("Comments field");
        assert!(
            comment.contains("Tundra v"),
            "Comments should credit Tundra, got {comment:?}"
        );
        assert!(
            !comment.contains("INSTRUMENT:"),
            "instrument must not be stored in Comments, got {comment:?}"
        );
        assert_eq!(
            riff_info(&audio).get(WAV_INSTRUMENT_KEY).map(str::to_string),
            Some("Kick".to_string()),
            "instrument should be stored in RIFF IKEY"
        );
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
        assert!(
            !bytes.windows(3).any(|chunk| chunk == b"ID3"),
            "WAV should not get an ID3 payload that hides LIST INFO from Explorer"
        );

        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn write_instrument_tag_replaces_existing_wav_comment() {
        use lofty::config::WriteOptions;
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::probe::Probe;
        use lofty::tag::{Accessor, Tag, TagType};

        let dir = unique_temp_dir("tundra_wav_existing_comment");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);

        let mut tagged = Probe::open(&audio)
            .expect("open wav")
            .read()
            .expect("read wav");
        tagged.insert_tag(Tag::new(TagType::RiffInfo));
        tagged
            .tag_mut(TagType::RiffInfo)
            .expect("RIFF INFO")
            .set_comment("already had a note".to_string());
        tagged
            .save_to_path(&audio, WriteOptions::default())
            .expect("save user comment");

        write_auto_tags(&audio, "Kick").expect("write instrument");
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
        let tagged = lofty::read_from_path(&audio).expect("re-read wav");
        let comment = tagged
            .tag(TagType::RiffInfo)
            .and_then(|tag| tag.comment())
            .expect("Comments field");
        assert!(
            comment.contains("already had a note"),
            "user comment should be preserved, got {comment:?}"
        );
        assert!(
            !comment.contains("Tundra v"),
            "custom user comments should not be replaced, got {comment:?}"
        );
        assert!(
            !comment.contains("INSTRUMENT:"),
            "instrument must not be stored in Comments, got {comment:?}"
        );
        assert_eq!(
            riff_info(&audio).get(WAV_INSTRUMENT_KEY).map(str::to_string),
            Some("Kick".to_string()),
            "instrument should be stored in RIFF IKEY"
        );

        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn instrument_hint_reads_folder_and_prefers_filename() {
        let folder_hint = instrument_hint_from_path(Path::new(r"F:\Samples\ADM Samples - Copy\snares\tight_01.wav"));
        assert_eq!(folder_hint.as_deref(), Some("Snare"));

        let file_hint = instrument_hint_from_path(Path::new(r"F:\Samples\snares\cymbal_roll.wav"));
        assert_eq!(file_hint.as_deref(), Some("Cymbal"));

        let kick_hint = instrument_hint_from_path(Path::new(r"C:\Drums\Kicks\808_kick_01.wav"));
        assert_eq!(kick_hint.as_deref(), Some("Kick"));

        assert_eq!(
            instrument_hint_from_path(Path::new(r"C:\Drums\808_hat.wav")).as_deref(),
            Some("Hi-Hat")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"C:\Drums\snow_01.wav")),
            None
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"C:\hats\tight_01.wav")).as_deref(),
            Some("Hi-Hat")
        );

        let deep_hint = instrument_hint_from_path(Path::new(
            r"F:\Libraries\Pack A\Drums\One Shots\Snares\tight_01.wav",
        ));
        assert_eq!(deep_hint.as_deref(), Some("Snare"));

        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Bongo\hit_01.wav")).as_deref(),
            Some("Percussion")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Bongos\layer.wav")).as_deref(),
            Some("Percussion")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\ADM\Perc\tight.wav")).as_deref(),
            Some("Percussion")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Perc\01.wav")).as_deref(),
            Some("Percussion"),
            "Perc must not false-match Kick"
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Bass\low.wav")).as_deref(),
            Some("Bass"),
            "Bass must not false-match Kick via bassdrum"
        );
    }

    #[test]
    fn bass_is_not_related_to_kick() {
        assert!(!instruments_related("Bass", "Kick"));
        assert!(!instruments_related("bass", "bassdrum"));
        assert!(!instruments_related("shot", "Rim"));
        assert!(instruments_related("Hat", "Hi-Hat"));
        assert!(instruments_related("kick", "kickdrum"));
    }

    #[test]
    fn artist_hint_reads_label_from_directory_layout() {
        assert_eq!(
            artist_hint_from_path(Path::new(r"F:\Samples\KSHMR\Vol4\Kicks\kick.wav")).as_deref(),
            Some("KSHMR")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"F:\Samples\KSHMR\Kicks\kick.wav")).as_deref(),
            Some("KSHMR")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"D:\Splice\packs\deadmau5\kick.wav")).as_deref(),
            Some("deadmau5")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"F:\Samples\Native Instruments\Battery 4\Snares\snare.wav"))
                .as_deref(),
            Some("Native Instruments")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"C:\Samples\snares\tight_01.wav")),
            None
        );
    }

    #[test]
    fn read_tag_fields_uses_artist_hint_when_file_is_untagged() {
        let dir = unique_temp_dir("tundra_artist_hint_read");
        let _ = std::fs::create_dir_all(dir.join("KSHMR").join("Kicks"));
        let audio = dir.join("KSHMR").join("Kicks").join("kick.wav");
        write_minimal_wav(&audio);

        let fields = read_tag_fields(&audio).expect("read tag fields");
        assert_eq!(fields.artist, "KSHMR");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_instrument_tag_sets_wav_artist_from_directory() {
        let dir = unique_temp_dir("tundra_wav_artist_tag");
        let _ = std::fs::create_dir_all(dir.join("KSHMR").join("Kicks"));
        let audio = dir.join("KSHMR").join("Kicks").join("kick.wav");
        write_minimal_wav(&audio);

        write_auto_tags(&audio, "Kick").expect("write instrument");

        assert_eq!(
            riff_info(&audio).get(WAV_ARTIST_KEY).map(str::to_string),
            Some("KSHMR".to_string())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_auto_tags_extends_file_with_existing_non_auto_tags() {
        use lofty::config::WriteOptions;
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::probe::Probe;
        use lofty::tag::{Accessor, Tag, TagType};

        let dir = unique_temp_dir("tundra_extend_existing_tags");
        let _ = std::fs::create_dir_all(dir.join("KSHMR").join("Kicks"));
        let audio = dir.join("KSHMR").join("Kicks").join("kick.wav");
        write_minimal_wav(&audio);

        let mut tagged = Probe::open(&audio)
            .expect("open wav")
            .read()
            .expect("read wav");
        tagged.insert_tag(Tag::new(TagType::RiffInfo));
        tagged
            .tag_mut(TagType::RiffInfo)
            .expect("RIFF INFO")
            .set_genre("Drums".to_string());
        tagged
            .save_to_path(&audio, WriteOptions::default())
            .expect("save genre");

        write_auto_tags(&audio, "Kick").expect("extend tags");

        let riff_info = riff_info(&audio);
        assert_eq!(
            riff_info.get(WAV_INSTRUMENT_KEY).map(str::to_string),
            Some("Kick".to_string())
        );
        assert_eq!(riff_info.get("IGNR").map(str::to_string), Some("Drums".to_string()));
        assert_eq!(
            riff_info.get(WAV_ARTIST_KEY).map(str::to_string),
            Some("KSHMR".to_string())
        );
        assert!(
            riff_info
                .get(WAV_COMMENT_KEY)
                .is_some_and(|comment| comment.contains("Tundra v"))
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_auto_tags_sets_flac_instrument_field_not_comment() {
        let audio = staged_fixture("flac", "tundra_flac_instrument_field");

        assert!(write_auto_tags(&audio, "Kick").expect("write flac tags"));

        let vorbis = vorbis_comments(&audio);
        assert_eq!(
            vorbis.get(VORBIS_INSTRUMENT_KEY).map(str::to_string),
            Some("Kick".to_string())
        );
        assert_ne!(
            vorbis.get(VORBIS_COMMENT_KEY).map(str::to_string),
            Some("Kick".to_string()),
            "instrument must not land in the comment field"
        );
        assert_eq!(
            vorbis.get(VORBIS_COMMENT_KEY).map(str::to_string),
            Some(format!("Tundra v{TUNDRA_TAG_VERSION}"))
        );

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    #[test]
    fn write_auto_tags_skips_retag_when_tag_version_is_current() {
        let dir = unique_temp_dir("tundra_retag_auto_tags");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);

        assert!(write_auto_tags(&audio, "Kick").expect("initial write"));
        assert!(
            !write_auto_tags(&audio, "Snare").expect("same-version retag should no-op"),
            "current-version tags must not be replaced by auto tag"
        );
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
        assert!(
            !write_auto_tags(&audio, "Kick").expect("same label should no-op"),
            "unchanged instrument should not rewrite the file"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_tundra_comment_is_eligible_for_upgrade() {
        let dir = unique_temp_dir("tundra_legacy_comment");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        write_minimal_wav(&audio);
        {
            use lofty::config::WriteOptions;
            use lofty::file::AudioFile;
            use lofty::iff::wav::RiffInfoList;

            let mut wav = {
                let mut file = std::fs::File::open(&audio).expect("open");
                lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse")
            };
            let mut info = RiffInfoList::new();
            info.insert(WAV_INSTRUMENT_KEY.to_string(), "Snare".to_string());
            info.insert(WAV_COMMENT_KEY.to_string(), "Tundra".to_string());
            wav.set_riff_info(info);
            wav.save_to_path(&audio, WriteOptions::default())
                .expect("save legacy tags");
        }

        let status = auto_tag_field_status(&audio).expect("status");
        assert!(!status.needs_instrument);
        assert!(status.needs_comment);
        assert!(status.can_retag_instrument);

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Copies a real encoder-produced fixture into a scratch directory. Named
    /// `sample.<ext>` so no instrument can be inferred from the filename.
    fn staged_fixture(ext: &str, label: &str) -> PathBuf {
        let dir = unique_temp_dir(label);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let audio = dir.join(format!("sample.{ext}"));
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/assets")
            .join(format!("tone.{ext}"));
        std::fs::copy(&fixture, &audio)
            .unwrap_or_else(|err| panic!("copy {}: {err}", fixture.display()));
        audio
    }

    fn riff_info(path: &Path) -> lofty::iff::wav::RiffInfoList {
        use lofty::config::ParseOptions;
        use lofty::file::AudioFile;

        let mut file = std::fs::File::open(path).expect("open wav");
        lofty::iff::wav::WavFile::read_from(&mut file, ParseOptions::new())
            .expect("parse wav")
            .remove_riff_info()
            .expect("wav should carry a RIFF INFO list")
    }

    fn vorbis_comments(path: &Path) -> VorbisComments {
        use lofty::config::ParseOptions;
        use lofty::file::AudioFile;

        let mut file = std::fs::File::open(path).expect("open flac");
        lofty::flac::FlacFile::read_from(&mut file, ParseOptions::new())
            .expect("parse flac")
            .remove_vorbis_comments()
            .expect("flac should carry vorbis comments")
    }

    fn finds_by_instrument(path: &Path, query: &str) -> bool {
        tag_search_paths(
            std::slice::from_ref(&path.to_path_buf()),
            &[TagFilter {
                field: TagField::Instrument,
                value: query.to_string(),
            }],
            Arc::new(HashMap::new()),
        )
        .paths
        .iter()
        .any(|hit| hit == path)
    }

    /// The end-to-end contract: auto-tagging a file of any supported format
    /// makes `instrument:Kick` find it, and the label reads back from the
    /// container so third-party taggers and re-scans agree.
    #[test]
    fn instrument_round_trips_and_is_searchable_for_every_format() {
        for ext in ["wav", "flac", "mp3", "ogg"] {
            let audio = staged_fixture(ext, &format!("tundra_round_trip_{ext}"));

            assert!(
                write_auto_tags(&audio, "Kick").unwrap_or_else(|err| panic!("{ext}: {err}")),
                "{ext}: first write should tag the file"
            );
            assert_eq!(
                instrument_tag(&audio).as_deref(),
                Some("Kick"),
                "{ext}: instrument must read back from the container"
            );
            assert_eq!(
                read_tag_fields(&audio)
                    .map(|fields| fields.explicit_instrument)
                    .as_deref(),
                Some("Kick"),
                "{ext}: instrument must reach the searchable field"
            );
            assert!(
                finds_by_instrument(&audio, "Kick"),
                "{ext}: instrument:Kick must match the tagged file"
            );
            assert!(
                !finds_by_instrument(&audio, "Snare"),
                "{ext}: instrument:Snare must not match a kick"
            );
            assert!(
                !write_auto_tags(&audio, "Kick").unwrap_or_else(|err| panic!("{ext}: {err}")),
                "{ext}: a tagged file should report no further work"
            );
            assert!(
                !auto_tag_field_status(&audio)
                    .unwrap_or_else(|| panic!("{ext}: status"))
                    .needs_instrument,
                "{ext}: tagged file must not be queued for re-tagging"
            );

            let _ = std::fs::remove_dir_all(audio.parent().unwrap());
        }
    }

    /// Auto-tagging must never destroy artwork the user already had.
    #[test]
    fn tagging_preserves_embedded_cover_art() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;
        use lofty::picture::{MimeType, Picture, PictureType};

        let art: Vec<u8> = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xde, 0xad, 0xbe, 0xef,
        ];

        let audio = staged_fixture("mp3", "tundra_cover_art");
        {
            let mut mp3 = {
                let mut file = std::fs::File::open(&audio).expect("open mp3");
                lofty::mpeg::MpegFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse mp3")
            };
            let mut id3 = mp3.remove_id3v2().unwrap_or_default();
            id3.insert_picture(
                Picture::unchecked(art.clone())
                    .pic_type(PictureType::CoverFront)
                    .mime_type(MimeType::Png)
                    .build(),
            );
            mp3.set_id3v2(id3);
            mp3.save_to_path(&audio, WriteOptions::default())
                .expect("save art");
        }

        assert!(write_auto_tags(&audio, "Kick").expect("tag"));

        let mut file = std::fs::File::open(&audio).expect("reopen mp3");
        let mut mp3 =
            lofty::mpeg::MpegFile::read_from(&mut file, lofty::config::ParseOptions::new())
                .expect("reparse mp3");
        let tag = mp3
            .remove_id3v2()
            .map(Tag::from)
            .expect("mp3 should still have an ID3v2 tag");
        assert_eq!(
            tag.pictures().len(),
            1,
            "cover art must survive auto-tagging, found {} pictures",
            tag.pictures().len()
        );
        assert_eq!(tag.pictures()[0].data(), art.as_slice());

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    /// Tags a file already carries are a hint source, so a file whose path says
    /// nothing still gets classified from what the tagger before us recorded.
    #[test]
    fn instrument_hint_falls_back_to_existing_tags() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;

        let audio = staged_fixture("wav", "tundra_tag_hint");
        assert_eq!(
            instrument_hint_from_path(&audio),
            None,
            "fixture path must not name an instrument, or this proves nothing"
        );

        let mut wav = {
            let mut file = std::fs::File::open(&audio).expect("open wav");
            lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                .expect("parse wav")
        };
        let mut info = wav.remove_riff_info().unwrap_or_default();
        info.insert("IGNR".to_string(), "Kick".to_string());
        wav.set_riff_info(info);
        wav.save_to_path(&audio, WriteOptions::default())
            .expect("save genre");

        assert_eq!(
            instrument_hint(&audio),
            Some(("Kick".to_string(), HintSource::Tags)),
            "an existing genre of Kick should hint the instrument"
        );

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    /// Files an older build tagged into the grouping field carry no canonical
    /// instrument, so the grouping has to survive as a hint and get rewritten
    /// to the canonical key.
    #[test]
    fn legacy_grouping_hints_instrument_and_is_rewritten_canonically() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;

        let audio = staged_fixture("ogg", "tundra_legacy_grouping");
        {
            let mut ogg = {
                let mut file = std::fs::File::open(&audio).expect("open ogg");
                lofty::ogg::VorbisFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse ogg")
            };
            ogg.vorbis_comments_mut()
                .insert("GROUPING".to_string(), "Kick".to_string());
            ogg.save_to_path(&audio, WriteOptions::default())
                .expect("save grouping");
        }

        assert_eq!(
            instrument_hint(&audio),
            Some(("Kick".to_string(), HintSource::Tags)),
            "a legacy grouping should still hint the instrument"
        );
        assert!(
            auto_tag_field_status(&audio)
                .expect("status")
                .needs_instrument,
            "a grouping is not the canonical key, so the file still needs tagging"
        );

        assert!(write_auto_tags(&audio, "Kick").expect("rewrite canonically"));
        assert_eq!(
            instrument_tag(&audio).as_deref(),
            Some("Kick"),
            "instrument should now read from the canonical INSTRUMENT key"
        );
        assert!(finds_by_instrument(&audio, "Kick"));

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    /// A container that cannot hold a native tag must not lose the label: it
    /// goes to the SQLite sidecar, and search still finds the file.
    #[test]
    fn unwritable_container_falls_back_to_sidecar_store_and_stays_searchable() {
        let dir = unique_temp_dir("tundra_sidecar_fallback");
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        // Audio extension, but not a container any writer can parse.
        let audio = dir.join("sample.wav");
        std::fs::write(&audio, b"not actually a RIFF container").expect("write junk");

        assert!(
            write_auto_tags(&audio, "Kick").expect("unwritable container should fall back"),
            "fallback should report the tag as written"
        );
        assert_eq!(
            crate::tag_store::instrument(&audio).as_deref(),
            Some("Kick"),
            "sidecar store should hold the label the container refused"
        );
        assert_eq!(
            instrument_tag(&audio).as_deref(),
            Some("Kick"),
            "sidecar label must surface through the normal instrument read"
        );
        assert!(
            finds_by_instrument(&audio, "Kick"),
            "instrument:Kick must match a sidecar-tagged file"
        );
        assert_eq!(
            crate::tag_store::tag_version(&audio),
            Some(TUNDRA_TAG_VERSION)
        );
        assert!(
            !write_auto_tags(&audio, "Snare").expect("same-version sidecar retag"),
            "current-version sidecar tags must not be replaced"
        );
        assert_eq!(
            crate::tag_store::instrument(&audio).as_deref(),
            Some("Kick")
        );
        assert_eq!(
            write_auto_tags(&audio, "Kick"),
            Ok(false),
            "unchanged sidecar label should no-op"
        );
        assert!(
            !auto_tag_field_status(&audio)
                .expect("status")
                .can_retag_instrument,
            "current sidecar version should skip auto tag"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sidecar_row_does_not_own_user_native_instrument() {
        let dir = unique_temp_dir("tundra_sidecar_user_native");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        write_minimal_wav(&audio);
        {
            use lofty::config::WriteOptions;
            use lofty::file::AudioFile;
            use lofty::iff::wav::RiffInfoList;

            let mut wav = {
                let mut file = std::fs::File::open(&audio).expect("open");
                lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse")
            };
            let mut info = RiffInfoList::new();
            info.insert(WAV_INSTRUMENT_KEY.to_string(), "Snare".to_string());
            info.insert(WAV_COMMENT_KEY.to_string(), "Recorded live".to_string());
            wav.set_riff_info(info);
            wav.save_to_path(&audio, WriteOptions::default())
                .expect("save user tags");
        }

        crate::tag_store::set_instrument(&audio, "Kick", 0).expect("stale sidecar");

        let status = auto_tag_field_status(&audio).expect("status");
        assert!(!status.can_retag_instrument);
        assert!(
            !write_auto_tags(&audio, "Kick").expect("user tag write attempt"),
            "sidecar must not unlock overwrite of a native user instrument"
        );
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Snare"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn instrument_bass_search_does_not_match_kick() {
        let dir = unique_temp_dir("tundra_bass_not_kick");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);
        assert!(write_auto_tags(&audio, "Kick").expect("tag kick"));
        assert!(finds_by_instrument(&audio, "Kick"));
        assert!(
            !finds_by_instrument(&audio, "Bass"),
            "instrument:bass must not match a Kick via bassdrum"
        );

        let _ = std::fs::remove_dir_all(dir);
    }
}
