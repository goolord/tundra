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

use super::settings::AllowedDirectories;
use crate::metadata::{refresh_cached_metadata, CachedMetadata, PersistedCaches, TagFields};

pub type Listings = HashMap<PathBuf, Vec<PathBuf>>;
pub type MetadataMap = HashMap<PathBuf, CachedMetadata>;
pub type Shared<V> = Arc<RwLock<Arc<HashMap<PathBuf, V>>>>;

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

fn lock_read<V>(map: &Shared<V>) -> Arc<HashMap<PathBuf, V>> {
    Arc::clone(&map.read().unwrap_or_else(std::sync::PoisonError::into_inner))
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

    pub(crate) fn share(&self) -> Shared<V> {
        Arc::clone(&self.map)
    }

    pub(crate) fn snapshot(&self) -> Arc<HashMap<PathBuf, V>> {
        lock_read(&self.map)
    }

    fn update<R>(&mut self, change: impl FnOnce(&mut HashMap<PathBuf, V>) -> R) -> R {
        let mut guard = self
            .map
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        change(Arc::make_mut(&mut guard))
    }

    /// Entries loaded from disk at start-up. Anything recorded before the load
    /// finished is newer and wins.
    pub(crate) fn finish_loading(&mut self, loaded: HashMap<PathBuf, V>) {
        self.update(|map| {
            for (key, value) in loaded {
                map.entry(key).or_insert(value);
            }
        });
        self.loaded.store(true, Ordering::SeqCst);
    }

    /// Replace everything with an empty map and save that.
    pub(crate) fn clear(&mut self) {
        self.update(HashMap::clear);
        self.loaded.store(true, Ordering::SeqCst);
        self.persist();
    }

    pub(crate) fn retain(&mut self, mut keep: impl FnMut(&PathBuf) -> bool) -> bool {
        self.update(|map| {
            let before = map.len();
            map.retain(|path, _| keep(path));
            before != map.len()
        })
    }

    pub(crate) fn persist_map_to(path: &Path, map: &HashMap<PathBuf, V>) {
        // Borrowed keys and values encode to the same bytes as the owned map.
        let persistable: HashMap<&PathBuf, &V> =
            map.iter().filter(|(_, value)| value.worth_saving()).collect();
        crate::path_util::write_bincode(path, &persistable, &path.display().to_string());
    }

    /// Schedule a save. Calls within `SAVE_DELAY` share one write, and saves
    /// are serialized so an older snapshot never lands after a newer one.
    pub(crate) fn persist(&self) {
        static SAVE_LOCK: Mutex<()> = Mutex::new(());
        if !self.loaded.load(Ordering::SeqCst) || self.save_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(path) = crate::path_util::cache_file(self.file) else {
            self.save_pending.store(false, Ordering::SeqCst);
            return;
        };
        let (map, pending) = (self.share(), Arc::clone(&self.save_pending));
        std::thread::spawn(move || {
            std::thread::sleep(SAVE_DELAY);
            let _serial = SAVE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
    pub(crate) fn new() -> Self {
        Self::empty(DIR_CACHE_FILE)
    }

    pub(crate) fn insert(&mut self, dir: PathBuf, listing: Vec<PathBuf>) {
        let key = crate::path_util::cache_key(dir);
        self.update(|map| map.insert(key, listing));
        self.persist();
    }

    pub(crate) fn contains_key(&self, dir: &Path) -> bool {
        self.snapshot()
            .contains_key(&crate::path_util::cache_key(dir.to_path_buf()))
    }
}

impl MetadataCache {
    pub(crate) fn new() -> Self {
        Self::empty(METADATA_CACHE_FILE)
    }

    pub(crate) fn merge(&mut self, entries: MetadataMap) {
        if entries.is_empty() {
            return;
        }
        self.update(|map| map.extend(entries));
        self.persist();
    }

    pub(crate) fn merge_path(&mut self, path: &Path, entry: CachedMetadata) {
        self.merge(HashMap::from([(
            crate::path_util::cache_key(path.to_path_buf()),
            entry,
        )]));
    }

    /// Indexed tags without touching the disk, for display.
    pub(crate) fn cached_fields(&self, path: &Path) -> Option<TagFields> {
        let map = self.snapshot();
        crate::path_util::cache_lookup_keys(path)
            .iter()
            .find_map(|key| map.get(key))
            .map(|cached| cached.fields.clone())
    }

    /// Tags for `path`, re-read when the file changed since it was indexed.
    pub(crate) fn tag_fields_for(&mut self, path: &Path) -> TagFields {
        if let Some(mtime_secs) = crate::metadata::file_mtime_secs(path) {
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

fn load_map<V: serde::de::DeserializeOwned>(file: &str) -> HashMap<PathBuf, V> {
    crate::path_util::cache_file(file)
        .and_then(|path| crate::path_util::read_bincode(&path))
        .unwrap_or_default()
}

/// Runs off the UI thread at start-up.
pub(crate) fn load_startup_caches(allowed: AllowedDirectories) -> PersistedCaches {
    // Temps from an atomic save that crashed; the live file is intact.
    for dir in [crate::path_util::tundra_cache_dir(), crate::path_util::tundra_config_dir()]
        .into_iter()
        .flatten()
    {
        crate::path_util::reclaim_write_sidecars(&dir);
    }

    let mut dirs: Listings = load_map::<Vec<PathBuf>>(DIR_CACHE_FILE)
        .into_iter()
        .map(|(key, listing)| (crate::path_util::cache_key(key), listing))
        .collect();
    let mut metadata: MetadataMap = load_map(METADATA_CACHE_FILE);
    if !allowed.is_empty() {
        dirs.retain(|path, _| allowed.contains_cached_path(path));
        metadata.retain(|path, _| allowed.contains_cached_path(path));
    }
    PersistedCaches { dirs, metadata }
}
