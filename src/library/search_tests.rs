use super::{cached_paths_for_root, execute_file_search, SearchOutput, SearchRequest, Shared};
use crate::metadata::{CachedMetadata, TagField, TagFields, TagFilter};
use crate::path_util::file_mtime_secs;
use crate::path_util::cache_key;
use crate::test_fixtures::ScratchDir;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

fn shared<V>(map: HashMap<PathBuf, V>) -> Shared<V> {
    Arc::new(RwLock::new(Arc::new(map)))
}

fn kick_tags(mtime_secs: u64) -> CachedMetadata {
    CachedMetadata {
        mtime_secs,
        fields: TagFields {
            explicit_instrument: "Kick".into(),
            instrument: "Kick".into(),
            ..TagFields::default()
        },
    }
}

/// A library with `Drums/one shot.wav` whose index entry says it is a kick.
fn library_with_tagged_kick() -> (ScratchDir, PathBuf, Shared<CachedMetadata>) {
    let root = ScratchDir::new("tag-search");
    let nested = root.path().join("Drums");
    std::fs::create_dir_all(&nested).unwrap();
    let audio = nested.join("one shot.wav");
    std::fs::write(&audio, b"RIFF").unwrap();
    let mtime = file_mtime_secs(&audio).expect("temp file mtime");
    let metadata = shared(HashMap::from([(cache_key(&audio), kick_tags(mtime))]));
    (root, audio, metadata)
}

fn search(roots: &[&Path], metadata: Shared<CachedMetadata>, file_query: &str, instrument: &str) -> SearchOutput {
    futures::executor::block_on(execute_file_search(SearchRequest {
        debounce: std::time::Duration::ZERO,
        allowed_roots: roots.iter().map(|root| root.to_path_buf()).collect(),
        dir_cache: shared(HashMap::new()),
        metadata_cache: metadata,
        file_query: file_query.into(),
        tag_filters: vec![TagFilter {
            field: TagField::Instrument,
            value: instrument.into(),
        }],
        case_sensitive: false,
        show_directories: false,
        tag_only: file_query.trim().is_empty(),
        favorites: None,
    }))
}

/// Results come back however the walk or the index spelled the path, so compare normalized.
fn contains_path(paths: &[PathBuf], wanted: &Path) -> bool {
    let wanted = cache_key(wanted);
    paths.iter().any(|path| cache_key(&path) == wanted)
}

#[test]
fn file_query_keeps_tagged_files_the_walk_cannot_reach() {
    let (root, _, metadata) = library_with_tagged_kick();
    // Renamed, or on a drive that is momentarily offline, but still in the index.
    let ghost = root.path().join("Drums").join("ghost.wav");
    Arc::make_mut(&mut metadata.write().unwrap()).insert(cache_key(&ghost), kick_tags(0));

    let tag_only = search(&[root.path()], Arc::clone(&metadata), "", "kick").result.paths;
    assert!(contains_path(&tag_only, &ghost), "tag-only search missed {ghost:?}, got {tag_only:?}");

    // Narrowing with a file query must not drop what the tag filter just surfaced.
    let narrowed = search(&[root.path()], metadata, "ghost", "kick").result.paths;
    assert!(contains_path(&narrowed, &ghost), "file query dropped {ghost:?}");
}

#[test]
fn tag_only_search_finds_tagged_file_under_an_unwalked_root() {
    let (root, audio, metadata) = library_with_tagged_kick();
    let result = search(&[root.path()], metadata, "", "kick");
    assert!(result.result.paths.contains(&audio), "tag-only search missed {audio:?}");
    assert!(
        result.walked_roots.is_empty(),
        "tag-only must not walk the library when the metadata index can answer, got {:?}",
        result.walked_roots.keys().collect::<Vec<_>>()
    );
}

#[test]
fn tag_only_search_walks_a_root_with_no_cache_or_metadata() {
    let (known, _, metadata) = library_with_tagged_kick();
    let cold = ScratchDir::new("tag-cold");
    std::fs::write(cold.path().join("snare.wav"), b"RIFF").unwrap();

    let result = search(&[known.path(), cold.path()], metadata, "", "kick");
    assert!(!result.walked_roots.contains_key(known.path()), "root with metadata should not be walked");
    assert!(
        result.walked_roots.contains_key(cold.path()),
        "root with no cache and no metadata must be walked, got {:?}",
        result.walked_roots.keys().collect::<Vec<_>>()
    );
}

#[test]
fn tag_filter_matches_regardless_of_query_case() {
    let (root, audio, metadata) = library_with_tagged_kick();
    let paths = search(&[root.path()], metadata, "", "KICK").result.paths;
    assert!(paths.contains(&audio), "uppercase filter missed {audio:?}");
}

#[test]
fn file_query_narrows_an_active_tag_filter() {
    let (root, audio, metadata) = library_with_tagged_kick();
    let hit = search(&[root.path()], Arc::clone(&metadata), "shot", "kick").result.paths;
    assert!(hit.contains(&audio), "matching query dropped {audio:?}");
    let miss = search(&[root.path()], metadata, "zzzz", "kick").result.paths;
    assert!(miss.is_empty(), "non-matching query still returned {miss:?}");
}

#[test]
fn cached_paths_for_root_reports_missing_when_only_subtrees_are_cached() {
    let root = PathBuf::from("/Samples");
    let sub = PathBuf::from("/Samples/ADM Samples - Copy");
    let cache = HashMap::from([(
        sub.clone(),
        vec![sub.join("Snare").join("01_Snare.flac"), sub.join("Snare").join("02_Snare.flac")],
    )]);

    let (paths, found) = cached_paths_for_root(&cache, &root);
    assert!(!found, "visited subtrees are a slice of the library, not coverage of the root");
    assert_eq!(paths.len(), 2);
    assert!(paths.iter().any(|p| p.ends_with("01_Snare.flac")));
}

#[test]
fn cached_paths_for_root_reports_found_once_the_root_itself_is_walked() {
    let root = PathBuf::from("/Samples");
    let sub = PathBuf::from("/Samples/ADM Samples - Copy");
    let cache = HashMap::from([
        (root.clone(), vec![root.join("kick.wav")]),
        (sub.clone(), vec![sub.join("01_Snare.flac")]),
    ]);

    let (paths, found) = cached_paths_for_root(&cache, &root);
    assert!(found);
    assert_eq!(paths.len(), 2);
}

#[test]
#[cfg(windows)]
fn cached_paths_for_root_unions_stale_parent_and_verbatim_root() {
    let root = PathBuf::from(r"\\?\F:\Samples");
    let child = PathBuf::from(r"F:\Samples\ADM Samples - Copy");
    let cache = HashMap::from([
        (root.clone(), vec![PathBuf::from(r"\\?\F:\Samples\Old\kick.wav")]),
        (child.clone(), vec![child.join("Snare").join("01_Snare.flac")]),
    ]);

    let (paths, found) = cached_paths_for_root(&cache, &root);
    assert!(found);
    assert!(
        paths.iter().any(|p| p.ends_with("01_Snare.flac")),
        "stale parent listing must not hide a later child cache"
    );
    assert!(paths.iter().any(|p| p.ends_with("kick.wav")));
}

#[test]
#[cfg(windows)]
fn cached_paths_for_root_keeps_one_listing_per_cache_key() {
    let root = PathBuf::from(r"F:\Samples");
    let cache = HashMap::from([
        (root.clone(), vec![PathBuf::from(r"F:\Samples\a.wav"), PathBuf::from(r"F:\Samples\b.wav")]),
        (PathBuf::from(r"\\?\F:\Samples"), vec![PathBuf::from(r"\\?\F:\Samples\stale.wav")]),
    ]);

    let (paths, found) = cached_paths_for_root(&cache, &root);
    assert!(found);
    assert_eq!(paths.len(), 2, "same directory under two spellings must not union");
    assert!(paths.iter().any(|p| p.ends_with("a.wav")));
    assert!(paths.iter().any(|p| p.ends_with("b.wav")));
    assert!(paths.iter().all(|p| !p.ends_with("stale.wav")));
}
