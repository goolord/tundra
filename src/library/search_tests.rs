use super::{SearchOutput, SearchRequest, Shared, cached_paths_for_root, execute_file_search};
use crate::metadata::{CachedMetadata, TagField, TagFields, TagFilter};
use crate::path_util::{cache_key, file_mtime_secs};
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
        fields: TagFields { explicit_instrument: "Kick".into(), instrument: "Kick".into(), ..TagFields::default() },
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
        tag_filters: vec![TagFilter { field: TagField::Instrument, value: instrument.into() }],
        case_sensitive: false,
        show_directories: false,
        favorites: None,
    }))
}

/// Results come back however the walk or the index spelled the path, so compare normalized.
fn contains_path(paths: &[PathBuf], wanted: &Path) -> bool {
    let wanted = cache_key(wanted);
    paths.iter().any(|path| cache_key(path) == wanted)
}

#[test]
fn file_query_narrows_the_tag_filter_but_keeps_files_the_walk_cannot_reach() {
    let (root, audio, metadata) = library_with_tagged_kick();
    // Renamed, or on a drive that is momentarily offline, but still in the index.
    let ghost = root.path().join("Drums").join("ghost.wav");
    Arc::make_mut(&mut metadata.write().unwrap()).insert(cache_key(&ghost), kick_tags(0));
    let found = |query: &str| search(&[root.path()], Arc::clone(&metadata), query, "kick").result.paths;

    let tag_only = found("");
    assert!(contains_path(&tag_only, &ghost), "tag-only search missed {ghost:?}, got {tag_only:?}");
    // Narrowing with a file query must not drop what the tag filter just surfaced.
    assert!(contains_path(&found("ghost"), &ghost), "file query dropped {ghost:?}");
    assert!(found("shot").contains(&audio), "matching query dropped {audio:?}");
    assert!(found("zzzz").is_empty(), "non-matching query still returned results");
}

#[test]
fn tag_only_search_answers_from_the_index_and_walks_only_cold_roots() {
    let (known, audio, metadata) = library_with_tagged_kick();
    let result = search(&[known.path()], Arc::clone(&metadata), "", "KICK");
    assert!(result.result.paths.contains(&audio), "uppercase filter missed {audio:?}");
    assert!(result.walked_roots.is_empty(), "the index can answer, so nothing is walked");

    let cold = ScratchDir::new("tag-cold");
    std::fs::write(cold.path().join("snare.wav"), b"RIFF").unwrap();
    let walked = search(&[known.path(), cold.path()], metadata, "", "kick").walked_roots;
    assert!(!walked.contains_key(known.path()), "root with metadata should not be walked");
    assert!(walked.contains_key(cold.path()), "root with no cache and no metadata must be walked");
}

/// `cached_paths_for_root` over listings given as `(dir, files)`.
fn cached(listings: &[(&str, &[&str])], root: &str) -> (Vec<PathBuf>, bool) {
    let cache =
        listings.iter().map(|(dir, files)| (PathBuf::from(dir), files.iter().map(PathBuf::from).collect())).collect();
    cached_paths_for_root(&cache, Path::new(root))
}

fn paths(files: &[&str]) -> Vec<PathBuf> {
    files.iter().map(PathBuf::from).collect()
}

#[test]
fn cached_paths_for_root_is_found_only_once_the_root_itself_is_walked() {
    let sub: (&str, &[&str]) = ("/Samples/ADM", &["/Samples/ADM/01_Snare.flac", "/Samples/ADM/02_Snare.flac"]);
    // Visited subtrees are a slice of the library, not coverage of the root.
    assert_eq!(cached(&[sub], "/Samples"), (paths(sub.1), false));

    let walked = cached(&[sub, ("/Samples", &["/Samples/kick.wav"])], "/Samples");
    assert_eq!(walked.0.len(), 3);
    assert!(walked.1);
}

#[test]
#[cfg(windows)]
fn cached_paths_for_root_merges_spellings_of_one_directory() {
    // A stale verbatim parent listing must not hide a later child cache.
    let (found, walked) = cached(
        &[
            (r"\\?\F:\Samples", &[r"\\?\F:\Samples\Old\kick.wav"]),
            (r"F:\Samples\ADM", &[r"F:\Samples\ADM\01_Snare.flac"]),
        ],
        r"\\?\F:\Samples",
    );
    assert!(walked);
    assert_eq!(found, paths(&[r"\\?\F:\Samples\Old\kick.wav", r"F:\Samples\ADM\01_Snare.flac"]));

    // The same directory under two spellings keeps only the fuller listing.
    let (found, walked) = cached(
        &[
            (r"F:\Samples", &[r"F:\Samples\a.wav", r"F:\Samples\b.wav"]),
            (r"\\?\F:\Samples", &[r"\\?\F:\Samples\stale.wav"]),
        ],
        r"F:\Samples",
    );
    assert!(walked);
    assert_eq!(found, paths(&[r"F:\Samples\a.wav", r"F:\Samples\b.wav"]));
}
