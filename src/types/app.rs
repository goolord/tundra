use super::*;
use crate::drag_out::NativeDrag;
use crate::metadata::{
    filter_search_paths, index_paths, parse_tag_filter, tag_field_best_match, tag_parse_message,
    CachedMetadata, TagParseError,
};
use super::settings::{self, AddDirectoryResult, AllowedDirectories};
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::collections::hash_map::HashMap;
use std::time::Duration;
use walkdir::WalkDir;

const DEFAULT_SIDEBAR_WIDTH: f32 = 280.0;
const MIN_SIDEBAR_WIDTH: f32 = 160.0;
const MAX_SIDEBAR_WIDTH: f32 = 720.0;
const SIDEBAR_RESIZER_HIT_WIDTH: f32 = 10.0;
const SIDEBAR_RESIZER_LINE_WIDTH: f32 = 2.0;
const FILE_DRAG_THRESHOLD: f32 = 8.0;

fn walk_directory(dir: &Path) -> Vec<PathBuf> {
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

fn transport_shortcut_allowed(modifiers: Modifiers) -> bool {
    !modifiers.shift() && !modifiers.control() && !modifiers.alt() && !modifiers.logo()
}

async fn pick_folder(start_dir: PathBuf) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Select Folder")
        .set_directory(&start_dir)
        .pick_folder()
        .await
        .map(|folder| folder.path().to_path_buf())
}

enum FileDragKind {
    File(PathBuf),
    Scroll,
}

struct FileDragPending {
    kind: FileDragKind,
    origin: Point,
    last: Point,
    threshold_met: bool,
    origin_locked: bool,
    awaiting_x11: bool,
}

struct SidebarResize {
    origin_x: f32,
    origin_width: f32,
    pending_origin: bool,
}

pub struct App {
    pub file_selector: FileSelector,
    pub menu: MainMenu,
    pub player: Player,
    pub search_thread: AbortHandle,
    pub dir_cache: DirCache,
    metadata_cache: MetadataCache,
    player_msgs: Option<futures::channel::mpsc::UnboundedReceiver<super::PlayerMsg>>,
    player_events_started: bool,
    drag_over: bool,
    dialog: Option<Dialog>,
    allowed_directories: AllowedDirectories,
    settings_open: bool,
    settings_first_run: bool,
    settings_error: Option<String>,
    waveform_hovered: bool,
    search_focused: bool,
    tag_search_focused: bool,
    sidebar_width: f32,
    sidebar_resize: Option<SidebarResize>,
    file_drag: Option<FileDragPending>,
    native_drag: NativeDrag,
    x11_drag_ready: bool,
    last_cursor: Point,
    modifiers: Modifiers,
}

pub struct DirCache(HashMap<PathBuf, Vec<PathBuf>>);

pub struct MetadataCache(HashMap<PathBuf, CachedMetadata>);

impl DirCache {
    fn new() -> DirCache {
        DirCache(HashMap::new())
    }

    fn insert(&mut self, k: PathBuf, v: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
        self.0.insert(k, v)
    }

    fn get(&self, k: &PathBuf) -> Option<&Vec<PathBuf>> {
        self.0.get(k)
    }

    fn contains_key(&self, k: &PathBuf) -> bool {
        self.0.contains_key(k)
    }

    fn retain(&mut self, mut keep: impl FnMut(&PathBuf) -> bool) {
        self.0.retain(|path, _| keep(path));
    }

    fn get_path() -> Option<std::path::PathBuf> {
        tundra_cache_dir().map(|mut cache_dir| {
            cache_dir.push("dir_cache");
            cache_dir.set_extension("bin");
            cache_dir
        })
    }

    fn get_dir_cache() -> DirCache {
        match DirCache::get_path() {
            Some(dir_cache) => match std::fs::read(dir_cache) {
                Ok(s) => bincode::deserialize(&s).map_or(DirCache::new(), DirCache),
                Err(_) => DirCache::new(),
            },
            None => DirCache::new(),
        }
    }

    fn persist(&self) {
        let Some(dir_cache) = DirCache::get_path() else {
            return;
        };
        let Ok(bytes) = bincode::serialize(&self.0) else {
            eprintln!("Failed to serialize directory cache");
            return;
        };
        if let Err(err) = std::fs::write(dir_cache, bytes) {
            eprintln!("Failed to write directory cache: {err}");
        }
    }
}

fn tundra_cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|mut cache_dir| {
        cache_dir.push("tundra");
        let _ = std::fs::create_dir_all(&cache_dir);
        cache_dir
    })
}

fn cache_file(name: &str) -> Option<PathBuf> {
    tundra_cache_dir().map(|mut path| {
        path.push(name);
        path
    })
}

impl MetadataCache {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn get_path() -> Option<std::path::PathBuf> {
        cache_file("metadata_cache_v4.bin")
    }

    fn load() -> Self {
        match MetadataCache::get_path() {
            Some(path) => match std::fs::read(path) {
                Ok(bytes) => bincode::deserialize::<HashMap<PathBuf, CachedMetadata>>(&bytes)
                    .map(MetadataCache)
                    .unwrap_or_else(|_| MetadataCache::new()),
                Err(_) => MetadataCache::new(),
            },
            None => MetadataCache::new(),
        }
    }

    fn persist(&self) {
        let Some(path) = MetadataCache::get_path() else {
            return;
        };
        let persistable: HashMap<PathBuf, CachedMetadata> = self
            .0
            .iter()
            .filter(|(_, cached)| cached.mtime_secs != 0)
            .map(|(path, cached)| (path.clone(), cached.clone()))
            .collect();
        let Ok(bytes) = bincode::serialize(&persistable) else {
            eprintln!("Failed to serialize metadata cache");
            return;
        };
        if let Err(err) = std::fs::write(path, bytes) {
            eprintln!("Failed to write metadata cache: {err}");
        }
    }

    fn snapshot(&self) -> Arc<HashMap<PathBuf, CachedMetadata>> {
        Arc::new(self.0.clone())
    }

    fn merge(&mut self, entries: HashMap<PathBuf, CachedMetadata>) {
        if entries.is_empty() {
            return;
        }
        self.0.extend(entries);
        self.persist();
    }

    fn retain(&mut self, mut keep: impl FnMut(&PathBuf) -> bool) {
        self.0.retain(|path, _| keep(path));
        self.persist();
    }
}

fn load_cached_f32(name: &str, default: f32, clamp: impl Fn(f32) -> f32) -> f32 {
    let Some(path) = cache_file(name) else {
        return default;
    };
    match std::fs::read(path) {
        Ok(bytes) => bincode::deserialize::<f32>(&bytes)
            .ok()
            .filter(|value| value.is_finite())
            .map(clamp)
            .unwrap_or(default),
        Err(_) => default,
    }
}

fn persist_cached_f32(name: &str, value: f32, label: &str) {
    let Some(path) = cache_file(name) else {
        return;
    };
    let Ok(bytes) = bincode::serialize(&value) else {
        eprintln!("Failed to serialize {label}");
        return;
    };
    if let Err(err) = std::fs::write(path, bytes) {
        eprintln!("Failed to write {label}: {err}");
    }
}

struct SidebarSettings;

impl SidebarSettings {
    fn load() -> f32 {
        load_cached_f32(
            "sidebar_width.bin",
            DEFAULT_SIDEBAR_WIDTH,
            |width| width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH),
        )
    }

    fn persist(width: f32) {
        let width = width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        persist_cached_f32("sidebar_width.bin", width, "sidebar width");
    }
}

struct VolumeSettings;

impl VolumeSettings {
    fn load() -> f32 {
        load_cached_f32("volume.bin", 1.0, clamp_volume)
    }

    fn persist(volume: f32) {
        persist_cached_f32("volume.bin", clamp_volume(volume), "volume");
    }
}

pub fn app() {
    iced::application(App::default, App::update, App::view)
        .title(App::title)
        .antialiasing(true)
        .font(ICED_AW_FONT_BYTES)
        .subscription(App::subscription)
        .run()
        .unwrap()
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
        let (player, player_msgs) = Player::new(VolumeSettings::load());
        let search_thread = AbortHandle::new_pair().0;
        let dir_cache = DirCache::get_dir_cache();
        let metadata_cache = MetadataCache::load();
        let mut app = App {
            file_selector,
            menu,
            player,
            search_thread,
            dir_cache,
            metadata_cache,
            player_msgs: Some(player_msgs),
            player_events_started: false,
            drag_over: false,
            dialog: None,
            allowed_directories,
            settings_open,
            settings_first_run,
            settings_error: None,
            waveform_hovered: false,
            search_focused: false,
            tag_search_focused: false,
            sidebar_width: SidebarSettings::load(),
            sidebar_resize: None,
            file_drag: None,
            native_drag: NativeDrag::new(),
            x11_drag_ready: cfg!(any(windows, target_os = "macos")),
            last_cursor: Point::ORIGIN,
            modifiers: Modifiers::default(),
        };
        if !app.allowed_directories.is_empty() {
            app.prune_caches();
        }
        app
    }
}

fn tag_search_can_autocomplete(input: &str) -> bool {
    !input.contains(':')
        && !input.trim().is_empty()
        && tag_field_best_match(input).is_some()
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
            Subscription::run_with((), |_| {
                stream::unfold((), |()| async {
                    async_io::Timer::after(Duration::from_millis(33)).await;
                    Some((Message::PlaybackTick, ()))
                })
            })
        } else {
            Subscription::none()
        };

        let transport_keys = if state.player.waveform.is_some()
            && state.dialog.is_none()
            && !state.settings_open
            && !state.search_focused
            && !state.tag_search_focused
            && state.waveform_hovered
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

        let file_drag = if state.sidebar_resize.is_none()
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

        Subscription::batch([
            file_events,
            playback_tick,
            waveform_keys,
            transport_keys,
            tag_autocomplete_keys,
            waveform_spring,
            sidebar_resize,
            file_drag,
            file_drag_tick,
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

    fn open_path(&mut self, path: &Path) -> Task<Message> {
        if path.is_dir() {
            return self.navigate_directory(path.to_path_buf());
        } else if is_audio(path) {
            self.play_audio(path)
        } else {
            Task::none()
        }
    }

    fn navigate_directory(&mut self, dir: PathBuf) -> Task<Message> {
        self.search_focused = false;
        self.tag_search_focused = false;
        self.file_selector.reload_directory(&dir);
        if self.allowed_directories.contains_path(&dir) && !self.dir_cache.contains_key(&dir) {
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
                self.file_selector.selected_file = self
                    .file_selector
                    .file_list
                    .iter()
                    .position(|entry| entry.file_path == file_path);
                self.ensure_player_events()
            }
            Err(err) => {
                self.show_error(err);
                self.file_selector.selected_file = None;
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
                return Task::perform(recv.into_future(), |x| {
                    Message::PlayerMsg((x.0, Arc::new(x.1)))
                });
            }
        }
        Task::none()
    }

    fn ensure_x11_drag() -> Task<Message> {
        window::latest().then(|id| match id {
            Some(id) => window::run(id, |window| crate::drag_out::x11_window_id(window))
                .map(Message::X11WindowId),
            None => Task::none(),
        })
    }

    #[cfg(any(windows, target_os = "macos"))]
    fn begin_platform_drag(path: PathBuf) -> Task<Message> {
        window::latest().then(move |id| match id {
            Some(id) => window::run(id, move |window| {
                crate::drag_out::start_blocking(window, path)
            })
            .map(Message::FileDragCompleted),
            None => Task::none(),
        })
    }

    fn start_file_drag(&mut self, path: PathBuf) -> Task<Message> {
        let canonical = match std::fs::canonicalize(&path) {
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
        String::from("Tundra Sample Browser")
    }

    fn reset_file_list(&mut self) {
        let (file_list, list_error) = FileList::list_buttons(&self.file_selector.current_dir);
        self.file_selector.file_list = file_list;
        self.file_selector.list_error = list_error;
    }

    fn prune_caches(&mut self) {
        let allowed = self.allowed_directories.clone();
        self.dir_cache
            .retain(|path| allowed.contains_path(path));
        self.dir_cache.persist();
        self.metadata_cache
            .retain(|path| allowed.contains_path(path));
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

    fn allowed_search_paths(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut paths = Vec::new();
        let mut missing_roots = Vec::new();
        for root in self.allowed_directories.roots() {
            if let Some(cached) = self.dir_cache.get(root) {
                paths.extend(cached.iter().cloned());
            } else {
                missing_roots.push(root.clone());
            }
        }
        paths.sort();
        paths.dedup();
        (paths, missing_roots)
    }

    fn start_file_search(&mut self) -> Task<Message> {
        self.search_thread.abort();
        let file_query = self.file_selector.search_value.clone();
        let tag_filters = self.file_selector.tag_filters.clone();
        let case_sensitive = self.file_selector.search_case_sensitive;

        if file_query.len() <= 2 && tag_filters.is_empty() {
            self.reset_file_list();
            return Task::none();
        }

        if self.allowed_directories.is_empty() {
            self.reset_file_list();
            return Task::none();
        }

        let (paths, missing_roots) = self.allowed_search_paths();
        let (abort_handle, abort_reg) = AbortHandle::new_pair();
        self.search_thread = abort_handle;
        let metadata = self.metadata_cache.snapshot();

        if missing_roots.is_empty() {
            return Task::perform(
                Abortable::new(
                    async move {
                        filter_search_paths(
                            200,
                            paths,
                            file_query,
                            tag_filters,
                            case_sensitive,
                            metadata,
                        )
                        .await
                    },
                    abort_reg,
                ),
                Message::SearchCompleted,
            );
        }

        Task::perform(
            Abortable::new(
                async move {
                    let mut paths = paths;
                    let mut cached_roots = HashMap::new();
                    for root in missing_roots {
                        let children = walk_directory(&root);
                        paths.extend(children.iter().cloned());
                        cached_roots.insert(root, children);
                    }
                    paths.sort();
                    paths.dedup();
                    let mut result = filter_search_paths(
                        300,
                        paths,
                        file_query,
                        tag_filters,
                        case_sensitive,
                        metadata,
                    )
                    .await;
                    result.cached_roots = cached_roots;
                    result
                },
                abort_reg,
            ),
            Message::SearchCompleted,
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectedFile(selected_file) => {
                self.search_focused = false;
                match &selected_file {
                Some(file_path) => self.open_path(file_path),
                None => {
                    self.player.clear_waveform();
                    Task::none()
                }
            }
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
                            &["flac", "wav", "mp3", "ogg"],
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

            Message::About => {
                self.dialog = Some(Dialog::about(format!(
                    "Tundra {}. FLAC, WAV, MP3, and OGG. \
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
                self.dialog = None;
                self.settings_open = true;
                self.settings_error = None;
                Task::none()
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
                let warm = self.warm_allowed_caches();
                let search = if self.file_selector.search_active() {
                    self.start_file_search()
                } else {
                    Task::none()
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
                                return Task::none();
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
                if self.file_selector.search_active() {
                    self.start_file_search()
                } else {
                    Task::none()
                }
            }

            Message::ChangeDirectory(parent_dir) => self.navigate_directory(parent_dir),

            Message::Search(search_str) => {
                self.search_focused = true;
                self.tag_search_focused = false;
                self.file_selector.search_value = search_str;
                self.start_file_search()
            }

            Message::ToggleSearchCaseSensitive => {
                self.file_selector.search_case_sensitive =
                    !self.file_selector.search_case_sensitive;
                self.start_file_search()
            }

            Message::SearchFocused(focused) => {
                self.search_focused = focused;
                if focused {
                    self.tag_search_focused = false;
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

            Message::SearchCompleted(file_list_res) => {
                if let Ok(result) = file_list_res {
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
                    self.file_selector.file_list = result
                        .paths
                        .iter()
                        .map(|x| FileButton::new(x.to_path_buf(), &self.file_selector.current_dir))
                        .collect();
                    self.file_selector.list_error = None;
                    self.metadata_cache.merge(result.new_metadata);
                }
                Task::none()
            }

            Message::TogglePlaying => {
                if self
                    .player
                    .controls
                    .is_playing
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.player.pause();
                } else {
                    self.player.play();
                }
                Task::none()
            }

            Message::StopPlayback => {
                self.player.stop();
                Task::none()
            }

            Message::InsertDircache((parent_dir, children)) => {
                if !self.allowed_directories.contains_path(&parent_dir) {
                    return Task::none();
                }
                self.dir_cache.insert(parent_dir, children.clone());
                self.dir_cache.persist();
                let metadata = self.metadata_cache.snapshot();
                let search_task = if self.file_selector.search_active() {
                    self.start_file_search()
                } else {
                    Task::none()
                };
                Task::batch([
                    Task::perform(
                        async move { index_paths(&children, metadata) },
                        Message::MetadataIndexed,
                    ),
                    search_task,
                ])
            }

            Message::MetadataIndexed(new_metadata) => {
                self.metadata_cache.merge(new_metadata);
                Task::none()
            }

            Message::InvalidateDircache => {
                self.dir_cache = DirCache::new();
                self.dir_cache.persist();
                self.metadata_cache = MetadataCache::new();
                self.metadata_cache.persist();
                let warm = self.warm_allowed_caches();
                let search = if self.file_selector.search_active() {
                    self.start_file_search()
                } else {
                    Task::none()
                };
                Task::batch([warm, search])
            }

            Message::PlayerMsg((msg, recv)) => {
                match msg {
                    Some(PlayerMsg::PlayingStored) => (),
                    Some(PlayerMsg::SinkEmpty) => self.player.pause(),
                    Some(PlayerMsg::StreamFailed) => {
                        self.show_error(
                            "Audio output unavailable. Check your sound device.".into(),
                        );
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
                    view.zoom_in(waveform.samples.len());
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformZoomOut => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    view.zoom_out(waveform.samples.len());
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformHoverChanged(hovered) => {
                self.waveform_hovered = hovered;
                if hovered {
                    self.search_focused = false;
                }
                Task::none()
            }

            Message::WaveformKey(key) => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    if WaveFormView::apply_key(&mut view, &key, waveform.samples.len()) {
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

            Message::FileDragPress(path) => {
                if self.modifiers.shift() {
                    self.file_drag = Some(FileDragPending {
                        kind: FileDragKind::Scroll,
                        origin: self.last_cursor,
                        last: self.last_cursor,
                        threshold_met: true,
                        origin_locked: true,
                        awaiting_x11: false,
                    });
                    return Task::none();
                }
                self.file_drag = Some(FileDragPending {
                    kind: FileDragKind::File(path),
                    origin: self.last_cursor,
                    last: self.last_cursor,
                    threshold_met: false,
                    origin_locked: false,
                    awaiting_x11: false,
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
                        if drag.threshold_met || drag.awaiting_x11 {
                            return Task::none();
                        }
                        let dx = point.x - drag.origin.x;
                        let dy = point.y - drag.origin.y;
                        if dx * dx + dy * dy < FILE_DRAG_THRESHOLD * FILE_DRAG_THRESHOLD {
                            return Task::none();
                        }
                        let path = path.clone();
                        if self.x11_drag_ready {
                            drag.threshold_met = true;
                            self.start_file_drag(path)
                        } else {
                            drag.awaiting_x11 = true;
                            Self::ensure_x11_drag()
                        }
                    }
                }
            }

            Message::FileDragRelease => {
                let mut task = Task::none();
                let click_to_open = self.file_drag.as_ref().and_then(|drag| {
                    if self.native_drag.is_active() || drag.threshold_met || drag.awaiting_x11 {
                        return None;
                    }
                    if let FileDragKind::File(path) = &drag.kind {
                        Some(path.clone())
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

            Message::X11WindowId(window_id) => {
                let Some(id) = window_id else {
                    self.x11_drag_ready = false;
                    self.show_notice(drag_out_notice(
                        "Drag-out from the file list requires X11 and is unavailable on native Wayland.",
                    ));
                    self.file_drag = None;
                    return Task::none();
                };
                match self.native_drag.init_with_window_id(id) {
                    Ok(()) => {
                        self.x11_drag_ready = true;
                    }
                    Err(err) => {
                        self.x11_drag_ready = false;
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
                    if !drag.awaiting_x11 {
                        return Task::none();
                    }
                    let path = match &drag.kind {
                        FileDragKind::File(path) => path.clone(),
                        FileDragKind::Scroll => {
                            drag.awaiting_x11 = false;
                            return Task::none();
                        }
                    };
                    drag.awaiting_x11 = false;
                    drag.threshold_met = true;
                    path
                };
                self.start_file_drag(path)
            }

            Message::CursorMoved(point) => {
                self.last_cursor = point;
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

            Message::Seek(p) => {
                self.player.begin_scrub(p);
                Task::none()
            }

            Message::SeekCommit => {
                if let Some(seekbar) = &self.player.controls.seekbar {
                    self.player.seek(seekbar.seeking);
                }
                Task::none()
            }

            Message::VolumeChanged(volume) => {
                self.player.set_volume(volume);
                Task::none()
            }

            Message::VolumeCommit => {
                VolumeSettings::persist(self.player.controls.volume);
                Task::none()
            }

            Message::WaveformSeek(progress) => {
                self.player.seek(progress);
                Task::none()
            }

            Message::WaveformCopyName => {
                if let Some(path) = &self.player.current_file
                    && let Some(name) = path.file_name().and_then(|name| name.to_str())
                {
                    return iced::clipboard::write(name.to_owned());
                }
                Task::none()
            }

            Message::WaveformCopyPath => {
                if let Some(path) = &self.player.current_file {
                    return iced::clipboard::write(path.display().to_string());
                }
                Task::none()
            }

            Message::WaveformRevealInFileManager => {
                if let Some(path) = &self.player.current_file {
                    reveal_in_file_manager(path);
                }
                Task::none()
            }

            Message::FileCopyName(path) => {
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    return iced::clipboard::write(name.to_owned());
                }
                Task::none()
            }

            Message::FileCopyPath(path) => iced::clipboard::write(path.display().to_string()),

            Message::FileRevealInFileManager(path) => {
                reveal_in_file_manager(&path);
                Task::none()
            }

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
                SidebarSettings::persist(self.sidebar_width);
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

    fn dialog_view(dialog: &Dialog) -> Element<'_, Message> {
        opaque(
            container(
                column![
                    text(&dialog.title).size(18),
                    text(&dialog.body)
                        .size(14)
                        .width(Length::Fill),
                    row![
                        Space::new().width(Length::Fill),
                        button(text("OK")).on_press(Message::DismissDialog),
                    ]
                    .align_y(iced::Alignment::Center),
                ]
                .spacing(12)
                .padding(16)
                .width(Length::Fixed(440.0)),
            )
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

    fn with_settings<'a>(
        base: Element<'a, Message>,
        allowed: &[PathBuf],
        first_run: bool,
        error: Option<String>,
    ) -> Element<'a, Message> {
        stack![
            base,
            opaque(
                container(center(settings::settings_view(allowed, first_run, error))).style(
                    |_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
                        ..Default::default()
                    },
                ),
            ),
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn with_dialog<'a>(base: Element<'a, Message>, dialog: &'a Dialog) -> Element<'a, Message> {
        stack![
            base,
            opaque(
                mouse_area(
                    center(Self::dialog_view(dialog)).style(|_theme| container::Style {
                        background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.55).into()),
                        ..Default::default()
                    }),
                )
                .on_press(Message::DismissDialog)
            )
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    pub fn view(&self) -> Element<'_, Message> {
        let menu = self.menu.view();

        let file_selector = container(self.file_selector.view())
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

        let player = container(if self.drag_over {
            stack![
                self.player.view(),
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
            stack![self.player.view()]
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

        let layout = column![menu, workspace]
            .width(Length::Fill)
            .height(Length::Fill);

        let layout: Element<_> = if self.settings_open {
            Self::with_settings(
                layout.into(),
                self.allowed_directories.roots(),
                self.settings_first_run,
                self.settings_error.clone(),
            )
        } else if let Some(dialog) = &self.dialog {
            Self::with_dialog(layout.into(), dialog)
        } else {
            layout.into()
        };

        layout
    }
}
