use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::metadata::{
    index_paths, search_paths, tag_field_best_match, CachedMetadata, SearchResult, TagFilter,
};
use iced::keyboard::Modifiers;
use iced::Point;
use walkdir::WalkDir;

use crate::types::file_selector::FileList;
use crate::types::{is_audio, AUDIO_EXTENSIONS};

pub(crate) enum FileDragKind {
    File(PathBuf),
    Scroll,
}

pub(crate) struct FileDragPending {
    pub kind: FileDragKind,
    pub origin: Point,
    pub last: Point,
    pub threshold_met: bool,
    pub origin_locked: bool,
    pub awaiting_drag: bool,
    pub open_on_click: bool,
}

pub(crate) struct SidebarResize {
    pub origin_x: f32,
    pub origin_width: f32,
    pub pending_origin: bool,
}

pub(crate) struct FileListScrollbarDrag {
    pub track_top: f32,
    pub track_height: f32,
    pub grab_offset: f32,
}

pub(crate) struct TitleBarInteraction {
    pub drag_armed: bool,
    pub press_origin: Option<Point>,
}

pub(crate) fn walk_directory(dir: &Path) -> Vec<PathBuf> {
    crate::path_util::reclaim_write_sidecars_tree(dir);
    WalkDir::new(dir)
        .max_depth(100)
        .max_open(100)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| FileList::file_filter(e.path()))
        .filter_map(|e| match e {
            Ok(e) => Some(e.path().to_path_buf()),
            Err(_) => None,
        })
        .collect()
}

/// Paths indexed for `root`: the exact listing plus every cached subtree.
///
/// Every cache entry is a full recursive walk of its own key, so `root` counts as covered only
/// when the root itself was walked. Subtree entries alone describe the directories the user
/// happened to visit, which is a small slice of the library — treating those as coverage is
/// what made tag-only searches miss most files.
pub(crate) fn cached_paths_for_root(
    cache: &HashMap<PathBuf, Vec<PathBuf>>,
    root: &Path,
) -> (Vec<PathBuf>, bool) {
    let root_key = crate::path_util::cache_key(root.to_path_buf());
    let mut listings: HashMap<PathBuf, &Vec<PathBuf>> = HashMap::new();
    let mut root_walked = false;
    for (key, cached) in cache {
        let listing_key = crate::path_util::cache_key(key.clone());
        if listing_key != root_key && !listing_key.starts_with(&root_key) {
            continue;
        }
        if listing_key == root_key {
            root_walked = true;
        }
        if listings
            .get(&listing_key)
            .is_some_and(|existing| existing.len() >= cached.len())
        {
            continue;
        }
        listings.insert(listing_key, cached);
    }
    // Ordered by key so the union is stable: the same file can appear in several listings
    // under different spellings, and whichever copy survives dedup decides how it sorts.
    let mut listings: Vec<_> = listings.into_iter().collect();
    listings.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut paths = Vec::new();
    for (_, cached) in listings {
        paths.extend(cached.iter().cloned());
    }
    (paths, root_walked)
}

pub(crate) async fn execute_file_search(
    debounce_ms: u64,
    allowed_roots: Vec<PathBuf>,
    dir_cache: Arc<RwLock<HashMap<PathBuf, Vec<PathBuf>>>>,
    metadata_cache: Arc<RwLock<HashMap<PathBuf, CachedMetadata>>>,
    file_query: String,
    tag_filters: Vec<TagFilter>,
    case_sensitive: bool,
    show_directories: bool,
    tag_only: bool,
    favorites_only: bool,
    favorite_keys: HashSet<PathBuf>,
) -> SearchResult {
    async_io::Timer::after(Duration::from_millis(debounce_ms)).await;

    let mut paths = Vec::new();
    let mut cached_roots = HashMap::new();
    let missing_roots = {
        let cache = dir_cache.read().unwrap();
        let mut missing = Vec::new();
        for root in &allowed_roots {
            let (cached, found) = cached_paths_for_root(&cache, root);
            // Keep visited subtrees even when the allowed root itself was never walked.
            // Tag-only search can answer from this plus the metadata index; throwing the
            // partial cache away forced a full-library walk and left the folder listing
            // on screen until that walk finished.
            paths.extend(cached);
            if !found {
                missing.push(root.clone());
            }
        }
        missing
    };

    let metadata_map = metadata_cache.read().unwrap().clone();

    let mut walked = Vec::new();
    for root in missing_roots {
        // Skip only this root when its own index or subtree cache can answer.
        // A new allowed root with neither must still be walked.
        let root_has_metadata = metadata_map
            .keys()
            .any(|path| crate::path_util::is_under(path, &root));
        let root_has_cache = paths
            .iter()
            .any(|path| crate::path_util::is_under(path, &root));
        if tag_only && (root_has_metadata || root_has_cache) {
            continue;
        }
        let children = walk_directory(&root);
        paths.extend(children.iter().cloned());
        walked.extend(children.iter().cloned());
        cached_roots.insert(root, children);
    }

    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(crate::path_util::cache_key(path.clone())));

    if !tag_filters.is_empty() {
        // Safety net for files a walk can no longer reach (renamed or temporarily offline
        // directories) but whose tags are still known. Every tag filter needs this, not just a
        // tag-only query, or adding a file query would drop the very files the tag filter just
        // surfaced. Metadata keys are already cache keys, so `seen` rejects the walked ones
        // before `is_under` has to normalize anything.
        for path in metadata_map.keys() {
            if seen.contains(path) {
                continue;
            }
            if allowed_roots
                .iter()
                .any(|root| crate::path_util::is_under(path, root))
            {
                let resolved =
                    crate::path_util::resolve_open_path(path, paths.iter().map(|p| p.as_path()));
                let key = crate::path_util::cache_key(resolved.clone());
                if seen.insert(key) {
                    paths.push(resolved);
                }
            }
        }
    }

    if tag_only {
        paths.retain(|path| is_audio(path));
    }

    let metadata_snapshot = Arc::new(metadata_map);
    // Tag-only reads the index as-is except for roots we just walked: those need
    // `index_paths` or the persisted listing would skip-walk forever with no tags.
    let (metadata, preindexed) = if tag_filters.is_empty() {
        (metadata_snapshot, HashMap::new())
    } else if tag_only {
        if walked.is_empty() {
            (metadata_snapshot, HashMap::new())
        } else {
            let indexed = index_paths(&walked, Arc::clone(&metadata_snapshot));
            let mut merged = (*metadata_snapshot).clone();
            merged.extend(indexed.clone());
            (Arc::new(merged), indexed)
        }
    } else {
        let indexed = index_paths(&paths, Arc::clone(&metadata_snapshot));
        let mut merged = (*metadata_snapshot).clone();
        merged.extend(indexed.clone());
        (Arc::new(merged), indexed)
    };

    let mut result = search_paths(
        &paths,
        &file_query,
        &tag_filters,
        case_sensitive,
        show_directories,
        metadata,
    );
    if favorites_only {
        result.paths.retain(|path| {
            favorite_keys.contains(&crate::path_util::favorite_lookup_key(path))
        });
    }
    let known = paths;
    result.paths = result
        .paths
        .into_iter()
        .map(|path| crate::path_util::resolve_open_path(&path, known.iter().map(|p| p.as_path())))
        .collect();
    result.new_metadata.extend(preindexed);
    result.cached_roots = cached_roots;
    result
}

pub(crate) fn transport_shortcut_allowed(modifiers: Modifiers) -> bool {
    !modifiers.shift() && !modifiers.control() && !modifiers.alt() && !modifiers.logo()
}

pub(crate) async fn pick_folder(start_dir: PathBuf) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Select Folder")
        .set_directory(&start_dir)
        .pick_folder()
        .await
        .map(|folder| folder.path().to_path_buf())
}

pub(crate) async fn pick_audio_file(start_dir: PathBuf) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Select audio file")
        .set_directory(&start_dir)
        .add_filter("Audio", AUDIO_EXTENSIONS)
        .pick_file()
        .await
        .map(|file| file.path().to_path_buf())
}

pub(crate) fn tag_search_can_autocomplete(input: &str) -> bool {
    !input.contains(':')
        && !input.trim().is_empty()
        && tag_field_best_match(input).is_some()
}

#[cfg(test)]
mod tests {
    use super::{cached_paths_for_root, execute_file_search};
    use crate::metadata::{file_mtime_secs, CachedMetadata, TagField, TagFields, TagFilter};
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    fn library_with_tagged_kick() -> (PathBuf, PathBuf, Arc<RwLock<HashMap<PathBuf, CachedMetadata>>>)
    {
        let root = std::env::temp_dir().join(format!(
            "tundra_tag_search_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = root.join("Drums");
        std::fs::create_dir_all(&nested).unwrap();
        let audio = nested.join("one shot.wav");
        std::fs::write(&audio, b"RIFF").unwrap();

        let mut metadata = HashMap::new();
        metadata.insert(
            crate::path_util::cache_key(audio.clone()),
            CachedMetadata {
                mtime_secs: file_mtime_secs(&audio).expect("temp file mtime"),
                fields: TagFields {
                    explicit_instrument: "Kick".into(),
                    instrument: "Kick".into(),
                    ..TagFields::default()
                },
            },
        );

        (root, audio, Arc::new(RwLock::new(metadata)))
    }

    fn run_search_result(
        root: &PathBuf,
        metadata: Arc<RwLock<HashMap<PathBuf, CachedMetadata>>>,
        file_query: &str,
        filter_value: &str,
    ) -> crate::metadata::SearchResult {
        let filters = vec![TagFilter {
            field: TagField::Instrument,
            value: filter_value.to_string(),
        }];
        let tag_only = file_query.trim().is_empty();
        futures::executor::block_on(execute_file_search(
            0,
            vec![root.clone()],
            Arc::new(RwLock::new(HashMap::new())),
            metadata,
            file_query.to_string(),
            filters,
            false,
            false,
            tag_only,
            false,
            HashSet::new(),
        ))
    }

    fn run_search(
        root: &PathBuf,
        metadata: Arc<RwLock<HashMap<PathBuf, CachedMetadata>>>,
        file_query: &str,
        filter_value: &str,
    ) -> Vec<PathBuf> {
        run_search_result(root, metadata, file_query, filter_value).paths
    }

    /// A file the directory walk cannot reach — renamed, or on a drive that is momentarily
    /// offline — but whose tags the index still remembers. Only `execute_file_search`'s
    /// metadata pass can surface it.
    fn add_unwalkable_tagged_file(
        root: &PathBuf,
        metadata: &Arc<RwLock<HashMap<PathBuf, CachedMetadata>>>,
    ) -> PathBuf {
        let ghost = root.join("Drums").join("ghost.wav");
        metadata.write().unwrap().insert(
            crate::path_util::cache_key(ghost.clone()),
            CachedMetadata {
                mtime_secs: 0,
                fields: TagFields {
                    explicit_instrument: "Kick".into(),
                    instrument: "Kick".into(),
                    ..TagFields::default()
                },
            },
        );
        ghost
    }

    /// Results come back however the walk or the index spelled the path, so compare normalized.
    fn contains_path(paths: &[PathBuf], wanted: &PathBuf) -> bool {
        let wanted = crate::path_util::cache_key(wanted.clone());
        paths
            .iter()
            .any(|path| crate::path_util::cache_key(path.clone()) == wanted)
    }

    #[test]
    fn file_query_keeps_tagged_files_the_walk_cannot_reach() {
        let (root, _, metadata) = library_with_tagged_kick();
        let ghost = add_unwalkable_tagged_file(&root, &metadata);

        let tag_only = run_search(&root, Arc::clone(&metadata), "", "kick");
        assert!(
            contains_path(&tag_only, &ghost),
            "tag-only search missed {ghost:?}, got {tag_only:?}"
        );

        // Narrowing with a file query must not drop what the tag filter just surfaced.
        let narrowed = run_search(&root, metadata, "ghost", "kick");
        assert!(
            contains_path(&narrowed, &ghost),
            "file query dropped {ghost:?}, which the tag-only search returned"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tag_only_search_finds_tagged_file_under_an_unwalked_root() {
        let (root, audio, metadata) = library_with_tagged_kick();
        let result = run_search_result(&root, metadata, "", "kick");
        assert!(
            result.paths.contains(&audio),
            "tag-only search missed {audio:?}"
        );
        assert!(
            result.cached_roots.is_empty(),
            "tag-only must not walk the library when the metadata index can answer, got {:?}",
            result.cached_roots.keys().collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tag_only_search_walks_a_root_with_no_cache_or_metadata() {
        let (known, _, metadata) = library_with_tagged_kick();
        let cold = std::env::temp_dir().join(format!(
            "tundra_tag_cold_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&cold).unwrap();
        let cold_audio = cold.join("snare.wav");
        std::fs::write(&cold_audio, b"RIFF").unwrap();

        let filters = vec![TagFilter {
            field: TagField::Instrument,
            value: "kick".into(),
        }];
        let result = futures::executor::block_on(execute_file_search(
            0,
            vec![known.clone(), cold.clone()],
            Arc::new(RwLock::new(HashMap::new())),
            metadata,
            String::new(),
            filters,
            false,
            false,
            true,
            false,
            HashSet::new(),
        ));
        assert!(
            !result.cached_roots.contains_key(&known),
            "root with metadata should not be walked"
        );
        assert!(
            result.cached_roots.contains_key(&cold),
            "root with no cache and no metadata must be walked, got {:?}",
            result.cached_roots.keys().collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&known);
        let _ = std::fs::remove_dir_all(&cold);
    }

    #[test]
    fn tag_filter_matches_regardless_of_query_case() {
        let (root, audio, metadata) = library_with_tagged_kick();
        let paths = run_search(&root, metadata, "", "KICK");
        assert!(paths.contains(&audio), "uppercase filter missed {audio:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_query_narrows_an_active_tag_filter() {
        let (root, audio, metadata) = library_with_tagged_kick();
        let hit = run_search(&root, Arc::clone(&metadata), "shot", "kick");
        assert!(hit.contains(&audio), "matching query dropped {audio:?}");
        let miss = run_search(&root, metadata, "zzzz", "kick");
        assert!(miss.is_empty(), "non-matching query still returned {miss:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn cached_paths_for_root_reports_missing_when_only_subtrees_are_cached() {
        let root = PathBuf::from(r"F:\Samples");
        let sub = PathBuf::from(r"F:\Samples\ADM Samples - Copy");
        let mut cache = HashMap::new();
        cache.insert(
            sub.clone(),
            vec![
                sub.join("Snare").join("01_Snare.flac"),
                sub.join("Snare").join("02_Snare.flac"),
            ],
        );

        let (paths, found) = cached_paths_for_root(&cache, &root);
        assert!(
            !found,
            "visited subtrees are a slice of the library, not coverage of the root"
        );
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|p| p.ends_with("01_Snare.flac")));
    }

    #[test]
    fn cached_paths_for_root_reports_found_once_the_root_itself_is_walked() {
        let root = PathBuf::from(r"F:\Samples");
        let sub = PathBuf::from(r"F:\Samples\ADM Samples - Copy");
        let mut cache = HashMap::new();
        cache.insert(root.clone(), vec![root.join("kick.wav")]);
        cache.insert(sub.clone(), vec![sub.join("01_Snare.flac")]);

        let (paths, found) = cached_paths_for_root(&cache, &root);
        assert!(found);
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn cached_paths_for_root_unions_stale_parent_and_verbatim_root() {
        let root = PathBuf::from(r"\\?\F:\Samples");
        let child = PathBuf::from(r"F:\Samples\ADM Samples - Copy");
        let mut cache = HashMap::new();
        cache.insert(
            root.clone(),
            vec![PathBuf::from(r"\\?\F:\Samples\Old\kick.wav")],
        );
        cache.insert(
            child.clone(),
            vec![child.join("Snare").join("01_Snare.flac")],
        );

        let (paths, found) = cached_paths_for_root(&cache, &root);
        assert!(found);
        assert!(
            paths.iter().any(|p| p.ends_with("01_Snare.flac")),
            "stale parent listing must not hide a later child cache"
        );
        assert!(paths.iter().any(|p| p.ends_with("kick.wav")));
    }

    #[test]
    fn cached_paths_for_root_keeps_one_listing_per_cache_key() {
        let root = PathBuf::from(r"F:\Samples");
        let verbatim = PathBuf::from(r"\\?\F:\Samples");
        let mut cache = HashMap::new();
        cache.insert(
            root.clone(),
            vec![
                PathBuf::from(r"F:\Samples\a.wav"),
                PathBuf::from(r"F:\Samples\b.wav"),
            ],
        );
        cache.insert(
            verbatim,
            vec![PathBuf::from(r"\\?\F:\Samples\stale.wav")],
        );

        let (paths, found) = cached_paths_for_root(&cache, &root);
        assert!(found);
        assert_eq!(paths.len(), 2, "same directory under two spellings must not union");
        assert!(paths.iter().any(|p| p.ends_with("a.wav")));
        assert!(paths.iter().any(|p| p.ends_with("b.wav")));
        assert!(paths.iter().all(|p| !p.ends_with("stale.wav")));
    }
}
