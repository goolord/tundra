//! Classification results remembered across runs, keyed by path and stamped
//! with the file's size and modification time.

use super::ClassificationResult;
use crate::app_data;
use crate::locks::lock;
use crate::path_util::{FileStamp, cache_key};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

// v6: tier 2 is YAMNet; labels from earlier models are not reused.
const CACHE_FILE: &str = "classify_cache_v6.bin";

/// Bincode writes tuples and structs alike (fields inline), so the tuple
/// values keep the v6 file layout.
#[derive(Default)]
struct ClassifyCache {
    entries: HashMap<PathBuf, (FileStamp, ClassificationResult)>,
    dirty: bool,
}

impl ClassifyCache {
    fn persist_to(&mut self, path: &Path) {
        if app_data::write_bincode(path, &self.entries, "classify cache") {
            self.dirty = false;
        }
    }

    fn get(&self, path: &Path) -> Option<ClassificationResult> {
        let (stamp, result) = self.entries.get(&cache_key(path))?;
        (Some(*stamp) == FileStamp::of(path)).then(|| result.clone())
    }
}

static CACHE: LazyLock<Mutex<ClassifyCache>> = LazyLock::new(|| {
    let entries = app_data::cache_file(CACHE_FILE).and_then(|path| app_data::read_bincode(&path));
    Mutex::new(ClassifyCache { entries: entries.unwrap_or_default(), dirty: false })
});

pub fn get_cached(path: &Path) -> Option<ClassificationResult> {
    lock(&CACHE).get(path)
}

/// Remembers `result` for the file as it was when `stamp` was taken, before
/// analysis started. If the file changed since, the entry never matches.
pub fn store_cached(path: &Path, stamp: FileStamp, result: &ClassificationResult) {
    let mut cache = lock(&CACHE);
    cache.entries.insert(cache_key(path), (stamp, result.clone()));
    cache.dirty = true;
}

pub fn flush_cache() {
    let mut cache = lock(&CACHE);
    if cache.dirty
        && let Some(path) = app_data::cache_file(CACHE_FILE)
    {
        cache.persist_to(&path);
    }
}

pub fn clear_cache() {
    let mut cache = lock(&CACHE);
    cache.entries.clear();
    cache.dirty = true;
    drop(cache);
    flush_cache();
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
        cache.entries.insert(cache_key(&audio), (FileStamp::of(&audio).expect("stamp"), kick));
        cache.dirty = true;

        let file = dir.path().join(CACHE_FILE);
        cache.persist_to(&file);
        assert!(!cache.dirty);
        assert_eq!(dir.sidecar_count(), 0);
        let cache = ClassifyCache { entries: app_data::read_bincode(&file).expect("reload"), dirty: false };
        assert_eq!(cache.get(&audio).map(|result| result.instrument), Some("Kick".into()));

        let modified = std::fs::metadata(&audio).and_then(|meta| meta.modified()).expect("mtime");
        std::fs::write(&audio, b"different audio").expect("replace");
        std::fs::File::options()
            .write(true)
            .open(&audio)
            .and_then(|file| file.set_modified(modified))
            .expect("keep mtime");
        assert!(cache.get(&audio).is_none(), "same mtime, different size");
    }
}
