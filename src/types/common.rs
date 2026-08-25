use futures::channel::mpsc::UnboundedReceiver;
use futures::future::Aborted;
use crate::metadata::SearchResult;
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::widget::{button, column, text, Button};
use iced::{Border, Element, Length, Padding, Shadow, Theme, theme};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

const RESOURCES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/resources");

static RESOURCE_PATHS: LazyLock<HashMap<&'static str, String>> = LazyLock::new(|| {
    [
        "pause.svg",
        "play.svg",
        "stop.svg",
        "up_chevron.svg",
        "folder-solid.svg",
        "music-solid.svg",
        "search-solid.svg",
    ]
    .into_iter()
    .map(|name| (name, format!("{RESOURCES}/{name}")))
    .collect()
});

pub fn resource_path(name: &str) -> &str {
    match RESOURCE_PATHS.get(name) {
        Some(path) => path.as_str(),
        None => {
            eprintln!("Unknown resource {name:?}; falling back to manifest path");
            &RESOURCE_PATHS["play.svg"]
        }
    }
}

#[derive(Debug, Clone)]
pub struct Dialog {
    pub title: String,
    pub body: String,
}

impl Dialog {
    pub fn about(body: String) -> Self {
        Self {
            title: "About Tundra".into(),
            body,
        }
    }

    pub fn notice(body: String) -> Self {
        Self {
            title: "Notice".into(),
            body,
        }
    }

    pub fn error(body: String) -> Self {
        Self {
            title: "Error".into(),
            body,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    SelectedFile(Option<PathBuf>),
    ChangeDirectory(PathBuf),
    Search(String),
    SearchFocused(bool),
    TagSearchInput(String),
    TagSearchSubmit,
    TagSearchAutocomplete,
    TagSearchFocused(bool),
    TagFilterRemove(crate::metadata::TagField),
    TagSuggestionSelect(crate::metadata::TagField),
    SearchCompleted(Result<SearchResult, Aborted>),
    MetadataIndexed(std::collections::HashMap<std::path::PathBuf, crate::metadata::CachedMetadata>),
    InsertDircache((PathBuf, Vec<PathBuf>)),
    InvalidateDircache,
    Seek(f64),
    SeekCommit,
    VolumeChanged(f32),
    VolumeCommit,
    WaveformSeek(f64),
    SidebarResizeStart,
    SidebarResizeMove(f32),
    SidebarResizeEnd,
    PlayerMsg((
        Option<super::PlayerMsg>,
        Arc<UnboundedReceiver<super::PlayerMsg>>,
    )),
    TogglePlaying,
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
    WaveformHoverChanged(bool),
    WaveformKey(iced::keyboard::Key),
    WaveformCopyName,
    WaveformCopyPath,
    WaveformRevealInFileManager,
    PlaybackTick,
    ModifiersChanged(iced::keyboard::Modifiers),
    FileDragPress(PathBuf),
    FileDragMove(iced::Point),
    FileDragRelease,
    FileDragTick,
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    FileDragCompleted(Result<(), String>),
    X11WindowId(Option<u32>),
    CursorMoved(iced::Point),
    FileRowHover(usize),
    FileRowLeave,
    FileCopyName(PathBuf),
    FileCopyPath(PathBuf),
    FileRevealInFileManager(PathBuf),
    OpenSettings,
    CloseSettings,
    PickAllowedDirectory,
    AllowedDirectoryPicked(Option<PathBuf>),
    RemoveAllowedDirectory(PathBuf),
    ToggleSearchCaseSensitive,
    ToggleSearchShowDirectories,
    OpenAutoTag,
    CloseAutoTag,
    AutoTagPickFile,
    AutoTagFilePicked(Option<PathBuf>),
    AutoTagRun,
    AutoTagCompleted(Result<crate::auto_tag::ClassificationResult, crate::auto_tag::ClassifyError>),
    AutoTagApply,
    ToggleAutoTagDetails,
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
}

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| ext == "flac" || ext == "wav" || ext == "mp3" || ext == "ogg")
}

pub fn is_hidden(entry: &Path) -> bool {
    match entry.file_name() {
        Some(s) => s.to_string_lossy().starts_with('.'),
        None => false,
    }
}

pub fn startup_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|err| {
        eprintln!("Could not read current directory: {err}");
        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
    })
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
    } else {
        "Open in File browser"
    }
}

pub fn drag_from_file_manager_hint() -> String {
    format!(
        "Right-click the file and choose \"{}\", then drag it from there.",
        file_manager_label()
    )
}

pub fn drag_out_notice(intro: impl AsRef<str>) -> String {
    format!("{} {}", intro.as_ref(), drag_from_file_manager_hint())
}

pub fn reveal_in_file_manager(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn();
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

pub fn context_menu_button<'a>(label: &'a str, message: Message) -> Button<'a, Message> {
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
) -> Element<'static, Message> {
    column![
        context_menu_button("Copy name", copy_name),
        context_menu_button("Copy full path", copy_path),
        context_menu_button(file_manager_label(), reveal),
    ]
    .spacing(2)
    .padding(Padding::from([4, 0]))
    .width(220)
    .into()
}
