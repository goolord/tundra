//! Application state and message routing.
//!
//! `update` only dispatches; each message group is handled in the file that
//! owns it: `input` (pointer, keyboard, drag-out, window frame), `library`
//! (navigation, search, caches), `modals` (settings, auto-tag, tag editor),
//! and `bulk_auto_tag`. `view` and `subscription` are read-only over the state.

mod bulk_auto_tag;
mod input;
mod library;
mod modals;
mod prefs;
mod subscription;
mod view;

use super::auto_tag::AutoTagState;
use super::bulk_auto_tag::BulkAutoTagState;
use super::dialog::Dialog;
use super::file_selector::FileSelector;
use super::menu::window_title;
use super::message::{AutoTagMsg, Message, TagEditorMsg, WaveformMsg, WindowMsg};
use super::player::Player;
use super::tag_editor::TagEditorState;
use super::waveform::WaveFormView;
use crate::drag_out::NativeDrag;
use crate::library::cache::{DirCache, MetadataCache, load_startup_caches};
use crate::library::{AllowedDirectories, FavoritesStore};
use crate::playback::{PlayerEvent, PlayerWorker};
use futures::channel::oneshot;
use futures::future::AbortHandle;
use iced::keyboard::Modifiers;
use iced::{Point, Task, window};
use input::{FileDrag, ScrollbarDrag, SidebarResize};
use prefs::on_window;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

pub fn run() {
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .antialiasing(true)
        .font(iced_aw::ICED_AW_FONT_BYTES)
        .subscription(App::subscription)
        .level(prefs::window_level(prefs::ALWAYS_ON_TOP.load()))
        .decorations(false)
        .resizable(true)
        .exit_on_close_request(false)
        .run()
        .expect("the UI event loop failed")
}

const ABOUT: &str = concat!(
    "Tundra ",
    env!("CARGO_PKG_VERSION"),
    ". FLAC, WAV, MP3, OGG, and AIFF. Drag samples from the file list into a DAW. ",
    "Drop onto the window on Windows, macOS, and X11. On Wayland, use File > Open File."
);

/// Which modal covers the workspace. Bulk auto-tag and message dialogs are tracked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modal {
    None,
    Settings,
    AutoTag,
    TagEditor,
}

pub struct App {
    file_selector: FileSelector,
    player: Player,
    dir_cache: DirCache,
    metadata_cache: MetadataCache,
    allowed_directories: AllowedDirectories,
    favorites: FavoritesStore,

    // Search and background walks.
    search_abort: AbortHandle,
    search_generation: u64,
    /// Directories being walked, so repeated requests share one walk.
    walks_in_progress: HashSet<PathBuf>,
    /// Whether `current_dir` is inside an allowed root, keyed by folder and root list.
    /// Resolving touches the filesystem and `view` asks every frame.
    search_enabled_memo: RefCell<Option<(PathBuf, Vec<PathBuf>, bool)>>,
    caches_ready: bool,
    /// File or folder the OS asked us to open, held until settings and audio are ready.
    pending_launch_path: Option<PathBuf>,

    // Modals.
    modal: Modal,
    dialog: Option<Dialog>,
    settings_first_run: bool,
    settings_error: Option<String>,
    auto_tag: AutoTagState,
    tag_editor: TagEditorState,
    bulk_auto_tag: BulkAutoTagState,

    // Pointer and keyboard.
    modifiers: Modifiers,
    last_cursor: Point,
    /// A file is dragged over the window from outside.
    drag_over: bool,
    waveform_hovered: bool,
    waveform_scrubbing: bool,
    last_scrub_progress: f64,
    /// The file list, not a filter input, gets unmodified key presses.
    file_list_focused: bool,
    file_drag: Option<FileDrag>,
    native_drag: NativeDrag,
    /// Drag-out is initialized (always on Windows and macOS; X11 needs the window id first).
    drag_ready: bool,
    sidebar_resize: Option<SidebarResize>,
    file_list_scrollbar_drag: Option<ScrollbarDrag>,
    /// Where a title bar press started, until it turns into a window drag.
    title_bar_press: Option<Point>,

    // Window and preferences.
    sidebar_width: f32,
    window_maximized: bool,
    always_on_top: bool,
}

/// Where to start browsing with no allowed folder: the working directory, else home.
fn startup_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|err| {
        eprintln!("Could not read current directory: {err}");
        dirs::home_dir().unwrap_or_else(std::env::temp_dir)
    })
}

/// Runs blocking work on its own thread. `Err` means it panicked.
async fn run_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Result<T, ()> {
    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    rx.await.map_err(|_| ())
}

/// Runs `work` on its own thread and reports it with `done`; `panicked` stands in for its result if it panics.
fn background<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    panicked: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) -> Message + Send + 'static,
) -> Task<Message> {
    Task::perform(run_blocking(work), move |result| done(result.unwrap_or_else(|()| panicked())))
}

impl App {
    fn new() -> Self {
        let allowed_directories = AllowedDirectories::load();
        let settings_first_run = allowed_directories.is_empty();
        let current_dir = allowed_directories.startup_directory().unwrap_or_else(startup_directory);
        App {
            file_selector: FileSelector::new(&current_dir),
            player: Player::new(prefs::VOLUME.load(), prefs::LOOPING.load()),
            dir_cache: DirCache::new(),
            metadata_cache: MetadataCache::new(),
            allowed_directories,
            favorites: FavoritesStore::load(),
            search_abort: AbortHandle::new_pair().0,
            search_generation: 0,
            walks_in_progress: HashSet::new(),
            search_enabled_memo: RefCell::default(),
            caches_ready: false,
            pending_launch_path: crate::launch::primary_open_target(&crate::launch::paths_from_args()),
            modal: if settings_first_run { Modal::Settings } else { Modal::None },
            dialog: None,
            settings_first_run,
            settings_error: None,
            auto_tag: AutoTagState::default(),
            tag_editor: TagEditorState::default(),
            bulk_auto_tag: BulkAutoTagState::default(),
            modifiers: Modifiers::default(),
            last_cursor: Point::ORIGIN,
            drag_over: false,
            waveform_hovered: false,
            waveform_scrubbing: false,
            last_scrub_progress: 0.0,
            file_list_focused: false,
            file_drag: None,
            native_drag: NativeDrag::new(),
            drag_ready: cfg!(any(windows, target_os = "macos")),
            sidebar_resize: None,
            file_list_scrollbar_drag: None,
            title_bar_press: None,
            sidebar_width: prefs::SIDEBAR_WIDTH.load(),
            window_maximized: false,
            always_on_top: prefs::ALWAYS_ON_TOP.load(),
        }
    }

    fn boot() -> (Self, Task<Message>) {
        let mut app = Self::new();
        let allowed = app.allowed_directories.clone();
        let (is_playing, looping) =
            (Arc::clone(&app.player.controls.is_playing), Arc::clone(&app.player.controls.looping));
        let volume = app.player.controls.volume;
        let tasks = [
            Task::perform(run_blocking(move || load_startup_caches(allowed)), |caches| {
                Message::StartupCachesReady(caches.expect("loading caches panicked"))
            }),
            Task::perform(run_blocking(move || PlayerWorker::spawn(is_playing, looping, volume)), |spawned| {
                let (worker, events) = spawned.expect("starting the audio thread panicked");
                Message::PlayerWorkerReady(worker, Arc::new(events))
            }),
            app.open_pending_launch(),
            on_window(|id| window::is_maximized(id).map(|maximized| WindowMsg::MaximizedChanged(maximized).into())),
        ];
        (app, Task::batch(tasks))
    }

    pub fn title(&self) -> String {
        window_title(self.current_file_name().as_deref())
    }

    fn current_file_name(&self) -> Option<String> {
        self.player.current_file.as_deref().and_then(crate::path_util::file_name_lossy)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CursorMoved(point) => return self.cursor_moved(point),
            Message::MouseReleased => return self.mouse_released(),
            Message::KeyPressed(key, modifiers) => return self.key_pressed(key, modifiers),
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.modifiers = modifiers;
                }
            }
            Message::FileDropped(path) => {
                self.drag_over = false;
                return self.open_path(&path);
            }
            Message::FileHovered(path) => self.drag_over = crate::metadata::is_audio(&path) || path.is_dir(),
            Message::FilesHoverLeft => self.drag_over = false,

            Message::FileListSelect(index) => return self.select_file_row(index),
            Message::FileListScrolled(viewport) => {
                self.file_selector.list_scroll_offset = viewport.absolute_offset().y;
                self.file_selector.list_viewport_height = viewport.bounds().height;
            }
            Message::FileListScrollbarPress { track_y, track_top, track_height } => {
                return self.press_scrollbar(track_y, track_top, track_height);
            }
            Message::FileListHoverChanged(hovered) => self.file_list_focused &= hovered,
            Message::FileRowHover(index) => {
                self.file_selector.filter_focus = Default::default();
                self.file_selector.hovered_file = Some(index);
            }
            Message::FileRowLeave => self.file_selector.hovered_file = None,
            Message::FileCopyName(path) => {
                return crate::path_util::file_name_lossy(&path).map_or_else(Task::none, iced::clipboard::write);
            }
            Message::FileCopyPath(path) => return iced::clipboard::write(crate::path_util::display_path(&path)),
            Message::FileRevealInFileManager(path) => crate::platform::reveal_in_file_manager(&path),
            Message::ToggleFavorite(path) => return self.toggle_favorite(&path),
            Message::ChangeDirectory(dir) => return self.navigate_directory(dir),
            Message::OpenFolder => return pick_folder(self.file_selector.current_dir.clone(), Message::FolderPicked),
            Message::OpenFile => return pick_audio_file(self.file_selector.current_dir.clone(), Message::FilePicked),
            Message::FolderPicked(Some(dir)) => return self.navigate_directory(dir),
            Message::FilePicked(Some(path)) => return self.open_path(&path),
            Message::FolderPicked(None) | Message::FilePicked(None) | Message::NoOp => {}
            Message::GoHome => {
                if let Some(home) = self.allowed_directories.startup_directory().or_else(dirs::home_dir) {
                    return self.navigate_directory(home);
                }
            }
            Message::RefreshDirectory => {
                let current_dir = self.file_selector.current_dir.clone();
                self.file_selector.reload_directory(&current_dir);
                return self.start_file_search();
            }
            Message::InvalidateDircache => return self.invalidate_caches(),
            Message::Filter(message) => return self.update_filter(message),

            Message::FileDragPress { path, from_file_list } => self.press_file(path, from_file_list),
            Message::FileDragTick => self.tick_native_drag(),
            Message::FileDragCompleted(Ok(())) => self.file_drag = None,
            Message::FileDragCompleted(Err(err)) => self.drag_failed(&format!("Drag failed: {err}.")),
            Message::DragWindowId(window_id) => return self.drag_window_ready(window_id),
            Message::SidebarResizeStart => {
                self.sidebar_resize = Some(SidebarResize { origin_x: None, origin_width: self.sidebar_width });
            }

            Message::StartupCachesReady(caches) => return self.startup_caches_ready(caches),
            Message::InsertDircache((dir, children)) => return self.insert_walked_directory(dir, children),
            Message::WalkFailed(key) => _ = self.walks_in_progress.remove(&key),
            Message::MetadataIndexed(entries) => {
                self.metadata_cache.merge(entries);
                return self.refresh_search_if_active();
            }
            Message::PlayerWorkerReady(worker, events) => {
                self.player.attach_worker(worker);
                let events =
                    Arc::try_unwrap(events).map_or_else(|_| Task::none(), |events| Task::run(events, Message::Player));
                return Task::batch([events, self.open_pending_launch()]);
            }

            Message::Player(event) => self.player_event(event),
            Message::TogglePlaying => self.player.toggle_playing(),
            Message::ToggleLoop => prefs::LOOPING.save(self.player.toggle_loop()),
            Message::StopPlayback => self.player.stop(),
            Message::VolumeChanged(volume) => self.player.set_volume(volume),
            Message::VolumeCommit => prefs::VOLUME.save(self.player.controls.volume),
            // The time label reads the shared playhead; the tick only rebuilds the view.
            Message::PlaybackTick => {}
            Message::Waveform(message) => return self.update_waveform(message),

            Message::Window(message) => return self.update_window(message),
            Message::SetAlwaysOnTop(always_on_top) => {
                self.always_on_top = always_on_top;
                prefs::ALWAYS_ON_TOP.save(always_on_top);
                return prefs::set_window_level(always_on_top);
            }
            Message::About => self.dialog = Some(Dialog::new("About Tundra", ABOUT.into())),
            Message::DismissDialog => self.dialog = None,
            Message::Quit => {
                self.dir_cache.flush();
                self.metadata_cache.flush();
                return iced::exit();
            }

            Message::Settings(message) => return self.update_settings(message),
            Message::AutoTag(message) => return self.update_auto_tag(message),
            Message::TagEditor(message) => return self.update_tag_editor(message),
            Message::BulkAutoTag(message) => return self.update_bulk_auto_tag(message),
        }
        Task::none()
    }

    /// Shift extends a click's selection; Ctrl (Cmd on macOS) toggles.
    fn click_modifiers(&self) -> (bool, bool) {
        (self.modifiers.shift(), self.modifiers.control() || self.modifiers.logo())
    }

    fn player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::DeviceUnavailable => {
                self.show_error("Audio output unavailable. Check your sound device.".into());
            }
            PlayerEvent::Ended(id) if self.player.is_current_track(id) => self.player.on_ended(),
            PlayerEvent::FileFailed(id, err) if self.player.is_current_track(id) => {
                self.show_error(format!("Couldn't play this file. {err}"));
            }
            // New peaks only need the redraw every update brings; the rest are for replaced tracks.
            PlayerEvent::Ended(_) | PlayerEvent::WaveformPeaksReady | PlayerEvent::FileFailed(..) => {}
        }
    }

    fn update_waveform(&mut self, message: WaveformMsg) -> Task<Message> {
        match message {
            WaveformMsg::Scrub(progress) => {
                self.last_scrub_progress = progress;
                self.set_scrubbing(Some(progress));
            }
            WaveformMsg::ScrubEnd(progress) => self.finish_scrub(progress),
            WaveformMsg::FileDragStart => {
                if let Some(path) = self.player.current_file.clone() {
                    self.file_drag = Some(FileDrag::file(path, false));
                }
            }
            WaveformMsg::ViewChanged(view) => self.edit_waveform_view(|current, _| *current = view),
            WaveformMsg::PanStarted => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.pan_active = true;
                }
            }
            WaveformMsg::PanEnded(view) => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.pan_active = false;
                    waveform.view = view;
                }
            }
            WaveformMsg::SpringTick => self.edit_waveform_view(|view, _| {
                view.spring_overscroll();
            }),
            WaveformMsg::ZoomIn => self.edit_waveform_view(WaveFormView::zoom_in),
            WaveformMsg::ZoomOut => self.edit_waveform_view(WaveFormView::zoom_out),
            WaveformMsg::Help => self.dialog = Some(Dialog::waveform_help()),
            WaveformMsg::HoverChanged(hovered) => self.waveform_hovered = hovered,
            WaveformMsg::CopyName => return self.on_current_file(Message::FileCopyName),
            WaveformMsg::CopyPath => return self.on_current_file(Message::FileCopyPath),
            WaveformMsg::RevealInFileManager => return self.on_current_file(Message::FileRevealInFileManager),
            WaveformMsg::OpenAutoTag => return self.on_current_file(|path| AutoTagMsg::OpenFor(path).into()),
            WaveformMsg::EditTags => return self.on_current_file(|path| TagEditorMsg::OpenFor(path).into()),
        }
        Task::none()
    }

    /// Changes the loaded waveform's zoom and pan; `change` also gets its sample count.
    fn edit_waveform_view(&mut self, change: impl FnOnce(&mut WaveFormView, usize)) {
        if let Some(waveform) = &mut self.player.waveform {
            let sample_count = waveform.sample_count();
            change(&mut waveform.view, sample_count);
        }
    }

    /// Starts or moves a scrub at `progress`, or ends it with `None`.
    fn set_scrubbing(&mut self, progress: Option<f64>) {
        let scrubbing = progress.is_some();
        self.waveform_scrubbing = scrubbing;
        self.player.controls.scrubbing = scrubbing;
        if let Some(waveform) = &mut self.player.waveform {
            waveform.ui_scrubbing.set(scrubbing);
            waveform.scrub_progress = progress;
        }
    }

    fn finish_scrub(&mut self, progress: f64) {
        if self.waveform_scrubbing {
            self.set_scrubbing(None);
            self.player.seek(progress);
        }
    }

    /// Handles `message` for the playing file, if there is one.
    fn on_current_file(&mut self, message: impl FnOnce(PathBuf) -> Message) -> Task<Message> {
        match self.player.current_file.clone() {
            Some(path) => self.update(message(path)),
            None => Task::none(),
        }
    }

    fn show_error(&mut self, message: String) {
        self.player.reset_on_error();
        self.dialog = Some(Dialog::new("Error", message));
    }

    fn show_notice(&mut self, message: String) {
        self.dialog = Some(Dialog::new("Notice", message));
    }
}

/// Asks for a folder, starting in `start_dir`.
fn pick_folder(start_dir: PathBuf, done: fn(Option<PathBuf>) -> Message) -> Task<Message> {
    let dialog = rfd::AsyncFileDialog::new().set_title("Select Folder");
    Task::perform(async move { dialog.set_directory(&start_dir).pick_folder().await }, move |folder| {
        done(folder.map(|folder| folder.path().to_path_buf()))
    })
}

/// Asks for an audio file, starting in `start_dir`.
fn pick_audio_file(start_dir: PathBuf, done: fn(Option<PathBuf>) -> Message) -> Task<Message> {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Select audio file")
        .add_filter("Audio", crate::metadata::AUDIO_EXTENSIONS);
    Task::perform(async move { dialog.set_directory(&start_dir).pick_file().await }, move |file| {
        done(file.map(|file| file.path().to_path_buf()))
    })
}
