//! Reading, writing, and searching tags on real files. Scoring details live in
//! `search.rs`; crash recovery and sidecar safety live in `data_safety_tests.rs`.

use super::*;
use lofty::ogg::tag::VorbisComments;
use lofty::tag::{Accessor, Tag};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::path_util::file_mtime_secs;
use crate::test_fixtures::{ASSET_FORMATS, ScratchDir, copy_asset, write_minimal_wav, write_riff_info};

use super::hints::artist_hint_from_path;
use super::read::{
    VORBIS_COMMENT_KEY, VORBIS_INSTRUMENT_KEY, WAV_ARTIST_KEY, WAV_COMMENT_KEY, WAV_GENRE_KEY, WAV_INSTRUMENT_KEY,
    WAV_TITLE_KEY, tundra_comment, tundra_tag_is_current,
};
use super::riff::encode_riff_wave;

// --- Helpers ---------------------------------------------------------------

/// Copies a real encoder-produced fixture into a scratch directory. Named
/// `sample.<ext>` so no instrument can be inferred from the filename.
fn staged_fixture(ext: &str, label: &str) -> (ScratchDir, PathBuf) {
    let dir = ScratchDir::new(label);
    let audio = copy_asset(dir.path(), "sample", ext);
    (dir, audio)
}

/// An index entry for `path` as it is on disk now, so the lookup trusts it.
fn index_entry(path: &Path, fields: TagFields) -> CachedMetadata {
    let mtime_secs = file_mtime_secs(path).expect("scratch file mtime");
    CachedMetadata { mtime_secs, fields }
}

fn instrument_filter(value: &str) -> TagFilter {
    TagFilter {
        field: TagField::Instrument,
        value: value.to_string(),
    }
}

/// Searches `paths` the way the library does, reading tags from `metadata`.
fn search_paths(
    paths: &[PathBuf],
    text: &str,
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> Vec<PathBuf> {
    let query = SearchQuery {
        text,
        tag_filters,
        ..SearchQuery::default()
    };
    search(paths, &query, MetadataLookup::new(metadata)).paths
}

/// Mirrors the app: index the file, then search the index.
fn finds_by_instrument(path: &Path, query: &str) -> bool {
    let paths = vec![path.to_path_buf()];
    let indexed = Arc::new(index_paths(&paths, Arc::new(HashMap::new())));
    search_paths(&paths, "", &[instrument_filter(query)], indexed)
        .iter()
        .any(|hit| hit == path)
}

fn riff_info(path: &Path) -> lofty::iff::wav::RiffInfoList {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;

    let mut file = std::fs::File::open(path).expect("open wav");
    lofty::iff::wav::WavFile::read_from(&mut file, ParseOptions::new())
        .expect("parse wav")
        .remove_riff_info()
        .expect("wav should carry a RIFF INFO list")
}

fn vorbis_comments(path: &Path) -> VorbisComments {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;

    let mut file = std::fs::File::open(path).expect("open flac");
    lofty::flac::FlacFile::read_from(&mut file, ParseOptions::new())
        .expect("parse flac")
        .remove_vorbis_comments()
        .expect("flac should carry vorbis comments")
}

// --- Search over the metadata index ----------------------------------------

#[test]
fn tag_only_search_matches_metadata_on_audio_files() {
    let dir = ScratchDir::new("tag-search");
    let audio = dir.path().join("kick.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let paths = vec![dir.path().join("nested"), audio.clone()];
    let mut cache = HashMap::new();
    let fields = TagFields {
        bpm: "120".into(),
        ..TagFields::default()
    };
    cache.insert(audio.clone(), index_entry(&audio, fields));
    let filters = vec![TagFilter {
        field: TagField::Bpm,
        value: "120".into(),
    }];
    let hits = search_paths(&paths, "", &filters, Arc::new(cache));
    assert_eq!(hits, vec![audio.clone()]);
}

#[test]
fn tag_match_requires_every_filter() {
    let dir = ScratchDir::new("tag-match-all");
    let audio = dir.path().join("kick.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let paths = vec![audio.clone()];
    let mut cache = HashMap::new();
    let fields = TagFields {
        bpm: "120".into(),
        key: "Am".into(),
        ..TagFields::default()
    };
    cache.insert(audio.clone(), index_entry(&audio, fields));
    let filters = vec![
        TagFilter {
            field: TagField::Bpm,
            value: "120".into(),
        },
        TagFilter {
            field: TagField::Key,
            value: "Bm".into(),
        },
    ];
    let hits = search_paths(&paths, "", &filters, Arc::new(cache));
    assert!(hits.is_empty(), "partial tag matches should be rejected");
}

#[test]
fn tag_only_search_skips_unindexed_files() {
    let dir = ScratchDir::new("cached-skip");
    let audio = dir.path().join("snare.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let filters = vec![instrument_filter("snare")];

    let hits = search_paths(std::slice::from_ref(&audio), "", &filters, Arc::new(HashMap::new()));
    assert!(
        hits.is_empty(),
        "a library-wide tag query must not parse unindexed files"
    );
}

#[test]
fn tag_search_narrows_with_single_char_filename_query() {
    let dir = ScratchDir::new("tag-narrow");
    let filters = vec![instrument_filter("Kick")];
    let names = ["1 kick.wav", "2 kick.wav", "3 kick.wav"];
    let mut paths = Vec::new();
    let mut cache = HashMap::new();
    for name in names {
        let path = dir.path().join(name);
        std::fs::write(&path, b"RIFF").unwrap();
        let fields = TagFields {
            instrument: "Kick".into(),
            ..Default::default()
        };
        cache.insert(path.clone(), index_entry(&path, fields));
        paths.push(path);
    }
    let metadata = Arc::new(cache);

    let all = search_paths(&paths, "", &filters, Arc::clone(&metadata));
    assert_eq!(all.len(), 3);

    let narrowed = search_paths(&paths, "2", &filters, metadata);
    assert_eq!(narrowed.len(), 1);
    assert!(narrowed[0].ends_with("2 kick.wav"));
}

#[test]
fn metadata_lookup_matches_cache_key_variants() {
    let dir = ScratchDir::new("lookup-keys");
    let audio = dir.path().join("snare.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let mut cache = HashMap::new();
    let fields = TagFields {
        explicit_instrument: "Snare".into(),
        instrument: "Snare".into(),
        ..TagFields::default()
    };
    cache.insert(crate::path_util::cache_key(&audio), index_entry(&audio, fields));
    let metadata = Arc::new(cache);
    let mut lookup = MetadataLookup::new(Arc::clone(&metadata));
    assert_eq!(lookup.tag_fields(&audio).explicit_instrument, "Snare");

    let hits = search_paths(
        std::slice::from_ref(&audio),
        "",
        &[instrument_filter("snare")],
        metadata,
    );
    assert_eq!(hits, vec![audio.clone()]);
}

#[test]
fn tag_fields_ignore_cache_when_file_missing() {
    let path = PathBuf::from(r"C:\missing\tundra-kick.wav");
    let mut cache = HashMap::new();
    cache.insert(
        path.clone(),
        CachedMetadata {
            mtime_secs: 1,
            fields: TagFields {
                bpm: "120".into(),
                ..TagFields::default()
            },
        },
    );
    let mut lookup = MetadataLookup::new(Arc::new(cache));
    assert!(lookup.tag_fields(&path).bpm.is_empty());
}

#[test]
fn tag_field_best_match_breaks_score_ties_by_label() {
    assert_eq!(tag_field_best_match("a"), Some(TagField::Album));
}

// --- Hints from paths ------------------------------------------------------

#[test]
fn instrument_hint_reads_folder_and_prefers_filename() {
    let hint = |path: &str| instrument_hint_from_path(Path::new(path));

    assert_eq!(
        hint("/Samples/ADM Samples - Copy/snares/tight_01.wav").as_deref(),
        Some("Snare")
    );
    assert_eq!(hint("/Samples/snares/cymbal_roll.wav").as_deref(), Some("Cymbal"));
    assert_eq!(hint("/Drums/Kicks/808_kick_01.wav").as_deref(), Some("Kick"));
    assert_eq!(hint("/Drums/808_hat.wav").as_deref(), Some("Hi-Hat"));
    assert_eq!(hint("/Drums/snow_01.wav"), None);
    assert_eq!(hint("/hats/tight_01.wav").as_deref(), Some("Hi-Hat"));
    assert_eq!(
        hint("/Libraries/Pack A/Drums/One Shots/Snares/tight_01.wav").as_deref(),
        Some("Snare")
    );
    assert_eq!(hint("/Samples/Bongo/hit_01.wav").as_deref(), Some("Percussion"));
    assert_eq!(hint("/Samples/Bongos/layer.wav").as_deref(), Some("Percussion"));
    assert_eq!(hint("/Samples/ADM/Perc/tight.wav").as_deref(), Some("Percussion"));
    assert_eq!(
        hint("/Samples/Perc/01.wav").as_deref(),
        Some("Percussion"),
        "Perc must not false-match Kick"
    );
    assert_eq!(
        hint("/Samples/Bass/low.wav").as_deref(),
        Some("Bass"),
        "Bass must not false-match Kick via bassdrum"
    );
}

#[test]
fn instrument_hint_understands_kit_codes_word_pairs_and_other_languages() {
    let hint = |path: &str| instrument_hint_from_path(Path::new(path));
    let cases = [
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
        assert_eq!(hint(path).as_deref(), expected, "{path}");
    }
}

#[test]
fn translated_aliases_widen_search_but_kit_codes_do_not() {
    assert!(instruments_related("キック", "Kick"));
    assert!(instruments_related("caja", "snare"));
    assert!(!instruments_related("ch", "Hi-Hat"));
    assert!(!instruments_related("ep", "Piano"));
}

#[test]
fn bass_is_not_related_to_kick() {
    assert!(!instruments_related("Bass", "Kick"));
    assert!(!instruments_related("bass", "bassdrum"));
    assert!(!instruments_related("shot", "Rim"));
    assert!(instruments_related("Hat", "Hi-Hat"));
    assert!(instruments_related("kick", "kickdrum"));
}

#[test]
fn artist_hint_reads_label_from_directory_layout() {
    let hint = |path: &str| artist_hint_from_path(Path::new(path));

    assert_eq!(hint("/Samples/KSHMR/Vol4/Kicks/kick.wav").as_deref(), Some("KSHMR"));
    assert_eq!(hint("/Samples/KSHMR/Kicks/kick.wav").as_deref(), Some("KSHMR"));
    assert_eq!(hint("/Splice/packs/deadmau5/kick.wav").as_deref(), Some("deadmau5"));
    assert_eq!(
        hint("/Samples/Native Instruments/Battery 4/Snares/snare.wav").as_deref(),
        Some("Native Instruments")
    );
    assert_eq!(hint("/Samples/snares/tight_01.wav"), None);
}

#[test]
fn read_tag_fields_uses_artist_hint_when_file_is_untagged() {
    let dir = ScratchDir::new("artist-hint-read");
    let kicks = dir.path().join("KSHMR").join("Kicks");
    std::fs::create_dir_all(&kicks).expect("create pack folders");
    let audio = kicks.join("kick.wav");
    write_minimal_wav(&audio);

    let fields = read_tag_fields(&audio).expect("read tag fields");
    assert_eq!(fields.artist, "KSHMR");
}

// --- Tag ownership: what auto-tag may write or replace ---------------------

#[test]
fn tundra_comment_keeps_user_lines_and_replaces_only_its_own() {
    let marker = format!("Tundra v{TUNDRA_TAG_VERSION}");
    assert_eq!(tundra_comment(None), marker);
    assert_eq!(tundra_comment(Some("  ")), marker);
    assert_eq!(tundra_comment(Some("Recorded live")), "Recorded live");
    assert_eq!(tundra_comment(Some("Tundra v0")), marker);
    assert_eq!(
        tundra_comment(Some("Recorded live\nINSTRUMENT: Kick\nTundra")),
        format!("Recorded live\n{marker}")
    );
}

#[test]
fn user_instrument_without_comment_is_not_claimed_by_tundra() {
    let dir = ScratchDir::new("user-instrument-no-comment");
    let audio = dir.path().join("snare.wav");
    write_minimal_wav(&audio);
    write_riff_info(&audio, &[(WAV_INSTRUMENT_KEY, "Snare")]);

    let status = auto_tag_field_status(&audio).expect("status");
    assert!(!status.needs_comment, "a marker would claim the user's instrument");
    write_auto_tags(&audio, "Snare").expect("auto tag");
    assert_eq!(riff_info(&audio).get(WAV_COMMENT_KEY), None);
}

#[test]
fn tundra_tagged_status_allows_retag_but_preserves_user_tags() {
    let dir = ScratchDir::new("retag-status");
    let audio = dir.path().join("snare.wav");
    write_minimal_wav(&audio);
    assert!(write_auto_tags(&audio, "Snare").expect("tundra write"));

    let status = auto_tag_field_status(&audio).expect("status");
    assert!(!status.needs_instrument);
    assert!(!status.can_retag_instrument);
    assert!(tundra_tag_is_current(
        &audio,
        riff_info(&audio).get(WAV_COMMENT_KEY).unwrap_or("")
    ));

    let user_dir = ScratchDir::new("user-tag");
    let user_audio = user_dir.path().join("snare.wav");
    write_minimal_wav(&user_audio);
    write_riff_info(
        &user_audio,
        &[(WAV_INSTRUMENT_KEY, "Snare"), (WAV_COMMENT_KEY, "Recorded live")],
    );

    let user_status = auto_tag_field_status(&user_audio).expect("user status");
    assert!(!user_status.needs_instrument);
    assert!(!user_status.can_retag_instrument);
    assert!(
        !write_auto_tags(&user_audio, "Kick").expect("user tag write attempt"),
        "user-owned instrument tags must not be overwritten"
    );
    assert_eq!(instrument_tag(&user_audio).as_deref(), Some("Snare"));
}

#[test]
fn legacy_tundra_comment_is_eligible_for_upgrade() {
    let dir = ScratchDir::new("legacy-comment");
    let audio = dir.path().join("snare.wav");
    write_minimal_wav(&audio);
    write_riff_info(&audio, &[(WAV_INSTRUMENT_KEY, "Snare"), (WAV_COMMENT_KEY, "Tundra")]);

    let status = auto_tag_field_status(&audio).expect("status");
    assert!(!status.needs_instrument);
    assert!(status.needs_comment);
    assert!(status.can_retag_instrument);
}

#[test]
fn write_auto_tags_skips_retag_when_tag_version_is_current() {
    let dir = ScratchDir::new("retag-auto-tags");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);

    assert!(write_auto_tags(&audio, "Kick").expect("initial write"));
    assert!(
        !write_auto_tags(&audio, "Snare").expect("same-version retag should no-op"),
        "current-version tags must not be replaced by auto tag"
    );
    assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
    assert!(
        !write_auto_tags(&audio, "Kick").expect("same label should no-op"),
        "unchanged instrument should not rewrite the file"
    );
}

#[test]
fn sidecar_row_does_not_own_user_native_instrument() {
    let dir = ScratchDir::new("sidecar-user-native");
    let audio = dir.path().join("snare.wav");
    write_minimal_wav(&audio);
    write_riff_info(
        &audio,
        &[(WAV_INSTRUMENT_KEY, "Snare"), (WAV_COMMENT_KEY, "Recorded live")],
    );

    crate::tag_store::with_test_db(dir.path().join("tags.db"), || {
        crate::tag_store::set_instrument(&audio, "Kick", 0).expect("stale sidecar");

        let status = auto_tag_field_status(&audio).expect("status");
        assert!(!status.can_retag_instrument);
        assert!(
            !write_auto_tags(&audio, "Kick").expect("user tag write attempt"),
            "sidecar must not unlock overwrite of a native user instrument"
        );
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Snare"));
    });
}

// --- WAV writes ------------------------------------------------------------

fn wav_chunk<'a>(chunks: &'a [([u8; 4], Vec<u8>)], id: &[u8; 4]) -> Option<&'a [u8]> {
    chunks
        .iter()
        .find(|(found, _)| found == id)
        .map(|(_, data)| data.as_slice())
}

fn wav_list_chunk<'a>(chunks: &'a [([u8; 4], Vec<u8>)], form: &[u8; 4]) -> Option<&'a [u8]> {
    chunks
        .iter()
        .find_map(|(id, data)| (id == b"LIST" && data.len() >= 4 && &data[..4] == form).then_some(data.as_slice()))
}

#[test]
fn write_auto_tags_preserves_wav_non_info_chunks() {
    let dir = ScratchDir::new("wav-chunk-preserve");
    let audio = dir.path().join("kick.wav");
    let smpl = vec![0x11; 60];
    let cue = vec![0x22; 28];
    let inst = vec![0x33; 8];
    let acid = vec![0x44; 24];
    let adtl = {
        let mut data = Vec::from(*b"adtl");
        data.extend_from_slice(b"labl");
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(b"cue\0");
        data
    };
    let mut chunks = parse_riff_wave_chunks(&crate::test_fixtures::minimal_wav_bytes()).expect("parse minimal wav");
    chunks.extend([
        (*b"smpl", smpl.clone()),
        (*b"cue ", cue.clone()),
        (*b"inst", inst.clone()),
        (*b"acid", acid.clone()),
        (*b"LIST", adtl.clone()),
    ]);
    std::fs::write(&audio, encode_riff_wave(&chunks)).expect("write wav");

    write_auto_tags(&audio, "Kick").expect("tag wav");

    let bytes = std::fs::read(&audio).expect("read tagged wav");
    let chunks = parse_riff_wave_chunks(&bytes).expect("parse tagged wav");
    assert_eq!(wav_chunk(&chunks, b"smpl"), Some(smpl.as_slice()));
    assert_eq!(wav_chunk(&chunks, b"cue "), Some(cue.as_slice()));
    assert_eq!(wav_chunk(&chunks, b"inst"), Some(inst.as_slice()));
    assert_eq!(wav_chunk(&chunks, b"acid"), Some(acid.as_slice()));
    assert_eq!(wav_list_chunk(&chunks, b"adtl"), Some(adtl.as_slice()));
    assert!(
        wav_list_chunk(&chunks, b"INFO").is_some(),
        "LIST INFO must be written without replacing adtl"
    );
    assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
}

#[test]
fn write_instrument_tag_sets_wav_riff_info_for_explorer() {
    use lofty::file::TaggedFileExt;
    use lofty::tag::TagType;

    let dir = ScratchDir::new("wav-instrument-tag");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);

    write_auto_tags(&audio, "Kick").expect("write instrument");

    let bytes = std::fs::read(&audio).expect("read tagged wav bytes");
    assert!(
        bytes
            .windows(4)
            .any(|chunk| chunk == b"LIST" || chunk == b"id3 " || chunk == b"ID3"),
        "no tag chunks written, len={}, head={:02x?}",
        bytes.len(),
        &bytes[..bytes.len().min(80)]
    );

    let tagged = lofty::read_from_path(&audio).expect("re-read wav");
    let types: Vec<_> = tagged.tags().iter().map(|tag| tag.tag_type()).collect();
    let ascii: String = bytes
        .iter()
        .map(|b| if b.is_ascii_graphic() { *b as char } else { '.' })
        .collect();
    assert!(
        tagged.tag(TagType::RiffInfo).is_some(),
        "expected RIFF INFO, found tags {types:?}, instrument={:?}, ascii={ascii}",
        instrument_tag(&audio)
    );
    let riff = tagged
        .tag(TagType::RiffInfo)
        .expect("RIFF INFO tag for Windows Explorer");
    assert!(
        riff.genre().is_none() || riff.genre().is_some_and(|genre| genre.trim().is_empty()),
        "instrument must not be written to Genre, got {:?}",
        riff.genre()
    );
    let comment = riff.comment().expect("Comments field");
    assert!(
        comment.contains("Tundra v"),
        "Comments should credit Tundra, got {comment:?}"
    );
    assert!(
        !comment.contains("INSTRUMENT:"),
        "instrument must not be stored in Comments, got {comment:?}"
    );
    assert_eq!(
        riff_info(&audio).get(WAV_INSTRUMENT_KEY).map(str::to_string),
        Some("Kick".to_string()),
        "instrument should be stored in RIFF IKEY"
    );
    assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
    assert!(
        !bytes.windows(3).any(|chunk| chunk == b"ID3"),
        "WAV should not get an ID3 payload that hides LIST INFO from Explorer"
    );
}

#[test]
fn write_instrument_tag_replaces_existing_wav_comment() {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::TagType;

    let dir = ScratchDir::new("wav-existing-comment");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);

    let mut tagged = Probe::open(&audio).expect("open wav").read().expect("read wav");
    tagged.insert_tag(Tag::new(TagType::RiffInfo));
    tagged
        .tag_mut(TagType::RiffInfo)
        .expect("RIFF INFO")
        .set_comment("already had a note".to_string());
    tagged
        .save_to_path(&audio, WriteOptions::default())
        .expect("save user comment");

    write_auto_tags(&audio, "Kick").expect("write instrument");
    assert_eq!(instrument_tag(&audio).as_deref(), Some("Kick"));
    let tagged = lofty::read_from_path(&audio).expect("re-read wav");
    let comment = tagged
        .tag(TagType::RiffInfo)
        .and_then(|tag| tag.comment())
        .expect("Comments field");
    assert!(
        comment.contains("already had a note"),
        "user comment should be preserved, got {comment:?}"
    );
    assert!(
        !comment.contains("Tundra v"),
        "custom user comments should not be replaced, got {comment:?}"
    );
    assert!(
        !comment.contains("INSTRUMENT:"),
        "instrument must not be stored in Comments, got {comment:?}"
    );
    assert_eq!(
        riff_info(&audio).get(WAV_INSTRUMENT_KEY).map(str::to_string),
        Some("Kick".to_string()),
        "instrument should be stored in RIFF IKEY"
    );
}

#[test]
fn write_instrument_tag_sets_wav_artist_from_directory() {
    let dir = ScratchDir::new("wav-artist-tag");
    let kicks = dir.path().join("KSHMR").join("Kicks");
    std::fs::create_dir_all(&kicks).expect("create pack folders");
    let audio = kicks.join("kick.wav");
    write_minimal_wav(&audio);

    write_auto_tags(&audio, "Kick").expect("write instrument");

    assert_eq!(
        riff_info(&audio).get(WAV_ARTIST_KEY).map(str::to_string),
        Some("KSHMR".to_string())
    );
}

#[test]
fn write_auto_tags_extends_file_with_existing_non_auto_tags() {
    use lofty::config::WriteOptions;
    use lofty::file::{AudioFile, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::TagType;

    let dir = ScratchDir::new("extend-existing-tags");
    let kicks = dir.path().join("KSHMR").join("Kicks");
    std::fs::create_dir_all(&kicks).expect("create pack folders");
    let audio = kicks.join("kick.wav");
    write_minimal_wav(&audio);

    let mut tagged = Probe::open(&audio).expect("open wav").read().expect("read wav");
    tagged.insert_tag(Tag::new(TagType::RiffInfo));
    tagged
        .tag_mut(TagType::RiffInfo)
        .expect("RIFF INFO")
        .set_genre("Drums".to_string());
    tagged
        .save_to_path(&audio, WriteOptions::default())
        .expect("save genre");

    write_auto_tags(&audio, "Kick").expect("extend tags");

    let riff_info = riff_info(&audio);
    assert_eq!(
        riff_info.get(WAV_INSTRUMENT_KEY).map(str::to_string),
        Some("Kick".to_string())
    );
    assert_eq!(riff_info.get("IGNR").map(str::to_string), Some("Drums".to_string()));
    assert_eq!(
        riff_info.get(WAV_ARTIST_KEY).map(str::to_string),
        Some("KSHMR".to_string())
    );
    assert!(
        riff_info
            .get(WAV_COMMENT_KEY)
            .is_some_and(|comment| comment.contains("Tundra v"))
    );
}

#[test]
fn write_manual_tags_round_trips_wav_fields_after_native_write() {
    let dir = ScratchDir::new("manual-wav-tags");
    let kicks = dir.path().join("KSHMR").join("Kicks");
    std::fs::create_dir_all(&kicks).expect("create pack folders");
    let audio = kicks.join("kick.wav");
    write_minimal_wav(&audio);

    write_auto_tags(&audio, "Kick").expect("seed native tags");

    let edits = ManualTagEdits {
        instrument: "Kick".to_string(),
        artist: "KSHMR".to_string(),
        title: "Punchy Kick".to_string(),
        genre: "Drums".to_string(),
        comment: "Manual edit".to_string(),
        ..ManualTagEdits::default()
    };
    write_manual_tags(&audio, &edits).expect("manual tag save");

    let fields = read_tag_fields(&audio).expect("read saved tags");
    assert_eq!(fields.instrument, "Kick");
    assert_eq!(fields.artist, "KSHMR");
    assert_eq!(fields.title, "Punchy Kick");
    assert_eq!(fields.genre, "Drums");
    assert_eq!(fields.comment, "Manual edit");

    let riff = riff_info(&audio);
    assert_eq!(
        riff.get(WAV_TITLE_KEY).map(str::to_string),
        Some("Punchy Kick".to_string())
    );
    assert_eq!(riff.get(WAV_GENRE_KEY).map(str::to_string), Some("Drums".to_string()));
}

// --- FLAC, MP3, and OGG writes ---------------------------------------------

#[test]
fn write_manual_tags_round_trips_flac_generic_fields() {
    let (_dir, audio) = staged_fixture("flac", "manual-flac-tags");
    write_auto_tags(&audio, "Kick").expect("seed native tags");

    let edits = ManualTagEdits {
        instrument: "Kick".to_string(),
        title: "Warm Kick".to_string(),
        genre: "Drums".to_string(),
        bpm: "128".to_string(),
        key: "Am".to_string(),
        ..ManualTagEdits::default()
    };
    write_manual_tags(&audio, &edits).expect("manual flac tag save");

    let fields = read_tag_fields(&audio).expect("read saved tags");
    assert_eq!(fields.instrument, "Kick");
    assert_eq!(fields.title, "Warm Kick");
    assert_eq!(fields.genre, "Drums");
    assert_eq!(fields.bpm, "128");
    assert_eq!(fields.key, "Am");

    let vorbis = vorbis_comments(&audio);
    assert_eq!(vorbis.title().as_deref(), Some("Warm Kick"));
    assert_eq!(vorbis.genre().as_deref(), Some("Drums"));
}

#[test]
fn write_auto_tags_sets_flac_instrument_field_not_comment() {
    let (_dir, audio) = staged_fixture("flac", "flac-instrument-field");

    assert!(write_auto_tags(&audio, "Kick").expect("write flac tags"));

    let vorbis = vorbis_comments(&audio);
    assert_eq!(
        vorbis.get(VORBIS_INSTRUMENT_KEY).map(str::to_string),
        Some("Kick".to_string())
    );
    assert_ne!(
        vorbis.get(VORBIS_COMMENT_KEY).map(str::to_string),
        Some("Kick".to_string()),
        "instrument must not land in the comment field"
    );
    assert_eq!(
        vorbis.get(VORBIS_COMMENT_KEY).map(str::to_string),
        Some(format!("Tundra v{TUNDRA_TAG_VERSION}"))
    );
}

/// Auto-tagging must never destroy artwork the user already had.
#[test]
fn tagging_preserves_embedded_cover_art() {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;
    use lofty::picture::{MimeType, Picture, PictureType};

    let art: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xde, 0xad, 0xbe, 0xef];

    let (_dir, audio) = staged_fixture("mp3", "cover-art");
    {
        let mut mp3 = {
            let mut file = std::fs::File::open(&audio).expect("open mp3");
            lofty::mpeg::MpegFile::read_from(&mut file, lofty::config::ParseOptions::new()).expect("parse mp3")
        };
        let mut id3 = mp3.remove_id3v2().unwrap_or_default();
        id3.insert_picture(
            Picture::unchecked(art.clone())
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .build(),
        );
        mp3.set_id3v2(id3);
        mp3.save_to_path(&audio, WriteOptions::default()).expect("save art");
    }

    assert!(write_auto_tags(&audio, "Kick").expect("tag"));

    let mut file = std::fs::File::open(&audio).expect("reopen mp3");
    let mut mp3 = lofty::mpeg::MpegFile::read_from(&mut file, lofty::config::ParseOptions::new()).expect("reparse mp3");
    let tag = mp3
        .remove_id3v2()
        .map(Tag::from)
        .expect("mp3 should still have an ID3v2 tag");
    assert_eq!(
        tag.pictures().len(),
        1,
        "cover art must survive auto-tagging, found {} pictures",
        tag.pictures().len()
    );
    assert_eq!(tag.pictures()[0].data(), art.as_slice());
}

/// Files an older build tagged into the grouping field carry no canonical
/// instrument, so the grouping has to survive as the instrument read back
/// and get rewritten to the canonical key.
#[test]
fn legacy_grouping_reads_as_instrument_and_is_rewritten_canonically() {
    use lofty::config::WriteOptions;
    use lofty::file::AudioFile;

    let (_dir, audio) = staged_fixture("ogg", "legacy-grouping");
    {
        let mut ogg = {
            let mut file = std::fs::File::open(&audio).expect("open ogg");
            lofty::ogg::VorbisFile::read_from(&mut file, lofty::config::ParseOptions::new()).expect("parse ogg")
        };
        ogg.vorbis_comments_mut()
            .insert("GROUPING".to_string(), "Kick".to_string());
        ogg.save_to_path(&audio, WriteOptions::default())
            .expect("save grouping");
    }

    assert_eq!(
        read_tag_fields(&audio).expect("read grouping").instrument,
        "Kick",
        "a legacy grouping should still read back as the instrument"
    );
    assert!(
        auto_tag_field_status(&audio).expect("status").needs_instrument,
        "a grouping is not the canonical key, so the file still needs tagging"
    );

    assert!(write_auto_tags(&audio, "Kick").expect("rewrite canonically"));
    assert_eq!(
        instrument_tag(&audio).as_deref(),
        Some("Kick"),
        "instrument should now read from the canonical INSTRUMENT key"
    );
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

        assert!(
            write_auto_tags(&audio, "Kick").unwrap_or_else(|err| panic!("{ext}: {err}")),
            "{ext}: first write should tag the file"
        );
        assert_eq!(
            instrument_tag(&audio).as_deref(),
            Some("Kick"),
            "{ext}: instrument must read back from the container"
        );
        assert_eq!(
            read_tag_fields(&audio)
                .map(|fields| fields.explicit_instrument)
                .as_deref(),
            Some("Kick"),
            "{ext}: instrument must reach the searchable field"
        );
        assert!(
            finds_by_instrument(&audio, "Kick"),
            "{ext}: instrument:Kick must match the tagged file"
        );
        assert!(
            !finds_by_instrument(&audio, "Snare"),
            "{ext}: instrument:Snare must not match a kick"
        );
        assert!(
            !write_auto_tags(&audio, "Kick").unwrap_or_else(|err| panic!("{ext}: {err}")),
            "{ext}: a tagged file should report no further work"
        );
        assert!(
            !auto_tag_field_status(&audio)
                .unwrap_or_else(|| panic!("{ext}: status"))
                .needs_instrument,
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
    let db = dir.path().join("tags.db");

    crate::tag_store::with_test_db(db, || {
        assert!(
            write_auto_tags(&audio, "Kick").expect("unwritable container should fall back"),
            "fallback should report the tag as written"
        );
        assert_eq!(
            crate::tag_store::instrument(&audio).as_deref(),
            Some("Kick"),
            "sidecar store should hold the label the container refused"
        );
        assert_eq!(
            instrument_tag(&audio).as_deref(),
            Some("Kick"),
            "sidecar label must surface through the normal instrument read"
        );
        assert!(
            finds_by_instrument(&audio, "Kick"),
            "instrument:Kick must match a sidecar-tagged file"
        );
        assert_eq!(crate::tag_store::tag_version(&audio), Some(TUNDRA_TAG_VERSION));
        assert!(
            !write_auto_tags(&audio, "Snare").expect("same-version sidecar retag"),
            "current-version sidecar tags must not be replaced"
        );
        assert_eq!(crate::tag_store::instrument(&audio).as_deref(), Some("Kick"));
        assert_eq!(
            write_auto_tags(&audio, "Kick"),
            Ok(false),
            "unchanged sidecar label should no-op"
        );
        assert!(
            !auto_tag_field_status(&audio).expect("status").can_retag_instrument,
            "current sidecar version should skip auto tag"
        );
    });
}

#[test]
fn instrument_bass_search_does_not_match_kick() {
    let dir = ScratchDir::new("bass-search");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);
    assert!(write_auto_tags(&audio, "Kick").expect("tag kick"));
    assert!(finds_by_instrument(&audio, "Kick"));
    assert!(
        !finds_by_instrument(&audio, "Bass"),
        "instrument:bass must not match a Kick via bassdrum"
    );
}

// --- stage_and_replace -----------------------------------------------------

/// Everything in `dir`, to prove a failed write left only the original behind.
fn dir_entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read dir")
        .flatten()
        .map(|entry| entry.path())
        .collect()
}

#[test]
fn stage_and_replace_preserves_original_when_edit_fails() {
    let dir = ScratchDir::new("stage-edit-fail");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);
    let original = std::fs::read(&audio).expect("original");

    let err = stage_and_replace(&audio, |_| Err("edit failed".into()));
    assert!(err.is_err());

    assert_eq!(std::fs::read(&audio).expect("dest"), original);
    assert_eq!(
        dir_entries(dir.path()),
        vec![audio],
        "failed edit must delete staged tmp"
    );
}

#[test]
fn stage_and_replace_removes_tmp_when_sync_fails() {
    let dir = ScratchDir::new("stage-sync-fail");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);
    let original = std::fs::read(&audio).expect("original");

    let err = stage_and_replace(&audio, |tmp| {
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
    assert!(err.is_err());
    assert_eq!(std::fs::read(&audio).expect("dest"), original);
    assert_eq!(
        dir_entries(dir.path()),
        vec![audio],
        "sync failure must delete staged tmp"
    );
}

#[test]
fn stage_and_replace_swaps_successfully_and_cleans_tmp() {
    let dir = ScratchDir::new("stage-success");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);

    stage_and_replace(&audio, |tmp| {
        let mut bytes = std::fs::read(tmp).map_err(|err| err.to_string())?;
        bytes.extend_from_slice(b"tail-marker");
        std::fs::write(tmp, bytes).map_err(|err| err.to_string())
    })
    .expect("stage and replace");

    let tagged = std::fs::read(&audio).expect("tagged");
    assert!(tagged.ends_with(b"tail-marker"));
    assert_eq!(dir.sidecar_count(), 0, "successful replace removes tmp");
}

#[test]
fn stage_and_replace_leaves_original_intact_during_edit() {
    let dir = ScratchDir::new("stage-copy-first");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);
    let original = std::fs::read(&audio).expect("original");

    stage_and_replace(&audio, |tmp| {
        assert_eq!(
            std::fs::read(&audio).expect("original during edit"),
            original,
            "original must stay untouched while tmp is edited"
        );
        std::fs::write(tmp, b"mutated-copy").expect("mutate tmp");
        Ok(())
    })
    .expect("replace");

    assert_eq!(std::fs::read(&audio).expect("dest"), b"mutated-copy");
}

#[test]
fn stage_and_replace_restores_readonly_permissions_on_success() {
    let dir = ScratchDir::new("stage-perms");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);
    let mut perms = std::fs::metadata(&audio).expect("meta").permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&audio, perms).expect("readonly");

    stage_and_replace(&audio, |tmp| {
        std::fs::write(tmp, std::fs::read(tmp).expect("read")).map_err(|err| err.to_string())
    })
    .expect("replace readonly");

    let restored = std::fs::metadata(&audio).expect("meta").permissions();
    assert!(restored.readonly(), "original read-only attribute must be restored");
}

#[test]
fn stage_and_replace_preserves_original_when_replace_fails() {
    use crate::test_fixtures::with_replace_blocked;

    let dir = ScratchDir::new("stage-replace-fail");
    let audio = dir.path().join("kick.wav");
    write_minimal_wav(&audio);
    let original = std::fs::read(&audio).expect("original");

    let err = with_replace_blocked(dir.path(), &audio, || {
        stage_and_replace(&audio, |tmp| {
            std::fs::write(tmp, b"mutated").expect("write tmp");
            Ok(())
        })
    });

    assert!(err.is_err(), "replace must fail while dest is locked");
    assert_eq!(std::fs::read(&audio).expect("dest"), original);
    assert_eq!(dir.sidecar_count(), 0, "failed replace must delete staged tmp");
}
