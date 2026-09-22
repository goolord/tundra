//! Browsing the library: opening files and folders, directory walks, search
//! and tag filters, favorites, and keeping the caches in step.

use super::{App, Modal, run_blocking};
use crate::library::cache::PersistedCaches;
use crate::library::search::SearchOutput;
use crate::library::search::{SearchRequest, execute_file_search};
use crate::library::walk_directory;
use crate::metadata::{
    FILE_SEARCH_MIN_QUERY_LEN, TagField, TagFields, TagParseError, file_search_active, index_paths, is_audio,
    parse_tag_filter, refresh_cached_metadata, tag_field_best_match,
};
use crate::ui::file_selector::{
    FILE_LIST_SCROLL_ID, FILE_SEARCH_INPUT_ID, FileButton, FilterFocus, TAG_SEARCH_INPUT_ID, list_buttons,
};
use crate::ui::message::{FilterMsg, Message};
use crate::ui::settings::{FILE_OUTSIDE_ALLOWED, UNSUPPORTED_AUDIO};
use futures::future::{AbortHandle, Abortable};
use iced::Task;
use iced::widget::Id;
use iced::widget::operation;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn focus(id: &'static str) -> Task<Message> {
    operation::focus(Id::new(id))
}

impl App {
    pub(super) fn select_file_row(&mut self, index: usize) -> Task<Message> {
        let (shift, control) = self.click_modifiers();
        self.file_selector.filter_focus = FilterFocus::None;
        self.file_list_focused = true;
        self.file_selector.select_row(index, shift, control);
        match self.file_selector.file_list.get(index) {
            Some(entry) if !shift && !control => self.open_path(&entry.file_path.clone()),
            _ => Task::none(),
        }
    }

    /// Opens a folder in the list, or plays an audio file.
    pub(super) fn open_path(&mut self, path: &Path) -> Task<Message> {
        let listings = self.dir_cache.snapshot();
        let path = crate::path_util::resolve_open_path(path, listings.values().flatten().map(PathBuf::as_path));
        let defocus = self.release_filter_focus();
        let open = if path.is_dir() {
            self.navigate_directory(path)
        } else if is_audio(&path) {
            if let Some(parent) = path.parent().filter(|dir| dir.is_dir())
                && !self.file_selector.favorites_only
            {
                self.file_selector.reload_directory(parent);
            }
            self.play_audio(&path)
        } else {
            Task::none()
        };
        Task::batch([defocus, open])
    }

    /// Opens the file the OS launched us with, once settings and audio are ready.
    pub(super) fn open_pending_launch(&mut self) -> Task<Message> {
        if self.modal == Modal::Settings || !self.player.audio_ready() {
            return Task::none();
        }
        match self.pending_launch_path.take() {
            Some(path) => self.open_path(&path),
            None => Task::none(),
        }
    }

    fn release_filter_focus(&mut self) -> Task<Message> {
        self.file_selector.filter_focus = FilterFocus::None;
        focus(FILE_LIST_SCROLL_ID)
    }

    /// Whether the current folder is inside an allowed root. Memoized per folder
    /// and root list, since resolving touches the filesystem and `view` asks every frame.
    pub(super) fn search_enabled(&self) -> bool {
        let dir = &self.file_selector.current_dir;
        let roots = self.allowed_directories.roots();
        let mut memo = self.search_enabled_memo.borrow_mut();
        if let Some((memo_dir, memo_roots, enabled)) = memo.as_ref()
            && memo_dir == dir
            && memo_roots == roots
        {
            return *enabled;
        }
        let enabled = self.allowed_directories.contains_path(dir);
        *memo = Some((dir.clone(), roots.to_vec(), enabled));
        enabled
    }

    pub(super) fn navigate_directory(&mut self, dir: PathBuf) -> Task<Message> {
        self.file_selector.filter_focus = FilterFocus::None;
        self.file_selector.reload_directory(&dir);
        if self.file_selector.favorites_only {
            self.reset_file_list();
            Task::none()
        } else if !self.search_enabled() {
            self.search_abort.abort();
            Task::none()
        } else if self.dir_cache.contains_key(&dir) {
            self.start_file_search()
        } else {
            self.walk_task(dir)
        }
    }

    fn play_audio(&mut self, path: &Path) -> Task<Message> {
        if let Err(err) = self.player.play_file(path) {
            self.show_error(err);
            self.file_selector.clear_selection();
            return Task::none();
        }
        // Playing a file only re-runs the search when its tags turned out to be
        // stale; otherwise the list (and selection) stay put.
        let refresh = if self.merge_path_metadata(path) {
            self.refresh_search_if_active()
        } else {
            Task::none()
        };
        self.file_selector.sync_selection_for_path(path);
        refresh
    }

    /// Why `path` cannot be tagged or starred, if it cannot.
    pub(super) fn allowed_audio_error(&self, path: &Path) -> Option<&'static str> {
        if !is_audio(path) {
            Some(UNSUPPORTED_AUDIO)
        } else if !self.allowed_directories.contains_path(path) {
            Some(FILE_OUTSIDE_ALLOWED)
        } else {
            None
        }
    }

    /// Re-read `path`'s tags into the index. True when they changed.
    pub(super) fn merge_path_metadata(&mut self, path: &Path) -> bool {
        let Some(cached) = refresh_cached_metadata(path) else {
            return false;
        };
        let changed = self.metadata_cache.cached_fields(path).as_ref() != Some(&cached.fields);
        self.metadata_cache.merge_path(path, cached);
        changed
    }

    /// Tags to show for the playing file, from the index.
    pub(super) fn current_file_tags(&self) -> Vec<(TagField, String)> {
        self.player
            .current_file
            .as_deref()
            .and_then(|path| self.metadata_cache.cached_fields(path))
            .map(|fields| toolbar_tags(&fields))
            .unwrap_or_default()
    }

    pub(super) fn refresh_search_if_active(&mut self) -> Task<Message> {
        if self.file_selector.search_active() {
            self.start_file_search()
        } else {
            Task::none()
        }
    }

    fn reset_file_list(&mut self) {
        let (list, error) = if self.file_selector.favorites_only {
            (self.favorite_file_buttons(), None)
        } else {
            list_buttons(&self.file_selector.current_dir)
        };
        self.file_selector.set_file_list(list, error);
    }

    /// Favorites that still exist inside the allowed folders, alphabetically.
    fn favorite_file_buttons(&self) -> Vec<FileButton> {
        let mut buttons: Vec<FileButton> = self
            .favorites
            .paths()
            .iter()
            .map(|stored| crate::path_util::canonical_path(stored).unwrap_or_else(|_| stored.clone()))
            .filter(|path| path.is_file() && self.allowed_audio_error(path).is_none())
            .map(|path| {
                let base = path.parent().unwrap_or(&path).to_path_buf();
                FileButton::new(path, &base, false)
            })
            .collect();
        buttons.sort_by_cached_key(|button| button.label.to_lowercase());
        buttons
    }

    pub(super) fn toggle_favorite(&mut self, path: &Path) -> Task<Message> {
        if self.allowed_audio_error(path).is_some() {
            return Task::none();
        }
        self.favorites.toggle(path);
        self.favorites.persist();
        self.start_file_search()
    }

    /// Walk `dir` on a background thread unless a walk of it is already running.
    pub(super) fn walk_task(&mut self, dir: PathBuf) -> Task<Message> {
        let key = crate::path_util::cache_key(&dir);
        if !self.walks_in_progress.insert(key.clone()) {
            return Task::none();
        }
        Task::perform(
            run_blocking(move || {
                let children = walk_directory(&dir);
                (dir, children)
            }),
            move |walked| walked.map_or_else(|()| Message::WalkFailed(key.clone()), Message::InsertDircache),
        )
    }

    /// Starts walks for allowed roots with no cached listing.
    pub(super) fn warm_allowed_caches(&mut self) -> Task<Message> {
        let missing: Vec<PathBuf> = self
            .allowed_directories
            .roots()
            .iter()
            .filter(|root| !self.dir_cache.contains_key(root))
            .cloned()
            .collect();
        Task::batch(missing.into_iter().map(|root| self.walk_task(root)))
    }

    /// Drop cache entries outside the allowed folders. Caches only; favorites
    /// are user data and are filtered at display time instead.
    pub(super) fn prune_caches(&mut self) {
        if self.allowed_directories.is_empty() {
            return;
        }
        let allowed = &self.allowed_directories;
        if self.dir_cache.retain(|path| allowed.contains_cached_path(path)) {
            self.dir_cache.persist();
        }
        if self.metadata_cache.retain(|path| allowed.contains_cached_path(path)) {
            self.metadata_cache.persist();
        }
    }

    pub(super) fn invalidate_caches(&mut self) -> Task<Message> {
        self.dir_cache.clear();
        self.metadata_cache.clear();
        crate::auto_tag::clear_classify_cache();
        Task::batch([self.warm_allowed_caches(), self.refresh_search_if_active()])
    }

    pub(super) fn startup_caches_ready(&mut self, caches: PersistedCaches) -> Task<Message> {
        self.dir_cache.finish_loading(caches.dirs);
        self.metadata_cache.finish_loading(caches.metadata);
        self.caches_ready = true;
        Task::batch([
            self.warm_allowed_caches(),
            self.refresh_search_if_active(),
            self.open_pending_launch(),
        ])
    }

    pub(super) fn insert_walked_directory(&mut self, dir: PathBuf, children: Vec<PathBuf>) -> Task<Message> {
        self.walks_in_progress.remove(&crate::path_util::cache_key(&dir));
        if !self.allowed_directories.contains_path(&dir) {
            return Task::none();
        }
        self.dir_cache.insert(dir, children.clone());
        let metadata = self.metadata_cache.snapshot();
        Task::perform(
            async move { index_paths(&children, metadata) },
            Message::MetadataIndexed,
        )
    }

    /// Runs the current file query and tag filters, replacing any search in flight.
    pub(super) fn start_file_search(&mut self) -> Task<Message> {
        self.search_abort.abort();
        self.search_generation = self.search_generation.wrapping_add(1);
        let selector = &self.file_selector;

        if !self.search_enabled() || !file_search_active(&selector.search_value, &selector.tag_filters) {
            self.reset_file_list();
            return Task::none();
        }
        let favorites = selector.favorites_only.then(|| {
            self.favorites
                .paths()
                .iter()
                .cloned()
                .collect::<std::collections::HashSet<_>>()
        });
        if favorites.as_ref().is_some_and(|favorites| favorites.is_empty()) {
            self.file_selector.set_file_list(Vec::new(), None);
            return Task::none();
        }
        // `StartupCachesReady` re-runs the active search, so bailing here only defers it.
        // Running early would race `warm_allowed_caches` into walking the same roots twice.
        if !self.caches_ready {
            return Task::none();
        }

        let tag_only = selector.tag_only_search();
        // Wait for typing to pause; a one- or two-letter query matches so much that it waits longer.
        let debounce_ms = if !tag_only && selector.search_value.len() <= FILE_SEARCH_MIN_QUERY_LEN {
            450
        } else {
            200
        };
        let request = SearchRequest {
            debounce: Duration::from_millis(debounce_ms),
            allowed_roots: self.allowed_directories.roots().to_vec(),
            dir_cache: self.dir_cache.share(),
            metadata_cache: self.metadata_cache.share(),
            file_query: selector.search_value.clone(),
            tag_filters: selector.tag_filters.clone(),
            case_sensitive: selector.search_case_sensitive,
            show_directories: selector.search_show_directories,
            tag_only,
            favorites,
        };
        let generation = self.search_generation;
        let (abort, registration) = AbortHandle::new_pair();
        self.search_abort = abort;
        Task::perform(
            Abortable::new(execute_file_search(request), registration),
            move |result| FilterMsg::SearchCompleted(generation, result).into(),
        )
    }

    pub(super) fn update_filter(&mut self, message: FilterMsg) -> Task<Message> {
        match message {
            FilterMsg::Search(value) => {
                self.focus_filter(FilterFocus::FileSearch);
                self.file_selector.search_value = value;
                Task::batch([self.start_file_search(), focus(FILE_SEARCH_INPUT_ID)])
            }
            FilterMsg::SearchFocused(true) => {
                self.focus_filter(FilterFocus::FileSearch);
                focus(FILE_SEARCH_INPUT_ID)
            }
            FilterMsg::SearchFocused(false) => Task::none(),
            FilterMsg::TagSearchInput(input) => {
                self.focus_filter(FilterFocus::TagSearch);
                self.file_selector.tag_search_error = None;
                self.file_selector.tag_search_value = input;
                focus(TAG_SEARCH_INPUT_ID)
            }
            FilterMsg::TagSearchSubmit => self.submit_tag_search(),
            FilterMsg::TagSearchFocused(true) => {
                self.focus_filter(FilterFocus::TagSearch);
                focus(TAG_SEARCH_INPUT_ID)
            }
            // Only Tab from the tag search sends this; the file list keeps focus.
            FilterMsg::TagSearchFocused(false) => self.release_filter_focus(),
            FilterMsg::TagFilterRemove(field) => {
                self.file_selector.tag_filters.retain(|filter| filter.field != field);
                self.start_file_search()
            }
            FilterMsg::TagSuggestionSelect(field) => self.select_tag_field(field),
            FilterMsg::ToggleCaseSensitive => {
                self.file_selector.search_case_sensitive = !self.file_selector.search_case_sensitive;
                self.start_file_search()
            }
            FilterMsg::ToggleShowDirectories => {
                self.file_selector.search_show_directories = !self.file_selector.search_show_directories;
                self.start_file_search()
            }
            FilterMsg::ToggleFavoritesOnly => {
                self.file_selector.favorites_only = !self.file_selector.favorites_only;
                self.start_file_search()
            }
            FilterMsg::SearchCompleted(generation, result) => {
                if generation == self.search_generation
                    && self.search_enabled()
                    && self.file_selector.search_active()
                    && let Ok(result) = result
                {
                    self.show_search_result(result);
                }
                Task::none()
            }
        }
    }

    fn focus_filter(&mut self, filter: FilterFocus) {
        self.file_selector.filter_focus = filter;
        self.file_list_focused = false;
    }

    fn show_search_result(&mut self, SearchOutput { result, walked_roots }: SearchOutput) {
        let mut walked_any = false;
        for (root, children) in walked_roots {
            if self.allowed_directories.contains_path(&root) {
                self.dir_cache.insert(root, children);
                walked_any = true;
            }
        }
        if walked_any {
            self.dir_cache.persist();
        }
        self.metadata_cache.merge(result.new_metadata);
        // Search paths are pre-filtered to directories and audio files, so
        // dir-ness follows from the extension; avoids one stat per result.
        let current_dir = &self.file_selector.current_dir;
        let rows = result
            .paths
            .into_iter()
            .map(|path| {
                let is_dir = !is_audio(&path);
                FileButton::new(path, current_dir, is_dir)
            })
            .collect();
        self.file_selector.set_file_list(rows, None);
    }

    /// Enter in the tag search: complete a bare field name, or add `field:value` as a filter.
    fn submit_tag_search(&mut self) -> Task<Message> {
        self.focus_filter(FilterFocus::TagSearch);
        let input = self.file_selector.tag_search_value.clone();
        if input.trim().is_empty() {
            return if self.file_selector.tag_filters.is_empty() {
                focus(TAG_SEARCH_INPUT_ID)
            } else {
                self.start_file_search()
            };
        }
        let parsed = match (input.contains(':'), tag_field_best_match(&input)) {
            (true, _) => parse_tag_filter(&input),
            (false, Some(_)) => return self.autocomplete_tag_field(),
            (false, None) => Err(TagParseError::UnknownField),
        };
        match parsed {
            Ok(filter) => {
                self.file_selector.tag_search_error = None;
                self.file_selector.add_tag_filter(filter);
                self.file_selector.tag_search_value.clear();
                Task::batch([self.start_file_search(), focus(TAG_SEARCH_INPUT_ID)])
            }
            Err(err) => {
                self.file_selector.tag_search_error = Some(err.to_string());
                focus(TAG_SEARCH_INPUT_ID)
            }
        }
    }

    pub(super) fn autocomplete_tag_field(&mut self) -> Task<Message> {
        match tag_field_best_match(&self.file_selector.tag_search_value) {
            Some(field) => self.select_tag_field(field),
            None => Task::none(),
        }
    }

    /// Put `field:` in the tag search box, ready for a value.
    fn select_tag_field(&mut self, field: TagField) -> Task<Message> {
        self.file_selector.filter_focus = FilterFocus::TagSearch;
        self.file_selector.tag_search_error = None;
        self.file_selector.tag_search_value = format!("{}:", field.as_str());
        focus(TAG_SEARCH_INPUT_ID)
    }
}

/// Tags worth showing in the waveform toolbar, in display order.
fn toolbar_tags(fields: &TagFields) -> Vec<(TagField, String)> {
    [TagField::Instrument, TagField::Bpm, TagField::Key, TagField::Genre]
        .into_iter()
        .map(|field| (field, fields.field_value(field)))
        .filter(|(_, value)| !value.is_empty())
        .map(|(field, value)| (field, value.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_shows_instrument_bpm_key_and_genre_when_set() {
        let fields = TagFields {
            explicit_instrument: "Snare".into(),
            instrument: "Drums".into(),
            bpm: "120".into(),
            key: "Am".into(),
            title: "ignored".into(),
            ..TagFields::default()
        };
        assert_eq!(
            toolbar_tags(&fields),
            [
                (TagField::Instrument, "Snare".to_string()),
                (TagField::Bpm, "120".to_string()),
                (TagField::Key, "Am".to_string()),
            ]
        );
    }
}
