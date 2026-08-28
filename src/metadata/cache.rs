use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::types::is_audio;

use super::fields::TagFields;
use super::read::{file_mtime_secs, read_tag_fields};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedMetadata {
    pub mtime_secs: u64,
    pub fields: TagFields,
}

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

    pub(crate) fn tag_fields_for_search(&mut self, path: &Path, allow_disk_read: bool) -> TagFields {
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
    Some(CachedMetadata {
        mtime_secs,
        fields,
    })
}
