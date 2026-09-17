//! In-memory directory listings and tag index, persisted to the cache dir.
//!
//! Maps sit behind `Arc` so a search takes a snapshot in O(1); writers copy
//! only while a snapshot is alive. Saves run on a background thread, coalesced,
//! so the UI never serializes or fsyncs the whole index.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use super::AllowedDirectories;
use crate::locks::{lock, read, write};
use crate::metadata::{CachedMetadata, TagFields, refresh_cached_metadata};

pub type Listings = HashMap<PathBuf, Vec<PathBuf>>;
pub type MetadataMap = HashMap<PathBuf, CachedMetadata>;
pub type Shared<V> = Arc<RwLock<Arc<HashMap<PathBuf, V>>>>;

/// Both caches as loaded from disk at start-up.
#[derive(Debug, Clone, Default)]
pub struct PersistedCaches {
    pub dirs: Listings,
    pub metadata: MetadataMap,
}

const DIR_CACHE_FILE: &str = "dir_cache.bin";
// v10: instrument now reads from each container's canonical key, so entries
// cached under the old per-format logic must be re-read.
const METADATA_CACHE_FILE: &str = "metadata_cache_v10.bin";
const SAVE_DELAY: Duration = Duration::from_secs(2);

/// A path-keyed map mirrored to one bincode file in the cache directory.
pub struct PersistedMap<V> {
    map: Shared<V>,
    file: &'static str,
    /// Nothing is written until the file on disk has been loaded, or an early
    /// save of a nearly empty map would replace the user's full index.
    loaded: Arc<AtomicBool>,
    save_pending: Arc<AtomicBool>,
}

pub type DirCache = PersistedMap<Vec<PathBuf>>;
pub type MetadataCache = PersistedMap<CachedMetadata>;

/// A snapshot of the map as it is now; later writes do not affect it.
pub fn lock_read<V>(map: &Shared<V>) -> Arc<HashMap<PathBuf, V>> {
    Arc::clone(&read(map))
}

impl<V> PersistedMap<V>
where
    V: Persistable + Clone + serde::Serialize + Send + Sync + 'static,
{
    fn empty(file: &'static str) -> Self {
        Self {
            map: Arc::new(RwLock::new(Arc::new(HashMap::new()))),
            file,
            loaded: Arc::new(AtomicBool::new(false)),
            save_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn share(&self) -> Shared<V> {
        Arc::clone(&self.map)
    }

    pub fn snapshot(&self) -> Arc<HashMap<PathBuf, V>> {
        lock_read(&self.map)
    }

    fn update<R>(&mut self, change: impl FnOnce(&mut HashMap<PathBuf, V>) -> R) -> R {
        let mut guard = write(&self.map);
        change(Arc::make_mut(&mut guard))
    }

    /// Entries loaded from disk at start-up. Anything recorded before the load
    /// finished is newer and wins.
    pub fn finish_loading(&mut self, loaded: HashMap<PathBuf, V>) {
        // Already cleared by the user while loading: the file is stale.
        if self.loaded.load(Ordering::SeqCst) {
            return;
        }
        self.update(|map| {
            for (key, value) in loaded {
                map.entry(key).or_insert(value);
            }
        });
        self.loaded.store(true, Ordering::SeqCst);
    }

    /// Replace everything with an empty map and save that.
    pub fn clear(&mut self) {
        self.update(HashMap::clear);
        self.loaded.store(true, Ordering::SeqCst);
        self.persist();
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&PathBuf) -> bool) -> bool {
        self.update(|map| {
            let before = map.len();
            map.retain(|path, _| keep(path));
            before != map.len()
        })
    }

    pub fn persist_map_to(path: &Path, map: &HashMap<PathBuf, V>) {
        // Borrowed keys and values encode to the same bytes as the owned map.
        let persistable: HashMap<&PathBuf, &V> = map.iter().filter(|(_, value)| value.worth_saving()).collect();
        crate::app_data::write_bincode(path, &persistable, &path.display().to_string());
    }

    /// Write now, on this thread. Used on exit, when a pending background save
    /// would be killed with the process.
    pub fn flush(&self) {
        if self.loaded.load(Ordering::SeqCst)
            && self.save_pending.swap(false, Ordering::SeqCst)
            && let Some(path) = crate::app_data::cache_file(self.file)
        {
            Self::persist_map_to(&path, &self.snapshot());
        }
    }

    /// Schedule a save. Calls within `SAVE_DELAY` share one write, and saves
    /// are serialized so an older snapshot never lands after a newer one.
    pub fn persist(&self) {
        static SAVE_LOCK: Mutex<()> = Mutex::new(());
        if !self.loaded.load(Ordering::SeqCst) || self.save_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(path) = crate::app_data::cache_file(self.file) else {
            self.save_pending.store(false, Ordering::SeqCst);
            return;
        };
        let (map, pending) = (self.share(), Arc::clone(&self.save_pending));
        std::thread::spawn(move || {
            std::thread::sleep(SAVE_DELAY);
            let _serial = lock(&SAVE_LOCK);
            pending.store(false, Ordering::SeqCst);
            Self::persist_map_to(&path, &lock_read(&map));
        });
    }
}

/// Which entries are worth writing to disk.
pub trait Persistable {
    fn worth_saving(&self) -> bool {
        true
    }
}

impl Persistable for Vec<PathBuf> {}

impl Persistable for CachedMetadata {
    fn worth_saving(&self) -> bool {
        self.mtime_secs != 0
    }
}

impl DirCache {
    pub fn new() -> Self {
        Self::empty(DIR_CACHE_FILE)
    }

    pub fn insert(&mut self, dir: PathBuf, listing: Vec<PathBuf>) {
        let key = crate::path_util::cache_key(&dir);
        self.update(|map| map.insert(key, listing));
        self.persist();
    }

    pub fn contains_key(&self, dir: &Path) -> bool {
        self.snapshot().contains_key(&crate::path_util::cache_key(dir))
    }
}

impl MetadataCache {
    pub fn new() -> Self {
        Self::empty(METADATA_CACHE_FILE)
    }

    /// Add entries, never replacing a newer one: a search that read a file
    /// before a tag save may finish after it.
    pub fn merge(&mut self, entries: MetadataMap) {
        if entries.is_empty() {
            return;
        }
        self.update(|map| {
            for (key, entry) in entries {
                keep_newer(map, key, entry);
            }
        });
        self.persist();
    }

    pub fn merge_path(&mut self, path: &Path, entry: CachedMetadata) {
        self.merge(HashMap::from([(crate::path_util::cache_key(path), entry)]));
    }

    /// Indexed tags without touching the disk, for display.
    pub fn cached_fields(&self, path: &Path) -> Option<TagFields> {
        let map = self.snapshot();
        crate::path_util::cache_lookup_keys(path)
            .iter()
            .find_map(|key| map.get(key))
            .map(|cached| cached.fields.clone())
    }

    /// Tags for `path`, re-read when the file changed since it was indexed.
    pub fn tag_fields_for(&mut self, path: &Path) -> TagFields {
        if let Some(mtime_secs) = crate::path_util::file_mtime_secs(path) {
            let map = self.snapshot();
            let current = crate::path_util::cache_lookup_keys(path)
                .iter()
                .find_map(|key| map.get(key))
                .filter(|cached| cached.mtime_secs == mtime_secs);
            if let Some(cached) = current {
                return cached.fields.clone();
            }
        }
        let Some(entry) = refresh_cached_metadata(path) else {
            return TagFields::default();
        };
        let fields = entry.fields.clone();
        self.merge_path(path, entry);
        fields
    }
}

fn keep_newer(map: &mut MetadataMap, key: PathBuf, entry: CachedMetadata) {
    match map.get(&key) {
        Some(existing) if existing.mtime_secs > entry.mtime_secs => {}
        _ => {
            map.insert(key, entry);
        }
    }
}

fn load_map<V: serde::de::DeserializeOwned>(file: &str) -> HashMap<PathBuf, V> {
    crate::app_data::cache_file(file)
        .and_then(|path| crate::app_data::read_bincode(&path))
        .unwrap_or_default()
}

/// Runs off the UI thread at start-up.
pub fn load_startup_caches(allowed: AllowedDirectories) -> PersistedCaches {
    // Temps from an atomic save that crashed; the live file is intact.
    for dir in [crate::app_data::cache_dir(), crate::app_data::config_dir()]
        .into_iter()
        .flatten()
    {
        crate::safe_write::reclaim_write_sidecars(&dir);
    }

    let mut dirs: Listings = load_map::<Vec<PathBuf>>(DIR_CACHE_FILE)
        .into_iter()
        .map(|(key, listing)| (crate::path_util::cache_key(&key), listing))
        .collect();
    // Older builds also stored raw path spellings next to cache keys; fold them
    // into one entry per file, keeping the newest.
    let mut metadata = MetadataMap::new();
    for (key, entry) in load_map::<CachedMetadata>(METADATA_CACHE_FILE) {
        keep_newer(&mut metadata, crate::path_util::cache_key(&key), entry);
    }
    if !allowed.is_empty() {
        dirs.retain(|path, _| allowed.contains_cached_path(path));
        metadata.retain(|path, _| allowed.contains_cached_path(path));
    }
    PersistedCaches { dirs, metadata }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mtime_secs: u64, title: &str) -> CachedMetadata {
        CachedMetadata {
            mtime_secs,
            fields: TagFields {
                title: title.into(),
                ..TagFields::default()
            },
        }
    }

    #[test]
    fn merges_never_replace_newer_entries() {
        let mut cache = MetadataCache::new();
        let path = PathBuf::from("/samples/kick.wav");
        cache.merge_path(&path, entry(20, "saved"));
        cache.merge_path(&path, entry(10, "stale search result"));
        assert_eq!(
            cache.cached_fields(&path).map(|fields| fields.title),
            Some("saved".into())
        );
        cache.merge_path(&path, entry(30, "edited again"));
        assert_eq!(
            cache.cached_fields(&path).map(|fields| fields.title),
            Some("edited again".into())
        );
    }

    #[test]
    fn legacy_spellings_fold_into_the_newest_entry() {
        let mut map = MetadataMap::new();
        let raw = PathBuf::from(r"C:\Samples\Kick.wav");
        keep_newer(&mut map, crate::path_util::cache_key(&raw), entry(50, "new"));
        keep_newer(
            &mut map,
            crate::path_util::cache_key(&raw),
            entry(40, "old raw spelling"),
        );
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.values().next().map(|cached| cached.fields.title.as_str()),
            Some("new")
        );
    }
}
