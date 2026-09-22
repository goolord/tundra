use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::fields::TagFields;
use super::read::{is_audio, read_tag_fields};
use crate::path_util::file_mtime_secs;

/// A file's tags as indexed, with the mtime they were read at.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedMetadata {
    pub mtime_secs: u64,
    pub fields: TagFields,
}

pub struct MetadataLookup {
    cache: Arc<HashMap<PathBuf, CachedMetadata>>,
    new_entries: HashMap<PathBuf, CachedMetadata>,
}

impl MetadataLookup {
    pub fn new(cache: Arc<HashMap<PathBuf, CachedMetadata>>) -> Self {
        Self::with_new_entries(cache, HashMap::new())
    }

    pub fn with_new_entries(
        cache: Arc<HashMap<PathBuf, CachedMetadata>>,
        new_entries: HashMap<PathBuf, CachedMetadata>,
    ) -> Self {
        Self { cache, new_entries }
    }

    pub fn into_new_entries(self) -> HashMap<PathBuf, CachedMetadata> {
        self.new_entries
    }

    fn lookup_cached(&self, path: &Path) -> Option<&CachedMetadata> {
        crate::path_util::cache_lookup_keys(path)
            .into_iter()
            .find_map(|key| self.new_entries.get(&key).or_else(|| self.cache.get(&key)))
    }

    /// Tags from the index only; an unindexed file reports nothing rather than being parsed.
    /// Search uses this because a tag filter spans the whole library, and opening every
    /// unindexed audio file to answer one query costs minutes. `index_paths` fills the index.
    pub(crate) fn indexed_tag_fields(&self, path: &Path) -> Option<&TagFields> {
        self.lookup_cached(path).map(|entry| &entry.fields)
    }

    /// Tags from the index while its mtime still matches (or the mtime is unreadable),
    /// otherwise read from disk and remembered as a new entry.
    pub fn tag_fields(&mut self, path: &Path) -> TagFields {
        let mtime = file_mtime_secs(path);
        if mtime.is_none() && !path.exists() {
            return TagFields::default();
        }
        if let Some(cached) = self.lookup_cached(path)
            && mtime.is_none_or(|mtime| mtime == cached.mtime_secs)
        {
            return cached.fields.clone();
        }
        let Some(fields) = read_tag_fields(path) else {
            return TagFields::default();
        };
        let entry = CachedMetadata { mtime_secs: mtime.unwrap_or(0), fields: fields.clone() };
        self.new_entries.insert(crate::path_util::cache_key(path), entry);
        fields
    }
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

pub fn refresh_cached_metadata(path: &Path) -> Option<CachedMetadata> {
    let mtime_secs = file_mtime_secs(path)?;
    let fields = read_tag_fields(path)?;
    Some(CachedMetadata { mtime_secs, fields })
}
