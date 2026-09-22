//! Every event the UI handles. `App::update` routes each group to the file
//! under `app/` that owns it.

use super::bulk_auto_tag::BulkFileKey;
use super::waveform::WaveFormView;
use crate::auto_tag::{ClassificationResult, ClassifyError};
use crate::bulk_auto_tag::{BulkApplySummary, BulkScanSummary, ScanError};
use crate::library::cache::PersistedCaches;
use crate::library::search::SearchOutput;
use crate::metadata::{CachedMetadata, SavedTo, TagField};
use crate::playback::{PlayerEvent, PlayerWorker};
use futures::channel::mpsc::UnboundedReceiver;
use futures::future::Aborted;
use iced::Point;
use iced::keyboard::{Key, Modifiers};
use iced::widget::scrollable::Viewport;
use iced::window::Direction;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub enum Message {
    // Raw input, routed by `app/input.rs`.
    CursorMoved(Point),
    MouseReleased,
    KeyPressed(Key, Modifiers),
    ModifiersChanged(Modifiers),
    FileDropped(PathBuf),
    FileHovered(PathBuf),
    FilesHoverLeft,

    // File list and navigation (`app/library.rs`). Clicks read Shift/Ctrl from the app's modifiers.
    FileListSelect(usize),
    FileListScrolled(Viewport),
    FileListScrollbarPress {
        track_y: f32,
        track_top: f32,
        track_height: f32,
    },
    FileListHoverChanged(bool),
    FileRowHover(usize),
    FileRowLeave,
    FileCopyName(PathBuf),
    FileCopyPath(PathBuf),
    FileRevealInFileManager(PathBuf),
    ToggleFavorite(PathBuf),
    ChangeDirectory(PathBuf),
    OpenFolder,
    OpenFile,
    FolderPicked(Option<PathBuf>),
    FilePicked(Option<PathBuf>),
    GoHome,
    RefreshDirectory,
    InvalidateDircache,
    Filter(FilterMsg),

    // Dragging files out to other apps (`app/input.rs`).
    FileDragPress {
        path: PathBuf,
        from_file_list: bool,
    },
    FileDragTick,
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    FileDragCompleted(Result<(), String>),
    DragWindowId(Option<u32>),
    SidebarResizeStart,

    // Background work finishing (`app/library.rs`).
    StartupCachesReady(PersistedCaches),
    InsertDircache((PathBuf, Vec<PathBuf>)),
    /// A directory walk panicked; the key lets it be retried.
    WalkFailed(PathBuf),
    MetadataIndexed(HashMap<PathBuf, CachedMetadata>),
    PlayerWorkerReady(PlayerWorker, Arc<UnboundedReceiver<PlayerEvent>>),

    // Playback (`app/mod.rs`).
    Player(PlayerEvent),
    TogglePlaying,
    ToggleLoop,
    StopPlayback,
    VolumeChanged(f32),
    VolumeCommit,
    PlaybackTick,
    Waveform(WaveformMsg),

    // App chrome (`app/mod.rs`).
    Window(WindowMsg),
    SetAlwaysOnTop(bool),
    About,
    DismissDialog,
    Quit,
    /// Enables iced button hover styling where the bar handles clicks itself.
    NoOp,

    // Modals (`app/modals.rs`, `app/bulk_auto_tag.rs`).
    Settings(SettingsMsg),
    AutoTag(AutoTagMsg),
    TagEditor(TagEditorMsg),
    BulkAutoTag(BulkAutoTagMsg),
}

/// The file search box, tag filters, and search results.
#[derive(Debug, Clone)]
pub enum FilterMsg {
    Search(String),
    SearchFocused(bool),
    TagSearchInput(String),
    TagSearchSubmit,
    TagSearchFocused(bool),
    TagFilterRemove(TagField),
    TagSuggestionSelect(TagField),
    ToggleCaseSensitive,
    ToggleShowDirectories,
    ToggleFavoritesOnly,
    /// The search generation it ran for, and its result.
    SearchCompleted(u64, Result<SearchOutput, Aborted>),
}

#[derive(Debug, Clone)]
pub enum WaveformMsg {
    Scrub(f64),
    ScrubEnd(f64),
    FileDragStart,
    ViewChanged(WaveFormView),
    PanStarted,
    PanEnded(WaveFormView),
    SpringTick,
    ZoomIn,
    ZoomOut,
    Help,
    HoverChanged(bool),
    CopyName,
    CopyPath,
    RevealInFileManager,
    OpenAutoTag,
    EditTags,
}

/// The custom title bar and window frame (the OS decorations are off).
#[derive(Debug, Clone)]
pub enum WindowMsg {
    TitleBarPress,
    TitleBarRelease,
    Minimize,
    ToggleMaximize,
    MaximizedChanged(bool),
    SyncMaximized,
    Resize(Direction),
}

#[derive(Debug, Clone)]
pub enum SettingsMsg {
    Open,
    Close,
    PickDirectory,
    DirectoryPicked(Option<PathBuf>),
    RemoveDirectory(PathBuf),
}

#[derive(Debug, Clone)]
pub enum AutoTagMsg {
    /// Open for the file selected in the list.
    Open,
    OpenFor(PathBuf),
    Close,
    PickFile,
    FilePicked(Option<PathBuf>),
    Run,
    Completed(PathBuf, Result<ClassificationResult, ClassifyError>),
    Apply,
    Applied(PathBuf, String, Result<bool, String>),
    ToggleDetails,
}

#[derive(Debug, Clone)]
pub enum TagEditorMsg {
    OpenFor(PathBuf),
    Close,
    Input(TagField, String),
    Save,
    Saved(PathBuf, Result<SavedTo, String>),
}

/// Completions carry the generation of the job that produced them.
#[derive(Debug, Clone)]
pub enum BulkAutoTagMsg {
    Open,
    Close,
    PickDirectory,
    DirectoryPicked(Option<PathBuf>),
    RunScan,
    ProgressTick,
    ScanCompleted(u64, Result<BulkScanSummary, ScanError>),
    SetFileAccepted(BulkFileKey, bool),
    SelectFile(BulkFileKey),
    SelectDirectory(usize),
    SelectAll,
    ClearSelection,
    /// Check (true) or uncheck the selected files.
    CheckSelected(bool),
    CheckAll(bool),
    ToggleDirectoryExpanded(usize),
    ExpandAll(bool),
    Apply,
    ApplyCompleted(u64, BulkApplySummary),
}

macro_rules! nested {
    ($($variant:ident($inner:ty)),* $(,)?) => {
        $(impl From<$inner> for Message {
            fn from(message: $inner) -> Self {
                Message::$variant(message)
            }
        })*
    };
}

nested!(
    Filter(FilterMsg),
    Waveform(WaveformMsg),
    Window(WindowMsg),
    Settings(SettingsMsg),
    AutoTag(AutoTagMsg),
    TagEditor(TagEditorMsg),
    BulkAutoTag(BulkAutoTagMsg),
);
