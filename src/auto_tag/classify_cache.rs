//! Classification results remembered across runs, keyed by path and stamped
//! with the file's size and modification time.

use super::ClassificationResult;
use crate::app_data;
use crate::path_util::{FileStamp, cache_key};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

// v6: tier 2 is YAMNet; labels from earlier models are not reused.
const CACHE_FILE: &str = "classify_cache_v6.bin";

/// Bincode writes nested structs inline, so this is still the v6 file layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedClassification {
    stamp: FileStamp,
    result: ClassificationResult,
}

#[derive(Default)]
struct ClassifyCache {
    entries: HashMap<PathBuf, CachedClassification>,
    dirty: bool,
}

impl ClassifyCache {
    fn persist_to(&mut self, path: &Path) {
        if app_data::write_bincode(path, &self.entries, "classify cache") {
            self.dirty = false;
        }
    }

    fn persist(&mut self) {
        if self.dirty
            && let Some(path) = app_data::cache_file(CACHE_FILE)
        {
            self.persist_to(&path);
        }
    }

    fn get(&self, path: &Path) -> Option<ClassificationResult> {
        let cached = self.entries.get(&cache_key(path))?;
        (Some(cached.stamp) == FileStamp::of(path)).then(|| cached.result.clone())
    }

    /// Remembers `result` for the file as it was when `stamp` was taken, before
    /// analysis started. If the file changed since, the entry never matches.
    fn insert(&mut self, path: &Path, stamp: FileStamp, result: &ClassificationResult) {
        let result = result.clone();
        self.entries
            .insert(cache_key(path), CachedClassification { stamp, result });
        self.dirty = true;
    }
}

static CACHE: LazyLock<Mutex<ClassifyCache>> = LazyLock::new(|| {
    let entries = app_data::cache_file(CACHE_FILE).and_then(|path| app_data::read_bincode(&path));
    Mutex::new(ClassifyCache {
        entries: entries.unwrap_or_default(),
        dirty: false,
    })
});

pub fn get_cached(path: &Path) -> Option<ClassificationResult> {
    crate::locks::lock(&CACHE).get(path)
}

pub fn store_cached(path: &Path, stamp: FileStamp, result: &ClassificationResult) {
    crate::locks::lock(&CACHE).insert(path, stamp, result);
}

pub fn flush_cache() {
    crate::locks::lock(&CACHE).persist();
}

pub fn clear_cache() {
    let mut cache = crate::locks::lock(&CACHE);
    cache.entries.clear();
    cache.dirty = true;
    cache.persist();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fixtures::ScratchDir;

    #[test]
    fn entries_round_trip_and_match_only_the_stamped_file() {
        let dir = ScratchDir::new("classify-cache");
        let audio = dir.path().join("kick.wav");
        std::fs::write(&audio, b"audio").expect("audio");
        let kick = ClassificationResult {
            instrument: "Kick".into(),
            tier: 1,
            zcr: None,
            confidence: Some(0.9),
            summary: "kick".into(),
        };
        let mut cache = ClassifyCache::default();
        cache.insert(&audio, FileStamp::of(&audio).expect("stamp"), &kick);

        let file = dir.path().join(CACHE_FILE);
        cache.persist_to(&file);
        assert!(!cache.dirty);
        assert_eq!(dir.sidecar_count(), 0);
        let cache = ClassifyCache {
            entries: app_data::read_bincode(&file).expect("reload"),
            dirty: false,
        };
        assert_eq!(cache.get(&audio).map(|result| result.instrument), Some("Kick".into()));

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
}
