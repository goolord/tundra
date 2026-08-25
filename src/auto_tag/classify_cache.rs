use super::ClassificationResult;
use crate::path_util;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

const CACHE_FILE: &str = "classify_cache_v2.bin";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct FileStamp {
    secs: u64,
    nanos: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedClassification {
    stamp: FileStamp,
    instrument: String,
    tier: u8,
    zcr: Option<f64>,
    confidence: Option<f64>,
    summary: String,
}

impl CachedClassification {
    fn from_result(stamp: FileStamp, result: &ClassificationResult) -> Self {
        Self {
            stamp,
            instrument: result.instrument.clone(),
            tier: result.tier,
            zcr: result.zcr,
            confidence: result.confidence,
            summary: result.summary.clone(),
        }
    }

    fn into_result(self) -> ClassificationResult {
        ClassificationResult {
            instrument: self.instrument,
            tier: self.tier,
            zcr: self.zcr,
            confidence: self.confidence,
            summary: self.summary,
        }
    }
}

struct ClassifyCache {
    entries: HashMap<PathBuf, CachedClassification>,
    path_keys: HashMap<PathBuf, PathBuf>,
    dirty: bool,
}

impl ClassifyCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            path_keys: HashMap::new(),
            dirty: false,
        }
    }

    fn cache_path() -> Option<PathBuf> {
        dirs::cache_dir().map(|mut path| {
            path.push("tundra");
            let _ = std::fs::create_dir_all(&path);
            path.push(CACHE_FILE);
            path
        })
    }

    fn load() -> Self {
        let Some(path) = Self::cache_path() else {
            return Self::new();
        };
        match std::fs::read(path) {
            Ok(bytes) => bincode::deserialize::<HashMap<PathBuf, CachedClassification>>(&bytes)
                .map(|entries| Self {
                    entries,
                    path_keys: HashMap::new(),
                    dirty: false,
                })
                .unwrap_or_else(|_| Self::new()),
            Err(_) => Self::new(),
        }
    }

    fn persist(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = Self::cache_path() else {
            return;
        };
        let Ok(bytes) = bincode::serialize(&self.entries) else {
            eprintln!("Failed to serialize classify cache");
            return;
        };
        if let Err(err) = std::fs::write(path, bytes) {
            eprintln!("Failed to write classify cache: {err}");
            return;
        }
        self.dirty = false;
    }

    fn file_stamp(path: &Path) -> Option<FileStamp> {
        let modified = std::fs::metadata(path).ok()?.modified().ok()?;
        stamp_from_system_time(modified)
    }

    fn lookup_key(&self, path: &Path) -> PathBuf {
        let direct = path_util::cache_key(path.to_path_buf());
        if self.entries.contains_key(&direct) {
            return direct;
        }
        if let Some(key) = self.path_keys.get(&direct) {
            return key.clone();
        }
        path_util::canonical_path(path)
            .map(path_util::cache_key)
            .unwrap_or(direct)
    }

    fn get(&self, path: &Path) -> Option<ClassificationResult> {
        let stamp = Self::file_stamp(path)?;
        let key = self.lookup_key(path);
        self.get_by_key(&key, stamp)
    }

    fn get_by_key(&self, key: &PathBuf, stamp: FileStamp) -> Option<ClassificationResult> {
        let cached = self.entries.get(key)?;
        if cached.stamp != stamp {
            return None;
        }
        Some(cached.clone().into_result())
    }

    fn remember_path_alias(&mut self, direct: PathBuf, canonical: PathBuf) {
        if direct != canonical {
            self.path_keys.insert(direct, canonical);
        }
    }

    fn insert(&mut self, path: &Path, result: &ClassificationResult) {
        let Some(stamp) = Self::file_stamp(path) else {
            return;
        };
        let entry = CachedClassification::from_result(stamp, result);
        let direct = path_util::cache_key(path.to_path_buf());
        let canonical = path_util::canonical_path(path)
            .map(path_util::cache_key)
            .unwrap_or_else(|_| direct.clone());
        self.entries.insert(canonical.clone(), entry.clone());
        self.remember_path_alias(direct.clone(), canonical.clone());
        if direct != canonical {
            self.entries.insert(direct, entry);
        }
        self.dirty = true;
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.path_keys.clear();
        self.dirty = true;
        self.persist();
    }
}

fn stamp_from_system_time(time: SystemTime) -> Option<FileStamp> {
    let duration = time.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(FileStamp {
        secs: duration.as_secs(),
        nanos: duration.subsec_nanos(),
    })
}

static CLASSIFY_CACHE: LazyLock<RwLock<ClassifyCache>> =
    LazyLock::new(|| RwLock::new(ClassifyCache::load()));

fn with_cache_read<T>(f: impl FnOnce(&ClassifyCache) -> T) -> Option<T> {
    match CLASSIFY_CACHE.read() {
        Ok(cache) => Some(f(&cache)),
        Err(_) => {
            eprintln!("classify cache lock poisoned");
            None
        }
    }
}

fn with_cache_write<T>(f: impl FnOnce(&mut ClassifyCache) -> T) -> Option<T> {
    match CLASSIFY_CACHE.write() {
        Ok(mut cache) => Some(f(&mut cache)),
        Err(_) => {
            eprintln!("classify cache lock poisoned");
            None
        }
    }
}

pub fn get_cached(path: &Path) -> Option<ClassificationResult> {
    with_cache_read(|cache| cache.get(path))?
}

pub fn store_cached(path: &Path, result: &ClassificationResult) {
    let _ = with_cache_write(|cache| cache.insert(path, result));
}

pub fn flush_cache() {
    let _ = with_cache_write(|cache| cache.persist());
}

pub fn clear_cache() {
    let _ = with_cache_write(|cache| cache.clear());
}
