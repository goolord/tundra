//! Classification results remembered across runs, keyed by path and stamped
//! with the file's size and modification time.

use super::ClassificationResult;
use crate::path_util;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::SystemTime;

// v5: stamps include the file size.
const CACHE_FILE: &str = "classify_cache_v5.bin";

/// Identifies one version of a file. Size is included because copies and
/// archive extraction often keep the original mtime (and exFAT has 2 s
/// resolution), so mtime alone can match a different file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileStamp {
    secs: u64,
    nanos: u32,
    len: u64,
}

impl FileStamp {
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta
            .modified()
            .ok()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?;
        Some(Self {
            secs: modified.as_secs(),
            nanos: modified.subsec_nanos(),
            len: meta.len(),
        })
    }
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

#[derive(Default)]
struct ClassifyCache {
    entries: HashMap<PathBuf, CachedClassification>,
    dirty: bool,
}

fn key(path: &Path) -> PathBuf {
    path_util::cache_key(path.to_path_buf())
}

impl ClassifyCache {
    fn load() -> Self {
        let entries = path_util::cache_file(CACHE_FILE)
            .and_then(|path| path_util::read_bincode(&path))
            .unwrap_or_default();
        Self {
            entries,
            dirty: false,
        }
    }

    fn persist(&mut self) {
        if self.dirty
            && let Some(path) = path_util::cache_file(CACHE_FILE)
        {
            self.persist_to(&path);
        }
    }

    fn persist_to(&mut self, path: &Path) {
        if path_util::write_bincode(path, &self.entries, "classify cache") {
            self.dirty = false;
        }
    }

    fn get(&self, path: &Path) -> Option<ClassificationResult> {
        let stamp = FileStamp::of(path)?;
        let cached = self.entries.get(&key(path))?;
        (cached.stamp == stamp).then(|| ClassificationResult {
            instrument: cached.instrument.clone(),
            tier: cached.tier,
            zcr: cached.zcr,
            confidence: cached.confidence,
            summary: cached.summary.clone(),
        })
    }

    fn insert(&mut self, path: &Path, stamp: FileStamp, result: &ClassificationResult) {
        self.entries.insert(
            key(path),
            CachedClassification {
                stamp,
                instrument: result.instrument.clone(),
                tier: result.tier,
                zcr: result.zcr,
                confidence: result.confidence,
                summary: result.summary.clone(),
            },
        );
        self.dirty = true;
    }
}

static CLASSIFY_CACHE: LazyLock<Mutex<ClassifyCache>> =
    LazyLock::new(|| Mutex::new(ClassifyCache::load()));

fn cache() -> MutexGuard<'static, ClassifyCache> {
    CLASSIFY_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn get_cached(path: &Path) -> Option<ClassificationResult> {
    cache().get(path)
}

/// Remember `result` for the file as it was when `stamp` was taken, before
/// analysis started. If the file changed since, the entry simply never matches.
pub fn store_cached(path: &Path, stamp: FileStamp, result: &ClassificationResult) {
    cache().insert(path, stamp, result);
}

pub fn flush_cache() {
    cache().persist();
}

pub fn clear_cache() {
    let mut cache = cache();
    cache.entries.clear();
    cache.dirty = true;
    cache.persist();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::ScratchDir;

    fn kick() -> ClassificationResult {
        ClassificationResult {
            instrument: "Kick".into(),
            tier: 1,
            zcr: None,
            confidence: Some(0.9),
            summary: "kick".into(),
        }
    }

    #[test]
    fn entries_match_only_the_stamped_file() {
        let dir = ScratchDir::new("classify-stamp");
        let audio = dir.path().join("kick.wav");
        std::fs::write(&audio, b"audio").expect("audio");
        let stamp = FileStamp::of(&audio).expect("stamp");

        let mut cache = ClassifyCache::default();
        cache.insert(&audio, stamp, &kick());
        assert_eq!(
            cache.get(&audio).map(|result| result.instrument),
            Some("Kick".into())
        );

        let modified = std::fs::metadata(&audio)
            .and_then(|meta| meta.modified())
            .expect("mtime");
        std::fs::write(&audio, b"different audio").expect("replace");
        std::fs::File::options()
            .write(true)
            .open(&audio)
            .and_then(|file| file.set_modified(modified))
            .expect("keep mtime");
        assert!(cache.get(&audio).is_none(), "same mtime, different size");
    }

    #[test]
    fn persisted_cache_round_trips() {
        let dir = ScratchDir::new("classify-persist");
        let path = dir.path().join(CACHE_FILE);
        let audio = dir.path().join("kick.wav");
        std::fs::write(&audio, b"audio").expect("audio");

        let mut cache = ClassifyCache::default();
        cache.insert(&audio, FileStamp::of(&audio).expect("stamp"), &kick());
        cache.persist_to(&path);
        assert!(!cache.dirty);
        assert_eq!(dir.sidecar_count(), 0);

        let entries: HashMap<PathBuf, CachedClassification> =
            path_util::read_bincode(&path).expect("reload");
        let reloaded = ClassifyCache {
            entries,
            dirty: false,
        };
        assert!(reloaded.get(&audio).is_some());
    }
}
