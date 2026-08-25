use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use lofty::file::TaggedFileExt;
use lofty::read_from_path;
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
}

impl TagFields {
    pub fn field_value(&self, field: TagField) -> &str {
        match field {
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

    fn cached_fields(&self, path: &Path, mtime_secs: u64) -> Option<TagFields> {
        let cached = self
            .new_entries
            .get(path)
            .or_else(|| self.cache.get(path))?;
        (cached.mtime_secs == mtime_secs).then(|| cached.fields.clone())
    }

    fn store_fields(&mut self, path: &Path, mtime_secs: u64, fields: TagFields) -> TagFields {
        let fields_clone = fields.clone();
        self.new_entries.insert(
            path.to_path_buf(),
            CachedMetadata {
                mtime_secs,
                fields,
            },
        );
        fields_clone
    }

    pub fn tag_fields(&mut self, path: &Path) -> TagFields {
        if let Some(mtime_secs) = file_mtime_secs(path) {
            if let Some(fields) = self.cached_fields(path, mtime_secs) {
                return fields;
            }
            let Some(fields) = read_tag_fields(path) else {
                return TagFields::default();
            };
            return self.store_fields(path, mtime_secs, fields);
        }

        if let Some(cached) = self.new_entries.get(path) {
            return cached.fields.clone();
        }

        let Some(fields) = read_tag_fields(path) else {
            return TagFields::default();
        };
        self.new_entries.insert(
            path.to_path_buf(),
            CachedMetadata {
                mtime_secs: 0,
                fields: fields.clone(),
            },
        );
        fields
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
    None
}

/// Returns `None` when metadata cannot be read (do not cache).
pub fn read_tag_fields(path: &Path) -> Option<TagFields> {
    if !is_audio(path) {
        return None;
    }

    let tagged_file = read_from_path(path).ok()?;

    let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) else {
        return Some(TagFields::default());
    };

    let mut fields = TagFields::default();
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
    push_instrument_field(&mut fields, tag);

    Some(fields)
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

fn path_match_scores(matcher: &SkimMatcherV2, path: &Path, query: &str) -> (i64, i64) {
    let path_score = path_search_strings(path)
        .iter()
        .filter_map(|candidate| matcher.fuzzy_match(candidate, query))
        .max()
        .unwrap_or(0) as i64;

    let mut name_score = 0i64;
    if let Some(name) = crate::path_util::file_name_lossy(path) {
        if let Some(score) = matcher.fuzzy_match(&name, query) {
            name_score = name_score.max(score as i64);
        }
    }
    if let Some(stem) = crate::path_util::file_stem_lossy(path) {
        if let Some(score) = matcher.fuzzy_match(&stem, query) {
            name_score = name_score.max(score as i64);
        }
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

fn path_search_strings(path: &Path) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut strings = Vec::new();
    let mut push = |value: &str| {
        if !value.is_empty() && seen.insert(value.to_owned()) {
            strings.push(value.to_string());
        }
    };
    push(&path.to_string_lossy());
    if let Some(name) = crate::path_util::file_name_lossy(path) {
        push(&name);
    }
    if let Some(stem) = crate::path_util::file_stem_lossy(path) {
        push(&stem);
    }
    strings
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
        "hihat", "hi-hat", "hi hat", "hh", "hat", "open hat", "closed hat", "openhat",
        "closedhat", "op hat", "cl hat",
    ],
    &["clap", "handclap", "hand clap"],
    &["tom", "toms", "floor tom", "floortom", "rack tom", "racktom"],
    &[
        "perc", "percussion", "conga", "bongo", "shaker", "tamb", "tambourine", "cowbell",
        "triangle", "woodblock", "cabasa",
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

const MIN_INSTRUMENT_SUBSTRING_LEN: usize = 4;

fn instrument_alias_matches(needle: &str, alias_norm: &str) -> bool {
    if needle.is_empty() || alias_norm.is_empty() {
        return false;
    }
    if needle == alias_norm {
        return true;
    }
    if alias_norm.starts_with(needle) || needle.starts_with(alias_norm) {
        return true;
    }
    if needle.len() >= MIN_INSTRUMENT_SUBSTRING_LEN && alias_norm.len() >= MIN_INSTRUMENT_SUBSTRING_LEN
    {
        return alias_norm.contains(needle) || needle.contains(alias_norm);
    }
    false
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
    if let Some(score) = matcher.fuzzy_match(value, query) {
        return Some(score as i64);
    }

    if instrument_terms_related(query, value) {
        return Some(80);
    }

    let terms = instrument_search_terms(query);
    terms
        .iter()
        .filter_map(|term| matcher.fuzzy_match(value, term))
        .max()
        .map(|score| score as i64)
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
        matcher.fuzzy_match(value, &filter.value).map(|score| score as i64)
    }
}

fn tag_match_score(
    matcher: &SkimMatcherV2,
    path: &Path,
    filters: &[TagFilter],
    lookup: &mut MetadataLookup,
) -> i64 {
    if filters.is_empty() || !is_audio(path) {
        return 0;
    }
    let fields = lookup.tag_fields(path);
    filters
        .iter()
        .filter_map(|filter| tag_field_score(matcher, &fields, filter))
        .min()
        .unwrap_or(0)
}

pub async fn filter_search_paths(
    debounce_ms: u64,
    paths: Vec<PathBuf>,
    file_query: String,
    tag_filters: Vec<TagFilter>,
    case_sensitive: bool,
    show_directories: bool,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    async_io::Timer::after(std::time::Duration::from_millis(debounce_ms)).await;
    let file_matcher = file_search_matcher(case_sensitive);
    let tag_matcher = tag_search_matcher();
    let file_active = file_query.len() > 2;
    let tag_active = !tag_filters.is_empty();
    let mut lookup = MetadataLookup::new(metadata);
    let mut matches = Vec::new();

    for path in paths {
        if !show_directories && !is_audio(&path) {
            continue;
        }

        let (name_score, path_score) = if file_active {
            path_match_scores(&file_matcher, &path, &file_query)
        } else {
            (0, 0)
        };

        if file_active && name_score == 0 && path_score == 0 {
            continue;
        }

        let tag_score = if tag_active {
            tag_match_score(&tag_matcher, &path, &tag_filters, &mut lookup)
        } else {
            0
        };

        if tag_active && tag_score == 0 {
            continue;
        }

        if !file_active && !tag_active {
            continue;
        }

        let rank = if file_active && tag_active {
            SearchRank::for_combined_search(
                path,
                &file_query,
                name_score,
                path_score,
                tag_score,
                case_sensitive,
            )
        } else if file_active {
            SearchRank::for_file_search(path, &file_query, name_score, path_score, case_sensitive)
        } else {
            SearchRank::for_tag_search(path, tag_score)
        };
        matches.push(rank);
    }

    matches.sort();
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

    let tagged_file = read_from_path(path).ok()?;
    let tag = tagged_file.primary_tag().or_else(|| tagged_file.first_tag())?;
    explicit_instrument_from_tag(tag)
}

pub fn write_instrument_tag_if_untagged(path: &Path, instrument: &str) -> Result<(), String> {
    write_instrument_tag(path, instrument)
}

pub fn write_instrument_tag(path: &Path, instrument: &str) -> Result<(), String> {
    use lofty::file::TaggedFileExt;
    use lofty::probe::Probe;
    use lofty::tag::{ItemKey, ItemValue, Tag, TagItem};

    let trimmed = instrument.trim();
    if trimmed.is_empty() {
        return Err("Instrument label cannot be empty".into());
    }

    let mut tagged_file = Probe::open(path)
        .map_err(|err| format!("Failed to open {}: {err}", path.display()))?
        .read()
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;

    if let Some(existing) = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag())
        .and_then(explicit_instrument_from_tag)
    {
        return Err(format!(
            "This file already has an instrument tag ({existing}). Auto Tag only fills untagged files."
        ));
    }

    let tag = if let Some(primary_tag) = tagged_file.primary_tag_mut() {
        primary_tag
    } else if let Some(first_tag) = tagged_file.first_tag_mut() {
        first_tag
    } else {
        let tag_type = tagged_file.primary_tag_type();
        tagged_file.insert_tag(Tag::new(tag_type));
        tagged_file
            .primary_tag_mut()
            .ok_or_else(|| "Failed to create tag".to_string())?
    };

    tag.retain(|item| {
        !item.description().eq_ignore_ascii_case("instrument")
            && !item.description().eq_ignore_ascii_case("instrumentname")
            && !item.description().eq_ignore_ascii_case("instrument type")
    });

    let mut item = TagItem::new(
        ItemKey::Comment,
        ItemValue::Text(trimmed.to_string()),
    );
    item.set_description("INSTRUMENT".to_string());
    tag.push(item);

    save_tags_atomically(path, tag)?;
    Ok(())
}

fn save_tags_atomically(path: &Path, tag: &lofty::tag::Tag) -> Result<(), String> {
    use lofty::config::WriteOptions;
    use lofty::tag::TagExt;

    let tmp = crate::path_util::sidecar(path, crate::path_util::TAG_TMP_SUFFIX);
    std::fs::copy(path, &tmp)
        .map_err(|err| format!("Failed to stage {}: {err}", path.display()))?;

    let write_result = tag
        .save_to_path(&tmp, WriteOptions::default())
        .map_err(|err| format!("Failed to write tags to {}: {err}", path.display()))
        .and_then(|_| {
            std::fs::File::open(&tmp)
                .and_then(|file| file.sync_all())
                .map_err(|err| format!("Failed to sync tagged file {}: {err}", path.display()))
        })
        .and_then(|_| {
            crate::path_util::replace_file(&tmp, path)
                .map_err(|err| format!("Failed to replace {}: {err}", path.display()))
        });

    if write_result.is_ok() {
        let _ = crate::path_util::sync_parent_dir(path);
    } else if path.exists() {
        let _ = std::fs::remove_file(&tmp);
    } else {
        let _ = std::fs::rename(&tmp, path);
    }
    write_result
}

pub fn refresh_cached_metadata(path: &Path) -> Option<CachedMetadata> {
    let mtime_secs = file_mtime_secs(path)?;
    let fields = read_tag_fields(path)?;
    Some(CachedMetadata {
        mtime_secs,
        fields,
    })
}
