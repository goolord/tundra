use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use crate::metadata::{
    refresh_cached_metadata, CachedMetadata, PersistedCaches, TagFields,
};
use super::settings::AllowedDirectories;

pub struct DirCache(Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>);

pub struct MetadataCache(Arc<RwLock<HashMap<PathBuf, CachedMetadata>>>);

impl DirCache {
    pub(crate) fn new() -> DirCache {
        DirCache(Arc::new(RwLock::new(HashMap::new())))
    }

    pub(crate) fn share(&self) -> Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>> {
        Arc::clone(&self.0)
    }

    pub(crate) fn insert(&mut self, k: PathBuf, v: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
        let key = crate::path_util::cache_key(k);
        let mut map = self.0.write().unwrap();
        let aliases: Vec<PathBuf> = map
            .keys()
            .filter(|existing| {
                *existing != &key && crate::path_util::cache_key((*existing).clone()) == key
            })
            .cloned()
            .collect();
        let mut previous = None;
        for alias in aliases {
            if let Some(old) = map.remove(&alias) {
                previous = Some(old);
            }
        }
        map.insert(key, v).or(previous)
    }

    pub(crate) fn contains_key(&self, k: &PathBuf) -> bool {
        let key = crate::path_util::cache_key(k.clone());
        self.0.read().unwrap().contains_key(&key)
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&PathBuf) -> bool) -> bool {
        retain_paths(&mut self.0.write().unwrap(), keep)
    }

    pub(crate) fn from_map(map: HashMap<PathBuf, Vec<PathBuf>>) -> Self {
        let mut normalized: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
        for (key, paths) in map {
            let key = crate::path_util::cache_key(key);
            let entry = normalized.entry(key).or_default();
            if paths.len() > entry.len() {
                *entry = paths;
            }
        }
        DirCache(Arc::new(RwLock::new(normalized)))
    }

    fn get_path() -> Option<PathBuf> {
        crate::path_util::cache_file("dir_cache.bin")
    }

    fn persist_map(map: &HashMap<PathBuf, Vec<PathBuf>>) {
        let Some(dir_cache) = DirCache::get_path() else {
            return;
        };
        Self::persist_map_to(&dir_cache, map);
    }

    pub(crate) fn persist_map_to(path: &Path, map: &HashMap<PathBuf, Vec<PathBuf>>) {
        crate::path_util::write_bincode(path, map, "directory cache");
    }

    pub(crate) fn persist(&self) {
        let Ok(map) = self.0.read() else {
            return;
        };
        Self::persist_map(&map);
    }
}

fn load_dir_cache_map() -> HashMap<PathBuf, Vec<PathBuf>> {
    load_cached_map(DirCache::get_path())
}

impl MetadataCache {
    pub(crate) fn new() -> Self {
        Self(Arc::new(RwLock::new(HashMap::new())))
    }

    pub(crate) fn share(&self) -> Arc<RwLock<HashMap<PathBuf, CachedMetadata>>> {
        Arc::clone(&self.0)
    }

    fn get_path() -> Option<PathBuf> {
        // v10: instrument now reads from each container's canonical key, so
        // entries cached under the old per-format logic must be re-read.
        crate::path_util::cache_file("metadata_cache_v10.bin")
    }

    fn persist_map(cache: &HashMap<PathBuf, CachedMetadata>) {
        let Some(path) = MetadataCache::get_path() else {
            return;
        };
        Self::persist_map_to(&path, cache);
    }

    pub(crate) fn persist_map_to(path: &Path, cache: &HashMap<PathBuf, CachedMetadata>) {
        let persistable: HashMap<PathBuf, CachedMetadata> = cache
            .iter()
            .filter(|(_, cached)| cached.mtime_secs != 0)
            .map(|(path, cached)| (path.clone(), cached.clone()))
            .collect();
        crate::path_util::write_bincode(path, &persistable, "metadata cache");
    }

    pub(crate) fn persist(&self) {
        let Ok(cache) = self.0.read() else {
            return;
        };
        Self::persist_map(&cache);
    }

    pub(crate) fn from_map(map: HashMap<PathBuf, CachedMetadata>) -> Self {
        Self(Arc::new(RwLock::new(map)))
    }

    pub(crate) fn retain(&mut self, keep: impl FnMut(&PathBuf) -> bool) -> bool {
        retain_paths(&mut self.0.write().unwrap(), keep)
    }

    pub(crate) fn snapshot(&self) -> Arc<HashMap<PathBuf, CachedMetadata>> {
        Arc::new(self.0.read().unwrap().clone())
    }

    pub(crate) fn merge(&mut self, entries: HashMap<PathBuf, CachedMetadata>) {
        if entries.is_empty() {
            return;
        }
        self.0.write().unwrap().extend(entries);
        self.persist();
    }

    pub(crate) fn merge_path(&mut self, path: &Path, entry: CachedMetadata) {
        let raw = path.to_path_buf();
        let key = crate::path_util::cache_key(raw.clone());
        let mut entries = HashMap::from([(key.clone(), entry.clone())]);
        if key != raw {
            entries.insert(raw, entry);
        }
        self.merge(entries);
    }

    pub(crate) fn tag_fields_for(&self, path: &Path) -> TagFields {
        if let Some(mtime_secs) = crate::metadata::file_mtime_secs(path) {
            if let Ok(cache) = self.0.read() {
                for key in crate::path_util::cache_lookup_keys(path) {
                    if let Some(cached) = cache.get(&key) {
                        if cached.mtime_secs == mtime_secs {
                            return cached.fields.clone();
                        }
                    }
                }
            }
        }

        let Some(entry) = refresh_cached_metadata(path) else {
            return TagFields::default();
        };
        let fields = entry.fields.clone();
        if let Ok(mut cache) = self.0.write() {
            let store_key = crate::path_util::cache_key(path.to_path_buf());
            cache.insert(store_key, entry);
        }
        fields
    }
}

fn load_metadata_cache_map() -> HashMap<PathBuf, CachedMetadata> {
    load_cached_map(MetadataCache::get_path())
}

fn load_cached_map<T: Default + serde::de::DeserializeOwned>(path: Option<PathBuf>) -> T {
    path.and_then(|path| crate::path_util::read_bincode(&path))
        .unwrap_or_default()
}

fn retain_paths<V>(map: &mut HashMap<PathBuf, V>, mut keep: impl FnMut(&PathBuf) -> bool) -> bool {
    let before = map.len();
    map.retain(|path, _| keep(path));
    before != map.len()
}

pub(crate) fn load_startup_caches(allowed: AllowedDirectories) -> PersistedCaches {
    let mut dirs = load_dir_cache_map();
    let mut metadata = load_metadata_cache_map();
    if !allowed.is_empty() {
        let allowed = allowed.clone();
        let before = dirs.len();
        dirs.retain(|path, _| allowed.contains_path(path));
        if dirs.len() != before {
            DirCache::persist_map(&dirs);
        }
        let before = metadata.len();
        metadata.retain(|path, _| allowed.contains_path(path));
        if metadata.len() != before {
            MetadataCache::persist_map(&metadata);
        }
    }
    PersistedCaches { dirs, metadata }
}
