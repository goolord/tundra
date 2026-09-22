//! Crash/recovery and sidecar-fallback flows across safe_write, metadata, and tag_store.

use std::fs;
use std::path::{Path, PathBuf};

use crate::metadata::{ManualTagEdits, TUNDRA_TAG_VERSION, read_tag_fields, write_auto_tags, write_manual_tags};
use crate::safe_write::{REPLACE_OLD_SUFFIX, reclaim_write_sidecars, sidecar};
use crate::test_fixtures::{ASSET_FORMATS, ScratchDir, copy_asset, write_minimal_wav};

#[test]
fn write_auto_tags_falls_back_to_the_sidecar_and_leaves_no_temps() {
    let dir = ScratchDir::new("auto-tags");
    let broken = dir.path().join("broken.wav");
    let junk = b"not a riff file";
    fs::write(&broken, junk).expect("junk");
    let wav = dir.path().join("kick.wav");
    write_minimal_wav(&wav);

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        assert!(
            write_auto_tags(&broken, "Kick").expect("fallback write"),
            "unwritable container uses the sidecar"
        );
        assert_eq!(fs::read(&broken).expect("bytes unchanged"), junk);
        assert_eq!(crate::tag_store::instrument(&broken).as_deref(), Some("Kick"));
        assert_eq!(crate::tag_store::tag_version(&broken), Some(TUNDRA_TAG_VERSION));
        write_auto_tags(&wav, "Kick").expect("native write");
    });
    assert_eq!(dir.sidecar_count(), 0);
}

/// Seeds a cache file with `persist`, simulates a crash that left only the
/// crash-aside copy, and checks reclaim restores it and a later save is clean.
fn cache_recovers_from_crash_aside(label: &str, file: &str, persist: impl Fn(&Path)) {
    let dir = ScratchDir::new(label);
    let path = dir.path().join(file);
    persist(&path);
    let bytes = fs::read(&path).expect("persisted");
    fs::rename(&path, sidecar(&path, REPLACE_OLD_SUFFIX)).expect("crash aside");

    reclaim_write_sidecars(dir.path());
    assert_eq!(fs::read(&path).expect("restored"), bytes);
    persist(&path);
    assert_eq!(dir.sidecar_count(), 0);
}

#[test]
fn caches_recover_from_crash_aside() {
    use crate::library::cache::{DirCache, MetadataCache};
    use std::collections::HashMap;

    let root = PathBuf::from("samples");
    let dirs = HashMap::from([(root.clone(), vec![root.join("kick.wav")])]);
    cache_recovers_from_crash_aside("dir-cache", "dir_cache.bin", |path| {
        DirCache::persist_map_to(path, &dirs)
    });

    let cached = crate::metadata::CachedMetadata {
        mtime_secs: 1,
        fields: crate::metadata::TagFields::default(),
    };
    let metadata = HashMap::from([(root.join("kick.wav"), cached)]);
    cache_recovers_from_crash_aside("metadata-cache", "metadata_cache.bin", |path| {
        MetadataCache::persist_map_to(path, &metadata)
    });
}

// ---------------------------------------------------------------------------
// Tag writes: every container keeps its audio and reads back what was written.
// ---------------------------------------------------------------------------

fn decoded_samples(path: &Path) -> Vec<f32> {
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
fn manual_tags_round_trip_and_clear_in_every_container_without_touching_audio() {
    for ext in ASSET_FORMATS {
        let dir = ScratchDir::new(&format!("round-trip-{ext}"));
        let audio = copy_asset(dir.path(), "tone", ext);
        let before = decoded_samples(&audio);

        crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
            let read_back = || ManualTagEdits::from_tag_fields(&read_tag_fields(&audio).expect("read back"));
            write_manual_tags(&audio, &full_edits()).unwrap_or_else(|err| panic!("{ext}: {err}"));
            assert!(
                crate::tag_store::manual_fields(&audio).is_none(),
                "{ext}: a native write must not leave sidecar fields behind"
            );
            assert_eq!(read_back(), full_edits(), "{ext}");

            let cleared = ManualTagEdits {
                title: String::new(),
                bpm: String::new(),
                key: String::new(),
                genre: String::new(),
                ..full_edits()
            };
            write_manual_tags(&audio, &cleared).expect("clear");
            assert_eq!(read_back(), cleared, "{ext}: cleared fields are removed");
        });
        assert_eq!(decoded_samples(&audio), before, "{ext}: audio must be unchanged");
        assert_eq!(dir.sidecar_count(), 0, "{ext}: no temp files left");
    }
}

#[test]
fn mp3_write_keeps_id3_frames_tundra_does_not_manage() {
    use lofty::config::{ParseOptions, WriteOptions};
    use lofty::file::AudioFile;
    use lofty::mpeg::MpegFile;

    let dir = ScratchDir::new("mp3-foreign-frames");
    let audio = copy_asset(dir.path(), "tone", "mp3");
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
fn write_aborts_when_the_file_changes_or_disappears_mid_write() {
    const EXTERNAL: &[u8] = b"another program saved this";
    type Edit = (&'static str, fn(&Path), Option<&'static [u8]>);
    let edits: [Edit; 2] = [
        (
            "changed-mid-write",
            |dest| fs::write(dest, EXTERNAL).expect("external write"),
            Some(EXTERNAL),
        ),
        (
            "deleted-mid-write",
            |dest| fs::remove_file(dest).expect("user deletes file"),
            None,
        ),
    ];
    for (case, edit, expected) in edits {
        let dir = ScratchDir::new(case);
        let dest = dir.path().join("kick.wav");
        write_minimal_wav(&dest);
        let result = crate::metadata::stage_and_replace(&dest, |_| {
            edit(&dest);
            Ok(())
        });
        let err = result.expect_err("the write must abort");
        assert!(expected.is_none() || err.contains("changed on disk"), "{case}: {err}");
        assert_eq!(
            fs::read(&dest).ok().as_deref(),
            expected,
            "{case}: never overwritten or resurrected"
        );
        assert_eq!(dir.sidecar_count(), 0, "{case}");
    }
}

#[test]
fn sidecar_instrument_survives_a_native_write_of_other_fields() {
    let dir = ScratchDir::new("sidecar-restamp");
    let audio = copy_asset(dir.path(), "tone", "flac");

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
    let real = copy_asset(dir.path(), "tone", "flac");
    let link = dir.path().join("link.flac");
    std::os::unix::fs::symlink(&real, &link).expect("symlink");

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        write_manual_tags(&link, &full_edits()).expect("write through link");
    });

    assert!(fs::symlink_metadata(&link).expect("link").file_type().is_symlink());
    assert_eq!(read_tag_fields(&real).expect("real").title, "Crack");
}

/// `tone.mp3` followed by an APE tag (with or without its header) and ID3v1.
fn mp3_with_trailing_tags(dir: &ScratchDir, ape_header: bool) -> (PathBuf, Vec<u8>) {
    let audio = copy_asset(dir.path(), "tone", "mp3");
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
