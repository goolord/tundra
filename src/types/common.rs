use futures::channel::mpsc::UnboundedReceiver;
use futures::future::Aborted;
use crate::metadata::SearchResult;
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::widget::svg::Handle;
use iced::widget::{button, column, text, Button, Svg};
use iced::window::Direction;
use iced::{Border, Element, Length, Padding, Shadow, Theme, theme};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod embedded_resources {
    include!(concat!(env!("OUT_DIR"), "/embedded_resources.rs"));
}

pub fn resource_handle(name: &str) -> Handle {
    match embedded_resources::handle(name) {
        Some(handle) => handle,
        None => {
            debug_assert!(false, "unknown embedded resource: {name}");
            eprintln!("Unknown resource {name:?}; falling back to play.svg");
            embedded_resources::handle("play.svg")
                .expect("play.svg must be embedded at build time")
        }
    }
}

pub fn resource_svg(name: &str) -> Svg<'_> {
    Svg::new(resource_handle(name))
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub title: String,
    pub body: String,
    pub rows: Vec<(String, String)>,
}

impl Dialog {
    pub fn about(body: String) -> Self {
        Self {
            title: "About Tundra".into(),
            body,
            rows: Vec::new(),
        }
    }

    pub fn notice(body: String) -> Self {
        Self {
            title: "Notice".into(),
            body,
            rows: Vec::new(),
        }
    }

    pub fn waveform_help() -> Self {
        Self {
            title: "Waveform controls".into(),
            body: "Hover the waveform for +/− and arrow keys.".into(),
            rows: vec![
                ("Drag".into(), "Seek".into()),
                ("Ctrl+drag".into(), "Drag file".into()),
                ("Scroll".into(), "Zoom".into()),
                ("Shift+scroll".into(), "Pan".into()),
                ("Shift+drag".into(), "Pan".into()),
                ("Space".into(), "Play/pause".into()),
                ("+ / −".into(), "Zoom".into()),
                ("Left / Right".into(), "Pan".into()),
            ],
        }
    }

    pub fn error(body: String) -> Self {
        Self {
            title: "Error".into(),
            body,
            rows: Vec::new(),
        }
    }
}

/// Window / title-bar label: `Tundra` or `Tundra - {filename}`.
pub fn window_title(active_file: Option<&str>) -> String {
    match active_file {
        Some(name) => format!("Tundra - {name}"),
        None => "Tundra".into(),
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    FileListSelect {
        index: usize,
        shift: bool,
        control: bool,
    },
    ChangeDirectory(PathBuf),
    FileListScrolled(iced::widget::scrollable::Viewport),
    FileListScrollbarPress {
        track_y: f32,
        track_top: f32,
        track_height: f32,
    },
    FileListScrollbarDrag(iced::Point),
    FileListScrollbarRelease,
    Search(String),
    SearchFocused(bool),
    TagSearchInput(String),
    TagSearchSubmit,
    TagSearchAutocomplete,
    TagSearchFocused(bool),
    TagFilterRemove(crate::metadata::TagField),
    TagSuggestionSelect(crate::metadata::TagField),
    SearchCompleted {
        generation: u64,
        result: Result<SearchResult, Aborted>,
    },
    MetadataIndexed(std::collections::HashMap<std::path::PathBuf, crate::metadata::CachedMetadata>),
    StartupCachesReady(crate::metadata::PersistedCaches),
    PlayerWorkerReady(
        super::PlayerWorker,
        Arc<futures::channel::mpsc::UnboundedReceiver<super::PlayerMsg>>,
    ),
    InsertDircache((PathBuf, Vec<PathBuf>)),
    InvalidateDircache,
    VolumeChanged(f32),
    VolumeCommit,
    WaveformScrub(f64),
    WaveformScrubEnd(f64),
    WaveformScrubRelease,
    WaveformFileDragStart,
    SidebarResizeStart,
    SidebarResizeMove(f32),
    SidebarResizeEnd,
    PlayerMsg((
        Option<super::PlayerMsg>,
        Arc<UnboundedReceiver<super::PlayerMsg>>,
    )),
    TogglePlaying,
    ToggleLoop,
    StopPlayback,
    DismissDialog,
    OpenFolder,
    OpenFile,
    FolderPicked(Option<PathBuf>),
    FilePicked(Option<PathBuf>),
    GoHome,
    RefreshDirectory,
    Quit,
    About,
    FileDropped(PathBuf),
    FileHovered(PathBuf),
    FilesHoverLeft,
    WaveformViewChanged(super::WaveFormView),
    WaveformPanStarted,
    WaveformPanEnded(super::WaveFormView),
    WaveformSpringTick,
    WaveformZoomIn,
    WaveformZoomOut,
    WaveformHelp,
    WaveformHoverChanged(bool),
    ControlsHoverChanged(bool),
    WaveformKey(iced::keyboard::Key),
    WaveformCopyName,
    WaveformCopyPath,
    WaveformRevealInFileManager,
    WaveformOpenAutoTag,
    WaveformEditTags,
    PlaybackTick,
    ModifiersChanged(iced::keyboard::Modifiers),
    FileDragPress {
        path: PathBuf,
        from_file_list: bool,
    },
    FileDragMove(iced::Point),
    FileDragRelease,
    FileDragTick,
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    FileDragCompleted(Result<(), String>),
    DragWindowId(Option<u32>),
    CursorMoved(iced::Point),
    FileRowHover(usize),
    FileRowLeave,
    FileListHoverChanged(bool),
    FileCopyName(PathBuf),
    FileCopyPath(PathBuf),
    FileRevealInFileManager(PathBuf),
    OpenSettings,
    CloseSettings,
    SetAlwaysOnTop(bool),
    PickAllowedDirectory,
    AllowedDirectoryPicked(Option<PathBuf>),
    RemoveAllowedDirectory(PathBuf),
    ToggleSearchCaseSensitive,
    ToggleSearchShowDirectories,
    ToggleFavoritesOnly,
    ToggleFavorite(PathBuf),
    OpenAutoTag,
    OpenAutoTagFor(PathBuf),
    CloseAutoTag,
    AutoTagPickFile,
    AutoTagFilePicked(Option<PathBuf>),
    AutoTagRun,
    AutoTagCompleted(Result<crate::auto_tag::ClassificationResult, crate::auto_tag::ClassifyError>),
    AutoTagApply,
    ToggleAutoTagDetails,
    OpenTagEditorFor(PathBuf),
    CloseTagEditor,
    TagEditorInput(super::tag_editor::TagEditorField, String),
    TagEditorSave,
    OpenBulkAutoTag,
    CloseBulkAutoTag,
    BulkAutoTagPickDirectory,
    BulkAutoTagDirectoryPicked(Option<PathBuf>),
    BulkAutoTagRunScan,
    BulkAutoTagProgressTick,
    BulkAutoTagScanCompleted {
        generation: u64,
        result: Result<crate::bulk_auto_tag::BulkScanSummary, String>,
    },
    BulkAutoTagSetFileAccepted {
        dir_idx: usize,
        file_idx: usize,
        accepted: bool,
    },
    BulkAutoTagSelectFile {
        dir_idx: usize,
        file_idx: usize,
        shift: bool,
        control: bool,
    },
    BulkAutoTagSelectDirectory {
        dir_idx: usize,
        shift: bool,
        control: bool,
    },
    BulkAutoTagSelectAll,
    BulkAutoTagClearSelection,
    BulkAutoTagCheckSelected,
    BulkAutoTagUncheckSelected,
    BulkAutoTagAcceptAll,
    BulkAutoTagRejectAll,
    BulkAutoTagToggleDirectoryExpanded(usize),
    BulkAutoTagExpandAllDirectories,
    BulkAutoTagCollapseAllDirectories,
    BulkAutoTagApply,
    BulkAutoTagApplyCompleted {
        generation: u64,
        summary: crate::bulk_auto_tag::BulkApplySummary,
    },
    WindowTitleBarPress,
    WindowTitleBarRelease,
    WindowMinimize,
    WindowToggleMaximize,
    WindowMaximizedChanged(bool),
    SyncWindowMaximized,
    WindowResize(Direction),
    /// Enables iced button hover styling where the bar handles clicks itself.
    NoOp,
}

pub const AUDIO_EXTENSIONS: &[&str] = &["flac", "wav", "mp3", "ogg", "aiff", "aif"];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| AUDIO_EXTENSIONS.iter().any(|supported| ext == *supported))
}

pub fn is_hidden(entry: &Path) -> bool {
    if entry
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
    {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if let Ok(meta) = std::fs::metadata(entry) {
            return meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0;
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        const UF_HIDDEN: u32 = 0x8000;
        if let Ok(meta) = std::fs::metadata(entry) {
            return meta.st_flags() & UF_HIDDEN != 0;
        }
    }
    false
}

pub fn startup_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|err| {
        eprintln!("Could not read current directory: {err}");
        dirs::home_dir().unwrap_or_else(fallback_directory)
    })
}

fn fallback_directory() -> PathBuf {
    std::env::temp_dir()
}

pub fn truncate_path(path: &Path, max_chars: usize) -> String {
    let rendered = path.display().to_string();
    if rendered.chars().count() <= max_chars {
        return rendered;
    }
    let tail: String = rendered
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

pub fn file_manager_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Open in Explorer"
    } else if cfg!(target_os = "macos") {
        "Open in Finder"
    } else {
        "Open in File browser"
    }
}

pub fn drag_from_file_manager_hint() -> String {
    let gesture = if cfg!(target_os = "macos") {
        "Control-click or use a two-finger click"
    } else {
        "Right-click"
    };
    format!(
        "{gesture} the file and choose \"{}\", then drag it from there.",
        file_manager_label()
    )
}

pub fn drag_out_notice(intro: impl AsRef<str>) -> String {
    format!("{} {}", intro.as_ref(), drag_from_file_manager_hint())
}

pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("explorer");
        if path.is_dir() {
            command.arg(path);
        } else {
            command.raw_arg(format!("/select,\"{}\"", path.display()));
        }
        crate::path_util::hide_console(&mut command);
        let _ = command.spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("open");
        if path.is_file() {
            command.arg("-R");
        }
        let _ = command.arg(path).spawn();
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let open_path = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent().unwrap_or(path).to_path_buf()
        };
        let _ = std::process::Command::new("xdg-open")
            .arg(open_path)
            .spawn();
    }
}

pub fn context_menu_button(label: impl Into<String>, message: Message) -> Button<'static, Message> {
    let label = label.into();
    button(
        text(label)
            .size(14)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left),
    )
    .width(Length::Fill)
    .padding([4, 12])
    .style(|theme: &Theme, status| {
        let palette = theme.extended_palette();
        let base = ButtonStyle {
            text_color: palette.background.base.text,
            border: Border::default(),
            shadow: Shadow::default(),
            ..ButtonStyle::default()
        };
        match status {
            ButtonStatus::Active | ButtonStatus::Disabled => {
                base.with_background(palette.background.base.color)
            }
            ButtonStatus::Hovered => {
                base.with_background(palette.primary.weak.color.scale_alpha(0.35))
            }
            ButtonStatus::Pressed => {
                base.with_background(palette.primary.weak.color.scale_alpha(0.55))
            }
        }
    })
    .on_press(message)
}

pub fn context_menu_style(
    theme: &theme::Theme,
    _status: iced_aw::style::Status,
) -> iced_aw::style::context_menu::Style {
    let palette = theme.extended_palette();
    iced_aw::style::context_menu::Style {
        background: palette.background.base.color.into(),
    }
}

pub fn file_context_menu(
    copy_name: Message,
    copy_path: Message,
    reveal: Message,
    auto_tag: Option<Message>,
    favorite: Option<(String, Message)>,
    edit_tags: Option<Message>,
) -> Element<'static, Message> {
    let mut items = column![].spacing(2);
    if let Some(message) = auto_tag {
        items = items.push(context_menu_button("Auto-tag", message));
    }
    if let Some(message) = edit_tags {
        items = items.push(context_menu_button("Edit tags…", message));
    }
    if let Some((label, message)) = favorite {
        items = items.push(context_menu_button(&label, message));
    }
    items = items
        .push(context_menu_button("Copy name", copy_name))
        .push(context_menu_button("Copy full path", copy_path))
        .push(context_menu_button(file_manager_label(), reveal));
    items
        .padding(Padding::from([4, 0]))
        .width(220)
        .into()
}
