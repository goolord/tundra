//! Background search, directory walks, drag interaction state.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::metadata::{
    index_paths, search_paths, tag_field_best_match, tag_search_cached_paths, CachedMetadata,
    SearchResult, TagFilter,
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
pub(crate) fn cached_paths_for_root(
    cache: &HashMap<PathBuf, Vec<PathBuf>>,
    root: &Path,
) -> (Vec<PathBuf>, bool) {
    let root_key = crate::path_util::cache_key(root.to_path_buf());
    let mut listings: HashMap<PathBuf, &Vec<PathBuf>> = HashMap::new();
    for (key, cached) in cache {
        let listing_key = crate::path_util::cache_key(key.clone());
        if listing_key != root_key && !listing_key.starts_with(&root_key) {
            continue;
        }
        if listings
            .get(&listing_key)
            .is_some_and(|existing| existing.len() >= cached.len())
        {
            continue;
        }
        listings.insert(listing_key, cached);
    }
    let mut paths = Vec::new();
    for cached in listings.into_values() {
        paths.extend(cached.iter().cloned());
    }
    let found = !paths.is_empty();
    (paths, found)
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
            if found {
                paths.extend(cached);
            } else {
                missing.push(root.clone());
            }
        }
        missing
    };

    for root in missing_roots {
        let children = walk_directory(&root);
        paths.extend(children.iter().cloned());
        cached_roots.insert(root, children);
    }

    let metadata_map = metadata_cache.read().unwrap().clone();
    if tag_only {
        for path in metadata_map.keys() {
            if allowed_roots
                .iter()
                .any(|root| crate::path_util::is_under(path, root))
            {
                paths.push(path.clone());
            }
        }
    }

    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(crate::path_util::cache_key(path.clone())));
    if tag_only {
        paths.retain(|path| is_audio(path));
    }

    let metadata_snapshot = Arc::new(metadata_map);
    let (metadata, preindexed) = if tag_filters.is_empty() {
        (metadata_snapshot, HashMap::new())
    } else {
        let indexed = index_paths(&paths, Arc::clone(&metadata_snapshot));
        let mut merged = (*metadata_snapshot).clone();
        merged.extend(indexed.clone());
        (Arc::new(merged), indexed)
    };

    let mut result = if tag_only {
        tag_search_cached_paths(&paths, &tag_filters, metadata)
    } else {
        search_paths(
            &paths,
            &file_query,
            &tag_filters,
            case_sensitive,
            show_directories,
            metadata,
        )
    };
    if favorites_only {
        result.paths.retain(|path| {
            favorite_keys.contains(&crate::path_util::cache_key(path.clone()))
        });
    }
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
