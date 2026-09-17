//! The Bulk Auto Tag modal: pick a folder, scan it, review proposals grouped
//! by directory, then apply the checked ones.

use super::message::{BulkAutoTagMsg, Message};
use super::selection::Selection;
use super::style::{self, ACCENT};
use super::widgets::{icon, modal_button, modal_shell, selection_stripe, spacer};
use crate::bulk_auto_tag::{BulkApplySummary, BulkDirGroup, BulkFileProposal, BulkScanProgress, BulkScanSummary};
use crate::path_util::truncate_path;
use iced::keyboard::Modifiers;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BulkFileKey {
    pub dir_idx: usize,
    pub file_idx: usize,
}

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
    pub groups: Vec<BulkDirGroup>,
    pub skipped_complete: usize,
    pub failed: usize,
    pub status: String,
    pub error: Option<String>,
    pub apply_summary: Option<BulkApplySummary>,
    pub selection: Selection<BulkFileKey>,
    pub progress_fraction: f32,
    pub progress_label: String,
    pub progress_detail: String,
    pub apply_stop_requested: bool,
    pub job: Option<BulkJob>,
    /// Survives closing the modal, so a job from an earlier session can never match.
    last_generation: u64,
}

impl BulkAutoTagState {
    /// Closed, or reopened fresh at the folder picker. Any running job is cancelled.
    fn reset(&mut self, phase: Option<BulkAutoTagPhase>) {
        self.cancel_job();
        *self = Self {
            phase,
            last_generation: self.last_generation,
            ..Self::default()
        };
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

    fn file(&self, key: BulkFileKey) -> Option<&BulkFileProposal> {
        self.groups.get(key.dir_idx)?.files.get(key.file_idx)
    }

    fn files_mut(&mut self) -> impl Iterator<Item = &mut BulkFileProposal> {
        self.groups.iter_mut().flat_map(|group| &mut group.files)
    }

    /// Every file that could be applied, in display order.
    fn actionable_keys(&self) -> Vec<BulkFileKey> {
        self.groups
            .iter()
            .enumerate()
            .flat_map(|(dir_idx, group)| {
                group
                    .files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| file.is_actionable())
                    .map(move |(file_idx, _)| BulkFileKey { dir_idx, file_idx })
            })
            .collect()
    }

    pub fn select_file(&mut self, key: BulkFileKey, shift: bool, control: bool) {
        if self.file(key).is_some_and(BulkFileProposal::is_actionable) {
            let order = self.actionable_keys();
            self.selection.click(key, shift, control, &order);
        }
    }

    /// Clicking a folder row selects its files. Ctrl toggles them as a block;
    /// Shift extends from the anchor through the folder's last file.
    pub fn select_directory(&mut self, dir_idx: usize, shift: bool, control: bool) {
        let keys = self.actionable_keys();
        let dir_keys: Vec<BulkFileKey> = keys.iter().copied().filter(|key| key.dir_idx == dir_idx).collect();
        let Some(&first) = dir_keys.first() else {
            return;
        };

        if control {
            if dir_keys.iter().all(|key| self.selection.contains(key)) {
                dir_keys.iter().for_each(|key| self.selection.remove(key));
            } else {
                dir_keys.into_iter().for_each(|key| self.selection.insert(key));
            }
            return;
        }
        if shift && let Some(anchor) = self.selection.anchor().copied() {
            match keys.iter().position(|key| *key == anchor) {
                Some(start) => {
                    let end = keys.iter().rposition(|key| key.dir_idx == dir_idx).unwrap_or(start);
                    self.selection
                        .set(keys[start.min(end)..=start.max(end)].iter().copied(), Some(anchor));
                }
                None => self.selection.set(dir_keys, Some(first)),
            }
            return;
        }
        // Shift with no anchor adds the folder to the selection.
        let mut selected: Vec<BulkFileKey> = if shift {
            self.selection.iter().copied().collect()
        } else {
            Vec::new()
        };
        selected.extend(dir_keys);
        self.selection.set(selected, Some(first));
    }

    pub fn select_all_files(&mut self) {
        let keys = self.actionable_keys();
        let first = keys.first().copied();
        self.selection.set(keys, first);
    }

    pub fn set_selected_accepted(&mut self, accepted: bool) {
        let selected: Vec<BulkFileKey> = self.selection.iter().copied().collect();
        for key in selected {
            self.set_file_accepted(key, accepted);
        }
    }

    pub fn set_file_accepted(&mut self, key: BulkFileKey, accepted: bool) {
        if let Some(file) = self
            .groups
            .get_mut(key.dir_idx)
            .and_then(|group| group.files.get_mut(key.file_idx))
            .filter(|file| file.is_actionable())
        {
            file.accepted = accepted;
        }
    }

    pub fn set_all_accepted(&mut self, accepted: bool) {
        self.files_mut()
            .filter(|file| file.is_actionable())
            .for_each(|file| file.accepted = accepted);
    }

    pub fn set_all_expanded(&mut self, expanded: bool) {
        self.groups.iter_mut().for_each(|group| group.expanded = expanded);
    }

    pub fn start_running(&mut self, root: PathBuf) {
        self.root = Some(root);
        self.phase = Some(BulkAutoTagPhase::Running);
        self.status.clear();
        self.progress_fraction = 0.0;
        self.progress_label = "Scanning folder…".into();
        self.progress_detail = "Starting…".into();
        self.error = None;
        self.groups.clear();
        self.selection.clear();
    }

    pub fn update_progress(&mut self) {
        if let Some(job) = &self.job {
            let snapshot = job.progress.snapshot();
            self.progress_fraction = snapshot.fraction();
            self.progress_label = snapshot.label().into();
            self.progress_detail = snapshot.detail();
        }
    }

    pub fn finish_scan(&mut self, summary: BulkScanSummary) {
        let summary_is_empty = summary.is_empty();
        self.root = Some(summary.root);
        self.skipped_complete = summary.skipped_complete;
        self.failed = summary.failed;
        self.groups = summary.groups;
        self.apply_summary = None;
        self.error = None;
        self.selection.clear();
        if summary_is_empty {
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
        self.progress_label = "Writing tags…".into();
        self.progress_fraction = 0.0;
        self.progress_detail = match self.accepted_count() {
            0 => "Starting…".into(),
            total => format!("0 / {total}"),
        };
        self.error = None;
    }

    pub fn request_stop_apply(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
        self.apply_stop_requested = true;
        self.progress_label = "Stopping… already-written tags stay.".into();
    }

    pub fn finish_apply(&mut self, summary: BulkApplySummary) {
        self.apply_summary = Some(summary);
        self.phase = Some(BulkAutoTagPhase::Done);
        self.status.clear();
        self.selection.clear();
    }

    pub fn actionable_count(&self) -> usize {
        self.groups.iter().map(BulkDirGroup::actionable_count).sum()
    }

    pub fn accepted_count(&self) -> usize {
        self.groups.iter().map(BulkDirGroup::accepted_count).sum()
    }
}

fn muted(label: impl text::IntoFragment<'static>, size: u32) -> Text<'static> {
    text(label).size(size).style(style::muted_text)
}

fn small_button(label: &'static str, message: BulkAutoTagMsg) -> Element<'static, Message> {
    button(text(label).size(11))
        .padding([4, 10])
        .on_press(message.into())
        .style(style::modal_button(false))
        .into()
}

fn folder_line(root: &Path, label: String) -> Element<'static, Message> {
    row![
        icon("folder-solid.svg", 12.0, |_| ACCENT.scale_alpha(0.75)),
        muted(label, 11).width(Length::Fill),
        button(text("Open folder").size(11))
            .padding([4, 10])
            .on_press(Message::FileRevealInFileManager(root.to_path_buf()))
            .style(style::modal_button(false)),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// A rounded, tinted label: counts, suggestions, confidence.
fn chip(
    content: impl Into<Element<'static, Message>>,
    tone: Color,
    padding: [u16; 2],
    radius: f32,
) -> Element<'static, Message> {
    container(content)
        .padding(padding)
        .style(style::tinted(tone, 0.14, 0.32, radius))
        .into()
}

fn stat_chip(label: &'static str, value: usize, accent: bool) -> Element<'static, Message> {
    let value = text(value.to_string())
        .size(11)
        .font(style::SEMIBOLD)
        .style(move |theme: &Theme| text::Style {
            color: Some(if accent {
                ACCENT.scale_alpha(0.95)
            } else {
                theme.extended_palette().background.base.text
            }),
        });
    let content = row![muted(label, 10), value].spacing(4).align_y(Alignment::Center);
    if accent {
        container(content)
            .padding([4, 8])
            .style(style::tinted(ACCENT, 0.12, 0.28, 10.0))
            .into()
    } else {
        container(content)
            .padding([4, 8])
            .style(style::panel(0.42, 0.18, 10.0))
            .into()
    }
}

fn dir_count_badge(accepted: usize, total: usize) -> Element<'static, Message> {
    let all_checked = total > 0 && accepted == total;
    let count_color = match (all_checked, accepted > 0) {
        (true, _) => CONF_HIGH,
        (false, true) => ACCENT,
        (false, false) => CONF_LOW,
    };
    let (tone, fill, border) = if all_checked {
        (CONF_HIGH, 0.12, 0.30)
    } else {
        (ACCENT, 0.10, 0.22)
    };
    container(
        row![
            text(accepted.to_string())
                .size(10)
                .font(style::SEMIBOLD)
                .color(count_color.scale_alpha(0.95)),
            muted(format!("/ {total}"), 10),
        ]
        .spacing(2)
        .align_y(Alignment::Center),
    )
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
    container(label)
        .padding([2, 7])
        .style(style::tinted(tone, 0.14, 0.35, 8.0))
        .into()
}

fn list_row_button_style(
    theme: &Theme,
    status: button::Status,
    selected: bool,
    accepted: bool,
    zebra: bool,
) -> button::Style {
    let idle = if selected {
        ACCENT.scale_alpha(0.22)
    } else if accepted {
        ACCENT.scale_alpha(0.10)
    } else if zebra {
        theme.extended_palette().background.weak.color.scale_alpha(0.16)
    } else {
        Color::TRANSPARENT
    };
    button::Style {
        text_color: theme.extended_palette().background.base.text,
        ..button::Style::default()
    }
    .with_background(style::by_status(
        status,
        idle,
        ACCENT.scale_alpha(if selected { 0.30 } else { 0.14 }),
        ACCENT.scale_alpha(0.36),
    ))
}

fn fixed_cell<'a>(content: impl Into<Element<'a, Message>>, width: f32) -> Element<'a, Message> {
    container(content)
        .width(Length::Fixed(width))
        .height(Length::Fixed(ROW_HEIGHT))
        .center_x(Length::Fixed(width))
        .align_y(Alignment::Center)
        .into()
}

fn list_row(cells: Vec<Element<'static, Message>>) -> Element<'static, Message> {
    Row::with_children(cells)
        .align_y(Alignment::Center)
        .height(Length::Fixed(ROW_HEIGHT))
        .width(Length::Fill)
        .into()
}

fn file_row(
    state: &BulkAutoTagState,
    modifiers: Modifiers,
    key: BulkFileKey,
    file: &BulkFileProposal,
    zebra: bool,
) -> Element<'static, Message> {
    let name = crate::path_util::file_label(&file.path);
    let indent = || -> Element<'static, Message> { spacer(Length::Fixed(FILE_INDENT), Length::Shrink).into() };

    if let Some(error) = file.error.clone() {
        let body = container(
            row![
                text("✕").size(11).color(style::ERROR),
                text(name).size(11).width(Length::Fill),
                muted(error, 10),
            ]
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
        return list_row(vec![indent(), body.into()]);
    }

    let selected = state.selection.contains(&key);
    let accepted = file.accepted;
    let label = row![
        icon("music-solid.svg", 13.0, move |theme| {
            if selected || accepted {
                ACCENT.scale_alpha(0.9)
            } else {
                style::muted(theme)
            }
        }),
        text(name)
            .size(12)
            .width(Length::FillPortion(2))
            .style(move |theme: &Theme| text::Style {
                color: Some(if selected {
                    theme.extended_palette().background.base.text
                } else {
                    style::muted(theme)
                }),
            }),
        chip(
            text(file.suggested.clone().unwrap_or_default())
                .size(11)
                .font(style::SEMIBOLD)
                .color(ACCENT.scale_alpha(0.95)),
            ACCENT,
            [3, 10],
            12.0,
        ),
        confidence_badge(file.confidence),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let select = BulkAutoTagMsg::SelectFile {
        key,
        shift: modifiers.shift(),
        control: modifiers.control() || modifiers.logo(),
    };
    list_row(vec![
        indent(),
        selection_stripe(selected, SELECTION_STRIPE_WIDTH, Length::Fixed(ROW_HEIGHT)),
        fixed_cell(
            checkbox(accepted).on_toggle(move |accepted| BulkAutoTagMsg::SetFileAccepted { key, accepted }.into()),
            CHECKBOX_COLUMN_WIDTH,
        ),
        button(label)
            .width(Length::Fill)
            .height(Length::Fixed(ROW_HEIGHT))
            .padding([0, 10])
            .on_press(select.into())
            .style(move |theme, status| list_row_button_style(theme, status, selected, accepted, zebra))
            .into(),
    ])
}

fn directory_group(
    state: &BulkAutoTagState,
    modifiers: Modifiers,
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
    let count = group.actionable_count();
    let accepted = group.accepted_count();
    let expanded = group.expanded;
    let dir_selected = count > 0
        && group
            .files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.is_actionable())
            .all(|(file_idx, _)| state.selection.contains(&BulkFileKey { dir_idx, file_idx }));

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
            button::Style {
                text_color: style::by_status(status, idle_text, Color::WHITE, Color::WHITE),
                border: style::outline(
                    style::by_status(status, idle_border, ACCENT.scale_alpha(0.70), ACCENT),
                    0.0,
                ),
                ..button::Style::default()
            }
            .with_background(style::by_status(
                status,
                idle_bg,
                ACCENT.scale_alpha(0.88),
                ACCENT.scale_alpha(0.95),
            ))
        });

    let select = BulkAutoTagMsg::SelectDirectory {
        dir_idx,
        shift: modifiers.shift(),
        control: modifiers.control() || modifiers.logo(),
    };
    let header_button = button(
        row![
            icon("folder-solid.svg", 14.0, move |theme| {
                if dir_selected || accepted > 0 {
                    ACCENT.scale_alpha(0.95)
                } else {
                    style::muted(theme)
                }
            }),
            text(label).size(12).font(style::SEMIBOLD).width(Length::Fill),
            dir_count_badge(accepted, count),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fixed(ROW_HEIGHT))
    .padding([0, 10])
    .on_press(select.into())
    .style(move |theme, status| list_row_button_style(theme, status, dir_selected, accepted > 0, false));

    let header = container(list_row(vec![
        selection_stripe(dir_selected, SELECTION_STRIPE_WIDTH, Length::Fixed(ROW_HEIGHT)),
        toggle.into(),
        header_button.into(),
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
    let files = group.files.iter().enumerate().map(|(file_idx, file)| {
        file_row(
            state,
            modifiers,
            BulkFileKey { dir_idx, file_idx },
            file,
            file_idx % 2 == 1,
        )
    });
    column![
        header,
        container(Column::with_children(files))
            .width(Length::Fill)
            .style(|theme: &Theme| {
                container::background(theme.extended_palette().background.base.color.scale_alpha(0.35))
                    .border(style::outline(ACCENT.scale_alpha(0.12), 0.0))
            }),
    ]
    .width(Length::Fill)
    .into()
}

fn review_body(state: &BulkAutoTagState, modifiers: Modifiers) -> Element<'static, Message> {
    let root = state.root.clone().unwrap_or_default();
    let dir_count = state.groups.len();
    let file_count: usize = state.groups.iter().map(|group| group.files.len()).sum();

    let stats = row![
        stat_chip("Ready", state.actionable_count(), true),
        stat_chip("Checked", state.accepted_count(), true),
        stat_chip("Selected", state.selection.len(), false),
        stat_chip("Skipped", state.skipped_complete, false),
        stat_chip("Classify failed", state.failed, state.failed > 0),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut toolbar = row![
        small_button("Select all", BulkAutoTagMsg::SelectAll),
        small_button("Clear selection", BulkAutoTagMsg::ClearSelection),
        small_button("Check selected", BulkAutoTagMsg::CheckSelected),
        small_button("Uncheck selected", BulkAutoTagMsg::UncheckSelected),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    if dir_count > 1 {
        toolbar = toolbar
            .push(small_button("Expand all", BulkAutoTagMsg::ExpandAllDirectories))
            .push(small_button("Collapse all", BulkAutoTagMsg::CollapseAllDirectories));
    }
    let toolbar = toolbar
        .push(spacer(Length::Fill, Length::Shrink))
        .push(small_button("Check all", BulkAutoTagMsg::AcceptAll))
        .push(small_button("Uncheck all", BulkAutoTagMsg::RejectAll))
        .width(Length::Fill);

    let header_cell = |label| muted(label, 10);
    let table_header = container(
        row![
            // Lines "File" up with the names, past each row's icon.
            spacer(
                Length::Fixed(FILE_INDENT + SELECTION_STRIPE_WIDTH + CHECKBOX_COLUMN_WIDTH + 16.0),
                Length::Shrink
            ),
            header_cell("File").width(Length::FillPortion(2)),
            header_cell("Suggested tag").width(Length::FillPortion(1)),
            header_cell("Confidence").width(Length::Fixed(72.0)),
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

    let groups = state
        .groups
        .iter()
        .enumerate()
        .map(|(dir_idx, group)| directory_group(state, modifiers, &root, dir_idx, group));
    let list = container(column![
        table_header,
        scrollable(Column::with_children(groups).spacing(4)).height(Length::Fill),
    ])
    .height(Length::Fill)
    .width(Length::Fill)
    .style(style::panel(0.18, 0.22, 8.0));

    let mut body = column![
        stats,
        folder_line(
            &root,
            format!(
                "{}  ·  {dir_count} folders · {file_count} files",
                truncate_path(&root, 64)
            )
        ),
    ]
    .spacing(10);
    if dir_count > 0 && state.groups.iter().all(|group| !group.expanded) {
        body = body.push(
            text("Folders start collapsed for large scans — expand one or use Expand all.")
                .size(10)
                .color(ACCENT.scale_alpha(0.80)),
        );
    }
    body.push(toolbar).push(list).height(Length::Fill).into()
}

fn done_body(state: &BulkAutoTagState) -> Element<'static, Message> {
    let mut done = Column::new().spacing(8);
    if let Some(summary) = &state.apply_summary {
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
        done = done.push(
            container(text(message).size(13).font(style::MEDIUM).color(tone))
                .padding([10, 12])
                .width(Length::Fill)
                .style(style::tinted(banner, 0.12, 0.35, 6.0)),
        );
        if !summary.failed.is_empty() {
            let lines = summary.failed.iter().map(|(path, err)| {
                let line = if path.as_os_str().is_empty() {
                    err.clone()
                } else {
                    format!("{} — {err}", truncate_path(path, 42))
                };
                text(line).size(11).into()
            });
            done = done.push(scrollable(Column::with_children(lines).spacing(4)).height(Length::Fixed(120.0)));
        }
    }
    if let Some(root) = &state.root {
        done = done.push(folder_line(root, truncate_path(root, 64)));
    }
    done.into()
}

pub fn bulk_auto_tag_view(state: &BulkAutoTagState, modifiers: Modifiers) -> Element<'_, Message> {
    let phase = state.phase.unwrap_or_default();
    let busy = matches!(phase, BulkAutoTagPhase::Running | BulkAutoTagPhase::Applying);
    let is_review = phase == BulkAutoTagPhase::Review;

    let mut header = column![
        row![
            text("Bulk Auto Tag").size(20),
            spacer(Length::Fill, Length::Shrink),
            muted(
                if is_review {
                    "Shift/Ctrl+click to multi-select"
                } else {
                    ""
                },
                10
            ),
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill),
        muted(
            "Pick a folder, analyze audio, then review and apply missing instrument, artist, and comment tags.",
            13
        )
        .width(Length::Fill),
    ]
    .spacing(12);
    if is_review {
        header = header.push(
            text("Apply writes tags permanently. There is no undo. Untagged files may get a new tag container (for example ID3 on WAV).")
                .size(11)
                .color(style::WARN)
                .width(Length::Fill),
        );
    }

    let content: Element<'_, Message> = match phase {
        BulkAutoTagPhase::PickDirectory => {
            let root_label = state
                .root
                .as_deref()
                .map_or_else(|| "No folder selected".to_string(), |path| truncate_path(path, 56));
            let mut pick = column![
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
            ]
            .spacing(10);
            if !state.status.is_empty() {
                pick = pick.push(text(&state.status).size(12));
            }
            if let Some(error) = &state.error {
                pick = pick.push(text(error).size(12).color(style::ERROR));
            }
            pick.into()
        }
        BulkAutoTagPhase::Running | BulkAutoTagPhase::Applying => container(
            column![
                text(&state.progress_label).size(12).width(Length::Fill),
                progress_bar(0.0..=1.0, state.progress_fraction),
                muted(state.progress_detail.clone(), 11),
            ]
            .spacing(8),
        )
        .padding([10, 12])
        .width(Length::Fill)
        .style(style::tinted(ACCENT, 0.10, 0.22, 6.0))
        .into(),
        BulkAutoTagPhase::Review => container(review_body(state, modifiers))
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        BulkAutoTagPhase::Done => done_body(state),
    };

    let can_scan = phase == BulkAutoTagPhase::PickDirectory;
    let footer = row![
        modal_button(
            "Choose folder…",
            can_scan.then(|| BulkAutoTagMsg::PickDirectory.into()),
            false
        ),
        modal_button(
            "Scan folder",
            (can_scan && state.root.is_some()).then(|| BulkAutoTagMsg::RunScan.into()),
            false
        ),
        modal_button(
            "Apply checked",
            (is_review && state.accepted_count() > 0).then(|| BulkAutoTagMsg::Apply.into()),
            true
        ),
        spacer(Length::Fill, Length::Shrink),
        modal_button(
            if busy { "Cancel" } else { "Close" },
            Some(BulkAutoTagMsg::Close.into()),
            false
        )
        .padding([6, 14]),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);

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

    fn state() -> BulkAutoTagState {
        let group = |path: &str, files| BulkDirGroup {
            path: PathBuf::from(path),
            files,
            expanded: true,
        };
        BulkAutoTagState {
            groups: vec![
                group(
                    "a",
                    vec![proposal("a1", true), proposal("a2", false), proposal("a3", true)],
                ),
                group("b", vec![proposal("b1", true), proposal("b2", true)]),
            ],
            ..BulkAutoTagState::default()
        }
    }

    fn key(dir_idx: usize, file_idx: usize) -> BulkFileKey {
        BulkFileKey { dir_idx, file_idx }
    }

    #[test]
    fn failed_files_cannot_be_selected_or_checked() {
        let mut state = state();
        state.select_file(key(0, 1), false, false);
        assert_eq!(state.selection.len(), 0);
        state.set_all_accepted(true);
        assert_eq!(state.accepted_count(), 4);
        assert!(!state.groups[0].files[1].accepted);
    }

    #[test]
    fn shift_click_ranges_skip_failed_files() {
        let mut state = state();
        state.select_file(key(0, 0), false, false);
        state.select_file(key(1, 0), true, false);
        assert_eq!(state.selection.len(), 3);
        assert!(!state.selection.contains(&key(0, 1)));
    }

    #[test]
    fn folder_clicks_select_toggle_and_extend() {
        let mut state = state();
        state.select_directory(1, false, false);
        assert_eq!(state.selection.len(), 2);
        state.select_directory(1, false, true);
        assert_eq!(
            state.selection.len(),
            0,
            "ctrl+click on a fully selected folder clears it"
        );

        state.select_file(key(0, 2), false, false);
        state.select_directory(1, true, false);
        assert_eq!(
            state.selection.len(),
            3,
            "shift extends from the anchor through the folder's last file"
        );
    }

    #[test]
    fn stale_job_results_are_ignored() {
        let mut state = state();
        let first = state.start_job();
        let second = state.start_job();
        assert!(
            first.cancel.load(Ordering::Relaxed),
            "starting a job cancels the previous one"
        );
        assert!(!state.finish_job(first.generation));
        assert!(state.finish_job(second.generation));

        let before_close = state.start_job();
        state.close();
        state.open();
        let after_reopen = state.start_job();
        assert_ne!(before_close.generation, after_reopen.generation);
    }
}
