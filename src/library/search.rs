//! Runs a file or tag search over the allowed roots, using the cached
//! listings and tag index, and walking only roots nothing covers yet.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::cache::{lock_read, Shared};
use crate::metadata::{index_paths, is_audio, search, CachedMetadata, MetadataLookup, SearchQuery, SearchResult, TagFilter};
use crate::path_util::{cache_key, is_under, resolve_open_path};

/// Everything one search needs, captured on the UI thread.
pub struct SearchRequest {
    pub debounce: Duration,
    pub allowed_roots: Vec<PathBuf>,
    pub dir_cache: Shared<Vec<PathBuf>>,
    pub metadata_cache: Shared<CachedMetadata>,
    pub file_query: String,
    pub tag_filters: Vec<TagFilter>,
    pub case_sensitive: bool,
    pub show_directories: bool,
    /// Tag filters and no file query: answered from the index where possible.
    pub tag_only: bool,
    /// Restrict results to these favorite keys.
    pub favorites: Option<HashSet<PathBuf>>,
}

/// Paths indexed for `root`: the exact listing plus every cached subtree.
///
/// Every cache entry is a full recursive walk of its own key, so `root` counts as covered only
/// when the root itself was walked. Subtree entries alone describe the directories the user
/// happened to visit, which is a small slice of the library — treating those as coverage is
/// what made tag-only searches miss most files.
pub fn cached_paths_for_root(cache: &HashMap<PathBuf, Vec<PathBuf>>, root: &Path) -> (Vec<PathBuf>, bool) {
    let root_key = cache_key(root);
    let mut listings: HashMap<PathBuf, &Vec<PathBuf>> = HashMap::new();
    let mut root_walked = false;
    for (key, cached) in cache {
        let listing_key = cache_key(key);
        if !listing_key.starts_with(&root_key) {
            continue;
        }
        root_walked |= listing_key == root_key;
        if listings.get(&listing_key).is_some_and(|existing| existing.len() >= cached.len()) {
            continue;
        }
        listings.insert(listing_key, cached);
    }
    // Ordered by key so the union is stable: the same file can appear in several listings
    // under different spellings, and whichever copy survives dedup decides how it sorts.
    let mut listings: Vec<_> = listings.into_iter().collect();
    listings.sort_by(|(a, _), (b, _)| a.cmp(b));
    let paths = listings.into_iter().flat_map(|(_, cached)| cached.iter().cloned()).collect();
    (paths, root_walked)
}

/// A search's matches, plus the listings of roots it had to walk.
#[derive(Debug, Clone)]
pub struct SearchOutput {
    pub result: SearchResult,
    pub walked_roots: HashMap<PathBuf, Vec<PathBuf>>,
}

pub async fn execute_file_search(request: SearchRequest) -> SearchOutput {
    let SearchRequest {
        debounce,
        allowed_roots,
        dir_cache,
        metadata_cache,
        file_query,
        tag_filters,
        case_sensitive,
        show_directories,
        tag_only,
        favorites,
    } = request;
    async_io::Timer::after(debounce).await;

    let mut paths = Vec::new();
    let mut missing_roots = Vec::new();
    let listings = lock_read(&dir_cache);
    for root in &allowed_roots {
        let (cached, found) = cached_paths_for_root(&listings, root);
        // Keep visited subtrees even when the allowed root itself was never walked.
        // Tag-only search can answer from this plus the metadata index; throwing the
        // partial cache away forced a full-library walk and left the folder listing
        // on screen until that walk finished.
        paths.extend(cached);
        if !found {
            missing_roots.push(root.clone());
        }
    }
    drop(listings);

    let metadata_map = lock_read(&metadata_cache);

    let mut walked = Vec::new();
    let mut walked_roots = HashMap::new();
    for root in missing_roots {
        // Skip only this root when its own index or subtree cache can answer.
        // A new allowed root with neither must still be walked.
        let answerable = metadata_map.keys().any(|path| is_under(path, &root))
            || paths.iter().any(|path| is_under(path, &root));
        if tag_only && answerable {
            continue;
        }
        let children = super::walk_directory(&root);
        paths.extend(children.iter().cloned());
        walked.extend(children.iter().cloned());
        walked_roots.insert(root, children);
    }

    let mut seen = HashSet::new();
    paths.retain(|path| seen.insert(cache_key(path)));

    if !tag_filters.is_empty() {
        // Safety net for files a walk can no longer reach (renamed or temporarily offline
        // directories) but whose tags are still known. Every tag filter needs this, not just a
        // tag-only query, or adding a file query would drop the very files the tag filter just
        // surfaced. Metadata keys are already cache keys, so `seen` rejects the walked ones
        // before `is_under` has to normalize anything.
        for path in metadata_map.keys() {
            if seen.contains(path) || !allowed_roots.iter().any(|root| is_under(path, root)) {
                continue;
            }
            let resolved = resolve_open_path(path, paths.iter().map(PathBuf::as_path));
            if seen.insert(cache_key(&resolved)) {
                paths.push(resolved);
            }
        }
    }

    if tag_only {
        paths.retain(|path| is_audio(path));
    }

    // Tag-only search answers from the index, except for roots just walked:
    // those need indexing or a persisted listing would skip-walk forever with
    // no tags. A filename query reads tags lazily for the paths it matches.
    let indexed = if tag_only && !walked.is_empty() {
        index_paths(&walked, Arc::clone(&metadata_map))
    } else {
        HashMap::new()
    };
    let lookup = MetadataLookup::with_new_entries(metadata_map, indexed);

    let query = SearchQuery {
        text: &file_query,
        tag_filters: &tag_filters,
        case_sensitive,
        show_directories,
    };
    let mut result = search(&paths, &query, lookup);
    if let Some(favorites) = favorites {
        result
            .paths
            .retain(|path| favorites.contains(&crate::path_util::favorite_lookup_key(path)));
    }
    for path in &mut result.paths {
        *path = resolve_open_path(path, paths.iter().map(PathBuf::as_path));
    }
    SearchOutput { result, walked_roots }
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
