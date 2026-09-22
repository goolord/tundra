//! Reading, writing, and searching tags on real files. Scoring details live in
//! `search.rs`; crash recovery and sidecar safety live in `data_safety_tests.rs`.

use super::*;
use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::AudioFile;
use lofty::tag::Tag;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::path_util::{cache_key, file_mtime_secs};
use crate::test_fixtures::{ASSET_FORMATS, ScratchDir, copy_asset, write_minimal_wav, write_riff_info};

use super::hints::artist_hint_from_path;
use super::read::tundra_comment;
use super::riff::encode_riff_wave;

// --- Helpers ---------------------------------------------------------------

/// Copies a real encoder-produced fixture into a scratch directory. Named
/// `sample.<ext>` so no instrument can be inferred from the filename.
fn staged_fixture(ext: &str, label: &str) -> (ScratchDir, PathBuf) {
    let dir = ScratchDir::new(label);
    let audio = copy_asset(dir.path(), "sample", ext);
    (dir, audio)
}

/// A minimal WAV at `dir/<folders>/<name>`.
fn wav_in(dir: &ScratchDir, folders: &str, name: &str) -> PathBuf {
    let folder = dir.path().join(folders);
    std::fs::create_dir_all(&folder).expect("create folders");
    let audio = folder.join(name);
    write_minimal_wav(&audio);
    audio
}

fn filter(field: TagField, value: &str) -> TagFilter {
    TagFilter { field, value: value.to_string() }
}

/// Searches `paths` the way the library does, reading tags from `metadata`.
fn search_paths(
    paths: &[PathBuf],
    text: &str,
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> Vec<PathBuf> {
    let query = SearchQuery { text, tag_filters, ..SearchQuery::default() };
    search(paths, &query, MetadataLookup::new(metadata)).paths
}

/// Mirrors the app: index the file, then search the index.
fn finds_by_instrument(path: &Path, query: &str) -> bool {
    let paths = vec![path.to_path_buf()];
    let indexed = Arc::new(index_paths(&paths, Arc::new(HashMap::new())));
    search_paths(&paths, "", &[filter(TagField::Instrument, query)], indexed) == paths
}

fn riff(path: &Path, key: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).expect("open wav");
    let info = lofty::iff::wav::WavFile::read_from(&mut file, ParseOptions::new())
        .expect("parse wav")
        .remove_riff_info()
        .expect("wav should carry a RIFF INFO list");
    info.get(key).map(str::to_string)
}

fn vorbis_comments(path: &Path) -> lofty::ogg::tag::VorbisComments {
    let mut file = std::fs::File::open(path).expect("open flac");
    lofty::flac::FlacFile::read_from(&mut file, ParseOptions::new())
        .expect("parse flac")
        .remove_vorbis_comments()
        .expect("flac should carry vorbis comments")
}

// --- Search over the metadata index ----------------------------------------

#[test]
fn tag_search_needs_every_filter_narrows_by_filename_and_skips_unindexed_paths() {
    let fields = TagFields { bpm: "120".into(), key: "Am".into(), instrument: "Kick".into(), ..TagFields::default() };
    let tagged: Vec<_> = ["/s/1 kick.wav", "/s/2 kick.wav"].map(PathBuf::from).into();
    let index = tagged.iter().map(|path| (cache_key(path), CachedMetadata { mtime_secs: 1, fields: fields.clone() }));
    let index = Arc::new(index.collect());
    // Folders, and files a library-wide tag query must not parse.
    let paths: Vec<_> = tagged.iter().cloned().chain(["/s/nested", "/s/snare.wav"].map(PathBuf::from)).collect();
    let search = |text, filters: &[TagFilter]| search_paths(&paths, text, filters, Arc::clone(&index));

    assert_eq!(search("", &[filter(TagField::Bpm, "120")]), tagged);
    let partial = search("", &[filter(TagField::Bpm, "120"), filter(TagField::Key, "Bm")]);
    assert!(partial.is_empty(), "partial tag matches should be rejected");
    assert_eq!(search("2", &[filter(TagField::Instrument, "kick")]), [tagged[1].clone()], "one-char narrowing");
}

#[test]
fn metadata_lookup_matches_key_variants_and_ignores_missing_files() {
    let dir = ScratchDir::new("lookup-keys");
    let audio = dir.path().join("snare.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let fields = TagFields { explicit_instrument: "Snare".into(), ..TagFields::default() };
    let mtime_secs = file_mtime_secs(&audio).expect("scratch file mtime");
    let missing = PathBuf::from(r"C:\missing\tundra-kick.wav");
    let index = Arc::new(HashMap::from([
        (cache_key(&audio), CachedMetadata { mtime_secs, fields: fields.clone() }),
        (missing.clone(), CachedMetadata { mtime_secs: 1, fields }),
    ]));
    let mut lookup = MetadataLookup::new(index);
    assert_eq!(lookup.tag_fields(&audio).explicit_instrument, "Snare");
    assert!(lookup.tag_fields(&missing).explicit_instrument.is_empty());
    assert_eq!(tag_field_best_match("a"), Some(TagField::Album), "ties go to the first label");
}

// --- Hints from paths ------------------------------------------------------

#[test]
fn instrument_hint_reads_names_kit_codes_word_pairs_and_other_languages() {
    let cases = [
        // Folders count, but the file name wins.
        ("/Samples/ADM Samples - Copy/snares/tight_01.wav", Some("Snare")),
        ("/Samples/snares/cymbal_roll.wav", Some("Cymbal")),
        ("/Drums/Kicks/808_kick_01.wav", Some("Kick")),
        ("/Drums/808_hat.wav", Some("Hi-Hat")),
        ("/Drums/snow_01.wav", None),
        ("/hats/tight_01.wav", Some("Hi-Hat")),
        ("/Libraries/Pack A/Drums/One Shots/Snares/tight_01.wav", Some("Snare")),
        ("/Samples/Bongo/hit_01.wav", Some("Percussion")),
        ("/Samples/Bongos/layer.wav", Some("Percussion")),
        ("/Samples/ADM/Perc/tight.wav", Some("Percussion")),
        // `perc` must not hit Kick, nor `bass` hit Kick via `bassdrum`.
        ("/Samples/Perc/01.wav", Some("Percussion")),
        ("/Samples/Bass/low.wav", Some("Bass")),
        // Drum-machine codes when nothing else names the instrument.
        ("/Samples/909 Kit/CH 01.wav", Some("Hi-Hat")),
        ("/Samples/909 Kit/OH.wav", Some("Hi-Hat")),
        ("/Samples/Kit/RD_02.wav", Some("Cymbal")),
        ("/Samples/Kit/CP.wav", Some("Clap")),
        ("/Samples/Kit/LT 3.wav", Some("Tom")),
        // ...but never over a real name anywhere in the path.
        ("/Samples/Vocals/Oh Yeah.wav", Some("Vocal")),
        ("/Samples/Snares/CH.wav", Some("Snare")),
        ("/Samples/Artist - Night EP/Kicks/hit.wav", Some("Kick")),
        // Kit codes count in the kit folder, not in folders further up.
        ("/Samples/909 Kit/OH/01.wav", Some("Hi-Hat")),
        ("/MA/Field Recordings/take_014.wav", None),
        ("/Users/cr/Music/untitled.wav", None),
        // Abbreviations, digits glued to words, and word pairs.
        ("/Samples/KCK_Deep.wav", Some("Kick")),
        ("/Samples/CRSH 1.wav", Some("Cymbal")),
        ("/Samples/Kick01.wav", Some("Kick")),
        ("/Samples/Bass Drum 3.wav", Some("Kick")),
        ("/Samples/808 Bass.wav", Some("Bass")),
        ("/Samples/Hi Hat Open.wav", Some("Hi-Hat")),
        ("/Samples/Finger Snap.wav", Some("Clap")),
        // Single letters (key names, take letters) are not instruments.
        ("/Samples/Pads/C major.wav", Some("Synth")),
        ("/Samples/Snares/Sample A.wav", Some("Snare")),
        ("/Samples/Misc/Sample A.wav", None),
        // Other languages.
        ("/Samples/Caja 01.wav", Some("Snare")),
        ("/Samples/Bombo.wav", Some("Kick")),
        ("/Samples/Grosse Caisse.wav", Some("Kick")),
        ("/Samples/Caisse Claire 2.wav", Some("Snare")),
        ("/Samples/Platillos/hit.wav", Some("Cymbal")),
        ("/Samples/Percusión/hit.wav", Some("Percussion")),
        ("/Samples/Gitarre.wav", Some("Guitar")),
        ("/Samples/Voz.wav", Some("Vocal")),
        ("/Samples/キック_01.wav", Some("Kick")),
        ("/Samples/スネアドラム.wav", Some("Snare")),
        ("/Samples/ハイハット/01.wav", Some("Hi-Hat")),
        ("/Samples/底鼓 01.wav", Some("Kick")),
        ("/Samples/踩镲.wav", Some("Hi-Hat")),
        ("/Samples/베이스.wav", Some("Bass")),
        // Loanwords that merely contain an instrument name.
        ("/Samples/カスタム/hit.wav", None),
        ("/Samples/グループA/hit.wav", None),
        ("/Samples/データベース/hit.wav", None),
        ("/Samples/タム/01.wav", Some("Tom")),
        ("/Samples/オープンハイハット.wav", Some("Hi-Hat")),
    ];
    for (path, expected) in cases {
        assert_eq!(instrument_hint_from_path(Path::new(path)).as_deref(), expected, "{path}");
    }
}

#[test]
fn related_instruments_include_translations_but_not_kit_codes_or_prefixes() {
    for (left, right, related) in [
        ("キック", "Kick", true),
        ("caja", "snare", true),
        ("Hat", "Hi-Hat", true),
        ("kick", "kickdrum", true),
        ("ch", "Hi-Hat", false),
        ("ep", "Piano", false),
        ("Bass", "Kick", false),
        ("bass", "bassdrum", false),
        ("shot", "Rim", false),
    ] {
        assert_eq!(instruments_related(left, right), related, "{left} / {right}");
    }
}

#[test]
fn artist_hint_reads_label_from_directory_layout() {
    for (path, expected) in [
        ("/Samples/KSHMR/Vol4/Kicks/kick.wav", Some("KSHMR")),
        ("/Samples/KSHMR/Kicks/kick.wav", Some("KSHMR")),
        ("/Splice/packs/deadmau5/kick.wav", Some("deadmau5")),
        ("/Samples/Native Instruments/Battery 4/Snares/snare.wav", Some("Native Instruments")),
        ("/Samples/snares/tight_01.wav", None),
    ] {
        assert_eq!(artist_hint_from_path(Path::new(path)).as_deref(), expected, "{path}");
    }
}

// --- Tag ownership: what auto-tag may write or replace ---------------------

#[test]
fn tundra_comment_keeps_user_lines_and_replaces_only_its_own() {
    let marker = format!("Tundra v{TUNDRA_TAG_VERSION}");
    assert_eq!(tundra_comment(None), marker);
    assert_eq!(tundra_comment(Some("  ")), marker);
    assert_eq!(tundra_comment(Some("Recorded live")), "Recorded live");
    assert_eq!(tundra_comment(Some("Tundra v0")), marker);
    assert_eq!(tundra_comment(Some("Recorded live\nINSTRUMENT: Kick\nTundra")), format!("Recorded live\n{marker}"));
}

#[test]
fn auto_tag_replaces_only_instruments_tundra_owns() {
    // Existing RIFF INFO, then whether auto-tag may add its marker and replace the instrument.
    let cases: [(&[(&str, &str)], bool); 3] = [
        // A marker would claim the user's instrument.
        (&[("IKEY", "Snare")], false),
        (&[("IKEY", "Snare"), ("ICMT", "Recorded live")], false),
        // A legacy Tundra marker is eligible for upgrade.
        (&[("IKEY", "Snare"), ("ICMT", "Tundra")], true),
    ];
    for (info, tundra_owned) in cases {
        let dir = ScratchDir::new("ownership");
        let audio = wav_in(&dir, "", "snare.wav");
        write_riff_info(&audio, info);

        let status = auto_tag_field_status(&audio).expect("status");
        let flags = (status.needs_instrument, status.needs_comment, status.can_retag_instrument);
        assert_eq!(flags, (false, tundra_owned, tundra_owned), "{info:?}");
        if !tundra_owned {
            assert!(!write_auto_tags(&audio, "Kick").expect("write"), "{info:?}: user tags stay");
            assert_eq!(instrument_tag(&audio).as_deref(), Some("Snare"));
            assert_eq!(riff(&audio, "ICMT").as_deref(), info.get(1).map(|(_, comment)| *comment));
        }
    }
}

#[test]
fn sidecar_row_does_not_own_user_native_instrument() {
    let dir = ScratchDir::new("sidecar-user-native");
    let audio = wav_in(&dir, "", "snare.wav");
    write_riff_info(&audio, &[("IKEY", "Snare"), ("ICMT", "Recorded live")]);

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        crate::tag_store::set_instrument(&audio, "Kick", 0).expect("stale sidecar");

        assert!(!auto_tag_field_status(&audio).expect("status").can_retag_instrument);
        assert!(
            !write_auto_tags(&audio, "Kick").expect("user tag write attempt"),
            "sidecar must not unlock overwrite of a native user instrument"
        );
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Snare"));
    });
}

// --- Writes ----------------------------------------------------------------

#[test]
fn write_auto_tags_preserves_wav_non_info_chunks() {
    let dir = ScratchDir::new("wav-chunk-preserve");
    let audio = dir.path().join("kick.wav");
    let mut adtl = Vec::from(*b"adtllabl");
    adtl.extend_from_slice(&8u32.to_le_bytes());
    adtl.extend_from_slice(&1u32.to_le_bytes());
    adtl.extend_from_slice(b"cue\0");
    let extra = [
        (*b"smpl", vec![0x11; 60]),
        (*b"cue ", vec![0x22; 28]),
        (*b"inst", vec![0x33; 8]),
        (*b"acid", vec![0x44; 24]),
        (*b"LIST", adtl),
    ];
    let mut chunks = parse_riff_wave_chunks(&crate::test_fixtures::minimal_wav_bytes()).expect("parse minimal wav");
    chunks.extend(extra.iter().cloned());
    std::fs::write(&audio, encode_riff_wave(&chunks)).expect("write wav");

    write_auto_tags(&audio, "Kick").expect("tag wav");

    let chunks = parse_riff_wave_chunks(&std::fs::read(&audio).expect("read")).expect("parse tagged wav");
    for chunk in &extra {
        assert!(chunks.contains(chunk), "{} chunk must survive", String::from_utf8_lossy(&chunk.0));
    }
    assert!(
        chunks.iter().any(|(id, data)| id == b"LIST" && data.starts_with(b"INFO")),
        "LIST INFO must be written without replacing adtl"
    );
    assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
}

#[test]
fn write_auto_tags_keeps_existing_wav_tags_and_user_comment() {
    let dir = ScratchDir::new("extend-existing-tags");
    let audio = wav_in(&dir, "KSHMR/Kicks", "kick.wav");
    write_riff_info(&audio, &[("IGNR", "Drums"), ("ICMT", "already had a note")]);
    assert_eq!(read_tag_fields(&audio).expect("read").artist, "KSHMR", "untagged artist reads the folder hint");

    write_auto_tags(&audio, "Kick").expect("extend tags");

    assert_eq!(riff(&audio, "IKEY").as_deref(), Some("Kick"));
    assert_eq!(riff(&audio, "IGNR").as_deref(), Some("Drums"));
    assert_eq!(riff(&audio, "IART").as_deref(), Some("KSHMR"), "artist from the folder");
    assert_eq!(
        riff(&audio, "ICMT").as_deref(),
        Some("already had a note"),
        "custom user comments must be kept as written"
    );
}

/// Auto-tagging must never destroy artwork the user already had.
#[test]
fn tagging_preserves_embedded_cover_art() {
    use lofty::picture::{MimeType, Picture, PictureType};

    let art = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xde, 0xad, 0xbe, 0xef];
    let (_dir, audio) = staged_fixture("mp3", "cover-art");
    let read_mp3 = || {
        let mut file = std::fs::File::open(&audio).expect("open mp3");
        lofty::mpeg::MpegFile::read_from(&mut file, ParseOptions::new()).expect("parse mp3")
    };
    let mut mp3 = read_mp3();
    let mut id3 = mp3.remove_id3v2().unwrap_or_default();
    id3.insert_picture(
        Picture::unchecked(art.clone()).pic_type(PictureType::CoverFront).mime_type(MimeType::Png).build(),
    );
    mp3.set_id3v2(id3);
    mp3.save_to_path(&audio, WriteOptions::default()).expect("save art");

    assert!(write_auto_tags(&audio, "Kick").expect("tag"));

    let tag = read_mp3().remove_id3v2().map(Tag::from).expect("mp3 keeps its ID3v2 tag");
    let pictures: Vec<_> = tag.pictures().iter().map(|picture| picture.data()).collect();
    assert_eq!(pictures, [art.as_slice()], "cover art must survive auto-tagging");
}

/// Files an older build tagged into the grouping field carry no canonical
/// instrument, so the grouping has to survive as the instrument read back
/// and get rewritten to the canonical key.
#[test]
fn legacy_grouping_reads_as_instrument_and_is_rewritten_canonically() {
    let (_dir, audio) = staged_fixture("ogg", "legacy-grouping");
    let mut ogg = {
        let mut file = std::fs::File::open(&audio).expect("open ogg");
        lofty::ogg::VorbisFile::read_from(&mut file, ParseOptions::new()).expect("parse ogg")
    };
    ogg.vorbis_comments_mut().insert("GROUPING".to_string(), "Kick".to_string());
    ogg.save_to_path(&audio, WriteOptions::default()).expect("save grouping");

    assert_eq!(read_tag_fields(&audio).expect("read grouping").instrument, "Kick");
    assert!(
        auto_tag_field_status(&audio).expect("status").needs_instrument,
        "a grouping is not the canonical key, so the file still needs tagging"
    );

    assert!(write_auto_tags(&audio, "Kick").expect("rewrite canonically"));
    assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
    assert!(finds_by_instrument(&audio, "Kick"));
}

// --- End to end: tag, read back, search ------------------------------------

/// The end-to-end contract: auto-tagging a file of any supported format
/// makes `instrument:Kick` find it, and the label reads back from the
/// container so third-party taggers and re-scans agree.
#[test]
fn instrument_round_trips_and_is_searchable_for_every_format() {
    for ext in ASSET_FORMATS {
        let (_dir, audio) = staged_fixture(ext, &format!("round-trip-{ext}"));
        let write = || write_auto_tags(&audio, "Kick").unwrap_or_else(|err| panic!("{ext}: {err}"));

        assert!(write(), "{ext}: first write should tag the file");
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"), "{ext}: container");
        let fields = read_tag_fields(&audio).unwrap_or_else(|| panic!("{ext}: read"));
        assert_eq!(fields.explicit_instrument, "Kick", "{ext}: searchable field");
        assert!(finds_by_instrument(&audio, "Kick"), "{ext}: instrument:Kick");
        // `bass` must not reach Kick through `bassdrum`.
        for other in ["Snare", "Bass"] {
            assert!(!finds_by_instrument(&audio, other), "{ext}: instrument:{other}");
        }
        let marker = format!("Tundra v{TUNDRA_TAG_VERSION}");
        match ext {
            // RIFF INFO, which Windows Explorer shows: the instrument in IKEY, never in
            // Genre or Comments, and no ID3 payload that would hide the list.
            "wav" => {
                assert_eq!(riff(&audio, "IKEY").as_deref(), Some("Kick"));
                assert_eq!(riff(&audio, "ICMT"), Some(marker));
                assert_eq!(riff(&audio, "IGNR"), None);
                let bytes = std::fs::read(&audio).expect("bytes");
                assert!(!bytes.windows(3).any(|window| window == b"ID3"), "no ID3 in WAV");
            }
            "flac" => {
                let vorbis = vorbis_comments(&audio);
                assert_eq!(vorbis.get("INSTRUMENT"), Some("Kick"));
                assert_eq!(vorbis.get("COMMENT"), Some(marker.as_str()));
            }
            _ => {}
        }
        assert!(!write(), "{ext}: a tagged file should report no further work");
        let retag = write_auto_tags(&audio, "Snare").expect("same-version retag");
        assert!(!retag && instrument_tag(&audio).as_deref() == Some("Kick"), "{ext}: current tags are not replaced");
        assert!(
            !auto_tag_field_status(&audio).expect("status").needs_instrument,
            "{ext}: tagged file must not be queued for re-tagging"
        );
    }
}

/// A container that cannot hold a native tag must not lose the label: it
/// goes to the SQLite sidecar, and search still finds the file.
#[test]
fn unwritable_container_falls_back_to_sidecar_store_and_stays_searchable() {
    let dir = ScratchDir::new("sidecar-fallback");
    let audio = dir.path().join("sample.wav");
    std::fs::write(&audio, b"not actually a RIFF container").expect("write junk");

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        assert_eq!(write_auto_tags(&audio, "Kick"), Ok(true), "fallback reports the tag as written");
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"), "sidecar surfaces in reads");
        assert!(finds_by_instrument(&audio, "Kick"), "sidecar-tagged files are searchable");

        assert_eq!(write_auto_tags(&audio, "Snare"), Ok(false), "current sidecar tags stay");
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
        assert_eq!(write_auto_tags(&audio, "Kick"), Ok(false), "unchanged label is a no-op");
        assert!(!auto_tag_field_status(&audio).expect("status").can_retag_instrument);
    });
}

// --- stage_and_replace -----------------------------------------------------

/// Everything in `dir`, to prove a failed write left only the original behind.
fn dir_entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).expect("read dir").flatten().map(|entry| entry.path()).collect()
}

#[test]
fn stage_and_replace_failures_keep_the_original_and_remove_the_tmp() {
    let dir = ScratchDir::new("stage-fail");
    let audio = wav_in(&dir, "", "kick.wav");
    let original = std::fs::read(&audio).expect("original");
    let check = |result: Result<(), String>, case: &str| {
        assert!(result.is_err(), "{case} must fail");
        assert_eq!(std::fs::read(&audio).expect("dest"), original, "{case}");
        assert_eq!(dir_entries(dir.path()), std::slice::from_ref(&audio), "{case}: staged tmp must be deleted");
    };

    check(stage_and_replace(&audio, |_| Err("edit failed".into())), "edit");

    let sync_fails = stage_and_replace(&audio, |tmp| {
        std::fs::write(tmp, b"partial").expect("write tmp");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o000)).expect("lock tmp");
        }
        #[cfg(windows)]
        {
            let mut perms = std::fs::metadata(tmp).expect("meta").permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(tmp, perms).expect("readonly tmp");
        }
        Ok(())
    });
    check(sync_fails, "sync");

    let replace_fails = crate::test_fixtures::with_replace_blocked(dir.path(), &audio, || {
        stage_and_replace(&audio, |tmp| std::fs::write(tmp, b"mutated").map_err(|err| err.to_string()))
    });
    check(replace_fails, "replace");
}

#[test]
fn stage_and_replace_edits_a_copy_then_swaps_it_in_keeping_read_only() {
    let dir = ScratchDir::new("stage-success");
    let audio = wav_in(&dir, "", "kick.wav");
    let original = std::fs::read(&audio).expect("original");
    let mut perms = std::fs::metadata(&audio).expect("meta").permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&audio, perms).expect("readonly");

    stage_and_replace(&audio, |tmp| {
        assert_eq!(std::fs::read(&audio).expect("during edit"), original, "original untouched");
        std::fs::write(tmp, b"mutated-copy").map_err(|err| err.to_string())
    })
    .expect("stage and replace");

    assert_eq!(std::fs::read(&audio).expect("dest"), b"mutated-copy");
    assert_eq!(dir.sidecar_count(), 0, "successful replace removes tmp");
    let restored = std::fs::metadata(&audio).expect("meta").permissions();
    assert!(restored.readonly(), "original read-only attribute must be restored");
}
