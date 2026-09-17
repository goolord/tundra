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

    crate::library::cache::DirCache::persist_map_to(&path, &map);
    let bytes = fs::read(&path).expect("persisted");

    crate::test_fixtures::restore_dest_from_crash_aside(dir.path(), &path, &bytes);
    assert_eq!(fs::read(&path).expect("restored"), bytes);

    crate::library::cache::DirCache::persist_map_to(&path, &map);
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

    crate::library::cache::MetadataCache::persist_map_to(&path, &map);
    let bytes = fs::read(&path).expect("persisted");

    crate::test_fixtures::restore_dest_from_crash_aside(dir.path(), &path, &bytes);
    assert_eq!(fs::read(&path).expect("restored"), bytes);

    crate::library::cache::MetadataCache::persist_map_to(&path, &map);
    assert_eq!(dir.sidecar_count(), 0);
}

// ---------------------------------------------------------------------------
// Tag writes: every container keeps its audio and reads back what was written.
// ---------------------------------------------------------------------------

use crate::metadata::{read_tag_fields, write_manual_tags, ManualTagEdits};

fn fixture_copy(dir: &ScratchDir, ext: &str) -> std::path::PathBuf {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/assets")
        .join(format!("tone.{ext}"));
    let dest = dir.path().join(format!("tone.{ext}"));
    fs::copy(&fixture, &dest).expect("copy fixture");
    dest
}

fn decoded_samples(path: &std::path::Path) -> Vec<f32> {
    let file = fs::File::open(path).expect("open audio");
    rodio::Decoder::try_from(file).expect("decode audio").collect()
}

fn full_edits() -> ManualTagEdits {
    ManualTagEdits {
        instrument: "Snare".into(),
        artist: "Tundra Test".into(),
        title: "Crack".into(),
        bpm: "128".into(),
        key: "F#m".into(),
        genre: "Drums".into(),
        comment: "hand tagged".into(),
    }
}

#[test]
fn manual_tags_round_trip_in_every_container_without_touching_audio() {
    for ext in ["wav", "flac", "mp3", "ogg", "aiff"] {
        let dir = ScratchDir::new(&format!("round-trip-{ext}"));
        let audio = fixture_copy(&dir, ext);
        let before = decoded_samples(&audio);

        crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
            write_manual_tags(&audio, &full_edits()).unwrap_or_else(|err| panic!("{ext}: {err}"));
            assert!(
                crate::tag_store::manual_fields(&audio).is_none(),
                "{ext}: a native write must not leave sidecar fields behind"
            );

            let fields = read_tag_fields(&audio).expect("read back");
            let edits = full_edits();
            assert_eq!(fields.instrument, edits.instrument, "{ext} instrument");
            assert_eq!(fields.artist, edits.artist, "{ext} artist");
            assert_eq!(fields.title, edits.title, "{ext} title");
            assert_eq!(fields.bpm, edits.bpm, "{ext} bpm");
            assert_eq!(fields.key, edits.key, "{ext} key");
            assert_eq!(fields.genre, edits.genre, "{ext} genre");
            assert_eq!(fields.comment, edits.comment, "{ext} comment");
        });
        assert_eq!(decoded_samples(&audio), before, "{ext}: audio must be unchanged");
        assert_eq!(dir.sidecar_count(), 0, "{ext}: no temp files left");
    }
}

#[test]
fn clearing_manual_fields_removes_them() {
    for ext in ["wav", "flac", "mp3", "ogg", "aiff"] {
        let dir = ScratchDir::new(&format!("clear-{ext}"));
        let audio = fixture_copy(&dir, ext);
        crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
            write_manual_tags(&audio, &full_edits()).expect("seed");
            let cleared = ManualTagEdits {
                title: String::new(),
                bpm: String::new(),
                key: String::new(),
                genre: String::new(),
                ..full_edits()
            };
            write_manual_tags(&audio, &cleared).expect("clear");

            let fields = read_tag_fields(&audio).expect("read back");
            assert_eq!(fields.title, "", "{ext} title");
            assert_eq!(fields.bpm, "", "{ext} bpm");
            assert_eq!(fields.key, "", "{ext} key");
            assert_eq!(fields.genre, "", "{ext} genre");
            assert_eq!(fields.instrument, "Snare", "{ext} instrument kept");
        });
    }
}

#[test]
fn mp3_write_keeps_id3_frames_tundra_does_not_manage() {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::mpeg::MpegFile;

    let dir = ScratchDir::new("mp3-foreign-frames");
    let audio = fixture_copy(&dir, "mp3");
    {
        let mut file = fs::File::open(&audio).expect("open");
        let mut mp3 = MpegFile::read_from(&mut file, ParseOptions::new()).expect("parse");
        drop(file);
        let mut id3 = mp3.remove_id3v2().unwrap_or_default();
        id3.insert_user_text("DAW_PROJECT".into(), "session-42".into());
        mp3.set_id3v2(id3);
        mp3.save_to_path(&audio, WriteOptions::default())
            .expect("seed foreign frame");
    }

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        write_manual_tags(&audio, &full_edits()).expect("manual write");
    });

    let mut file = fs::File::open(&audio).expect("open");
    let mp3 = MpegFile::read_from(&mut file, ParseOptions::new()).expect("parse");
    assert_eq!(
        mp3.id3v2().and_then(|tag| tag.get_user_text("DAW_PROJECT")),
        Some("session-42")
    );
}

#[test]
fn wav_bpm_and_key_go_to_an_id3_chunk_and_sampler_chunks_survive() {
    let dir = ScratchDir::new("wav-id3-chunk");
    let audio = dir.path().join("loop.wav");
    let mut bytes = crate::test_fixtures::minimal_wav_bytes();
    let smpl = [0x5A_u8; 36];
    bytes.extend_from_slice(b"smpl");
    bytes.extend_from_slice(&(smpl.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&smpl);
    let riff_len = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff_len.to_le_bytes());
    fs::write(&audio, &bytes).expect("wav");

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        write_manual_tags(&audio, &full_edits()).expect("manual write");
    });

    let tagged = fs::read(&audio).expect("read");
    let chunks = crate::metadata::parse_riff_wave_chunks(&tagged).expect("parse");
    assert!(chunks.iter().any(|(id, data)| id == b"smpl" && data[..] == smpl[..]));
    assert!(chunks.iter().any(|(id, _)| id == b"id3 "));
    let fields = read_tag_fields(&audio).expect("read back");
    assert_eq!((fields.bpm.as_str(), fields.key.as_str()), ("128", "F#m"));
}

#[test]
fn write_refuses_when_the_file_changes_mid_write() {
    let dir = ScratchDir::new("changed-mid-write");
    let dest = dir.path().join("kick.wav");
    write_minimal_wav(&dest);

    let err = crate::metadata::stage_and_replace(&dest, |_| {
        fs::write(&dest, b"another program saved this").expect("external write");
        Ok(())
    })
    .expect_err("concurrent change must abort the write");

    assert!(err.contains("changed on disk"), "{err}");
    assert_eq!(fs::read(&dest).expect("dest"), b"another program saved this");
    assert_eq!(dir.sidecar_count(), 0);
}

#[test]
fn write_does_not_resurrect_a_file_deleted_mid_write() {
    let dir = ScratchDir::new("deleted-mid-write");
    let dest = dir.path().join("kick.wav");
    write_minimal_wav(&dest);

    let result = crate::metadata::stage_and_replace(&dest, |_| {
        fs::remove_file(&dest).expect("user deletes file");
        Ok(())
    });

    assert!(result.is_err());
    assert!(!dest.exists());
    assert_eq!(dir.sidecar_count(), 0);
}

#[test]
fn sidecar_instrument_survives_a_native_write_of_other_fields() {
    let dir = ScratchDir::new("sidecar-restamp");
    let audio = fixture_copy(&dir, "flac");

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        crate::tag_store::set_instrument(&audio, "Kick", TUNDRA_TAG_VERSION).expect("sidecar");
        let edits = ManualTagEdits {
            title: "Boom".into(),
            ..ManualTagEdits::default()
        };
        write_manual_tags(&audio, &edits).expect("native write");
        assert_eq!(
            crate::tag_store::instrument(&audio).as_deref(),
            Some("Kick"),
            "Tundra's own write must not orphan the sidecar row"
        );
    });
}

#[cfg(unix)]
#[test]
fn tags_are_written_through_a_symlink() {
    let dir = ScratchDir::new("symlink-write");
    let real = fixture_copy(&dir, "flac");
    let link = dir.path().join("link.flac");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        write_manual_tags(&link, &full_edits()).expect("write through link");
    });

    assert!(fs::symlink_metadata(&link).expect("link").file_type().is_symlink());
    assert_eq!(read_tag_fields(&real).expect("real").title, "Crack");
}

/// `tone.mp3` followed by an APE tag (with or without its header) and ID3v1.
fn mp3_with_trailing_tags(dir: &ScratchDir, ape_header: bool) -> (std::path::PathBuf, Vec<u8>) {
    let audio = fixture_copy(dir, "mp3");
    let mut bytes = fs::read(&audio).expect("mp3");
    let ape_part = |flags: u32| {
        let mut part = Vec::from(*b"APETAGEX");
        part.extend_from_slice(&2000u32.to_le_bytes());
        part.extend_from_slice(&32u32.to_le_bytes());
        part.extend_from_slice(&0u32.to_le_bytes());
        part.extend_from_slice(&flags.to_le_bytes());
        part.extend_from_slice(&[0u8; 8]);
        part
    };
    if ape_header {
        bytes.extend(ape_part(0xA000_0000));
        bytes.extend(ape_part(0x8000_0000));
    } else {
        bytes.extend(ape_part(0));
    }
    let mut id3v1 = vec![0u8; 128];
    id3v1[..3].copy_from_slice(b"TAG");
    id3v1[3..8].copy_from_slice(b"Old!!");
    bytes.extend_from_slice(&id3v1);
    fs::write(&audio, &bytes).expect("write");
    (audio, id3v1)
}

#[test]
fn mp3_with_trailing_ape_and_id3v1_tags_is_written_in_place() {
    let dir = ScratchDir::new("mp3-trailing-tags");
    let (audio, id3v1) = mp3_with_trailing_tags(&dir, true);
    let before = decoded_samples(&audio);

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        write_manual_tags(&audio, &full_edits()).expect("manual write");
        assert!(crate::tag_store::manual_fields(&audio).is_none(), "written natively");
    });

    let tagged = fs::read(&audio).expect("tagged");
    assert_eq!(&tagged[tagged.len() - 128..], &id3v1[..], "ID3v1 kept");
    assert_eq!(read_tag_fields(&audio).expect("read").title, "Crack");
    assert_eq!(decoded_samples(&audio), before);
}

/// lofty removes 32 bytes too many when it drops a header-less (APEv1-style)
/// APE tag. The audio check must catch that and keep the original.
#[test]
fn write_that_would_truncate_audio_is_refused_and_falls_back_to_sidecar() {
    let dir = ScratchDir::new("mp3-apev1");
    let (audio, _) = mp3_with_trailing_tags(&dir, false);
    let original = fs::read(&audio).expect("original");

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        write_manual_tags(&audio, &full_edits()).expect("sidecar fallback");
        assert_eq!(fs::read(&audio).expect("unchanged"), original);
        assert_eq!(
            crate::tag_store::manual_fields(&audio).map(|fields| fields.title),
            Some("Crack".into())
        );
    });
    assert_eq!(dir.sidecar_count(), 0);
}

#[test]
fn misnamed_files_are_not_tagged_in_a_format_reads_will_not_find() {
    let dir = ScratchDir::new("misnamed");
    let audio = dir.path().join("actually-a-wav.mp3");
    write_minimal_wav(&audio);
    let original = fs::read(&audio).expect("original");

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        let saved = write_manual_tags(&audio, &full_edits()).expect("sidecar fallback");
        assert!(matches!(saved, crate::metadata::SavedTo::Sidecar(_)), "{saved:?}");
        assert_eq!(fs::read(&audio).expect("unchanged"), original);
        assert_eq!(read_tag_fields(&audio).expect("read").title, "Crack");
    });
}
