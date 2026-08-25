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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedMetadata {
    pub mtime_secs: u64,
    pub text: String,
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

    fn cached_text(&self, path: &Path, mtime_secs: u64) -> Option<String> {
        let cached = self
            .new_entries
            .get(path)
            .or_else(|| self.cache.get(path))?;
        (cached.mtime_secs == mtime_secs).then(|| cached.text.clone())
    }

    pub fn search_text(&mut self, path: &Path) -> String {
        let Some(mtime_secs) = file_mtime_secs(path) else {
            return read_search_text(path).unwrap_or_default();
        };

        if let Some(text) = self.cached_text(path, mtime_secs) {
            return text;
        }

        let Some(text) = read_search_text(path) else {
            return String::new();
        };

        self.new_entries.insert(
            path.to_path_buf(),
            CachedMetadata {
                mtime_secs,
                text: text.clone(),
            },
        );
        text
    }
}

fn push_part(parts: &mut Vec<String>, value: Option<impl AsRef<str>>) {
    if let Some(text) = value
        .as_ref()
        .map(AsRef::as_ref)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        parts.push(text.to_owned());
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
/// Returns `Some("")` when the file was read but has no tags.
pub fn read_search_text(path: &Path) -> Option<String> {
    if !is_audio(path) {
        return None;
    }

    let tagged_file = read_from_path(path).ok()?;

    let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) else {
        return Some(String::new());
    };

    let mut parts = Vec::new();
    push_part(&mut parts, tag.title());
    push_part(&mut parts, tag.artist());
    push_part(&mut parts, tag.album());
    push_part(&mut parts, tag.genre());
    push_part(&mut parts, tag.comment());

    for key in [
        ItemKey::AlbumArtist,
        ItemKey::Composer,
        ItemKey::Label,
        ItemKey::TrackTitle,
        ItemKey::TrackArtist,
    ] {
        push_part(&mut parts, tag.get_string(key));
    }

    Some(parts.join(" "))
}

pub fn index_paths(
    paths: &[PathBuf],
    cache: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> HashMap<PathBuf, CachedMetadata> {
    let mut lookup = MetadataLookup::new(cache);
    for path in paths {
        if is_audio(path) {
            lookup.search_text(path);
        }
    }
    lookup.into_new_entries()
}

pub fn path_matches(matcher: &SkimMatcherV2, path: &Path, query: &str) -> bool {
    matcher
        .fuzzy_match(path.to_string_lossy().as_ref(), query)
        .is_some()
}

pub fn metadata_matches(
    matcher: &SkimMatcherV2,
    path: &Path,
    query: &str,
    lookup: &mut MetadataLookup,
) -> bool {
    if !is_audio(path) {
        return false;
    }
    let text = lookup.search_text(path);
    !text.is_empty() && matcher.fuzzy_match(text.as_str(), query).is_some()
}

pub fn file_matches_search(
    matcher: &SkimMatcherV2,
    path: &Path,
    query: &str,
    lookup: &mut MetadataLookup,
) -> bool {
    path_matches(matcher, path, query) || metadata_matches(matcher, path, query, lookup)
}
