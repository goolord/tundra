use super::*;
use lofty::ogg::tag::VorbisComments;
use lofty::tag::{Accessor, Tag};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::hints::{hint_label_for_term, hint_name_tokens, HintSource};
use super::read::{
    encode_riff_wave, file_mtime_secs, instrument_tag, parse_riff_wave_chunks, read_tag_fields,
    stage_and_replace, tundra_tag_is_current, WAV_ARTIST_KEY, WAV_COMMENT_KEY, WAV_GENRE_KEY,
    WAV_INSTRUMENT_KEY, WAV_TITLE_KEY, VORBIS_COMMENT_KEY, VORBIS_INSTRUMENT_KEY,
    TUNDRA_TAG_VERSION,
};
use super::search::{
    collect_file_matches, collect_tag_matches, file_search_matcher, path_match_scores,
    tag_field_score, tag_search_matcher, FILE_SEARCH_CONFIDENT_RESULT_CAP,
    FILE_SEARCH_DEBOUNCE_MS, FILE_SEARCH_DEBOUNCE_MS_SHORT,
};

fn instrument_hint(path: &Path) -> Option<(String, HintSource)> {
    if let Some(hint) = instrument_hint_from_path(path) {
        return Some((hint, HintSource::Path));
    }
    let fields = read_tag_fields(path)?;
    for value in [
        fields.instrument.as_str(),
        fields.title.as_str(),
        fields.genre.as_str(),
        fields.album.as_str(),
        fields.comment.as_str(),
    ] {
        if value.trim().is_empty() {
            continue;
        }
        for token in hint_name_tokens(value) {
            if let Some(label) = hint_label_for_term(&token) {
                return Some((label.to_string(), HintSource::Tags));
            }
        }
    }
    None
}

    #[test]
    fn file_search_matches_snare_in_filename() {
        let matcher = file_search_matcher(false);
        let path = PathBuf::from(r"C:\Samples\Snare Drum 01.wav");
        let (name_score, path_score) = path_match_scores(&matcher, &path, "snare", false, false);
        assert!(name_score > 0 || path_score > 0, "expected snare to match filename");
    }

    #[test]
    fn file_search_requires_all_terms() {
        let matcher = file_search_matcher(false);
        let path = PathBuf::from(r"C:\Samples\Snare Drum 01.wav");
        let (both, _) = path_match_scores(&matcher, &path, "snare drum", false, false);
        let (snare_only, _) = path_match_scores(&matcher, &path, "snare", false, false);
        let (missing, _) = path_match_scores(&matcher, &path, "snare kick", false, false);
        assert!(both > 0);
        assert!(snare_only > 0);
        assert_eq!(missing, 0);
    }

    #[test]
    fn file_search_rejects_weak_fuzzy_matches() {
        let matcher = file_search_matcher(false);
        let path = PathBuf::from(r"C:\Samples\Synth Pad 01.wav");
        let (weak, _) = path_match_scores(&matcher, &path, "snare", false, false);
        assert_eq!(weak, 0, "unrelated fuzzy matches should be rejected");
    }

    #[test]
    fn file_search_confident_cap_skips_fuzzy_only_matches() {
        let paths = vec![
            PathBuf::from(r"C:\Samples\01 Snare.wav"),
            PathBuf::from(r"C:\Samples\Synth Pad 01.wav"),
        ];
        let confident: Vec<_> = (0..FILE_SEARCH_CONFIDENT_RESULT_CAP)
            .map(|index| PathBuf::from(format!(r"C:\Samples\snare-{index:04}.wav")))
            .collect();
        let mut all_paths = confident;
        all_paths.extend(paths);

        let result = collect_file_matches(
            &all_paths,
            "snare",
            &[],
            false,
            true,
            Arc::new(HashMap::new()),
        );

        assert!(
            result.paths.iter().any(|path| path.ends_with("01 Snare.wav")),
            "substring matches should remain"
        );
        assert!(
            !result
                .paths
                .iter()
                .any(|path| path.ends_with("Synth Pad 01.wav")),
            "fuzzy-only matches should be dropped once the confident cap is full"
        );
    }

    #[test]
    fn tag_only_search_matches_metadata_on_audio_files() {
        let dir = std::env::temp_dir().join("tundra_tag_search_test");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let mtime_secs = file_mtime_secs(&audio).expect("temp file mtime");
        let paths = vec![dir.join("nested"), audio.clone()];
        let mut cache = HashMap::new();
        cache.insert(
            audio.clone(),
            CachedMetadata {
                mtime_secs,
                fields: TagFields {
                    bpm: "120".into(),
                    ..TagFields::default()
                },
            },
        );
        let filters = vec![TagFilter {
            field: TagField::Bpm,
            value: "120".into(),
        }];
        let result = collect_tag_matches(&paths, &filters, Arc::new(cache), true);
        assert_eq!(result.paths, vec![audio.clone()]);
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn tag_match_requires_every_filter() {
        let dir = std::env::temp_dir().join("tundra_tag_match_all_test");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let mtime_secs = file_mtime_secs(&audio).expect("temp file mtime");
        let paths = vec![audio.clone()];
        let mut cache = HashMap::new();
        cache.insert(
            audio.clone(),
            CachedMetadata {
                mtime_secs,
                fields: TagFields {
                    bpm: "120".into(),
                    key: "Am".into(),
                    ..TagFields::default()
                },
            },
        );
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
        let result = collect_tag_matches(&paths, &filters, Arc::new(cache), true);
        assert!(result.paths.is_empty(), "partial tag matches should be rejected");
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn instrument_filter_uses_explicit_instrument() {
        let fields = TagFields {
            explicit_instrument: "Snare Drum".into(),
            instrument: "Drums".into(),
            ..TagFields::default()
        };
        let matcher = tag_search_matcher();
        let filter = TagFilter {
            field: TagField::Instrument,
            value: "snare".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &filter).is_some(),
            "explicit instrument should satisfy instrument filter"
        );
    }

    #[test]
    fn format_control_bar_tags_shows_instrument_bpm_and_key() {
        let fields = TagFields {
            explicit_instrument: "Snare".into(),
            bpm: "120".into(),
            key: "Am".into(),
            ..TagFields::default()
        };
        assert_eq!(
            control_bar_tags(&fields),
            vec![
                (TagField::Instrument, "Snare".into()),
                (TagField::Bpm, "120".into()),
                (TagField::Key, "Am".into()),
            ]
        );
        let summary = format_control_bar_tags(&fields).expect("tag summary");
        assert!(summary.contains("Instrument: Snare"));
        assert!(summary.contains("BPM: 120"));
        assert!(summary.contains("Key: Am"));
    }

    #[test]
    fn tag_filter_fuzzy_matches_partial_values() {
        let matcher = tag_search_matcher();
        let fields = TagFields {
            bpm: "120.00".into(),
            ..TagFields::default()
        };
        let filter = TagFilter {
            field: TagField::Bpm,
            value: "120".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &filter).is_some(),
            "tag filters should fuzzy-match partial field values"
        );
    }

    #[test]
    fn tag_filter_fuzzy_matches_multi_word_values() {
        let matcher = tag_search_matcher();
        let fields = TagFields {
            comment: "dark snare loop".into(),
            ..TagFields::default()
        };
        let filter = TagFilter {
            field: TagField::Comment,
            value: "snare loop".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &filter).is_some(),
            "multi-word tag filters should require every term to match"
        );
        let miss = TagFilter {
            field: TagField::Comment,
            value: "snare kick".into(),
        };
        assert!(
            tag_field_score(&matcher, &fields, &miss).is_none(),
            "multi-word tag filters should reject partial term matches"
        );
    }

    #[test]
    fn tundra_tagged_status_allows_retag_but_preserves_user_tags() {
        let dir = unique_temp_dir("tundra_retag_status");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        write_minimal_wav(&audio);
        assert!(write_auto_tags(&audio, "Snare").expect("tundra write"));

        let status = auto_tag_field_status(&audio).expect("status");
        assert!(!status.needs_instrument);
        assert!(!status.can_retag_instrument);
        assert!(tundra_tag_is_current(&audio, riff_info(&audio).get(WAV_COMMENT_KEY).unwrap_or("")));

        let user_dir = unique_temp_dir("tundra_user_tag");
        std::fs::create_dir_all(&user_dir).expect("temp dir");
        let user_audio = user_dir.join("snare.wav");
        write_minimal_wav(&user_audio);
        {
            use lofty::config::WriteOptions;
            use lofty::file::AudioFile;
            use lofty::iff::wav::RiffInfoList;

            let mut wav = {
                let mut file = std::fs::File::open(&user_audio).expect("open");
                lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse")
            };
            let mut info = RiffInfoList::new();
            info.insert(WAV_INSTRUMENT_KEY.to_string(), "Snare".to_string());
            info.insert(WAV_COMMENT_KEY.to_string(), "Recorded live".to_string());
            wav.set_riff_info(info);
            wav.save_to_path(&user_audio, WriteOptions::default())
                .expect("save user tags");
        }

        let user_status = auto_tag_field_status(&user_audio).expect("user status");
        assert!(!user_status.needs_instrument);
        assert!(!user_status.can_retag_instrument);
        assert!(
            !write_auto_tags(&user_audio, "Kick").expect("user tag write attempt"),
            "user-owned instrument tags must not be overwritten"
        );
        assert_eq!(instrument_tag(&user_audio).as_deref(), Some("Snare"));

        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(user_dir);
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
    fn metadata_lookup_matches_cache_key_variants() {
        let dir = unique_temp_dir("tundra_lookup_keys");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let mtime_secs = file_mtime_secs(&audio).expect("temp file mtime");
        let key = crate::path_util::cache_key(audio.clone());
        let mut cache = HashMap::new();
        cache.insert(
            key,
            CachedMetadata {
                mtime_secs,
                fields: TagFields {
                    explicit_instrument: "Snare".into(),
                    instrument: "Snare".into(),
                    ..TagFields::default()
                },
            },
        );
        let metadata = Arc::new(cache);
        let mut lookup = MetadataLookup::new(Arc::clone(&metadata));
        assert_eq!(lookup.tag_fields(&audio).explicit_instrument, "Snare");

        let hits = tag_search_cached_paths(
            std::slice::from_ref(&audio),
            &[TagFilter {
                field: TagField::Instrument,
                value: "snare".to_string(),
            }],
            metadata,
        );
        assert_eq!(hits.paths, vec![audio.clone()]);
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn cached_tag_search_skips_unindexed_files() {
        let dir = unique_temp_dir("tundra_cached_skip");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        std::fs::write(&audio, b"RIFF").unwrap();
        let hits = tag_search_cached_paths(
            std::slice::from_ref(&audio),
            &[TagFilter {
                field: TagField::Instrument,
                value: "snare".to_string(),
            }],
            Arc::new(HashMap::new()),
        );
        assert!(
            hits.paths.is_empty(),
            "tag-only search must not parse unindexed files"
        );
        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn file_search_debounce_is_longer_for_two_chars() {
        assert_eq!(file_search_debounce_ms(2), FILE_SEARCH_DEBOUNCE_MS_SHORT);
        assert_eq!(file_search_debounce_ms(3), FILE_SEARCH_DEBOUNCE_MS);
    }

    #[test]
    fn file_search_active_requires_two_chars_without_tags() {
        assert!(!file_search_active("2", &[]));
        assert!(!file_search_active(" ", &[]));
        assert!(file_search_active("ki", &[]));
        assert!(file_search_active("", &[TagFilter {
            field: TagField::Instrument,
            value: "Kick".into(),
        }]));
    }

    #[test]
    fn tag_search_narrows_with_single_char_filename_query() {
        let dir = unique_temp_dir("tundra_tag_narrow");
        std::fs::create_dir_all(&dir).unwrap();
        let filters = vec![TagFilter {
            field: TagField::Instrument,
            value: "Kick".into(),
        }];
        let names = ["1 kick.wav", "2 kick.wav", "3 kick.wav"];
        let mut paths = Vec::new();
        let mut cache = HashMap::new();
        for name in names {
            let path = dir.join(name);
            std::fs::write(&path, b"RIFF").unwrap();
            let mtime_secs = file_mtime_secs(&path).expect("mtime");
            cache.insert(
                path.clone(),
                CachedMetadata {
                    mtime_secs,
                    fields: TagFields {
                        instrument: "Kick".into(),
                        ..Default::default()
                    },
                },
            );
            paths.push(path);
        }
        let metadata = Arc::new(cache);

        let all = search_paths(&paths, "", &filters, false, false, Arc::clone(&metadata));
        assert_eq!(all.paths.len(), 3);

        let narrowed = search_paths(&paths, "2", &filters, false, false, metadata);
        assert_eq!(narrowed.paths.len(), 1);
        assert!(narrowed.paths[0].ends_with("2 kick.wav"));

        let _ = std::fs::remove_dir_all(dir);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{prefix}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    fn write_minimal_wav(path: &Path) {
        write_wav_chunks(
            path,
            vec![
                (*b"fmt ", wav_pcm_fmt_chunk()),
                (*b"data", vec![0; 512]),
            ],
        );
    }

    fn wav_pcm_fmt_chunk() -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&44100u32.to_le_bytes());
        fmt.extend_from_slice(&88200u32.to_le_bytes());
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&16u16.to_le_bytes());
        fmt
    }

    fn write_wav_chunks(path: &Path, chunks: Vec<([u8; 4], Vec<u8>)>) {
        std::fs::write(path, encode_riff_wave(&chunks)).unwrap();
    }

    fn wav_chunk<'a>(chunks: &'a [([u8; 4], Vec<u8>)], id: &[u8; 4]) -> Option<&'a [u8]> {
        chunks
            .iter()
            .find(|(found, _)| found == id)
            .map(|(_, data)| data.as_slice())
    }

    fn wav_list_chunk<'a>(
        chunks: &'a [([u8; 4], Vec<u8>)],
        form: &[u8; 4],
    ) -> Option<&'a [u8]> {
        chunks.iter().find_map(|(id, data)| {
            (id == b"LIST" && data.len() >= 4 && &data[..4] == form).then_some(data.as_slice())
        })
    }

    #[test]
    fn write_auto_tags_preserves_wav_non_info_chunks() {
        let dir = unique_temp_dir("tundra_wav_chunk_preserve");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
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
        write_wav_chunks(
            &audio,
            vec![
                (*b"fmt ", wav_pcm_fmt_chunk()),
                (*b"data", vec![0; 512]),
                (*b"smpl", smpl.clone()),
                (*b"cue ", cue.clone()),
                (*b"inst", inst.clone()),
                (*b"acid", acid.clone()),
                (*b"LIST", adtl.clone()),
            ],
        );

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

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_instrument_tag_sets_wav_riff_info_for_explorer() {
        use lofty::file::TaggedFileExt;
        use lofty::tag::TagType;

        let dir = unique_temp_dir("tundra_wav_instrument_tag");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);

        write_auto_tags(&audio, "Kick").expect("write instrument");

        let bytes = std::fs::read(&audio).expect("read tagged wav bytes");
        assert!(
            bytes.windows(4).any(|chunk| chunk == b"LIST" || chunk == b"id3 " || chunk == b"ID3"),
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

        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn write_instrument_tag_replaces_existing_wav_comment() {
        use lofty::config::WriteOptions;
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::probe::Probe;
        use lofty::tag::{Accessor, Tag, TagType};

        let dir = unique_temp_dir("tundra_wav_existing_comment");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);

        let mut tagged = Probe::open(&audio)
            .expect("open wav")
            .read()
            .expect("read wav");
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

        let _ = std::fs::remove_file(&audio);
        let _ = std::fs::remove_dir(dir);
    }

    #[test]
    fn instrument_hint_reads_folder_and_prefers_filename() {
        let folder_hint = instrument_hint_from_path(Path::new(r"F:\Samples\ADM Samples - Copy\snares\tight_01.wav"));
        assert_eq!(folder_hint.as_deref(), Some("Snare"));

        let file_hint = instrument_hint_from_path(Path::new(r"F:\Samples\snares\cymbal_roll.wav"));
        assert_eq!(file_hint.as_deref(), Some("Cymbal"));

        let kick_hint = instrument_hint_from_path(Path::new(r"C:\Drums\Kicks\808_kick_01.wav"));
        assert_eq!(kick_hint.as_deref(), Some("Kick"));

        assert_eq!(
            instrument_hint_from_path(Path::new(r"C:\Drums\808_hat.wav")).as_deref(),
            Some("Hi-Hat")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"C:\Drums\snow_01.wav")),
            None
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"C:\hats\tight_01.wav")).as_deref(),
            Some("Hi-Hat")
        );

        let deep_hint = instrument_hint_from_path(Path::new(
            r"F:\Libraries\Pack A\Drums\One Shots\Snares\tight_01.wav",
        ));
        assert_eq!(deep_hint.as_deref(), Some("Snare"));

        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Bongo\hit_01.wav")).as_deref(),
            Some("Percussion")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Bongos\layer.wav")).as_deref(),
            Some("Percussion")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\ADM\Perc\tight.wav")).as_deref(),
            Some("Percussion")
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Perc\01.wav")).as_deref(),
            Some("Percussion"),
            "Perc must not false-match Kick"
        );
        assert_eq!(
            instrument_hint_from_path(Path::new(r"F:\Samples\Bass\low.wav")).as_deref(),
            Some("Bass"),
            "Bass must not false-match Kick via bassdrum"
        );
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
        assert_eq!(
            artist_hint_from_path(Path::new(r"F:\Samples\KSHMR\Vol4\Kicks\kick.wav")).as_deref(),
            Some("KSHMR")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"F:\Samples\KSHMR\Kicks\kick.wav")).as_deref(),
            Some("KSHMR")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"D:\Splice\packs\deadmau5\kick.wav")).as_deref(),
            Some("deadmau5")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"F:\Samples\Native Instruments\Battery 4\Snares\snare.wav"))
                .as_deref(),
            Some("Native Instruments")
        );
        assert_eq!(
            artist_hint_from_path(Path::new(r"C:\Samples\snares\tight_01.wav")),
            None
        );
    }

    #[test]
    fn read_tag_fields_uses_artist_hint_when_file_is_untagged() {
        let dir = unique_temp_dir("tundra_artist_hint_read");
        let _ = std::fs::create_dir_all(dir.join("KSHMR").join("Kicks"));
        let audio = dir.join("KSHMR").join("Kicks").join("kick.wav");
        write_minimal_wav(&audio);

        let fields = read_tag_fields(&audio).expect("read tag fields");
        assert_eq!(fields.artist, "KSHMR");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_instrument_tag_sets_wav_artist_from_directory() {
        let dir = unique_temp_dir("tundra_wav_artist_tag");
        let _ = std::fs::create_dir_all(dir.join("KSHMR").join("Kicks"));
        let audio = dir.join("KSHMR").join("Kicks").join("kick.wav");
        write_minimal_wav(&audio);

        write_auto_tags(&audio, "Kick").expect("write instrument");

        assert_eq!(
            riff_info(&audio).get(WAV_ARTIST_KEY).map(str::to_string),
            Some("KSHMR".to_string())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_manual_tags_round_trips_wav_fields_after_native_write() {
        let dir = unique_temp_dir("tundra_manual_wav_tags");
        let _ = std::fs::create_dir_all(dir.join("KSHMR").join("Kicks"));
        let audio = dir.join("KSHMR").join("Kicks").join("kick.wav");
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
        assert_eq!(
            riff.get(WAV_GENRE_KEY).map(str::to_string),
            Some("Drums".to_string())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_manual_tags_round_trips_flac_generic_fields() {
        let audio = staged_fixture("flac", "tundra_manual_flac_tags");
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
        use lofty::tag::Accessor;
        assert_eq!(vorbis.title().as_deref(), Some("Warm Kick"));
        assert_eq!(vorbis.genre().as_deref(), Some("Drums"));

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    #[test]
    fn write_auto_tags_extends_file_with_existing_non_auto_tags() {
        use lofty::config::WriteOptions;
        use lofty::file::{AudioFile, TaggedFileExt};
        use lofty::probe::Probe;
        use lofty::tag::{Accessor, Tag, TagType};

        let dir = unique_temp_dir("tundra_extend_existing_tags");
        let _ = std::fs::create_dir_all(dir.join("KSHMR").join("Kicks"));
        let audio = dir.join("KSHMR").join("Kicks").join("kick.wav");
        write_minimal_wav(&audio);

        let mut tagged = Probe::open(&audio)
            .expect("open wav")
            .read()
            .expect("read wav");
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

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn write_auto_tags_sets_flac_instrument_field_not_comment() {
        let audio = staged_fixture("flac", "tundra_flac_instrument_field");

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

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    #[test]
    fn write_auto_tags_skips_retag_when_tag_version_is_current() {
        let dir = unique_temp_dir("tundra_retag_auto_tags");
        let _ = std::fs::create_dir_all(&dir);
        let audio = dir.join("kick.wav");
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

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_tundra_comment_is_eligible_for_upgrade() {
        let dir = unique_temp_dir("tundra_legacy_comment");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        write_minimal_wav(&audio);
        {
            use lofty::config::WriteOptions;
            use lofty::file::AudioFile;
            use lofty::iff::wav::RiffInfoList;

            let mut wav = {
                let mut file = std::fs::File::open(&audio).expect("open");
                lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse")
            };
            let mut info = RiffInfoList::new();
            info.insert(WAV_INSTRUMENT_KEY.to_string(), "Snare".to_string());
            info.insert(WAV_COMMENT_KEY.to_string(), "Tundra".to_string());
            wav.set_riff_info(info);
            wav.save_to_path(&audio, WriteOptions::default())
                .expect("save legacy tags");
        }

        let status = auto_tag_field_status(&audio).expect("status");
        assert!(!status.needs_instrument);
        assert!(status.needs_comment);
        assert!(status.can_retag_instrument);

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Copies a real encoder-produced fixture into a scratch directory. Named
    /// `sample.<ext>` so no instrument can be inferred from the filename.
    fn staged_fixture(ext: &str, label: &str) -> PathBuf {
        let dir = unique_temp_dir(label);
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let audio = dir.join(format!("sample.{ext}"));
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/assets")
            .join(format!("tone.{ext}"));
        std::fs::copy(&fixture, &audio)
            .unwrap_or_else(|err| panic!("copy {}: {err}", fixture.display()));
        audio
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

    fn finds_by_instrument(path: &Path, query: &str) -> bool {
        tag_search_paths(
            std::slice::from_ref(&path.to_path_buf()),
            &[TagFilter {
                field: TagField::Instrument,
                value: query.to_string(),
            }],
            Arc::new(HashMap::new()),
        )
        .paths
        .iter()
        .any(|hit| hit == path)
    }

    /// The end-to-end contract: auto-tagging a file of any supported format
    /// makes `instrument:Kick` find it, and the label reads back from the
    /// container so third-party taggers and re-scans agree.
    #[test]
    fn instrument_round_trips_and_is_searchable_for_every_format() {
        for ext in ["wav", "flac", "mp3", "ogg", "aiff"] {
            let audio = staged_fixture(ext, &format!("tundra_round_trip_{ext}"));

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

            let _ = std::fs::remove_dir_all(audio.parent().unwrap());
        }
    }

    /// Auto-tagging must never destroy artwork the user already had.
    #[test]
    fn tagging_preserves_embedded_cover_art() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;
        use lofty::picture::{MimeType, Picture, PictureType};

        let art: Vec<u8> = vec![
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xde, 0xad, 0xbe, 0xef,
        ];

        let audio = staged_fixture("mp3", "tundra_cover_art");
        {
            let mut mp3 = {
                let mut file = std::fs::File::open(&audio).expect("open mp3");
                lofty::mpeg::MpegFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse mp3")
            };
            let mut id3 = mp3.remove_id3v2().unwrap_or_default();
            id3.insert_picture(
                Picture::unchecked(art.clone())
                    .pic_type(PictureType::CoverFront)
                    .mime_type(MimeType::Png)
                    .build(),
            );
            mp3.set_id3v2(id3);
            mp3.save_to_path(&audio, WriteOptions::default())
                .expect("save art");
        }

        assert!(write_auto_tags(&audio, "Kick").expect("tag"));

        let mut file = std::fs::File::open(&audio).expect("reopen mp3");
        let mut mp3 =
            lofty::mpeg::MpegFile::read_from(&mut file, lofty::config::ParseOptions::new())
                .expect("reparse mp3");
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

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    /// Tags a file already carries are a hint source, so a file whose path says
    /// nothing still gets classified from what the tagger before us recorded.
    #[test]
    fn instrument_hint_falls_back_to_existing_tags() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;

        let audio = staged_fixture("wav", "tundra_tag_hint");
        assert_eq!(
            instrument_hint_from_path(&audio),
            None,
            "fixture path must not name an instrument, or this proves nothing"
        );

        let mut wav = {
            let mut file = std::fs::File::open(&audio).expect("open wav");
            lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                .expect("parse wav")
        };
        let mut info = wav.remove_riff_info().unwrap_or_default();
        info.insert("IGNR".to_string(), "Kick".to_string());
        wav.set_riff_info(info);
        wav.save_to_path(&audio, WriteOptions::default())
            .expect("save genre");

        assert_eq!(
            instrument_hint(&audio),
            Some(("Kick".to_string(), HintSource::Tags)),
            "an existing genre of Kick should hint the instrument"
        );

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    /// Files an older build tagged into the grouping field carry no canonical
    /// instrument, so the grouping has to survive as a hint and get rewritten
    /// to the canonical key.
    #[test]
    fn legacy_grouping_hints_instrument_and_is_rewritten_canonically() {
        use lofty::config::WriteOptions;
        use lofty::file::AudioFile;

        let audio = staged_fixture("ogg", "tundra_legacy_grouping");
        {
            let mut ogg = {
                let mut file = std::fs::File::open(&audio).expect("open ogg");
                lofty::ogg::VorbisFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse ogg")
            };
            ogg.vorbis_comments_mut()
                .insert("GROUPING".to_string(), "Kick".to_string());
            ogg.save_to_path(&audio, WriteOptions::default())
                .expect("save grouping");
        }

        assert_eq!(
            instrument_hint(&audio),
            Some(("Kick".to_string(), HintSource::Tags)),
            "a legacy grouping should still hint the instrument"
        );
        assert!(
            auto_tag_field_status(&audio)
                .expect("status")
                .needs_instrument,
            "a grouping is not the canonical key, so the file still needs tagging"
        );

        assert!(write_auto_tags(&audio, "Kick").expect("rewrite canonically"));
        assert_eq!(
            instrument_tag(&audio).as_deref(),
            Some("Kick"),
            "instrument should now read from the canonical INSTRUMENT key"
        );
        assert!(finds_by_instrument(&audio, "Kick"));

        let _ = std::fs::remove_dir_all(audio.parent().unwrap());
    }

    /// A container that cannot hold a native tag must not lose the label: it
    /// goes to the SQLite sidecar, and search still finds the file.
    #[test]
    fn unwritable_container_falls_back_to_sidecar_store_and_stays_searchable() {
        let dir = unique_temp_dir("tundra_sidecar_fallback");
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        let audio = dir.join("sample.wav");
        std::fs::write(&audio, b"not actually a RIFF container").expect("write junk");
        let db = dir.join("tags.db");

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
            assert_eq!(
                crate::tag_store::tag_version(&audio),
                Some(TUNDRA_TAG_VERSION)
            );
            assert!(
                !write_auto_tags(&audio, "Snare").expect("same-version sidecar retag"),
                "current-version sidecar tags must not be replaced"
            );
            assert_eq!(
                crate::tag_store::instrument(&audio).as_deref(),
                Some("Kick")
            );
            assert_eq!(
                write_auto_tags(&audio, "Kick"),
                Ok(false),
                "unchanged sidecar label should no-op"
            );
            assert!(
                !auto_tag_field_status(&audio)
                    .expect("status")
                    .can_retag_instrument,
                "current sidecar version should skip auto tag"
            );
        });

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sidecar_row_does_not_own_user_native_instrument() {
        let dir = unique_temp_dir("tundra_sidecar_user_native");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("snare.wav");
        write_minimal_wav(&audio);
        {
            use lofty::config::WriteOptions;
            use lofty::file::AudioFile;
            use lofty::iff::wav::RiffInfoList;

            let mut wav = {
                let mut file = std::fs::File::open(&audio).expect("open");
                lofty::iff::wav::WavFile::read_from(&mut file, lofty::config::ParseOptions::new())
                    .expect("parse")
            };
            let mut info = RiffInfoList::new();
            info.insert(WAV_INSTRUMENT_KEY.to_string(), "Snare".to_string());
            info.insert(WAV_COMMENT_KEY.to_string(), "Recorded live".to_string());
            wav.set_riff_info(info);
            wav.save_to_path(&audio, WriteOptions::default())
                .expect("save user tags");
        }

        crate::tag_store::set_instrument(&audio, "Kick", 0).expect("stale sidecar");

        let status = auto_tag_field_status(&audio).expect("status");
        assert!(!status.can_retag_instrument);
        assert!(
            !write_auto_tags(&audio, "Kick").expect("user tag write attempt"),
            "sidecar must not unlock overwrite of a native user instrument"
        );
        assert_eq!(instrument_tag(&audio).as_deref(), Some("Snare"));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn instrument_bass_search_does_not_match_kick() {
        let dir = unique_temp_dir("tundra_bass_not_kick");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);
        assert!(write_auto_tags(&audio, "Kick").expect("tag kick"));
        assert!(finds_by_instrument(&audio, "Kick"));
        assert!(
            !finds_by_instrument(&audio, "Bass"),
            "instrument:bass must not match a Kick via bassdrum"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_and_replace_preserves_original_when_edit_fails() {
        let dir = unique_temp_dir("tundra_stage_edit_fail");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);
        let original = std::fs::read(&audio).expect("original");

        let err = stage_and_replace(&audio, |_| Err("edit failed".into()));
        assert!(err.is_err());

        assert_eq!(std::fs::read(&audio).expect("dest"), original);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(leftovers.len(), 1, "failed edit must delete staged tmp");
        assert_eq!(leftovers[0], audio);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_and_replace_removes_tmp_when_sync_fails() {
        let dir = unique_temp_dir("tundra_stage_sync_fail");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);
        let original = std::fs::read(&audio).expect("original");

        let err = stage_and_replace(&audio, |tmp| {
            std::fs::write(tmp, b"partial").expect("write tmp");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o000))
                    .expect("lock tmp");
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
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(leftovers.len(), 1, "sync failure must delete staged tmp");
        assert_eq!(leftovers[0], audio);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn stage_and_replace_succeeds_when_dest_removed_before_replace() {
        let dir = unique_temp_dir("tundra_stage_missing_dest");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);

        stage_and_replace(&audio, |tmp| {
            std::fs::remove_file(&audio).expect("dest gone before replace");
            std::fs::write(tmp, b"mutated-copy").expect("mutate tmp");
            Ok(())
        })
        .expect("replace into previously missing dest");

        assert_eq!(std::fs::read(&audio).expect("dest"), b"mutated-copy");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_and_replace_swaps_successfully_and_cleans_tmp() {
        let dir = unique_temp_dir("tundra_stage_success");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);

        stage_and_replace(&audio, |tmp| {
            let mut bytes = std::fs::read(tmp).map_err(|err| err.to_string())?;
            bytes.extend_from_slice(b"tail-marker");
            std::fs::write(tmp, bytes).map_err(|err| err.to_string())
        })
        .expect("stage and replace");

        let tagged = std::fs::read(&audio).expect("tagged");
        assert!(tagged.ends_with(b"tail-marker"));
        let sidecars: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".tundra-")
            })
            .collect();
        assert!(sidecars.is_empty(), "successful replace removes tmp");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_and_replace_leaves_original_intact_during_edit() {
        let dir = unique_temp_dir("tundra_stage_copy_first");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
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
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_and_replace_restores_readonly_permissions_on_success() {
        let dir = unique_temp_dir("tundra_stage_perms");
        std::fs::create_dir_all(&dir).expect("temp dir");
        let audio = dir.join("kick.wav");
        write_minimal_wav(&audio);
        let mut perms = std::fs::metadata(&audio).expect("meta").permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&audio, perms).expect("readonly");

        stage_and_replace(&audio, |tmp| {
            std::fs::write(tmp, std::fs::read(tmp).expect("read")).map_err(|err| err.to_string())
        })
        .expect("replace readonly");

        let restored = std::fs::metadata(&audio).expect("meta").permissions();
        assert!(
            restored.readonly(),
            "original read-only attribute must be restored"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stage_and_replace_preserves_original_when_replace_fails() {
        use crate::test_fixtures::with_replace_blocked;

        let dir = crate::test_fixtures::ScratchDir::new("stage-replace-fail");
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
        assert_eq!(
            crate::test_fixtures::count_tundra_sidecars(dir.path()),
            0,
            "failed replace must delete staged tmp"
        );
    }
