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
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub fn run() {
    iced::application(App::boot, App::update, App::view)
        .title(App::title)
        .antialiasing(true)
        .font(iced_aw::ICED_AW_FONT_BYTES)
        .subscription(App::subscription)
        .level(prefs::window_level(prefs::load_always_on_top()))
        .decorations(false)
        .resizable(true)
        .exit_on_close_request(false)
        .run()
        .expect("the UI event loop failed")
}

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

impl App {
    fn new() -> Self {
        let allowed_directories = AllowedDirectories::load();
        let settings_first_run = allowed_directories.is_empty();
        let current_dir = allowed_directories
            .startup_directory()
            .unwrap_or_else(startup_directory);
        App {
            file_selector: FileSelector::new(&current_dir),
            player: Player::new(prefs::load_volume(), prefs::load_looping()),
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
            modal: if settings_first_run {
                Modal::Settings
            } else {
                Modal::None
            },
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
            sidebar_width: prefs::load_sidebar_width(),
            window_maximized: false,
            always_on_top: prefs::load_always_on_top(),
        }
    }

    fn boot() -> (Self, Task<Message>) {
        let mut app = Self::new();
        let allowed = app.allowed_directories.clone();
        let (is_playing, looping) = (
            Arc::clone(&app.player.controls.is_playing),
            Arc::clone(&app.player.controls.looping),
        );
        let volume = app.player.controls.volume;
        let tasks = [
            Task::perform(run_blocking(move || load_startup_caches(allowed)), |caches| {
                Message::StartupCachesReady(caches.expect("loading caches panicked"))
            }),
            Task::perform(
                run_blocking(move || PlayerWorker::spawn(is_playing, looping, volume)),
                |spawned| {
                    let (worker, events) = spawned.expect("starting the audio thread panicked");
                    Message::PlayerWorkerReady(worker, Arc::new(events))
                },
            ),
            app.open_pending_launch(),
            on_window(|id| window::is_maximized(id).map(|maximized| WindowMsg::MaximizedChanged(maximized).into())),
        ];
        (app, Task::batch(tasks))
    }

    pub fn title(&self) -> String {
        window_title(self.current_file_name().as_deref())
    }

    fn current_file_name(&self) -> Option<String> {
        self.player
            .current_file
            .as_deref()
            .and_then(crate::path_util::file_name_lossy)
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::CursorMoved(point) => self.cursor_moved(point),
            Message::MouseReleased => self.mouse_released(),
            Message::KeyPressed(key, modifiers) => self.key_pressed(key, modifiers),
            Message::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers;
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_modifiers(modifiers);
                }
                Task::none()
            }
            Message::FileDropped(path) => {
                self.drag_over = false;
                self.open_path(&path)
            }
            Message::FileHovered(path) => {
                self.drag_over = crate::metadata::is_audio(&path) || path.is_dir();
                Task::none()
            }
            Message::FilesHoverLeft => {
                self.drag_over = false;
                Task::none()
            }

            Message::FileListSelect { index, shift, control } => self.select_file_row(index, shift, control),
            Message::FileListScrolled(viewport) => {
                self.file_selector.list_scroll_offset = viewport.absolute_offset().y;
                self.file_selector.list_viewport_height = viewport.bounds().height;
                Task::none()
            }
            Message::FileListScrollbarPress {
                track_y,
                track_top,
                track_height,
            } => self.press_scrollbar(track_y, track_top, track_height),
            Message::FileListHoverChanged(hovered) => {
                self.file_list_focused &= hovered;
                Task::none()
            }
            Message::FileRowHover(index) => {
                self.file_selector.filter_focus = Default::default();
                self.file_selector.hovered_file = Some(index);
                Task::none()
            }
            Message::FileRowLeave => {
                self.file_selector.hovered_file = None;
                Task::none()
            }
            Message::FileCopyName(path) => copy_name(&path),
            Message::FileCopyPath(path) => copy_path(&path),
            Message::FileRevealInFileManager(path) => reveal(&path),
            Message::ToggleFavorite(path) => self.toggle_favorite(&path),
            Message::ChangeDirectory(dir) => self.navigate_directory(dir),
            Message::OpenFolder => Task::perform(
                pick_folder(self.file_selector.current_dir.clone()),
                Message::FolderPicked,
            ),
            Message::OpenFile => Task::perform(
                pick_audio_file(self.file_selector.current_dir.clone()),
                Message::FilePicked,
            ),
            Message::FolderPicked(folder) => folder.map_or_else(Task::none, |dir| self.navigate_directory(dir)),
            Message::FilePicked(file) => file.map_or_else(Task::none, |path| self.open_path(&path)),
            Message::GoHome => match self.allowed_directories.startup_directory().or_else(dirs::home_dir) {
                Some(home) => self.navigate_directory(home),
                None => Task::none(),
            },
            Message::RefreshDirectory => {
                let current_dir = self.file_selector.current_dir.clone();
                self.file_selector.reload_directory(&current_dir);
                self.start_file_search()
            }
            Message::InvalidateDircache => self.invalidate_caches(),
            Message::Filter(message) => self.update_filter(message),

            Message::FileDragPress { path, from_file_list } => self.press_file(path, from_file_list),
            Message::FileDragTick => self.tick_native_drag(),
            Message::FileDragCompleted(result) => {
                if let Err(err) = result {
                    self.show_notice(input::drag_out_notice(&format!("Drag failed: {err}.")));
                }
                self.file_drag = None;
                Task::none()
            }
            Message::DragWindowId(window_id) => self.drag_window_ready(window_id),
            Message::SidebarResizeStart => {
                self.sidebar_resize = Some(SidebarResize {
                    origin_x: None,
                    origin_width: self.sidebar_width,
                });
                Task::none()
            }

            Message::StartupCachesReady(caches) => self.startup_caches_ready(caches),
            Message::InsertDircache((dir, children)) => self.insert_walked_directory(dir, children),
            Message::WalkFailed(key) => {
                self.walks_in_progress.remove(&key);
                Task::none()
            }
            Message::MetadataIndexed(entries) => {
                self.metadata_cache.merge(entries);
                self.refresh_search_if_active()
            }
            Message::PlayerWorkerReady(worker, events) => {
                self.player.attach_worker(worker);
                let events =
                    Arc::try_unwrap(events).map_or_else(|_| Task::none(), |events| Task::run(events, Message::Player));
                Task::batch([events, self.open_pending_launch()])
            }

            Message::Player(event) => {
                self.player_event(event);
                Task::none()
            }
            Message::TogglePlaying => {
                self.player.toggle_playing();
                Task::none()
            }
            Message::ToggleLoop => {
                prefs::persist_looping(self.player.toggle_loop());
                Task::none()
            }
            Message::StopPlayback => {
                self.player.stop();
                Task::none()
            }
            Message::VolumeChanged(volume) => {
                self.player.set_volume(volume);
                Task::none()
            }
            Message::VolumeCommit => {
                prefs::persist_volume(self.player.controls.volume);
                Task::none()
            }
            Message::PlaybackTick => {
                self.player.sync_playback_ui();
                Task::none()
            }
            Message::Waveform(message) => self.update_waveform(message),

            Message::Window(message) => self.update_window(message),
            Message::SetAlwaysOnTop(always_on_top) => {
                self.always_on_top = always_on_top;
                prefs::persist_always_on_top(always_on_top);
                prefs::set_window_level(always_on_top)
            }
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
            Message::Quit => {
                self.dir_cache.flush();
                self.metadata_cache.flush();
                iced::exit()
            }
            Message::NoOp => Task::none(),

            Message::Settings(message) => self.update_settings(message),
            Message::AutoTag(message) => self.update_auto_tag(message),
            Message::TagEditor(message) => self.update_tag_editor(message),
            Message::BulkAutoTag(message) => self.update_bulk_auto_tag(message),
        }
    }

    fn player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::DeviceUnavailable => {
                self.show_error("Audio output unavailable. Check your sound device.".into());
            }
            PlayerEvent::Ended(id) if self.player.is_current_track(id) => self.player.on_ended(),
            PlayerEvent::Looped(id) if self.player.is_current_track(id) => self.player.set_progress(0.0),
            PlayerEvent::WaveformPeaksReady(id) if self.player.is_current_track(id) => {
                self.player.on_waveform_peaks_ready();
            }
            PlayerEvent::FileFailed(id, err) if self.player.is_current_track(id) => {
                self.show_error(format!("Couldn't play this file. {err}"));
            }
            // Events for a track that has since been replaced.
            PlayerEvent::Ended(_)
            | PlayerEvent::Looped(_)
            | PlayerEvent::WaveformPeaksReady(_)
            | PlayerEvent::FileFailed(..) => {}
        }
    }

    fn update_waveform(&mut self, message: WaveformMsg) -> Task<Message> {
        match message {
            WaveformMsg::Scrub(progress) => {
                self.waveform_scrubbing = true;
                self.last_scrub_progress = progress;
                self.player.controls.scrubbing = true;
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_ui_scrubbing(true);
                    waveform.set_scrub_progress(Some(progress));
                }
                self.player.set_progress(progress);
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
                    waveform.set_pan_active(true);
                }
            }
            WaveformMsg::PanEnded(view) => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_pan_active(false);
                    waveform.set_view(view);
                }
            }
            WaveformMsg::SpringTick => self.edit_waveform_view(|view, _| {
                view.spring_overscroll();
            }),
            WaveformMsg::ZoomIn => self.edit_waveform_view(WaveFormView::zoom_in),
            WaveformMsg::ZoomOut => self.edit_waveform_view(WaveFormView::zoom_out),
            WaveformMsg::Help => self.dialog = Some(Dialog::waveform_help()),
            WaveformMsg::HoverChanged(hovered) => self.waveform_hovered = hovered,
            WaveformMsg::CopyName => return self.with_current_file(copy_name),
            WaveformMsg::CopyPath => return self.with_current_file(copy_path),
            WaveformMsg::RevealInFileManager => return self.with_current_file(reveal),
            WaveformMsg::OpenAutoTag => {
                if let Some(path) = self.player.current_file.clone() {
                    return self.update_auto_tag(AutoTagMsg::OpenFor(path));
                }
            }
            WaveformMsg::EditTags => {
                if let Some(path) = self.player.current_file.clone() {
                    return self.update_tag_editor(TagEditorMsg::OpenFor(path));
                }
            }
        }
        Task::none()
    }

    /// Changes the loaded waveform's zoom and pan; `change` also gets its sample count.
    fn edit_waveform_view(&mut self, change: impl FnOnce(&mut WaveFormView, usize)) {
        if let Some(waveform) = &mut self.player.waveform {
            let mut view = waveform.view_state();
            change(&mut view, waveform.sample_count());
            waveform.set_view(view);
        }
    }

    fn finish_scrub(&mut self, progress: f64) {
        if !std::mem::take(&mut self.waveform_scrubbing) {
            return;
        }
        self.player.controls.scrubbing = false;
        if let Some(waveform) = &mut self.player.waveform {
            waveform.set_ui_scrubbing(false);
            waveform.set_scrub_progress(None);
        }
        self.player.seek(progress);
    }

    fn with_current_file(&self, action: impl FnOnce(&Path) -> Task<Message>) -> Task<Message> {
        self.player.current_file.as_deref().map_or_else(Task::none, action)
    }

    fn show_error(&mut self, message: String) {
        self.player.reset_on_error();
        self.dialog = Some(Dialog::error(message));
    }

    fn show_notice(&mut self, message: String) {
        self.dialog = Some(Dialog::notice(message));
    }
}

fn copy_name(path: &Path) -> Task<Message> {
    crate::path_util::file_name_lossy(path).map_or_else(Task::none, iced::clipboard::write)
}

fn copy_path(path: &Path) -> Task<Message> {
    iced::clipboard::write(path.to_string_lossy().into_owned())
}

fn reveal(path: &Path) -> Task<Message> {
    crate::platform::reveal_in_file_manager(path);
    Task::none()
}

async fn pick_folder(start_dir: PathBuf) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Select Folder")
        .set_directory(&start_dir)
        .pick_folder()
        .await
        .map(|folder| folder.path().to_path_buf())
}

async fn pick_audio_file(start_dir: PathBuf) -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .set_title("Select audio file")
        .set_directory(&start_dir)
        .add_filter("Audio", crate::metadata::AUDIO_EXTENSIONS)
        .pick_file()
        .await
        .map(|file| file.path().to_path_buf())
}
