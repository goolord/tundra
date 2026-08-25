use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use lofty::file::TaggedFileExt;
use lofty::read_from_path;
use lofty::tag::{Accessor, ItemKey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
}

impl TagField {
    pub const ALL: [TagField; 8] = [
        TagField::Title,
        TagField::Artist,
        TagField::Album,
        TagField::Genre,
        TagField::Comment,
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
            _ => None,
        }
    }

    fn matches_query(self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        self.as_str().contains(needle)
            || self
                .label()
                .to_ascii_lowercase()
                .contains(needle)
    }
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
            "Unknown tag field. Try title, artist, album, genre…"
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

pub fn path_matches(matcher: &SkimMatcherV2, path: &Path, query: &str) -> bool {
    matcher
        .fuzzy_match(path.to_string_lossy().as_ref(), query)
        .is_some()
}

fn filter_matches_field(
    matcher: &SkimMatcherV2,
    fields: &TagFields,
    filter: &TagFilter,
) -> bool {
    let value = fields.field_value(filter.field);
    !value.is_empty() && matcher.fuzzy_match(value, &filter.value).is_some()
}

pub fn tag_filters_match(
    matcher: &SkimMatcherV2,
    path: &Path,
    filters: &[TagFilter],
    lookup: &mut MetadataLookup,
) -> bool {
    if filters.is_empty() {
        return true;
    }
    if !is_audio(path) {
        return false;
    }
    let fields = lookup.tag_fields(path);
    filters
        .iter()
        .all(|filter| filter_matches_field(matcher, &fields, filter))
}

pub fn file_matches_search(
    matcher: &SkimMatcherV2,
    path: &Path,
    file_query: &str,
    tag_filters: &[TagFilter],
    lookup: &mut MetadataLookup,
) -> bool {
    let file_active = file_query.len() > 2;
    let tag_active = !tag_filters.is_empty();

    if file_active && !path_matches(matcher, path, file_query) {
        return false;
    }

    if tag_active && !tag_filters_match(matcher, path, tag_filters, lookup) {
        return false;
    }

    file_active || tag_active
}

pub async fn filter_search_paths(
    debounce_ms: u64,
    paths: Vec<PathBuf>,
    file_query: String,
    tag_filters: Vec<TagFilter>,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    async_io::Timer::after(std::time::Duration::from_millis(debounce_ms)).await;
    let matcher = SkimMatcherV2::default();
    let mut lookup = MetadataLookup::new(metadata);
    let paths = paths
        .into_iter()
        .filter(|path| {
            file_matches_search(
                &matcher,
                path,
                &file_query,
                &tag_filters,
                &mut lookup,
            )
        })
        .collect();
    SearchResult {
        paths,
        new_metadata: lookup.into_new_entries(),
    }
}
