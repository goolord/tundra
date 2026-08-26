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

        if !path.exists() {
            return TagFields::default();
        }

        if let Some(cached) = self.cache.get(path) {
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

fn collect_tag_matches(
    paths: &[PathBuf],
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    let tag_matcher = tag_search_matcher();
    let mut lookup = MetadataLookup::new(metadata);
    let mut matches = Vec::new();

    for path in paths.iter().filter(|path| is_audio(path)) {
        let tag_score = tag_match_score(&tag_matcher, path, tag_filters, &mut lookup);
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
    collect_tag_matches(paths, tag_filters, metadata)
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
        return collect_tag_matches(paths, tag_filters, metadata);
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
            tag_match_score(&tag_matcher, path, tag_filters, &mut lookup)
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
        let result = collect_tag_matches(&paths, &filters, Arc::new(cache));
        assert_eq!(result.paths, vec![audio.clone()]);
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
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
    fn file_search_debounce_is_longer_for_two_chars() {
        assert_eq!(file_search_debounce_ms(2), FILE_SEARCH_DEBOUNCE_MS_SHORT);
        assert_eq!(file_search_debounce_ms(3), FILE_SEARCH_DEBOUNCE_MS);
    }
}
