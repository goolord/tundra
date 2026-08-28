use super::*;
use crate::auto_tag;
use crate::bulk_auto_tag;
use crate::drag_out::NativeDrag;
use crate::metadata::{
    control_bar_tags, file_search_debounce_ms, index_paths, instrument_tag, parse_tag_filter,
    refresh_cached_metadata, tag_field_best_match,
    tag_parse_message, write_auto_tags, write_manual_tags,
    AUTO_TAG_ALREADY_COMPLETE, AUTO_TAG_INSTRUMENT_PRESENT,
    auto_tag_field_status,
    PersistedCaches, TagParseError, file_search_active,
    TAG_SEARCH_DEBOUNCE_MS,
};
use super::auto_tag::{auto_tag_view, AutoTagState};
use super::bulk_auto_tag::{bulk_auto_tag_view, BulkAutoTagPhase, BulkAutoTagState};
use super::settings::{
    self, AddDirectoryResult, AllowedDirectories, FavoritesStore, FILE_OUTSIDE_ALLOWED,
    FOLDER_OUTSIDE_ALLOWED, SELECT_AUDIO_FIRST, UNSUPPORTED_AUDIO,
};
use super::tag_editor::{tag_editor_view, TagEditorState};
use futures::channel::oneshot;
use futures::future::{AbortHandle, Abortable};
use futures::*;

use iced::widget::{button, center, column, container, mouse_area, opaque, operation, row, stack, text, Space};
use iced::widget::Id;
use iced::widget::operation::AbsoluteOffset;
use iced::{Border, Color, Element, Event, Length, Point, Shadow, Subscription, Task, Theme};
use iced::event;
use iced::futures::stream;
use iced::keyboard::{Key, Modifiers};
use iced::mouse;
use iced::window;
use iced_aw::ICED_AW_FONT_BYTES;
use futures::StreamExt;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

mod cache;
mod helpers;
mod prefs;

pub use cache::{DirCache, MetadataCache};
pub use prefs::{window_level, set_window_level};
use cache::load_startup_caches;
use helpers::{
    execute_file_search, pick_audio_file, pick_folder,
    tag_search_can_autocomplete, transport_shortcut_allowed, walk_directory, FileDragKind,
    FileDragPending, FileListScrollbarDrag, SidebarResize, TitleBarInteraction,
};
use prefs::{
    load_always_on_top, load_looping, load_sidebar_width, load_volume, persist_always_on_top,
    persist_looping, persist_sidebar_width, persist_volume, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH,
    SIDEBAR_RESIZER_HIT_WIDTH, SIDEBAR_RESIZER_LINE_WIDTH,
    TITLE_DRAG_THRESHOLD, WINDOW_RESIZE_BORDER,
};

pub struct App {
    pub file_selector: FileSelector,
    pub menu: MainMenu,
    pub player: Player,
    pub search_thread: AbortHandle,
    search_generation: u64,
    pub dir_cache: DirCache,
    metadata_cache: MetadataCache,
    player_msgs: Option<Arc<futures::channel::mpsc::UnboundedReceiver<super::PlayerMsg>>>,
    player_events_started: bool,
    drag_over: bool,
    dialog: Option<Dialog>,
    /// File or folder the OS asked us to open on startup.
    pending_launch_path: Option<PathBuf>,
    allowed_directories: AllowedDirectories,
    favorites: FavoritesStore,
    settings_open: bool,
    settings_first_run: bool,
    settings_error: Option<String>,
    always_on_top: bool,
    auto_tag_open: bool,
    auto_tag: AutoTagState,
    tag_editor_open: bool,
    tag_editor: TagEditorState,
    bulk_auto_tag: BulkAutoTagState,
    bulk_scan_progress: Option<Arc<bulk_auto_tag::BulkScanProgress>>,
    bulk_scan_cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    bulk_scan_generation: u64,
    bulk_scan_active: Option<u64>,
    bulk_apply_generation: u64,
    bulk_apply_active: Option<u64>,
    waveform_hovered: bool,
    waveform_scrubbing: bool,
    last_scrub_progress: f64,
    controls_hovered: bool,
    file_list_hovered: bool,
    file_list_focused: bool,
    search_focused: bool,
    tag_search_focused: bool,
    sidebar_width: f32,
    sidebar_resize: Option<SidebarResize>,
    title_bar: TitleBarInteraction,
    window_maximized: bool,
    file_list_scrollbar_drag: Option<FileListScrollbarDrag>,
    file_drag: Option<FileDragPending>,
    native_drag: NativeDrag,
    drag_ready: bool,
    caches_ready: bool,
    last_cursor: Point,
    modifiers: Modifiers,
}


pub fn app() {
    let always_on_top = load_always_on_top();
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .antialiasing(true)
        .font(ICED_AW_FONT_BYTES)
        .subscription(App::subscription)
        .level(window_level(always_on_top))
        .decorations(false)
        .resizable(true)
        .run()
        .unwrap()
}


async fn run_blocking<T>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, ()>
where
    T: Send + 'static,
{
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.await.map_err(|_| ())
}

async fn load_startup_caches_async(allowed: AllowedDirectories) -> PersistedCaches {
    run_blocking(move || load_startup_caches(allowed))
        .await
        .unwrap_or_else(|_| panic!("blocking task dropped"))
}

fn clipboard_name(path: &Path) -> Task<Message> {
    crate::path_util::file_name_lossy(path)
        .map(iced::clipboard::write)
        .unwrap_or_else(Task::none)
}

fn clipboard_path(path: &Path) -> Task<Message> {
    iced::clipboard::write(path.to_string_lossy().into_owned())
}

fn reveal_path(path: &Path) -> Task<Message> {
    reveal_in_file_manager(path);
    Task::none()
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let mut app = Self::default();
        app.pending_launch_path =
            crate::launch::primary_open_target(&crate::launch::paths_from_args());
        let allowed = app.allowed_directories.clone();
        let volume = app.player.controls.volume;
        let is_playing = app.player.controls.is_playing.clone();
        let looping = app.player.controls.looping.clone();
        let launch = app
            .maybe_open_pending_launch()
            .unwrap_or_else(Task::none);
        (
            app,
            Task::batch([
                Task::perform(
                    load_startup_caches_async(allowed),
                    Message::StartupCachesReady,
                ),
                Task::perform(
                    async move {
                        run_blocking(move || PlayerWorker::spawn(is_playing, looping, volume))
                            .await
                            .unwrap_or_else(|_| panic!("blocking task dropped"))
                    },
                    |(worker, receiver)| {
                        Message::PlayerWorkerReady(worker, Arc::new(receiver))
                    },
                ),
                launch,
                window::latest().then(|id| match id {
                    Some(id) => window::is_maximized(id).map(Message::WindowMaximizedChanged),
                    None => Task::none(),
                }),
            ]),
        )
    }
}

impl Default for App {
    fn default() -> App {
        let allowed_directories = AllowedDirectories::load();
        let settings_first_run = allowed_directories.is_empty();
        let settings_open = settings_first_run;
        let current_dir = allowed_directories
            .startup_directory()
            .unwrap_or_else(startup_directory);
        let file_selector = FileSelector::new(&current_dir);
        let menu = MainMenu::new();
        let player = Player::new(load_volume(), load_looping());
        let search_thread = AbortHandle::new_pair().0;
        let dir_cache = DirCache::new();
        let metadata_cache = MetadataCache::new();
        App {
            file_selector,
            menu,
            player,
            search_thread,
            search_generation: 0,
            dir_cache,
            metadata_cache,
            player_msgs: None,
            player_events_started: false,
            drag_over: false,
            dialog: None,
            pending_launch_path: None,
            allowed_directories,
            favorites: FavoritesStore::load(),
            settings_open,
            settings_first_run,
            settings_error: None,
            always_on_top: load_always_on_top(),
            auto_tag_open: false,
            auto_tag: AutoTagState::default(),
            tag_editor_open: false,
            tag_editor: TagEditorState::default(),
            bulk_auto_tag: BulkAutoTagState::default(),
            bulk_scan_progress: None,
            bulk_scan_cancel: None,
            bulk_scan_generation: 0,
            bulk_scan_active: None,
            bulk_apply_generation: 0,
            bulk_apply_active: None,
            waveform_hovered: false,
            controls_hovered: false,
            file_list_hovered: false,
            file_list_focused: false,
            search_focused: false,
            tag_search_focused: false,
            sidebar_width: load_sidebar_width(),
            sidebar_resize: None,
            title_bar: TitleBarInteraction {
                drag_armed: false,
                press_origin: None,
            },
            window_maximized: false,
            waveform_scrubbing: false,
            last_scrub_progress: 0.0,
            file_list_scrollbar_drag: None,
            file_drag: None,
            native_drag: NativeDrag::new(),
            drag_ready: cfg!(any(windows, target_os = "macos")),
            caches_ready: false,
            last_cursor: Point::ORIGIN,
            modifiers: Modifiers::default(),
        }
    }
}

#[cfg(test)]
mod always_on_top_tests {
    use super::*;

    #[test]
    fn window_level_follows_flag() {
        assert_eq!(window_level(true), window::Level::AlwaysOnTop);
        assert_eq!(window_level(false), window::Level::Normal);
    }
}


impl App {
    pub fn subscription(state: &App) -> Subscription<Message> {
        let file_events = event::listen_with(|event, _status, _window| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::FileHovered(path)) => Some(Message::FileHovered(path)),
            Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FilesHoverLeft),
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::ModifiersChanged(modifiers))
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                Some(Message::CursorMoved(position))
            }
            _ => None,
        });

        let playing = state.player.controls.is_playing.load(Ordering::Relaxed)
            && state.player.waveform.is_some();
        let playback_tick = if playing {
            window::frames().map(|_| Message::PlaybackTick)
        } else {
            Subscription::none()
        };

        let transport_keys = if state.player.waveform.is_some()
            && state.dialog.is_none()
            && !state.settings_open
            && !state.auto_tag_open
            && !state.tag_editor_open
            && !state.bulk_auto_tag.is_open()
            && !state.search_focused
            && (!state.tag_search_focused || state.file_list_focused)
        {
            event::listen_with(|event, status, _window| {
                if status != event::Status::Ignored {
                    return None;
                }
                match event {
                    Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key,
                        modifiers,
                        repeat,
                        ..
                    }) => {
                        if repeat || !transport_shortcut_allowed(modifiers) {
                            return None;
                        }
                        if key == Key::Named(iced::keyboard::key::Named::Space) {
                            Some(Message::TogglePlaying)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
        } else {
            Subscription::none()
        };

        let waveform_keys = if state.waveform_hovered && state.player.waveform.is_some() {
            event::listen_with(|event, status, _window| {
                if status != event::Status::Ignored {
                    return None;
                }
                match event {
                    Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key,
                        modifiers,
                        repeat,
                        ..
                    }) => {
                        if repeat || modifiers.control() || modifiers.logo() || modifiers.alt() {
                            return None;
                        }
                        if key == Key::Named(iced::keyboard::key::Named::Space) {
                            return None;
                        }
                        Some(Message::WaveformKey(key))
                    }
                    _ => None,
                }
            })
        } else {
            Subscription::none()
        };

        let sidebar_resize = if state.sidebar_resize.is_some() {
            event::listen_with(|event, _status, _window| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::SidebarResizeMove(position.x))
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::SidebarResizeEnd)
                }
                _ => None,
            })
        } else {
            Subscription::none()
        };

        let file_list_scrollbar = if state.file_list_scrollbar_drag.is_some() {
            event::listen_with(|event, _status, _window| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::FileListScrollbarDrag(position))
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::FileListScrollbarRelease)
                }
                _ => None,
            })
        } else {
            Subscription::none()
        };

        let file_drag = if state.sidebar_resize.is_none()
            && state.file_list_scrollbar_drag.is_none()
            && (state.file_drag.is_some() || state.native_drag.is_active())
        {
            event::listen_with(|event, _status, _window| match event {
                Event::Mouse(mouse::Event::CursorMoved { position }) => {
                    Some(Message::FileDragMove(position))
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::FileDragRelease)
                }
                _ => None,
            })
        } else {
            Subscription::none()
        };

        let file_drag_tick = if state.native_drag.is_active() {
            Subscription::run_with((), |_| {
                stream::unfold((), |()| async {
                    async_io::Timer::after(Duration::from_millis(16)).await;
                    Some((Message::FileDragTick, ()))
                })
            })
        } else {
            Subscription::none()
        };

        let waveform_spring = if state.player.waveform.as_ref().is_some_and(|wf| {
            wf.view_state().overscroll_active() && !wf.pan_active()
        }) {
            Subscription::run_with((), |_| {
                stream::unfold((), |()| async {
                    async_io::Timer::after(Duration::from_millis(16)).await;
                    Some((Message::WaveformSpringTick, ()))
                })
            })
        } else {
            Subscription::none()
        };

        let tag_autocomplete_keys = if state.tag_search_focused
            && tag_search_can_autocomplete(&state.file_selector.tag_search_value)
        {
            event::listen_with(|event, status, _window| {
                if status != event::Status::Ignored {
                    return None;
                }
                match event {
                    Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key,
                        modifiers,
                        repeat,
                        ..
                    }) => {
                        if repeat
                            || modifiers.shift()
                            || modifiers.control()
                            || modifiers.logo()
                            || modifiers.alt()
                        {
                            return None;
                        }
                        if key == Key::Named(iced::keyboard::key::Named::Tab) {
                            Some(Message::TagSearchAutocomplete)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            })
        } else {
            Subscription::none()
        };

        let bulk_scan_tick = if state.bulk_scan_progress.is_some() {
            Subscription::run_with((), |_| {
                stream::unfold((), |()| async {
                    async_io::Timer::after(Duration::from_millis(100)).await;
                    Some((Message::BulkAutoTagProgressTick, ()))
                })
            })
        } else {
            Subscription::none()
        };

        let window_resize = window::resize_events().map(|(_id, _size)| Message::SyncWindowMaximized);

        let waveform_scrub = if state.waveform_scrubbing {
            event::listen_with(|event, _status, _window| match event {
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Message::WaveformScrubRelease)
                }
                _ => None,
            })
        } else {
            Subscription::none()
        };

        Subscription::batch([
            file_events,
            playback_tick,
            waveform_keys,
            transport_keys,
            tag_autocomplete_keys,
            waveform_spring,
            sidebar_resize,
            file_list_scrollbar,
            file_drag,
            file_drag_tick,
            bulk_scan_tick,
            window_resize,
            waveform_scrub,
        ])
    }

    fn autocomplete_tag_field(&mut self) -> Task<Message> {
        let input = self.file_selector.tag_search_value.clone();
        if input.contains(':') || input.trim().is_empty() {
            return Task::none();
        }
        if let Some(field) = tag_field_best_match(&input) {
            self.file_selector.tag_search_error = None;
            self.file_selector.tag_search_value = format!("{}:", field.as_str());
            operation::focus(Id::new(TAG_SEARCH_INPUT_ID))
        } else {
            Task::none()
        }
    }

    fn release_filter_focus(&mut self) -> Task<Message> {
        self.search_focused = false;
        self.tag_search_focused = false;
        operation::focus(Id::new(FILE_LIST_SCROLL_ID))
    }

    fn open_path(&mut self, path: &Path) -> Task<Message> {
        let path = if path.exists() {
            crate::path_util::canonical_path(path).unwrap_or_else(|_| path.to_path_buf())
        } else {
            crate::path_util::normalize_path(path.to_path_buf())
        };
        let defocus = self.release_filter_focus();
        if path.is_dir() {
            return Task::batch([defocus, self.navigate_directory(path)]);
        }
        if is_audio(&path) {
            return Task::batch([defocus, self.open_audio_at(path)]);
        }
        defocus
    }

    fn open_audio_at(&mut self, path: PathBuf) -> Task<Message> {
        if let Some(parent) = path.parent().filter(|dir| dir.is_dir()) {
            self.search_focused = false;
            self.tag_search_focused = false;
            if !self.file_selector.favorites_only {
                self.file_selector.reload_directory(parent);
                self.search_thread.abort();
            }
        }
        self.play_audio(&path)
    }

    fn maybe_open_pending_launch(&mut self) -> Option<Task<Message>> {
        if self.settings_open || self.pending_launch_path.is_none() {
            return None;
        }
        if !self.player.audio_ready() {
            return None;
        }
        let path = self.pending_launch_path.take().expect("checked");
        Some(self.open_path(&path))
    }

    fn search_enabled(&self) -> bool {
        self.allowed_directories
            .contains_path(&self.file_selector.current_dir)
    }

    fn navigate_directory(&mut self, dir: PathBuf) -> Task<Message> {
        self.search_focused = false;
        self.tag_search_focused = false;
        self.file_selector.reload_directory(&dir);
        if self.file_selector.favorites_only {
            self.reset_file_list();
            return Task::none();
        }
        if !self.search_enabled() {
            self.search_thread.abort();
            return Task::none();
        }
        if !self.dir_cache.contains_key(&dir) {
            let walker = future::lazy(move |_| {
                let children = walk_directory(&dir);
                (dir, children)
            });
            Task::perform(walker, Message::InsertDircache)
        } else {
            self.start_file_search()
        }
    }

    fn play_audio(&mut self, file_path: &Path) -> Task<Message> {
        match self.player.play_file(file_path) {
            Ok(()) => {
                self.merge_path_metadata(file_path);
                self.file_selector.sync_selection_for_path(file_path);
                Task::batch([self.refresh_search_if_active(), self.ensure_player_events()])
            }
            Err(err) => {
                self.show_error(err);
                self.file_selector.clear_selection();
                Task::none()
            }
        }
    }

    fn show_error(&mut self, message: String) {
        self.player.reset_on_error();
        self.dialog = Some(Dialog::error(message));
    }

    fn show_notice(&mut self, message: impl Into<String>) {
        self.dialog = Some(Dialog::notice(message.into()));
    }

    fn ensure_player_events(&mut self) -> Task<Message> {
        if !self.player_events_started {
            self.player_events_started = true;
            if let Some(recv) = self.player_msgs.take() {
                match Arc::try_unwrap(recv) {
                    Ok(recv) => {
                        return Task::perform(recv.into_future(), |x| {
                            Message::PlayerMsg((x.0, Arc::new(x.1)))
                        });
                    }
                    Err(recv) => {
                        eprintln!("ensure_player_events: Arc::try_unwrap failed");
                        self.player_msgs = Some(recv);
                    }
                }
            }
        }
        Task::none()
    }

    fn ensure_drag() -> Task<Message> {
        window::latest().then(|id| match id {
            Some(id) => window::run(id, |window| crate::drag_out::x11_window_id(window))
                .map(Message::DragWindowId),
            None => Task::none(),
        })
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn begin_platform_drag(path: PathBuf) -> Task<Message> {
        window::latest().then(move |id| match id {
            Some(id) => {
                let drag_path = path.clone();
                window::run(id, move |window| {
                    crate::drag_out::start_blocking(window, drag_path)
                })
                .map(Message::FileDragCompleted)
            }
            None => Task::none(),
        })
    }

    fn start_file_drag(&mut self, path: PathBuf) -> Task<Message> {
        let canonical = match crate::path_util::canonical_path(&path) {
            Ok(path) => path,
            Err(err) => {
                self.show_notice(drag_out_notice(format!(
                    "Cannot drag {}: {err}.",
                    path.display()
                )));
                self.file_drag = None;
                return Task::none();
            }
        };

        #[cfg(any(windows, target_os = "macos"))]
        {
            self.file_drag = None;
            return Self::begin_platform_drag(canonical);
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            match self.native_drag.start(canonical) {
                Ok(()) => Task::none(),
                Err(err) => {
                    self.show_notice(drag_out_notice(format!("Drag failed: {err}.")));
                    self.file_drag = None;
                    Task::none()
                }
            }
        }
    }

    pub fn title(&self) -> String {
        let active_file = self
            .player
            .current_file
            .as_ref()
            .and_then(|path| crate::path_util::file_name_lossy(path));
        window_title(active_file.as_deref())
    }

    fn prepare_feature_modal(&mut self) {
        self.dialog = None;
        self.cancel_bulk_scan();
    }

    fn allowed_audio_error(&self, path: &Path) -> Option<&'static str> {
        if !is_audio(path) {
            Some(UNSUPPORTED_AUDIO)
        } else if !self.allowed_directories.contains_path(path) {
            Some(FILE_OUTSIDE_ALLOWED)
        } else {
            None
        }
    }

    fn with_current_file(&self, f: impl FnOnce(&Path) -> Task<Message>) -> Task<Message> {
        self.player
            .current_file
            .as_deref()
            .map(f)
            .unwrap_or_else(Task::none)
    }

    fn merge_path_metadata(&mut self, path: &Path) {
        if let Some(cached) = refresh_cached_metadata(path) {
            self.metadata_cache.merge_path(path, cached);
        }
    }

    fn refresh_search_if_active(&mut self) -> Task<Message> {
        if self.file_selector.search_active() {
            self.start_file_search()
        } else {
            Task::none()
        }
    }

    fn open_tag_editor_for(&mut self, path: PathBuf) -> Task<Message> {
        self.prepare_feature_modal();
        self.auto_tag_open = false;
        self.tag_editor_open = true;
        if let Some(err) = self.allowed_audio_error(&path) {
            self.tag_editor = TagEditorState::default();
            self.tag_editor.set_error(err);
            return Task::none();
        }
        let fields = self.metadata_cache.tag_fields_for(&path);
        self.tag_editor.reset_for_path(path, fields);
        Task::none()
    }

    fn reset_file_list(&mut self) {
        if self.file_selector.favorites_only {
            self.file_selector.file_list = self.favorite_file_buttons();
            self.file_selector.list_error = None;
            return;
        }
        let (file_list, list_error) = FileList::list_buttons(&self.file_selector.current_dir);
        self.file_selector.file_list = file_list;
        self.file_selector.list_error = list_error;
    }

    fn favorite_file_buttons(&self) -> Vec<FileButton> {
        let mut buttons: Vec<FileButton> = self
            .favorites
            .paths()
            .iter()
            .filter(|path| path.is_file() && self.allowed_audio_error(path).is_none())
            .map(|path| {
                let base = path.parent().unwrap_or(path);
                FileButton::with_kind(path.clone(), base, false)
            })
            .collect();
        buttons.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
        buttons
    }

    fn favorite_key_set(&self) -> HashSet<PathBuf> {
        self.favorites
            .paths()
            .iter()
            .map(|path| crate::path_util::cache_key(path.clone()))
            .collect()
    }

    fn refresh_after_favorites_change(&mut self) -> Task<Message> {
        if self.file_selector.search_active() {
            self.start_file_search()
        } else {
            self.reset_file_list();
            Task::none()
        }
    }

    fn prune_caches(&mut self) {
        let allowed = self.allowed_directories.clone();
        if self
            .dir_cache
            .retain(|path| allowed.contains_path(path))
        {
            self.dir_cache.persist();
        }
        if self
            .metadata_cache
            .retain(|path| allowed.contains_path(path))
        {
            self.metadata_cache.persist();
        }
        let favorites_changed = {
            let before = self.favorites.paths().len();
            self.favorites
                .retain(|path| allowed.contains_path(path));
            before != self.favorites.paths().len()
        };
        if favorites_changed {
            self.favorites.persist();
        }
    }

    fn warm_allowed_caches(&self) -> Task<Message> {
        let tasks: Vec<Task<Message>> = self
            .allowed_directories
            .roots()
            .iter()
            .filter(|root| !self.dir_cache.contains_key(*root))
            .map(|root| {
                let dir = root.clone();
                let walker = future::lazy(move |_| {
                    let children = walk_directory(&dir);
                    (dir, children)
                });
                Task::perform(walker, Message::InsertDircache)
            })
            .collect();
        Task::batch(tasks)
    }

    fn start_file_search(&mut self) -> Task<Message> {
        self.search_thread.abort();
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let file_query = self.file_selector.search_value.clone();
        let tag_filters = self.file_selector.tag_filters.clone();
        let case_sensitive = self.file_selector.search_case_sensitive;
        let show_directories = self.file_selector.search_show_directories;
        let tag_only = self.file_selector.tag_only_search();
        let favorites_only = self.file_selector.favorites_only;
        let favorite_keys = self.favorite_key_set();

        if !self.search_enabled() || !file_search_active(&file_query, &tag_filters) {
            self.reset_file_list();
            return Task::none();
        }

        if favorites_only && favorite_keys.is_empty() {
            self.file_selector.file_list = Vec::new();
            self.file_selector.list_error = None;
            return Task::none();
        }

        if !self.caches_ready {
            return Task::none();
        }

        let debounce_ms = if tag_only {
            TAG_SEARCH_DEBOUNCE_MS
        } else {
            file_search_debounce_ms(file_query.len())
        };
        let allowed_roots = self.allowed_directories.roots().to_vec();
        let dir_cache = self.dir_cache.share();
        let metadata_cache = self.metadata_cache.share();
        let (abort_handle, abort_reg) = AbortHandle::new_pair();
        self.search_thread = abort_handle;

        Task::perform(
            Abortable::new(
                async move {
                    let result = execute_file_search(
                        debounce_ms,
                        allowed_roots,
                        dir_cache,
                        metadata_cache,
                        file_query,
                        tag_filters,
                        case_sensitive,
                        show_directories,
                        tag_only,
                        favorites_only,
                        favorite_keys,
                    )
                    .await;
                    (generation, result)
                },
                abort_reg,
            ),
            move |result| match result {
                Ok(payload) => Message::SearchCompleted {
                    generation: payload.0,
                    result: Ok(payload.1),
                },
                Err(aborted) => Message::SearchCompleted {
                    generation,
                    result: Err(aborted),
                },
            },
        )
    }

    fn abort_bulk_scan(&mut self) {
        if let Some(cancel) = &self.bulk_scan_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.bulk_scan_cancel = None;
        self.bulk_scan_generation = self.bulk_scan_generation.wrapping_add(1);
        self.bulk_scan_progress = None;
        self.bulk_scan_active = None;
        self.bulk_apply_generation = self.bulk_apply_generation.wrapping_add(1);
        self.bulk_apply_active = None;
        auto_tag::shutdown_classifier_pool();
    }

    fn request_cancel_bulk_apply(&mut self) {
        if let Some(cancel) = &self.bulk_scan_cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.bulk_auto_tag.request_stop_apply();
    }

    fn cancel_bulk_scan(&mut self) {
        self.abort_bulk_scan();
        self.bulk_auto_tag.close();
    }

    fn start_bulk_scan(&mut self) -> Task<Message> {
        let Some(root) = self.bulk_auto_tag.root.clone() else {
            self.bulk_auto_tag
                .set_error("Choose a folder before scanning.");
            return Task::none();
        };
        if !root.is_dir() {
            self.bulk_auto_tag.set_error("That folder no longer exists.");
            return Task::none();
        }
        if !self.allowed_directories.contains_path(&root) {
            self.bulk_auto_tag
                .set_error(FOLDER_OUTSIDE_ALLOWED);
            return Task::none();
        }

        self.abort_bulk_scan();
        let generation = self.bulk_scan_generation;
        self.bulk_scan_active = Some(generation);
        self.bulk_auto_tag.start_running(root.clone());
        let progress = bulk_auto_tag::BulkScanProgress::new();
        self.bulk_scan_progress = Some(progress.clone());
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.bulk_scan_cancel = Some(Arc::clone(&cancel));
        let metadata = self.metadata_cache.snapshot();

        Task::perform(
            async move {
                let (tx, rx) = futures::channel::oneshot::channel();
                std::thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        bulk_auto_tag::scan_and_classify(root, metadata, progress, cancel)
                    }));
                    let summary = match result {
                        Ok(Ok(summary)) => Ok(summary),
                        Ok(Err(message)) => Err(message),
                        Err(_) => {
                            eprintln!("bulk auto tag scan panicked");
                            Err("Scan failed unexpectedly.".into())
                        }
                    };
                    let _ = tx.send((generation, summary));
                });
                rx.await.unwrap_or_else(|_| {
                    eprintln!("bulk auto tag scan channel dropped");
                    (
                        generation,
                        Err("Scan failed unexpectedly.".into()),
                    )
                })
            },
            |(generation, result)| Message::BulkAutoTagScanCompleted { generation, result },
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::FileListSelect { index, shift, control } => {
                self.search_focused = false;
                self.tag_search_focused = false;
                self.file_list_focused = true;
                self.file_selector.select_row(index, shift, control);
                if shift || control {
                    return Task::none();
                }
                let Some(path) = self
                    .file_selector
                    .file_list
                    .get(index)
                    .map(|entry| entry.file_path.clone())
                else {
                    return Task::none();
                };
                self.open_path(&path)
            }

            Message::FileDropped(path) => {
                self.drag_over = false;
                self.open_path(&path)
            }

            Message::FileHovered(path) => {
                self.drag_over = is_audio(&path) || path.is_dir();
                Task::none()
            }

            Message::FilesHoverLeft => {
                self.drag_over = false;
                Task::none()
            }

            Message::OpenFolder => {
                let start_dir = self.file_selector.current_dir.clone();
                Task::perform(pick_folder(start_dir), Message::FolderPicked)
            }

            Message::OpenFile => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter(
                            "Audio",
                            AUDIO_EXTENSIONS,
                        )
                        .pick_file()
                        .await
                        .map(|file| file.path().to_path_buf())
                },
                Message::FilePicked,
            ),

            Message::FilePicked(file) => {
                if let Some(path) = file {
                    return self.open_path(&path);
                }
                Task::none()
            }

            Message::FolderPicked(folder) => {
                if let Some(path) = folder {
                    return self.navigate_directory(path);
                }
                Task::none()
            }

            Message::GoHome => {
                if let Some(home) = self.allowed_directories.startup_directory() {
                    return self.navigate_directory(home);
                }
                if let Some(home) = dirs::home_dir() {
                    return self.navigate_directory(home);
                }
                Task::none()
            }

            Message::RefreshDirectory => {
                let current_dir = self.file_selector.current_dir.clone();
                self.file_selector.reload_directory(&current_dir);
                self.start_file_search()
            }

            Message::Quit => iced::exit(),

            Message::WindowTitleBarPress => {
                self.title_bar.drag_armed = true;
                self.title_bar.press_origin = Some(self.last_cursor);
                Task::none()
            }

            Message::WindowTitleBarRelease => {
                self.title_bar.drag_armed = false;
                self.title_bar.press_origin = None;
                Task::none()
            }

            Message::WindowMinimize => window::latest().then(|id| match id {
                Some(id) => window::minimize(id, true),
                None => Task::none(),
            }),

            Message::WindowToggleMaximize => {
                self.title_bar.drag_armed = false;
                self.title_bar.press_origin = None;
                window::latest().then(|id| match id {
                    Some(id) => window::toggle_maximize(id).chain(
                        window::is_maximized(id).map(Message::WindowMaximizedChanged),
                    ),
                    None => Task::none(),
                })
            }

            Message::WindowMaximizedChanged(maximized) => {
                self.window_maximized = maximized;
                Task::none()
            }

            Message::SyncWindowMaximized => window::latest().then(|id| match id {
                Some(id) => window::is_maximized(id).map(Message::WindowMaximizedChanged),
                None => Task::none(),
            }),

            Message::WindowResize(direction) => window::latest().then(move |id| match id {
                Some(id) => window::drag_resize(id, direction),
                None => Task::none(),
            }),

            Message::NoOp => Task::none(),

            Message::About => {
                self.dialog = Some(Dialog::about(format!(
                    "Tundra {}. FLAC, WAV, MP3, OGG, and AIFF. \
                     Drag samples from the file list into a DAW. \
                     Drop onto the window on Windows, macOS, and X11. On Wayland, use File > Open File.",
                    env!("CARGO_PKG_VERSION")
                )));
                Task::none()
            }

            Message::DismissDialog => {
                self.dialog = None;
                Task::none()
            }

            Message::OpenSettings => {
                self.prepare_feature_modal();
                self.auto_tag_open = false;
                self.settings_open = true;
                self.settings_error = None;
                Task::none()
            }

            Message::SetAlwaysOnTop(always_on_top) => {
                self.always_on_top = always_on_top;
                persist_always_on_top(always_on_top);
                set_window_level(always_on_top)
            }

            Message::CloseSettings => {
                if self.allowed_directories.is_empty() {
                    self.settings_error =
                        Some("Add at least one directory to search.".into());
                    return Task::none();
                }
                self.settings_open = false;
                self.settings_first_run = false;
                self.settings_error = None;
                self.prune_caches();
                let warm = self.warm_allowed_caches();
                let search = self.refresh_search_if_active();
                let launch = self.maybe_open_pending_launch();
                if let Some(task) = launch {
                    return Task::batch([task, warm, search]);
                }
                let navigate = if let Some(dir) = self.allowed_directories.startup_directory() {
                    if !self
                        .allowed_directories
                        .contains_path(&self.file_selector.current_dir)
                    {
                        Some(self.navigate_directory(dir))
                    } else {
                        None
                    }
                } else {
                    None
                };
                match navigate {
                    Some(task) => Task::batch([task, warm, search]),
                    None => Task::batch([warm, search]),
                }
            }

            Message::PickAllowedDirectory => {
                let start_dir = self
                    .allowed_directories
                    .startup_directory()
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(startup_directory);
                Task::perform(pick_folder(start_dir), Message::AllowedDirectoryPicked)
            }

            Message::AllowedDirectoryPicked(path) => {
                if let Some(path) = path {
                    match self.allowed_directories.add(path) {
                        (AddDirectoryResult::Added, Some(resolved)) => {
                            self.allowed_directories.persist();
                            self.settings_error = None;
                            if self.dir_cache.contains_key(&resolved) {
                                return self.refresh_search_if_active();
                            }
                            let walker = future::lazy(move |_| {
                                let children = walk_directory(&resolved);
                                (resolved, children)
                            });
                            return Task::perform(walker, Message::InsertDircache);
                        }
                        (AddDirectoryResult::Unresolved, _) => {
                            self.settings_error =
                                Some("Could not resolve that directory.".into());
                        }
                        (AddDirectoryResult::Duplicate, _) => {}
                        (AddDirectoryResult::Added, None) => {}
                    }
                }
                Task::none()
            }

            Message::RemoveAllowedDirectory(path) => {
                self.allowed_directories.remove(&path);
                self.allowed_directories.persist();
                self.prune_caches();
                self.refresh_search_if_active()
            }

            Message::ChangeDirectory(parent_dir) => self.navigate_directory(parent_dir),

            Message::Search(search_str) => {
                self.search_focused = true;
                self.tag_search_focused = false;
                self.file_selector.search_value = search_str;
                Task::batch([
                    self.start_file_search(),
                    operation::focus(Id::new(FILE_SEARCH_INPUT_ID)),
                ])
            }

            Message::ToggleSearchCaseSensitive => {
                self.file_selector.search_case_sensitive =
                    !self.file_selector.search_case_sensitive;
                self.start_file_search()
            }

            Message::ToggleSearchShowDirectories => {
                self.file_selector.search_show_directories =
                    !self.file_selector.search_show_directories;
                self.start_file_search()
            }

            Message::ToggleFavoritesOnly => {
                self.file_selector.favorites_only = !self.file_selector.favorites_only;
                if self.file_selector.search_active() {
                    self.start_file_search()
                } else {
                    self.reset_file_list();
                    Task::none()
                }
            }

            Message::ToggleFavorite(path) => {
                if self.allowed_audio_error(&path).is_some() {
                    return Task::none();
                }
                self.favorites.toggle(path);
                self.favorites.persist();
                self.refresh_after_favorites_change()
            }

            Message::OpenAutoTag => {
                self.prepare_feature_modal();
                self.tag_editor_open = false;
                self.auto_tag_open = true;
                let target = self.file_selector.selected_audio_path();
                let existing = target.as_ref().and_then(|path| instrument_tag(path));
                self.auto_tag.reset_for_target(target, existing);
                Task::none()
            }

            Message::OpenAutoTagFor(path) => {
                self.prepare_feature_modal();
                self.tag_editor_open = false;
                self.auto_tag_open = true;
                if let Some(err) = self.allowed_audio_error(&path) {
                    self.auto_tag.reset_for_target(None, None);
                    self.auto_tag.set_error(err);
                    return Task::none();
                }
                let existing = instrument_tag(&path);
                self.auto_tag.reset_for_target(Some(path), existing);
                Task::none()
            }

            Message::CloseAutoTag => {
                self.auto_tag_open = false;
                Task::none()
            }

            Message::OpenTagEditorFor(path) => self.open_tag_editor_for(path),

            Message::CloseTagEditor => {
                self.tag_editor_open = false;
                self.tag_editor = TagEditorState::default();
                Task::none()
            }

            Message::TagEditorInput(field, value) => {
                self.tag_editor.set_field(field, value);
                Task::none()
            }

            Message::TagEditorSave => {
                let Some(path) = self.tag_editor.target.clone() else {
                    self.tag_editor.set_error(SELECT_AUDIO_FIRST);
                    return Task::none();
                };
                if let Some(err) = self.allowed_audio_error(&path) {
                    self.tag_editor.set_error(err);
                    return Task::none();
                }
                let edits = self.tag_editor.edits.clone();
                match write_manual_tags(&path, &edits) {
                    Ok(()) => {
                        self.merge_path_metadata(&path);
                        self.tag_editor.error = None;
                        self.tag_editor.status = Some("Tags saved.".into());
                        self.refresh_search_if_active()
                    }
                    Err(err) => {
                        self.tag_editor.set_error(err);
                        Task::none()
                    }
                }
            }

            Message::AutoTagPickFile => {
                let start_dir = self
                    .auto_tag
                    .target
                    .as_ref()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| self.file_selector.current_dir.clone());
                Task::perform(pick_audio_file(start_dir), Message::AutoTagFilePicked)
            }

            Message::AutoTagFilePicked(path) => {
                if let Some(candidate) = path {
                    if let Some(err) = self.allowed_audio_error(&candidate) {
                        self.auto_tag.set_error(err);
                    } else {
                        let existing = instrument_tag(&candidate);
                        self.auto_tag.reset_for_target(Some(candidate), existing);
                    }
                }
                Task::none()
            }

            Message::AutoTagRun => {
                let Some(path) = self.auto_tag.target.clone() else {
                    self.auto_tag.set_error(SELECT_AUDIO_FIRST);
                    return Task::none();
                };
                if let Some(err) = self.allowed_audio_error(&path) {
                    self.auto_tag.set_error(err);
                    return Task::none();
                }
                if let Some(existing) = instrument_tag(&path) {
                    self.auto_tag.existing_instrument = Some(existing);
                }
                if auto_tag_field_status(&path).is_some_and(|status| !status.allows_instrument_work()) {
                    self.auto_tag.clear_error();
                    self.auto_tag.status = AUTO_TAG_INSTRUMENT_PRESENT.into();
                    return Task::none();
                }
                self.auto_tag.begin_run();
                Task::perform(
                    async move {
                        run_blocking(move || auto_tag::classify_file(&path))
                            .await
                            .unwrap_or_else(|_| {
                                Err(auto_tag::ClassifyError::new(
                                    "Couldn't analyze this file.",
                                    "Classifier thread stopped unexpectedly.",
                                ))
                            })
                    },
                    Message::AutoTagCompleted,
                )
            }

            Message::AutoTagCompleted(result) => {
                self.auto_tag.finish_run(result);
                Task::none()
            }

            Message::ToggleAutoTagDetails => {
                self.auto_tag.details_open = !self.auto_tag.details_open;
                Task::none()
            }

            Message::AutoTagApply => {
                let Some(path) = self.auto_tag.target.clone() else {
                    self.auto_tag.set_error(SELECT_AUDIO_FIRST);
                    return Task::none();
                };
                if let Some(err) = self.allowed_audio_error(&path) {
                    self.auto_tag.set_error(err);
                    return Task::none();
                }
                let needs = auto_tag_field_status(&path);
                if needs.is_none_or(|status| status.is_complete()) {
                    self.auto_tag
                        .set_error(AUTO_TAG_ALREADY_COMPLETE);
                    return Task::none();
                }
                let instrument = if needs.is_some_and(|status| status.needs_instrument) {
                    let Some(result) = self.auto_tag.result.clone() else {
                        self.auto_tag
                            .set_error("Detect an instrument before applying tags.");
                        return Task::none();
                    };
                    result.instrument
                } else if needs.is_some_and(|status| status.can_retag_instrument) {
                    self.auto_tag
                        .result
                        .as_ref()
                        .map(|result| result.instrument.clone())
                        .or_else(|| instrument_tag(&path))
                        .unwrap_or_default()
                } else {
                    instrument_tag(&path).unwrap_or_default()
                };
                match write_auto_tags(&path, &instrument) {
                    Ok(written) => {
                        if let Some(existing) = instrument_tag(&path) {
                            self.auto_tag.existing_instrument = Some(existing);
                        }
                        if written {
                            self.merge_path_metadata(&path);
                        }
                        self.auto_tag.applied = true;
                        self.auto_tag.result = None;
                        self.auto_tag.clear_error();
                        self.auto_tag.status = if !written {
                            AUTO_TAG_ALREADY_COMPLETE.into()
                        } else if instrument.is_empty() {
                            "Applied missing tags.".into()
                        } else {
                            format!("Applied tags (instrument: {instrument}).")
                        };
                        if written {
                            self.start_file_search()
                        } else {
                            Task::none()
                        }
                    }
                    Err(err) => {
                        self.auto_tag.applied = false;
                        self.auto_tag.set_error(err);
                        Task::none()
                    }
                }
            }

            Message::OpenBulkAutoTag => {
                self.dialog = None;
                self.auto_tag_open = false;
                self.tag_editor_open = false;
                self.abort_bulk_scan();
                self.bulk_auto_tag.open();
                Task::none()
            }

            Message::CloseBulkAutoTag => {
                let applying = matches!(
                    self.bulk_auto_tag.phase,
                    Some(BulkAutoTagPhase::Applying)
                ) && self.bulk_apply_active.is_some();
                if applying {
                    if self.bulk_auto_tag.apply_stop_requested {
                        self.abort_bulk_scan();
                        self.bulk_auto_tag.close();
                    } else {
                        self.request_cancel_bulk_apply();
                    }
                    return Task::none();
                }
                self.abort_bulk_scan();
                self.bulk_auto_tag.close();
                Task::none()
            }

            Message::BulkAutoTagPickDirectory => {
                let start_dir = self
                    .bulk_auto_tag
                    .root
                    .clone()
                    .unwrap_or_else(|| self.file_selector.current_dir.clone());
                Task::perform(pick_folder(start_dir), Message::BulkAutoTagDirectoryPicked)
            }

            Message::BulkAutoTagDirectoryPicked(path) => {
                if let Some(dir) = path {
                    if self.allowed_directories.contains_path(&dir) {
                        self.bulk_auto_tag.root = Some(dir);
                        self.bulk_auto_tag.error = None;
                    } else {
                        self.bulk_auto_tag
                            .set_error(FOLDER_OUTSIDE_ALLOWED);
                    }
                }
                Task::none()
            }

            Message::BulkAutoTagRunScan => self.start_bulk_scan(),

            Message::BulkAutoTagProgressTick => {
                if let Some(progress) = &self.bulk_scan_progress {
                    self.bulk_auto_tag
                        .update_progress(progress.snapshot());
                }
                Task::none()
            }

            Message::BulkAutoTagScanCompleted { generation, result } => {
                if self.bulk_scan_active != Some(generation) {
                    return Task::none();
                }
                self.bulk_scan_progress = None;
                self.bulk_scan_active = None;
                match result {
                    Ok(summary) if bulk_auto_tag::is_empty_scan(&summary) => {
                        self.bulk_auto_tag
                            .set_scan_all_complete(summary.root, summary.skipped_complete);
                    }
                    Ok(summary) => {
                        self.bulk_auto_tag.finish_scan(summary);
                    }
                    Err(message) if message == "Scan cancelled." => {}
                    Err(message) => {
                        self.bulk_auto_tag.set_error(message);
                    }
                }
                Task::none()
            }

            Message::BulkAutoTagSetFileAccepted {
                dir_idx,
                file_idx,
                accepted,
            } => {
                self.bulk_auto_tag
                    .set_file_accepted(dir_idx, file_idx, accepted);
                Task::none()
            }

            Message::BulkAutoTagSelectFile {
                dir_idx,
                file_idx,
                shift,
                control,
            } => {
                self.bulk_auto_tag
                    .select_file(dir_idx, file_idx, shift, control);
                Task::none()
            }

            Message::BulkAutoTagSelectDirectory {
                dir_idx,
                shift,
                control,
            } => {
                self.bulk_auto_tag
                    .select_directory(dir_idx, shift, control);
                Task::none()
            }

            Message::BulkAutoTagSelectAll => {
                self.bulk_auto_tag.select_all_files();
                Task::none()
            }

            Message::BulkAutoTagClearSelection => {
                self.bulk_auto_tag.selected.clear();
                self.bulk_auto_tag.selection_anchor = None;
                Task::none()
            }

            Message::BulkAutoTagCheckSelected => {
                self.bulk_auto_tag.set_selected_accepted(true);
                Task::none()
            }

            Message::BulkAutoTagUncheckSelected => {
                self.bulk_auto_tag.set_selected_accepted(false);
                Task::none()
            }

            Message::BulkAutoTagAcceptAll => {
                bulk_auto_tag::set_all_accepted(&mut self.bulk_auto_tag.groups, true);
                Task::none()
            }

            Message::BulkAutoTagRejectAll => {
                bulk_auto_tag::set_all_accepted(&mut self.bulk_auto_tag.groups, false);
                Task::none()
            }

            Message::BulkAutoTagToggleDirectoryExpanded(dir_idx) => {
                if let Some(group) = self.bulk_auto_tag.groups.get_mut(dir_idx) {
                    group.expanded = !group.expanded;
                }
                Task::none()
            }

            Message::BulkAutoTagExpandAllDirectories => {
                for group in &mut self.bulk_auto_tag.groups {
                    group.expanded = true;
                }
                Task::none()
            }

            Message::BulkAutoTagCollapseAllDirectories => {
                for group in &mut self.bulk_auto_tag.groups {
                    group.expanded = false;
                }
                Task::none()
            }

            Message::BulkAutoTagApply => {
                let items = bulk_auto_tag::collect_accepted(&self.bulk_auto_tag.groups);
                let accepted = items.len();
                if accepted == 0 {
                    return Task::none();
                }
                self.bulk_apply_generation = self.bulk_apply_generation.wrapping_add(1);
                let generation = self.bulk_apply_generation;
                self.bulk_apply_active = Some(generation);
                self.bulk_auto_tag.start_apply();
                let progress = bulk_auto_tag::BulkScanProgress::new();
                progress.set_applying(accepted);
                self.bulk_scan_progress = Some(progress.clone());
                let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
                self.bulk_scan_cancel = Some(Arc::clone(&cancel));
                Task::perform(
                    async move {
                        (
                            generation,
                            bulk_auto_tag::apply_items(&items, Some(&progress), &cancel),
                        )
                    },
                    |(generation, summary)| Message::BulkAutoTagApplyCompleted { generation, summary },
                )
            }

            Message::BulkAutoTagApplyCompleted { generation, summary } => {
                self.bulk_scan_progress = None;
                for path in &summary.written_paths {
                    self.merge_path_metadata(path);
                }
                if self.bulk_apply_active != Some(generation) {
                    return Task::none();
                }
                self.bulk_apply_active = None;
                if self.bulk_auto_tag.is_open() {
                    self.bulk_auto_tag.finish_apply(summary);
                }
                self.refresh_search_if_active()
            }

            Message::SearchFocused(focused) => {
                self.search_focused = focused;
                if focused {
                    self.tag_search_focused = false;
                    self.file_list_focused = false;
                }
                Task::none()
            }

            Message::TagSearchInput(input) => {
                self.tag_search_focused = true;
                self.search_focused = false;
                self.file_selector.tag_search_error = None;
                self.file_selector.tag_search_value = input;
                operation::focus(Id::new(TAG_SEARCH_INPUT_ID))
            }

            Message::TagSearchSubmit => {
                self.tag_search_focused = true;
                self.search_focused = false;
                let input = self.file_selector.tag_search_value.clone();
                if !input.contains(':') {
                    let trimmed = input.trim();
                    if !trimmed.is_empty() {
                        if tag_field_best_match(&input).is_some() {
                            return self.autocomplete_tag_field();
                        }
                        self.file_selector.tag_search_error =
                            Some(tag_parse_message(TagParseError::UnknownField).into());
                        return operation::focus(Id::new(TAG_SEARCH_INPUT_ID));
                    }
                }
                match parse_tag_filter(&input) {
                    Ok(filter) => {
                        self.file_selector.tag_search_error = None;
                        self.file_selector.add_tag_filter(filter);
                        self.file_selector.tag_search_value.clear();
                        Task::batch([
                            self.start_file_search(),
                            operation::focus(Id::new(TAG_SEARCH_INPUT_ID)),
                        ])
                    }
                    Err(err) => {
                        self.file_selector.tag_search_error =
                            Some(tag_parse_message(err).into());
                        operation::focus(Id::new(TAG_SEARCH_INPUT_ID))
                    }
                }
            }

            Message::TagSearchAutocomplete => {
                self.tag_search_focused = true;
                self.search_focused = false;
                self.autocomplete_tag_field()
            }

            Message::TagSearchFocused(focused) => {
                self.tag_search_focused = focused;
                if focused {
                    self.search_focused = false;
                }
                Task::none()
            }

            Message::TagFilterRemove(field) => {
                self.file_selector
                    .tag_filters
                    .retain(|filter| filter.field != field);
                self.start_file_search()
            }

            Message::TagSuggestionSelect(field) => {
                self.tag_search_focused = true;
                self.search_focused = false;
                self.file_selector.tag_search_error = None;
                self.file_selector.tag_search_value = format!("{}:", field.as_str());
                operation::focus(Id::new(TAG_SEARCH_INPUT_ID))
            }

            Message::SearchCompleted { generation, result } => {
                if generation != self.search_generation {
                    return Task::none();
                }
                if !self.search_enabled() || !self.file_selector.search_active() {
                    return Task::none();
                }
                if let Ok(result) = result {
                    let mut cache_updated = false;
                    for (root, children) in result.cached_roots {
                        if self.allowed_directories.contains_path(&root) {
                            self.dir_cache.insert(root, children);
                            cache_updated = true;
                        }
                    }
                    if cache_updated {
                        self.dir_cache.persist();
                    }
                    // Search paths are pre-filtered to directories and audio files,
                    // so dir-ness follows from the extension; avoids one stat per result.
                    self.file_selector.file_list = result
                        .paths
                        .iter()
                        .map(|x| {
                            FileButton::with_kind(
                                x.to_path_buf(),
                                &self.file_selector.current_dir,
                                !is_audio(x),
                            )
                        })
                        .collect();
                    self.file_selector.list_error = None;
                    self.file_selector.clear_selection();
                    self.metadata_cache.merge(result.new_metadata);
                }
                Task::none()
            }

            Message::TogglePlaying => {
                if self
                    .player
                    .controls
                    .is_playing
                    .load(Ordering::Acquire)
                {
                    self.player.pause();
                } else {
                    self.player.play();
                }
                Task::none()
            }

            Message::ToggleLoop => {
                self.player.toggle_loop();
                persist_looping(
                    self.player
                        .controls
                        .looping
                        .load(std::sync::atomic::Ordering::Relaxed),
                );
                Task::none()
            }

            Message::StopPlayback => {
                self.player.stop();
                Task::none()
            }

            Message::StartupCachesReady(caches) => {
                self.dir_cache = DirCache::from_map(caches.dirs);
                self.metadata_cache = MetadataCache::from_map(caches.metadata);
                self.caches_ready = true;
                let warm = self.warm_allowed_caches();
                let search = self.refresh_search_if_active();
                Task::batch([
                    warm,
                    search,
                    self.maybe_open_pending_launch()
                        .unwrap_or_else(Task::none),
                ])
            }

            Message::PlayerWorkerReady(worker, receiver) => {
                self.player.attach_worker(worker);
                match Arc::try_unwrap(receiver) {
                    Ok(recv) => self.player_msgs = Some(Arc::new(recv)),
                    Err(recv) => self.player_msgs = Some(recv),
                }
                self.maybe_open_pending_launch()
                    .unwrap_or_else(Task::none)
            }

            Message::InsertDircache((parent_dir, children)) => {
                if !self.allowed_directories.contains_path(&parent_dir) {
                    return Task::none();
                }
                self.dir_cache.insert(parent_dir, children.clone());
                self.dir_cache.persist();
                let metadata = self.metadata_cache.snapshot();
                Task::perform(
                    async move { index_paths(&children, metadata) },
                    Message::MetadataIndexed,
                )
            }

            Message::MetadataIndexed(new_metadata) => {
                self.metadata_cache.merge(new_metadata);
                self.refresh_search_if_active()
            }

            Message::InvalidateDircache => {
                self.dir_cache = DirCache::new();
                self.dir_cache.persist();
                self.metadata_cache = MetadataCache::new();
                self.metadata_cache.persist();
                auto_tag::clear_classify_cache();
                let warm = self.warm_allowed_caches();
                let search = self.refresh_search_if_active();
                Task::batch([warm, search])
            }

            Message::PlayerMsg((msg, recv)) => {
                match msg {
                    Some(PlayerMsg::SinkEmpty) => {
                        self.player.on_ended();
                    }
                    Some(PlayerMsg::Looped) => {
                        if let Some(state) = &mut self.player.controls.playback_progress {
                            state.progress = 0.0;
                        }
                    }
                    Some(PlayerMsg::StreamFailed) => {
                        self.show_error(
                            "Audio output unavailable. Check your sound device.".into(),
                        );
                    }
                    Some(PlayerMsg::WaveformPeaksReady) => {
                        self.player.on_waveform_peaks_ready();
                    }
                    None => return Task::none(),
                }
                match Arc::into_inner(recv) {
                    None => {
                        eprintln!("Message::PlayerMsg Arc::into_inner failed");
                        Task::none()
                    }
                    Some(recv) => Task::perform(recv.into_future(), |x| {
                        Message::PlayerMsg((x.0, Arc::new(x.1)))
                    }),
                }
            }

            Message::WaveformViewChanged(view) => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformPanStarted => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_pan_active(true);
                }
                Task::none()
            }

            Message::WaveformPanEnded(view) => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_pan_active(false);
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformSpringTick => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    if view.overscroll != 0.0 {
                        view.spring_overscroll();
                        waveform.set_view(view);
                    }
                }
                Task::none()
            }

            Message::WaveformZoomIn => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    view.zoom_in(waveform.sample_count());
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformZoomOut => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    view.zoom_out(waveform.sample_count());
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformHelp => {
                self.dialog = Some(Dialog::waveform_help());
                Task::none()
            }

            Message::WaveformHoverChanged(hovered) => {
                self.waveform_hovered = hovered;
                if hovered {
                    self.search_focused = false;
                }
                Task::none()
            }

            Message::ControlsHoverChanged(hovered) => {
                self.controls_hovered = hovered;
                Task::none()
            }

            Message::FileListHoverChanged(hovered) => {
                self.file_list_hovered = hovered;
                if hovered {
                    self.search_focused = false;
                    self.tag_search_focused = false;
                } else {
                    self.file_list_focused = false;
                }
                Task::none()
            }

            Message::WaveformKey(key) => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    if WaveFormView::apply_key(&mut view, &key, waveform.sample_count()) {
                        waveform.set_view(view);
                    }
                }
                Task::none()
            }

            Message::PlaybackTick => {
                self.player.sync_playback_ui();
                Task::none()
            }

            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_modifiers(modifiers);
                }
                Task::none()
            }

            Message::FileDragPress {
                path,
                from_file_list,
            } => {
                if from_file_list {
                    self.search_focused = false;
                    self.tag_search_focused = false;
                    self.file_list_focused = true;
                }
                if from_file_list && self.modifiers.shift() {
                    self.file_drag = Some(FileDragPending {
                        kind: FileDragKind::Scroll,
                        origin: self.last_cursor,
                        last: self.last_cursor,
                        threshold_met: true,
                        origin_locked: true,
                        awaiting_drag: false,
                        open_on_click: false,
                    });
                    return Task::none();
                }
                self.file_drag = Some(FileDragPending {
                    kind: FileDragKind::File(path),
                    origin: self.last_cursor,
                    last: self.last_cursor,
                    threshold_met: false,
                    origin_locked: false,
                    awaiting_drag: false,
                    open_on_click: from_file_list,
                });
                Task::none()
            }

            Message::FileDragMove(point) => {
                let Some(drag) = &mut self.file_drag else {
                    return Task::none();
                };
                if !drag.origin_locked {
                    drag.origin = point;
                    drag.last = point;
                    drag.origin_locked = true;
                    return Task::none();
                }
                match &drag.kind {
                    FileDragKind::Scroll => {
                        let dy = point.y - drag.last.y;
                        drag.last = point;
                        if dy.abs() < 0.5 {
                            return Task::none();
                        }
                        operation::scroll_by(
                            FILE_LIST_SCROLL_ID,
                            AbsoluteOffset { x: 0.0, y: dy },
                        )
                    }
                    FileDragKind::File(path) => {
                        if drag.threshold_met || drag.awaiting_drag {
                            return Task::none();
                        }
                        let dx = point.x - drag.origin.x;
                        let dy = point.y - drag.origin.y;
                        if dx * dx + dy * dy < FILE_DRAG_THRESHOLD * FILE_DRAG_THRESHOLD {
                            return Task::none();
                        }
                        let path = path.clone();
                        if self.drag_ready {
                            drag.threshold_met = true;
                            self.start_file_drag(path)
                        } else {
                            drag.awaiting_drag = true;
                            Self::ensure_drag()
                        }
                    }
                }
            }

            Message::FileDragRelease => {
                let mut task = Task::none();
                let click_to_open = self.file_drag.as_ref().and_then(|drag| {
                    if self.native_drag.is_active() || drag.threshold_met || drag.awaiting_drag {
                        return None;
                    }
                    if drag.open_on_click {
                        if let FileDragKind::File(path) = &drag.kind {
                            Some(path.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                if self.native_drag.is_active() {
                    self.native_drag.update(true, true);
                }
                if !self.native_drag.is_active() {
                    self.file_drag = None;
                }
                if let Some(path) = click_to_open {
                    task = self.open_path(&path);
                }
                task
            }

            Message::FileDragTick => {
                if self.native_drag.is_active() {
                    self.native_drag.update(true, false);
                    if !self.native_drag.is_active() {
                        self.file_drag = None;
                    }
                }
                Task::none()
            }

            Message::FileDragCompleted(result) => {
                if let Err(err) = result {
                    self.show_notice(drag_out_notice(format!("Drag failed: {err}.")));
                }
                self.file_drag = None;
                Task::none()
            }

            Message::DragWindowId(window_id) => {
                let Some(id) = window_id else {
                    self.drag_ready = false;
                    #[cfg(all(unix, not(target_os = "macos")))]
                    self.show_notice(drag_out_notice(
                        "Drag-out from the file list requires X11 and is unavailable on native Wayland.",
                    ));
                    #[cfg(not(all(unix, not(target_os = "macos"))))]
                    self.show_notice(drag_out_notice("Could not initialize drag-out."));
                    self.file_drag = None;
                    return Task::none();
                };
                match self.native_drag.init_with_window_id(id) {
                    Ok(()) => {
                        self.drag_ready = true;
                    }
                    Err(err) => {
                        self.drag_ready = false;
                        self.show_notice(drag_out_notice(format!(
                            "Could not initialize drag-out: {err}."
                        )));
                        self.file_drag = None;
                        return Task::none();
                    }
                }
                let path = {
                    let Some(drag) = &mut self.file_drag else {
                        return Task::none();
                    };
                    if !drag.awaiting_drag {
                        return Task::none();
                    }
                    let path = match &drag.kind {
                        FileDragKind::File(path) => path.clone(),
                        FileDragKind::Scroll => {
                            drag.awaiting_drag = false;
                            return Task::none();
                        }
                    };
                    drag.awaiting_drag = false;
                    drag.threshold_met = true;
                    path
                };
                self.start_file_drag(path)
            }

            Message::CursorMoved(point) => {
                self.last_cursor = point;
                if self.title_bar.drag_armed {
                    let Some(origin) = self.title_bar.press_origin else {
                        return Task::none();
                    };
                    let dx = point.x - origin.x;
                    let dy = point.y - origin.y;
                    if dx * dx + dy * dy < TITLE_DRAG_THRESHOLD * TITLE_DRAG_THRESHOLD {
                        return Task::none();
                    }
                    self.title_bar.drag_armed = false;
                    self.title_bar.press_origin = None;
                    return window::latest().then(|id| match id {
                        Some(id) => window::drag(id),
                        None => Task::none(),
                    });
                }
                Task::none()
            }

            Message::FileRowHover(index) => {
                self.search_focused = false;
                self.file_selector.hovered_file = Some(index);
                Task::none()
            }

            Message::FileRowLeave => {
                self.file_selector.hovered_file = None;
                Task::none()
            }

            Message::FileListScrolled(viewport) => {
                self.file_selector.list_scroll_offset = viewport.absolute_offset().y;
                self.file_selector.list_viewport_height = viewport.bounds().height;
                Task::none()
            }

            Message::FileListScrollbarPress {
                track_y,
                track_top,
                track_height,
            } => {
                let metrics = file_list_scroll_metrics_for(&self.file_selector);
                if metrics.max_scroll <= 0.0 {
                    return Task::none();
                }
                let grab_offset = file_list_scrollbar_grab_offset(&metrics, track_y);
                self.file_list_scrollbar_drag = Some(FileListScrollbarDrag {
                    track_top,
                    track_height,
                    grab_offset,
                });
                let offset = file_list_scroll_offset_for_track_y(&metrics, track_y, grab_offset);
                operation::scroll_to(
                    Id::new(FILE_LIST_SCROLL_ID),
                    AbsoluteOffset {
                        x: Some(0.0),
                        y: Some(offset),
                    },
                )
            }

            Message::FileListScrollbarDrag(point) => {
                let Some(drag) = self.file_list_scrollbar_drag.as_ref() else {
                    return Task::none();
                };
                let metrics = file_list_scroll_metrics_for(&self.file_selector);
                if metrics.max_scroll <= 0.0 {
                    return Task::none();
                }
                let track_y = (point.y - drag.track_top).clamp(0.0, drag.track_height);
                let offset =
                    file_list_scroll_offset_for_track_y(&metrics, track_y, drag.grab_offset);
                operation::scroll_to(
                    Id::new(FILE_LIST_SCROLL_ID),
                    AbsoluteOffset {
                        x: Some(0.0),
                        y: Some(offset),
                    },
                )
            }

            Message::FileListScrollbarRelease => {
                self.file_list_scrollbar_drag = None;
                Task::none()
            }

            Message::VolumeChanged(volume) => {
                self.player.set_volume(volume);
                Task::none()
            }

            Message::VolumeCommit => {
                persist_volume(self.player.controls.volume);
                Task::none()
            }

            Message::WaveformScrub(progress) => {
                self.waveform_scrubbing = true;
                self.last_scrub_progress = progress;
                self.player.controls.scrubbing = true;
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_ui_scrubbing(true);
                    waveform.set_scrub_progress(Some(progress));
                }
                if let Some(state) = &mut self.player.controls.playback_progress {
                    state.progress = progress;
                }
                Task::none()
            }

            Message::WaveformScrubEnd(progress) => {
                if !self.waveform_scrubbing {
                    return Task::none();
                }
                self.waveform_scrubbing = false;
                self.player.controls.scrubbing = false;
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_ui_scrubbing(false);
                    waveform.set_scrub_progress(None);
                }
                self.player.seek(progress);
                Task::none()
            }

            Message::WaveformScrubRelease => {
                if !self.waveform_scrubbing {
                    return Task::none();
                }
                let progress = self.last_scrub_progress;
                self.waveform_scrubbing = false;
                self.player.controls.scrubbing = false;
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_ui_scrubbing(false);
                    waveform.set_scrub_progress(None);
                }
                self.player.seek(progress);
                Task::none()
            }

            Message::WaveformFileDragStart => {
                let Some(path) = self.player.current_file.clone() else {
                    return Task::none();
                };
                self.file_drag = Some(FileDragPending {
                    kind: FileDragKind::File(path),
                    origin: self.last_cursor,
                    last: self.last_cursor,
                    threshold_met: false,
                    origin_locked: false,
                    awaiting_drag: false,
                    open_on_click: false,
                });
                Task::none()
            }

            Message::WaveformCopyName => self.with_current_file(clipboard_name),
            Message::WaveformCopyPath => self.with_current_file(clipboard_path),
            Message::WaveformRevealInFileManager => self.with_current_file(reveal_path),
            Message::WaveformOpenAutoTag => match self.player.current_file.clone() {
                Some(path) => self.update(Message::OpenAutoTagFor(path)),
                None => Task::none(),
            },
            Message::WaveformEditTags => match self.player.current_file.clone() {
                Some(path) => self.open_tag_editor_for(path),
                None => Task::none(),
            },

            Message::FileCopyName(path) => clipboard_name(&path),
            Message::FileCopyPath(path) => clipboard_path(&path),
            Message::FileRevealInFileManager(path) => reveal_path(&path),

            Message::SidebarResizeStart => {
                self.sidebar_resize = Some(SidebarResize {
                    origin_x: 0.0,
                    origin_width: self.sidebar_width,
                    pending_origin: true,
                });
                Task::none()
            }

            Message::SidebarResizeMove(cursor_x) => {
                let Some(resize) = &mut self.sidebar_resize else {
                    return Task::none();
                };
                if resize.pending_origin {
                    resize.origin_x = cursor_x;
                    resize.pending_origin = false;
                    return Task::none();
                }
                self.sidebar_width = (resize.origin_width + (cursor_x - resize.origin_x))
                    .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                Task::none()
            }

            Message::SidebarResizeEnd => {
                self.sidebar_resize = None;
                persist_sidebar_width(self.sidebar_width);
                Task::none()
            }
        }
    }

    fn sidebar_resizer(resizing: bool) -> Element<'static, Message> {
        let gutter = (SIDEBAR_RESIZER_HIT_WIDTH - SIDEBAR_RESIZER_LINE_WIDTH) / 2.0;
        let line = container(
            Space::new()
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fixed(SIDEBAR_RESIZER_LINE_WIDTH))
        .height(Length::Fill)
        .style(move |theme: &Theme| {
            let palette = theme.extended_palette();
            let alpha = if resizing { 0.85 } else { 0.45 };
            container::Style {
                background: Some(
                    palette
                        .background
                        .strong
                        .color
                        .scale_alpha(alpha)
                        .into(),
                ),
                ..Default::default()
            }
        });

        // Visual line underneath; transparent mouse_area on top receives hover/cursor.
        stack![
            row![
                Space::new().width(Length::Fixed(gutter)),
                line,
                Space::new().width(Length::Fixed(gutter)),
            ]
            .width(Length::Fixed(SIDEBAR_RESIZER_HIT_WIDTH))
            .height(Length::Fill),
            mouse_area(
                Space::new()
                    .width(Length::Fixed(SIDEBAR_RESIZER_HIT_WIDTH))
                    .height(Length::Fill),
            )
            .interaction(mouse::Interaction::ResizingColumn)
            .on_press(Message::SidebarResizeStart),
        ]
        .width(Length::Fixed(SIDEBAR_RESIZER_HIT_WIDTH))
        .height(Length::Fill)
        .into()
    }

    fn sidebar_resize_overlay(resizing: bool) -> Element<'static, Message> {
        if !resizing {
            return Space::new().into();
        }
        mouse_area(
            Space::new()
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .interaction(mouse::Interaction::ResizingColumn)
        .on_move(|point| Message::SidebarResizeMove(point.x))
        .on_release(Message::SidebarResizeEnd)
        .into()
    }

    fn window_resize_strip(
        width: Length,
        height: Length,
        direction: window::Direction,
        cursor: mouse::Interaction,
    ) -> Element<'static, Message> {
        mouse_area(
            Space::new()
                .width(width)
                .height(height),
        )
        .on_press(Message::WindowResize(direction))
        .interaction(cursor)
        .into()
    }

    fn window_resize_frame(content: Element<'_, Message>) -> Element<'_, Message> {
        let border = WINDOW_RESIZE_BORDER;
        let corner = Length::Fixed(border);
        let edge = Length::Fixed(border);
        let fill = Length::Fill;

        let edges = column![
            row![
                Self::window_resize_strip(
                    corner,
                    corner,
                    window::Direction::NorthWest,
                    mouse::Interaction::ResizingDiagonallyDown,
                ),
                Space::new()
                    .width(Length::FillPortion(1))
                    .height(edge),
                Space::new()
                    .width(Length::FillPortion(2))
                    .height(edge),
                Space::new()
                    .width(Length::FillPortion(1))
                    .height(edge),
                Self::window_resize_strip(
                    corner,
                    corner,
                    window::Direction::NorthEast,
                    mouse::Interaction::ResizingDiagonallyUp,
                ),
            ]
            .width(fill)
            .height(edge),
            row![
                Self::window_resize_strip(
                    edge,
                    fill,
                    window::Direction::West,
                    mouse::Interaction::ResizingHorizontally,
                ),
                Space::new().width(fill).height(fill),
                Self::window_resize_strip(
                    edge,
                    fill,
                    window::Direction::East,
                    mouse::Interaction::ResizingHorizontally,
                ),
            ]
            .width(fill)
            .height(fill),
            row![
                Self::window_resize_strip(
                    corner,
                    corner,
                    window::Direction::SouthWest,
                    mouse::Interaction::ResizingDiagonallyUp,
                ),
                Self::window_resize_strip(
                    fill,
                    edge,
                    window::Direction::South,
                    mouse::Interaction::ResizingVertically,
                ),
                Self::window_resize_strip(
                    corner,
                    corner,
                    window::Direction::SouthEast,
                    mouse::Interaction::ResizingDiagonallyDown,
                ),
            ]
            .width(fill)
            .height(edge),
        ]
        .width(fill)
        .height(fill);

        stack![
            container(content).width(fill).height(fill),
            edges,
        ]
        .width(fill)
        .height(fill)
        .into()
    }

    fn dialog_table_header(label: &'static str) -> Element<'static, Message> {
        text(label)
            .size(10)
            .width(Length::FillPortion(1))
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(
                    theme
                        .extended_palette()
                        .background
                        .base
                        .text
                        .scale_alpha(0.62),
                ),
            })
            .into()
    }

    fn dialog_control_table(rows: &[(String, String)]) -> Element<'static, Message> {
        let header = container(
            row![
                Self::dialog_table_header("Input"),
                Self::dialog_table_header("Action"),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center)
            .width(Length::Fill),
        )
        .padding([6, 10])
        .width(Length::Fill)
        .style(|theme: &Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette.background.weak.color.scale_alpha(0.32).into()),
                border: Border {
                    width: 1.0,
                    color: palette.background.strong.color.scale_alpha(0.18),
                    radius: 0.0.into(),
                },
                ..Default::default()
            }
        });

        let mut table = column![header].spacing(0).width(Length::Fill);
        for (index, (input, action)) in rows.iter().enumerate() {
            let zebra = index % 2 == 1;
            table = table.push(
                container(
                    row![
                        text(input.clone())
                            .size(13)
                            .width(Length::FillPortion(1)),
                        text(action.clone())
                            .size(13)
                            .width(Length::FillPortion(1)),
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center)
                    .width(Length::Fill),
                )
                .padding([7, 10])
                .width(Length::Fill)
                .style(move |theme: &Theme| {
                    let palette = theme.extended_palette();
                    container::Style {
                        background: Some(
                            if zebra {
                                palette.background.weak.color.scale_alpha(0.18)
                            } else {
                                Color::TRANSPARENT
                            }
                            .into(),
                        ),
                        ..Default::default()
                    }
                }),
            );
        }
        container(table)
            .width(Length::Fill)
            .style(|theme: &Theme| {
                let palette = theme.extended_palette();
                container::Style {
                    border: Border {
                        width: 1.0,
                        color: palette.background.strong.color.scale_alpha(0.22),
                        radius: 6.0.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }

    fn dialog_view(dialog: &Dialog) -> Element<'_, Message> {
        let mut body = column![text(&dialog.title).size(18)]
            .spacing(12)
            .padding(16)
            .width(Length::Fixed(440.0));
        if !dialog.body.is_empty() {
            body = body.push(text(&dialog.body).size(14).width(Length::Fill));
        }
        if !dialog.rows.is_empty() {
            body = body.push(Self::dialog_control_table(&dialog.rows));
        }
        body = body.push(
            row![
                Space::new().width(Length::Fill),
                button(text("OK")).on_press(Message::DismissDialog),
            ]
            .align_y(iced::Alignment::Center),
        );
        opaque(
            container(body)
            .style(|theme: &Theme| {
                let palette = theme.extended_palette();
                container::Style {
                    background: Some(palette.background.base.color.into()),
                    border: Border {
                        width: 1.0,
                        color: palette.background.strong.color,
                        radius: 8.0.into(),
                    },
                    shadow: Shadow {
                        color: palette.background.base.text.scale_alpha(0.25),
                        offset: iced::Vector::new(0.0, 4.0),
                        blur_radius: 16.0,
                    },
                    ..Default::default()
                }
            }),
        )
    }

    fn dim_scrim(_theme: &Theme) -> container::Style {
        container::Style {
            background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
            ..Default::default()
        }
    }

    fn with_dim_overlay<'a>(
        base: Element<'a, Message>,
        overlay: Element<'a, Message>,
    ) -> Element<'a, Message> {
        stack![
            base,
            opaque(container(center(overlay)).style(Self::dim_scrim)),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn with_dialog<'a>(base: Element<'a, Message>, dialog: &'a Dialog) -> Element<'a, Message> {
        stack![
            base,
            opaque(
                mouse_area(center(Self::dialog_view(dialog)).style(Self::dim_scrim))
                    .on_press(Message::DismissDialog)
            )
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let active_file = self
            .player
            .current_file
            .as_ref()
            .and_then(|path| crate::path_util::file_name_lossy(path));
        let menu = self.menu.view(self.always_on_top, active_file.as_deref());

        let file_selector = container(
            self.file_selector
                .view(self.search_enabled(), &self.favorites, self.modifiers),
        )
            .width(Length::Fixed(self.sidebar_width))
            .height(Length::Fill)
            .style(|theme| {
                let palette = theme.extended_palette();
                let base = palette.background.base.color;
                container::Style {
                    background: Some(
                        Color::from_rgb(base.r * 0.56, base.g * 0.56, base.b * 0.58).into(),
                    ),
                    border: Border {
                        width: 1.0,
                        color: palette.background.strong.color.scale_alpha(0.42),
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                }
            });

        let resizing = self.sidebar_resize.is_some();
        let resizer = Self::sidebar_resizer(resizing);

        let tag_summary = self
            .player
            .current_file
            .as_ref()
            .map(|path| control_bar_tags(&self.metadata_cache.tag_fields_for(path)))
            .unwrap_or_default();

        let player = container(if self.drag_over {
            stack![
                self.player.view(tag_summary.clone()),
                container(
                    text("Drop audio file or folder")
                        .size(18)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_theme| container::Style {
                    background: Some(Color::from_rgba(0.08, 0.12, 0.18, 0.82).into()),
                    ..Default::default()
                }),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
        } else {
            stack![self.player.view(tag_summary)]
                .width(Length::Fill)
                .height(Length::Fill)
        })
        .width(Length::Fill)
        .height(Length::Fill);

        let workspace = stack![
            row![file_selector, resizer, player]
                .spacing(0)
                .height(Length::Fill)
                .width(Length::Fill),
            Self::sidebar_resize_overlay(resizing),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        let workspace: Element<_> = if self.settings_open {
            Self::with_dim_overlay(
                workspace.into(),
                settings::settings_view(
                    self.allowed_directories.roots(),
                    self.settings_first_run,
                    self.settings_error.clone(),
                ),
            )
        } else if self.bulk_auto_tag.is_open() {
            Self::with_dim_overlay(
                workspace.into(),
                bulk_auto_tag_view(&self.bulk_auto_tag, self.modifiers),
            )
        } else if self.tag_editor_open {
            Self::with_dim_overlay(workspace.into(), tag_editor_view(&self.tag_editor))
        } else if self.auto_tag_open {
            let path_status = self
                .auto_tag
                .target
                .as_ref()
                .and_then(|path| auto_tag_field_status(path));
            Self::with_dim_overlay(
                workspace.into(),
                auto_tag_view(&self.auto_tag, path_status),
            )
        } else if let Some(dialog) = &self.dialog {
            Self::with_dialog(workspace.into(), dialog)
        } else {
            workspace.into()
        };

        let layout = column![menu, workspace]
            .width(Length::Fill)
            .height(Length::Fill);

        if self.window_maximized {
            container(layout)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            Self::window_resize_frame(layout.into())
        }
    }
}
