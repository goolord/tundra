//! Crash/recovery and sidecar-fallback flows across path_util, metadata, and tag_store.

use std::fs;

use crate::metadata::{write_auto_tags, TUNDRA_TAG_VERSION};
use crate::path_util::{
    reclaim_write_sidecars, sidecar, write_atomic, REPLACE_OLD_SUFFIX, TAG_BAK_SUFFIX,
    TAG_TMP_SUFFIX,
};
use crate::test_fixtures::{dead_pid_tag_tmp, write_minimal_wav, ScratchDir};

#[test]
fn write_atomic_leaves_dest_unchanged_when_replace_fails() {
    let dir = ScratchDir::new("atomic-replace-fail");
    let dest = dir.path().join("settings.bin");
    fs::write(&dest, b"stable").expect("seed");
    fs::remove_file(&dest).expect("remove file");
    fs::create_dir(&dest).expect("dest is dir");

    let err = write_atomic(&dest, b"partial");
    assert!(err.is_err(), "replace into a directory must fail");
    assert!(dest.is_dir());
    assert_eq!(dir.sidecar_count(), 0, "failed replace must delete tmp");
}

#[test]
fn reclaim_then_write_atomic_leaves_no_stale_sidecars() {
    let dir = ScratchDir::new("reclaim-atomic");
    let dest = dir.path().join("kick.wav");
    fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"recovered").expect("aside");
    fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"stale").expect("legacy tmp");

    reclaim_write_sidecars(dir.path());
    assert_eq!(fs::read(&dest).expect("restored"), b"recovered");

    write_atomic(&dest, b"tagged").expect("atomic write");
    assert_eq!(fs::read(&dest).expect("read"), b"tagged");
    assert_eq!(dir.sidecar_count(), 0);
}

#[test]
fn simulated_tag_crash_leaves_original_and_reclaim_cleans_stale_tmp() {
    let dir = ScratchDir::new("tag-crash");
    let dest = dir.path().join("kick.wav");
    write_minimal_wav(&dest);
    let original = fs::read(&dest).expect("original bytes");

    let tmp = dead_pid_tag_tmp(&dest);
    fs::copy(&dest, &tmp).expect("stage copy");
    fs::write(&tmp, b"corrupt partial write").expect("failed edit simulation");

    assert_eq!(fs::read(&dest).expect("dest"), original);
    assert!(tmp.exists());

    reclaim_write_sidecars(dir.path());
    assert_eq!(fs::read(&dest).expect("dest"), original);
    assert!(!tmp.exists(), "dead pid tmp must be deleted");
}

#[test]
fn write_auto_tags_failed_container_preserves_bytes_and_uses_sidecar() {
    let dir = ScratchDir::new("sidecar-fallback");
    let dest = dir.path().join("broken.wav");
    let junk = b"not a riff file";
    fs::write(&dest, junk).expect("junk");
    let db = dir.path().join("tags.db");

    crate::tag_store::with_test_db(db, || {
        assert!(
            write_auto_tags(&dest, "Kick").expect("fallback write"),
            "unwritable container should record sidecar"
        );
        assert_eq!(fs::read(&dest).expect("bytes unchanged"), junk);
        assert_eq!(
            crate::tag_store::instrument(&dest).as_deref(),
            Some("Kick")
        );
        assert_eq!(
            crate::tag_store::tag_version(&dest),
            Some(TUNDRA_TAG_VERSION)
        );
    });
}

#[test]
fn write_auto_tags_success_leaves_no_tag_tmp_sidecars() {
    let dir = ScratchDir::new("tag-success");
    let dest = dir.path().join("kick.wav");
    write_minimal_wav(&dest);
    let db = dir.path().join("tags.db");

    crate::tag_store::with_test_db(db, || {
        write_auto_tags(&dest, "Kick").expect("native write");
    });
    assert_eq!(dir.sidecar_count(), 0);

    let aside = sidecar(&dest, REPLACE_OLD_SUFFIX);
    let legacy_tmp = sidecar(&dest, TAG_TMP_SUFFIX);
    let legacy_bak = sidecar(&dest, TAG_BAK_SUFFIX);
    assert!(!aside.exists());
    assert!(!legacy_tmp.exists());
    assert!(!legacy_bak.exists());
}

#[test]
fn user_deleted_audio_is_not_resurrected_from_tmp_or_bak() {
    let dir = ScratchDir::new("user-delete");
    let dest = dir.path().join("gone.wav");
    fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"tmp-body").expect("tmp");
    fs::write(sidecar(&dest, TAG_BAK_SUFFIX), b"bak-body").expect("bak");

    reclaim_write_sidecars(dir.path());

    assert!(!dest.exists());
    assert!(sidecar(&dest, TAG_TMP_SUFFIX).exists());
    assert!(sidecar(&dest, TAG_BAK_SUFFIX).exists());
}

#[test]
fn replace_old_restore_does_not_resurrect_from_tmp_or_bak_when_dest_missing() {
    let dir = ScratchDir::new("restore-priority");
    let dest = dir.path().join("hat.wav");
    fs::write(sidecar(&dest, TAG_TMP_SUFFIX), b"from-tmp").expect("tmp");
    fs::write(sidecar(&dest, TAG_BAK_SUFFIX), b"from-bak").expect("bak");
    fs::write(sidecar(&dest, REPLACE_OLD_SUFFIX), b"from-aside").expect("aside");

    reclaim_write_sidecars(dir.path());

    assert_eq!(fs::read(&dest).expect("restored"), b"from-aside");
}

#[test]
fn dir_cache_persist_recovers_from_crash_aside() {
    use std::collections::HashMap;

    let dir = ScratchDir::new("dir-cache-persist");
    let path = dir.path().join("dir_cache.bin");
    let root = dir.path().join("samples");
    let mut map = HashMap::new();
    map.insert(root.clone(), vec![root.join("kick.wav")]);

    crate::types::DirCache::persist_map_to(&path, &map);
    let bytes = fs::read(&path).expect("persisted");

    crate::test_fixtures::restore_dest_from_crash_aside(dir.path(), &path, &bytes);
    assert_eq!(fs::read(&path).expect("restored"), bytes);

    crate::types::DirCache::persist_map_to(&path, &map);
    assert_eq!(dir.sidecar_count(), 0);
}

#[test]
fn metadata_cache_persist_recovers_from_crash_aside() {
    use std::collections::HashMap;

    let dir = ScratchDir::new("metadata-cache-persist");
    let path = dir.path().join("metadata_cache_v10.bin");
    let audio = dir.path().join("kick.wav");
    fs::write(&audio, b"audio").expect("audio");
    let mut map = HashMap::new();
    map.insert(
        audio.clone(),
        crate::metadata::CachedMetadata {
            mtime_secs: 1,
            fields: crate::metadata::TagFields::default(),
        },
    );

    crate::types::MetadataCache::persist_map_to(&path, &map);
    let bytes = fs::read(&path).expect("persisted");

    crate::test_fixtures::restore_dest_from_crash_aside(dir.path(), &path, &bytes);
    assert_eq!(fs::read(&path).expect("restored"), bytes);

    crate::types::MetadataCache::persist_map_to(&path, &map);
    assert_eq!(dir.sidecar_count(), 0);
}
