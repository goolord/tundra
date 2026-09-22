//! The Bulk Auto Tag modal: pick a folder, scan it, review proposals grouped
//! by directory, then apply the checked ones.

use super::message::{BulkAutoTagMsg, Message};
use super::selection::Selection;
use super::style::{self, ACCENT};
use super::widgets::{icon, modal_button, modal_footer, modal_shell, selection_stripe, spacer};
use crate::bulk_auto_tag::{
    BulkApplySummary, BulkDirGroup, BulkFileProposal, BulkScanProgress, BulkScanSummary, actionable_and_accepted,
};
use crate::path_util::truncate_path;
use iced::widget::{Column, Row, Text, button, checkbox, column, container, progress_bar, row, scrollable, text};
use iced::{Alignment, Color, Element, Length, Theme};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const CONF_HIGH: Color = Color::from_rgb8(0x5c, 0xb8, 0x85);
const CONF_MED: Color = Color::from_rgb8(0xd4, 0xa5, 0x4a);
const CONF_LOW: Color = Color::from_rgb8(0x9a, 0x9a, 0xa8);
const REVIEW_MODAL_HEIGHT: f32 = 600.0;
const FILE_INDENT: f32 = 18.0;
const ROW_HEIGHT: f32 = 36.0;
const SELECTION_STRIPE_WIDTH: f32 = 3.0;
const CHECKBOX_COLUMN_WIDTH: f32 = 28.0;
const EXPAND_TOGGLE_WIDTH: f32 = 32.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BulkAutoTagPhase {
    #[default]
    PickDirectory,
    Running,
    Review,
    Applying,
    Done,
}

/// A scan or apply running in the background.
#[derive(Debug, Clone)]
pub struct BulkJob {
    /// Completions carrying another generation belong to an abandoned job.
    pub generation: u64,
    pub progress: Arc<BulkScanProgress>,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Default)]
pub struct BulkAutoTagState {
    /// `None` while the modal is closed.
    pub phase: Option<BulkAutoTagPhase>,
    pub root: Option<PathBuf>,
    /// Every proposal in display order; groups own ranges of it and the
    /// selection holds indices into it.
    pub files: Vec<BulkFileProposal>,
    pub groups: Vec<BulkDirGroup>,
    pub skipped_complete: usize,
    pub failed: usize,
    pub status: String,
    pub error: Option<String>,
    pub apply_summary: Option<BulkApplySummary>,
    pub selection: Selection,
    pub apply_stop_requested: bool,
    pub job: Option<BulkJob>,
    /// Survives closing the modal, so a job from an earlier session can never match.
    last_generation: u64,
}

impl BulkAutoTagState {
    /// Closed, or reopened fresh at the folder picker. Any running job is cancelled.
    fn reset(&mut self, phase: Option<BulkAutoTagPhase>) {
        self.cancel_job();
        *self = Self { phase, last_generation: self.last_generation, ..Self::default() };
    }

    pub fn open(&mut self) {
        self.reset(Some(BulkAutoTagPhase::PickDirectory));
    }

    pub fn close(&mut self) {
        self.reset(None);
    }

    pub fn is_open(&self) -> bool {
        self.phase.is_some()
    }

    /// Signals the running job to stop and stops listening for its result.
    pub fn cancel_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Replaces any running job with a new one.
    pub fn start_job(&mut self) -> BulkJob {
        self.cancel_job();
        self.last_generation = self.last_generation.wrapping_add(1);
        let job = BulkJob {
            generation: self.last_generation,
            progress: BulkScanProgress::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        self.job = Some(job.clone());
        job
    }

    /// Ends the job if `generation` is the one running. False for a stale result.
    pub fn finish_job(&mut self, generation: u64) -> bool {
        let current = self.job.as_ref().is_some_and(|job| job.generation == generation);
        if current {
            self.job = None;
        }
        current
    }

    /// The `rows` with a suggestion Apply could write; only these can be
    /// selected or checked.
    fn actionable(&self, rows: impl Iterator<Item = usize>) -> impl Iterator<Item = usize> {
        rows.filter(|&row| self.files.get(row).is_some_and(BulkFileProposal::is_actionable))
    }

    pub fn select_file(&mut self, row: usize, shift: bool, control: bool) {
        let files = &self.files;
        let actionable = |row: usize| files.get(row).is_some_and(BulkFileProposal::is_actionable);
        if actionable(row) {
            self.selection.click(row, shift, control, actionable);
        }
    }

    /// Clicking a folder row selects its files. Ctrl toggles them as a block;
    /// Shift extends from the anchor through the folder's last file.
    pub fn select_directory(&mut self, dir_idx: usize, shift: bool, control: bool) {
        let Some(group) = self.groups.get(dir_idx) else {
            return;
        };
        let dir_rows: Vec<usize> = self.actionable(group.files.clone()).collect();
        let (Some(&first), Some(&last)) = (dir_rows.first(), dir_rows.last()) else {
            return;
        };
        if control {
            let all_selected = dir_rows.iter().all(|&row| self.selection.contains(row));
            for row in dir_rows {
                if all_selected { self.selection.remove(row) } else { self.selection.insert(row) }
            }
        } else if shift && let Some(anchor) = self.selection.anchor() {
            let rows: Vec<usize> = self.actionable(anchor.min(last)..anchor.max(last) + 1).collect();
            self.selection.set(rows, Some(anchor));
        } else {
            // A plain click replaces the selection; Shift with no anchor adds the folder to it.
            let kept: Vec<usize> = self.selection.iter().filter(|_| shift).collect();
            self.selection.set(kept.into_iter().chain(dir_rows), Some(first));
        }
    }

    pub fn select_all_files(&mut self) {
        let rows: Vec<usize> = self.actionable(0..self.files.len()).collect();
        self.selection.set(rows.iter().copied(), rows.first().copied());
    }

    /// Checks or unchecks `rows`, skipping any Apply could not write.
    pub fn set_accepted(&mut self, rows: impl IntoIterator<Item = usize>, accepted: bool) {
        for row in rows {
            if let Some(file) = self.files.get_mut(row).filter(|file| file.is_actionable()) {
                file.accepted = accepted;
            }
        }
    }

    pub fn start_running(&mut self, root: PathBuf) {
        self.root = Some(root);
        self.phase = Some(BulkAutoTagPhase::Running);
        self.status.clear();
        self.error = None;
        self.files.clear();
        self.groups.clear();
        self.selection.clear();
    }

    pub fn finish_scan(&mut self, summary: BulkScanSummary) {
        self.root = Some(summary.root);
        (self.files, self.groups) = (summary.files, summary.groups);
        (self.skipped_complete, self.failed) = (summary.skipped_complete, summary.failed);
        self.apply_summary = None;
        self.error = None;
        self.selection.clear();
        if self.files.is_empty() {
            // Nothing to review: stay on the picker and say why.
            self.phase = Some(BulkAutoTagPhase::PickDirectory);
            self.status = match self.skipped_complete {
                0 => "No files need auto tags.".into(),
                skipped => format!("No files need auto tags ({skipped} already complete)."),
            };
        } else {
            self.phase = Some(BulkAutoTagPhase::Review);
            self.status.clear();
            self.select_all_files();
        }
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.phase = Some(BulkAutoTagPhase::PickDirectory);
        self.status.clear();
    }

    pub fn start_apply(&mut self) {
        self.phase = Some(BulkAutoTagPhase::Applying);
        self.apply_stop_requested = false;
        self.error = None;
    }

    pub fn request_stop_apply(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.apply_stop_requested = true;
    }

    pub fn finish_apply(&mut self, summary: BulkApplySummary) {
        self.apply_summary = Some(summary);
        self.phase = Some(BulkAutoTagPhase::Done);
        self.status.clear();
        self.selection.clear();
    }
}

fn muted(label: impl text::IntoFragment<'static>, size: u32) -> Text<'static> {
    text(label).size(size).style(style::muted_text)
}

fn small_button(label: &'static str, message: impl Into<Message>) -> Element<'static, Message> {
    button(text(label).size(11)).padding([4, 10]).on_press(message.into()).style(style::modal_button(false)).into()
}

fn folder_line(root: &Path, label: String) -> Element<'static, Message> {
    row![
        icon("folder-solid.svg", 12.0, |_| ACCENT.scale_alpha(0.75)),
        muted(label, 11).width(Length::Fill),
        small_button("Open folder", Message::FileRevealInFileManager(root.to_path_buf())),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn stat_chip(label: &'static str, value: usize, accent: bool) -> Element<'static, Message> {
    let value = text(value.to_string()).size(11).font(style::SEMIBOLD).style(style::text_color(move |theme| {
        if accent { ACCENT.scale_alpha(0.95) } else { style::text_alpha(theme, 1.0) }
    }));
    container(row![muted(label, 10), value].spacing(4).align_y(Alignment::Center))
        .padding([4, 8])
        .style(move |theme| {
            if accent { style::tinted(ACCENT, 0.12, 0.28, 10.0)(theme) } else { style::panel(0.42, 0.18, 10.0)(theme) }
        })
        .into()
}

fn dir_count_badge(accepted: usize, total: usize) -> Element<'static, Message> {
    let all_checked = total > 0 && accepted == total;
    let (count_color, tone, fill, border) = match (all_checked, accepted > 0) {
        (true, _) => (CONF_HIGH, CONF_HIGH, 0.12, 0.30),
        (false, true) => (ACCENT, ACCENT, 0.10, 0.22),
        (false, false) => (CONF_LOW, ACCENT, 0.10, 0.22),
    };
    let count = text(accepted.to_string()).size(10).font(style::SEMIBOLD).color(count_color.scale_alpha(0.95));
    container(row![count, muted(format!("/ {total}"), 10)].spacing(2).align_y(Alignment::Center))
        .padding([3, 8])
        .style(style::tinted(tone, fill, border, 10.0))
        .into()
}

fn confidence_badge(confidence: Option<f64>) -> Element<'static, Message> {
    let tone = match confidence {
        Some(value) if value >= crate::auto_tag::HIGH_CLASSIFIER_CONFIDENCE => CONF_HIGH,
        Some(value) if value >= crate::auto_tag::MEDIUM_CLASSIFIER_CONFIDENCE => CONF_MED,
        _ => CONF_LOW,
    };
    let label = text(crate::auto_tag::confidence_percent(confidence))
        .size(10)
        .font(style::MEDIUM)
        .color(tone.scale_alpha(0.95));
    container(label).padding([2, 7]).style(style::tinted(tone, 0.14, 0.35, 8.0)).into()
}

fn list_row_button_style(
    selected: bool,
    accepted: bool,
    zebra: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let idle = if selected {
            ACCENT.scale_alpha(0.22)
        } else if accepted {
            ACCENT.scale_alpha(0.10)
        } else if zebra {
            theme.extended_palette().background.weak.color.scale_alpha(0.16)
        } else {
            Color::TRANSPARENT
        };
        let hovered = ACCENT.scale_alpha(if selected { 0.30 } else { 0.14 });
        let background = style::by_status(status, idle, hovered, ACCENT.scale_alpha(0.36));
        style::solid_button(style::text_alpha(theme, 1.0), iced::Border::default(), background)
    }
}

/// A fixed-height table row of `cells`, or a button spanning the rest of it.
fn list_row<'a>(cells: impl IntoIterator<Item = Element<'a, Message>>) -> Row<'a, Message> {
    Row::with_children(cells).align_y(Alignment::Center).height(Length::Fixed(ROW_HEIGHT)).width(Length::Fill)
}

fn row_button(label: Row<'static, Message>, message: BulkAutoTagMsg) -> button::Button<'static, Message> {
    button(label.spacing(8).align_y(Alignment::Center).width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .padding([0, 10])
        .on_press(message.into())
}

fn file_row(state: &BulkAutoTagState, row: usize, file: &BulkFileProposal, zebra: bool) -> Element<'static, Message> {
    let name = crate::path_util::file_label(&file.path);
    let indent: Element<'static, Message> = spacer(Length::Fixed(FILE_INDENT), Length::Shrink).into();

    if let Some(error) = file.error.clone() {
        let body = container(
            row![text("✕").size(11).color(style::ERROR), text(name).size(11).width(Length::Fill), muted(error, 10),]
                .spacing(8)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(ROW_HEIGHT))
        .align_y(Alignment::Center)
        .padding([0, 10])
        .style(move |theme: &Theme| {
            let strong = theme.extended_palette().background.strong.color;
            container::background(style::DANGER.scale_alpha(if zebra { 0.06 } else { 0.08 }))
                .border(style::outline(strong.scale_alpha(0.10), 0.0))
        });
        return list_row([indent, body.into()]).into();
    }

    let selected = state.selection.contains(row);
    let accepted = file.accepted;
    let label = row![
        icon("music-solid.svg", 13.0, move |theme| {
            if selected || accepted { ACCENT.scale_alpha(0.9) } else { style::muted(theme) }
        }),
        text(name).size(12).width(Length::FillPortion(2)).style(style::highlight_text(selected)),
        container(
            text(file.suggested.clone().unwrap_or_default())
                .size(11)
                .font(style::SEMIBOLD)
                .color(ACCENT.scale_alpha(0.95))
        )
        .padding([3, 10])
        .style(style::tinted(ACCENT, 0.14, 0.32, 12.0)),
        confidence_badge(file.confidence),
    ];
    let checkbox = checkbox(accepted).on_toggle(move |accepted| BulkAutoTagMsg::SetFileAccepted(row, accepted).into());
    list_row([
        indent,
        selection_stripe(selected, SELECTION_STRIPE_WIDTH, Length::Fixed(ROW_HEIGHT)),
        container(checkbox)
            .height(Length::Fixed(ROW_HEIGHT))
            .center_x(Length::Fixed(CHECKBOX_COLUMN_WIDTH))
            .align_y(Alignment::Center)
            .into(),
        row_button(label, BulkAutoTagMsg::SelectFile(row))
            .style(list_row_button_style(selected, accepted, zebra))
            .into(),
    ])
    .into()
}

fn directory_group(
    state: &BulkAutoTagState,
    root: &Path,
    dir_idx: usize,
    group: &BulkDirGroup,
) -> Element<'static, Message> {
    let label = group.path.strip_prefix(root).map_or_else(
        |_| truncate_path(&group.path, 48),
        |relative| match relative.to_string_lossy() {
            text if text.is_empty() => ".".into(),
            text => text.into_owned(),
        },
    );
    let files = &state.files[group.files.clone()];
    let (count, accepted) = actionable_and_accepted(files);
    let expanded = group.expanded;
    let dir_selected = count > 0 && state.actionable(group.files.clone()).all(|row| state.selection.contains(row));

    let toggle = button(text(if expanded { "▾" } else { "▸" }).size(17).font(style::SEMIBOLD))
        .width(Length::Fixed(EXPAND_TOGGLE_WIDTH))
        .height(Length::Fixed(ROW_HEIGHT))
        .padding(0)
        .on_press(BulkAutoTagMsg::ToggleDirectoryExpanded(dir_idx).into())
        .style(move |theme: &Theme, status| {
            let palette = theme.extended_palette();
            let (idle_text, idle_bg, idle_border) = if expanded {
                (Color::WHITE, ACCENT.scale_alpha(0.72), ACCENT.scale_alpha(0.55))
            } else {
                (
                    ACCENT.scale_alpha(0.92),
                    palette.background.weak.color.scale_alpha(0.50),
                    palette.background.strong.color.scale_alpha(0.28),
                )
            };
            style::solid_button(
                style::by_status(status, idle_text, Color::WHITE, Color::WHITE),
                style::outline(style::by_status(status, idle_border, ACCENT.scale_alpha(0.70), ACCENT), 0.0),
                style::by_status(status, idle_bg, ACCENT.scale_alpha(0.88), ACCENT.scale_alpha(0.95)),
            )
        });

    let header_label = row![
        icon("folder-solid.svg", 14.0, move |theme| {
            if dir_selected || accepted > 0 { ACCENT.scale_alpha(0.95) } else { style::muted(theme) }
        }),
        text(label).size(12).font(style::SEMIBOLD).width(Length::Fill),
        dir_count_badge(accepted, count),
    ];
    let header = container(list_row([
        selection_stripe(dir_selected, SELECTION_STRIPE_WIDTH, Length::Fixed(ROW_HEIGHT)),
        toggle.into(),
        row_button(header_label, BulkAutoTagMsg::SelectDirectory(dir_idx))
            .style(list_row_button_style(dir_selected, accepted > 0, false))
            .into(),
    ]))
    .height(Length::Fixed(ROW_HEIGHT))
    .width(Length::Fill)
    .style(move |theme: &Theme| {
        let palette = theme.extended_palette();
        let (border, radius) = if expanded {
            (ACCENT.scale_alpha(0.22), 8.0)
        } else {
            (palette.background.strong.color.scale_alpha(0.18), 6.0)
        };
        container::background(palette.background.weak.color.scale_alpha(0.32)).border(style::outline(border, radius))
    });

    if !expanded {
        return header.into();
    }
    let start = group.files.start;
    let files = files.iter().enumerate().map(|(offset, file)| file_row(state, start + offset, file, offset % 2 == 1));
    column![
        header,
        container(Column::with_children(files)).width(Length::Fill).style(|theme: &Theme| {
            container::background(theme.extended_palette().background.base.color.scale_alpha(0.35))
                .border(style::outline(ACCENT.scale_alpha(0.12), 0.0))
        }),
    ]
    .width(Length::Fill)
    .into()
}

fn review_body(state: &BulkAutoTagState) -> Element<'static, Message> {
    let root = state.root.clone().unwrap_or_default();
    let dir_count = state.groups.len();
    let file_count = state.files.len();
    let (ready, checked) = actionable_and_accepted(&state.files);

    let stats = row![
        stat_chip("Ready", ready, true),
        stat_chip("Checked", checked, true),
        stat_chip("Selected", state.selection.len(), false),
        stat_chip("Skipped", state.skipped_complete, false),
        stat_chip("Classify failed", state.failed, state.failed > 0),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let expand_buttons = (dir_count > 1).then(|| {
        [
            small_button("Expand all", BulkAutoTagMsg::ExpandAll(true)),
            small_button("Collapse all", BulkAutoTagMsg::ExpandAll(false)),
        ]
    });
    let toolbar = row![
        small_button("Select all", BulkAutoTagMsg::SelectAll),
        small_button("Clear selection", BulkAutoTagMsg::ClearSelection),
        small_button("Check selected", BulkAutoTagMsg::CheckSelected(true)),
        small_button("Uncheck selected", BulkAutoTagMsg::CheckSelected(false)),
    ]
    .extend(expand_buttons.into_iter().flatten())
    .push(spacer(Length::Fill, Length::Shrink))
    .push(small_button("Check all", BulkAutoTagMsg::CheckAll(true)))
    .push(small_button("Uncheck all", BulkAutoTagMsg::CheckAll(false)))
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let table_header = container(
        row![
            // Lines "File" up with the names, past each row's icon.
            spacer(Length::Fixed(FILE_INDENT + SELECTION_STRIPE_WIDTH + CHECKBOX_COLUMN_WIDTH + 16.0), Length::Shrink),
            muted("File", 10).width(Length::FillPortion(2)),
            muted("Suggested tag", 10).width(Length::FillPortion(1)),
            muted("Confidence", 10).width(Length::Fixed(72.0)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .height(Length::Fixed(ROW_HEIGHT))
    .align_y(Alignment::Center)
    .padding([0, 10])
    .width(Length::Fill)
    .style(style::panel(0.28, 0.16, 0.0));

    let groups = state.groups.iter().enumerate().map(|(dir_idx, group)| directory_group(state, &root, dir_idx, group));
    let list =
        container(column![table_header, scrollable(Column::with_children(groups).spacing(4)).height(Length::Fill),])
            .height(Length::Fill)
            .width(Length::Fill)
            .style(style::panel(0.18, 0.22, 8.0));

    let all_collapsed = dir_count > 0 && state.groups.iter().all(|group| !group.expanded);
    let folder_summary = format!("{}  ·  {dir_count} folders · {file_count} files", truncate_path(&root, 64));
    column![
        stats,
        folder_line(&root, folder_summary),
        all_collapsed.then(|| {
            text("Folders start collapsed for large scans — expand one or use Expand all.")
                .size(10)
                .color(ACCENT.scale_alpha(0.80))
        }),
        toolbar,
        list,
    ]
    .spacing(10)
    .height(Length::Fill)
    .into()
}

fn done_body(state: &BulkAutoTagState) -> Element<'static, Message> {
    let summary = state.apply_summary.as_ref().map(|summary| {
        let plural = |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
        let (message, tone, banner) = if summary.cancelled {
            let message = match summary.written {
                0 => "Apply cancelled. No files were tagged.".to_string(),
                written => format!("Apply cancelled after {}.", plural(written, "file")),
            };
            (message, style::WARN, CONF_MED)
        } else {
            let mut message = format!("Wrote tags to {}", plural(summary.written, "file"));
            if summary.unchanged > 0 {
                message.push_str(&format!(". {} unchanged", summary.unchanged));
            }
            message.push_str(&match summary.failed.len() {
                0 => ". Done.".to_string(),
                failed => format!(". {failed} failed. See errors below."),
            });
            (message, style::OK, CONF_HIGH)
        };
        let failures = summary.failed.iter().map(|(path, err)| {
            let line = if path.as_os_str().is_empty() {
                err.clone()
            } else {
                format!("{} — {err}", truncate_path(path, 42))
            };
            text(line).size(11).into()
        });
        column![
            container(text(message).size(13).font(style::MEDIUM).color(tone))
                .padding([10, 12])
                .width(Length::Fill)
                .style(style::tinted(banner, 0.12, 0.35, 6.0)),
            (!summary.failed.is_empty())
                .then(|| scrollable(Column::with_children(failures).spacing(4)).height(Length::Fixed(120.0))),
        ]
        .spacing(8)
    });
    column![summary, state.root.as_deref().map(|root| folder_line(root, truncate_path(root, 64))),].spacing(8).into()
}

pub fn bulk_auto_tag_view(state: &BulkAutoTagState) -> Element<'_, Message> {
    let phase = state.phase.unwrap_or_default();
    let busy = matches!(phase, BulkAutoTagPhase::Running | BulkAutoTagPhase::Applying);
    let is_review = phase == BulkAutoTagPhase::Review;

    let header = column![
        row![
            text("Bulk Auto Tag").size(20),
            spacer(Length::Fill, Length::Shrink),
            muted(if is_review { "Shift/Ctrl+click to multi-select" } else { "" }, 10),
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill),
        muted(
            "Pick a folder, analyze audio, then review and apply missing instrument, artist, and comment tags.",
            13
        )
        .width(Length::Fill),
        is_review.then(|| {
            text("Apply writes tags permanently. There is no undo. Untagged files may get a new tag container (for example ID3 on WAV).")
                .size(11)
                .color(style::WARN)
                .width(Length::Fill)
        }),
    ]
    .spacing(12);

    let content: Element<'_, Message> = match phase {
        BulkAutoTagPhase::PickDirectory => {
            let root_label =
                state.root.as_deref().map_or_else(|| "No folder selected".to_string(), |path| truncate_path(path, 56));
            column![
                container(
                    row![
                        icon("folder-solid.svg", 14.0, |_| ACCENT.scale_alpha(0.9)),
                        text(root_label).size(12).width(Length::Fill),
                    ]
                    .spacing(10)
                    .align_y(Alignment::Center),
                )
                .padding([8, 10])
                .width(Length::Fill)
                .style(style::panel(0.35, 0.22, 6.0)),
                (!state.status.is_empty()).then(|| text(&state.status).size(12)),
                state.error.as_ref().map(|error| text(error).size(12).color(style::ERROR)),
            ]
            .spacing(10)
            .into()
        }
        BulkAutoTagPhase::Running | BulkAutoTagPhase::Applying => {
            // Read straight from the job; the progress subscription only triggers redraws.
            let progress = state.job.as_ref().map(|job| job.progress.snapshot()).unwrap_or_default();
            let stopping = state.apply_stop_requested && phase == BulkAutoTagPhase::Applying;
            let label = if stopping { "Stopping… already-written tags stay." } else { progress.label() };
            container(
                column![
                    text(label).size(12).width(Length::Fill),
                    progress_bar(0.0..=1.0, progress.fraction()),
                    muted(progress.detail(), 11),
                ]
                .spacing(8),
            )
            .padding([10, 12])
            .width(Length::Fill)
            .style(style::tinted(ACCENT, 0.10, 0.22, 6.0))
            .into()
        }
        BulkAutoTagPhase::Review => container(review_body(state)).width(Length::Fill).height(Length::Fill).into(),
        BulkAutoTagPhase::Done => done_body(state),
    };

    let can_scan = phase == BulkAutoTagPhase::PickDirectory;
    let footer = modal_footer(
        [
            modal_button("Choose folder…", can_scan.then(|| BulkAutoTagMsg::PickDirectory.into()), false),
            modal_button(
                "Scan folder",
                (can_scan && state.root.is_some()).then(|| BulkAutoTagMsg::RunScan.into()),
                false,
            ),
            modal_button(
                "Apply checked",
                (is_review && state.files.iter().any(|file| file.accepted && file.is_actionable()))
                    .then(|| BulkAutoTagMsg::Apply.into()),
                true,
            ),
        ],
        modal_button(if busy { "Cancel" } else { "Close" }, Some(BulkAutoTagMsg::Close.into()), false),
    );

    let layout = column![header, content, footer].spacing(12).padding(20);
    let (layout, height) = if is_review {
        (layout.height(Length::Fill), Length::Fixed(REVIEW_MODAL_HEIGHT))
    } else {
        (layout, Length::Shrink)
    };
    modal_shell(layout, 820.0).height(height).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(name: &str, actionable: bool) -> BulkFileProposal {
        BulkFileProposal {
            path: PathBuf::from(name),
            suggested: actionable.then(|| "Kick".into()),
            confidence: None,
            accepted: false,
            error: (!actionable).then(|| "failed".into()),
        }
    }

    /// Folder `a` holds rows 0..3 (row 1 failed), folder `b` rows 3..5.
    fn state() -> BulkAutoTagState {
        let group = |path: &str, files| BulkDirGroup { path: PathBuf::from(path), files, expanded: true };
        BulkAutoTagState {
            files: [("a1", true), ("a2", false), ("a3", true), ("b1", true), ("b2", true)]
                .map(|(name, actionable)| proposal(name, actionable))
                .into(),
            groups: vec![group("a", 0..3), group("b", 3..5)],
            ..BulkAutoTagState::default()
        }
    }

    #[test]
    fn failed_files_cannot_be_selected_or_checked() {
        let mut state = state();
        state.select_file(1, false, false);
        assert_eq!(state.selection.len(), 0);
        state.set_accepted(0..5, true);
        assert_eq!(actionable_and_accepted(&state.files), (4, 4));
        assert!(!state.files[1].accepted);
    }

    #[test]
    fn shift_click_ranges_skip_failed_files() {
        let mut state = state();
        state.select_file(0, false, false);
        state.select_file(3, true, false);
        assert_eq!(state.selection.len(), 3);
        assert!(!state.selection.contains(1));
    }

    #[test]
    fn folder_clicks_select_toggle_and_extend() {
        let mut state = state();
        state.select_directory(1, false, false);
        assert_eq!(state.selection.len(), 2);
        // Ctrl+click on a fully selected folder clears it.
        state.select_directory(1, false, true);
        assert_eq!(state.selection.len(), 0);
        // Shift extends from the anchor through the folder's last file.
        state.select_file(2, false, false);
        state.select_directory(1, true, false);
        assert_eq!(state.selection.len(), 3);
    }

    #[test]
    fn stale_job_results_are_ignored() {
        let mut state = state();
        let first = state.start_job();
        let second = state.start_job();
        assert!(first.cancel.load(Ordering::Relaxed), "a new job cancels the old");
        assert!(!state.finish_job(first.generation));
        assert!(state.finish_job(second.generation));

        let before_close = state.start_job();
        state.close();
        state.open();
        assert_ne!(before_close.generation, state.start_job().generation);
    }
}
