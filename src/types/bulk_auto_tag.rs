use super::common::{resource_path, truncate_path, Message};
use crate::bulk_auto_tag::{BulkApplySummary, BulkDirGroup, BulkProgressSnapshot, BulkScanSummary};
use iced::widget::{button, checkbox, container, progress_bar, row, scrollable, text, Button, Column, Row, Space, Svg};
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::widget::Id;
use iced::{Alignment, Border, Color, Element, Length, Shadow, Theme, keyboard::Modifiers, theme};
use std::collections::HashSet;
use std::path::PathBuf;

const BULK_LIST_SCROLL_ID: &str = "bulk-auto-tag-scroll";
const TUNDRA_ACCENT: Color = Color::from_rgb8(0x50, 0x7a, 0xe0);
const CONF_HIGH: Color = Color::from_rgb8(0x5c, 0xb8, 0x85);
const CONF_MED: Color = Color::from_rgb8(0xd4, 0xa5, 0x4a);
const CONF_LOW: Color = Color::from_rgb8(0x9a, 0x9a, 0xa8);
const REVIEW_MODAL_HEIGHT: f32 = 600.0;
const FILE_INDENT: f32 = 18.0;
const ROW_HEIGHT: f32 = 36.0;
const SELECTION_STRIPE_WIDTH: f32 = 3.0;
const CHECKBOX_COLUMN_WIDTH: f32 = 28.0;
const EXPAND_TOGGLE_WIDTH: f32 = 32.0;
const EXPAND_TOGGLE_MARGIN: f32 = 3.0;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BulkFileKey {
    pub dir_idx: usize,
    pub file_idx: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BulkAutoTagPhase {
    PickDirectory,
    Running,
    Review,
    Applying,
    Done,
}

#[derive(Debug, Clone, Default)]
pub struct BulkAutoTagState {
    pub phase: Option<BulkAutoTagPhase>,
    pub root: Option<PathBuf>,
    pub groups: Vec<BulkDirGroup>,
    pub skipped_tagged: usize,
    pub failed: usize,
    pub status: String,
    pub error: Option<String>,
    pub apply_summary: Option<BulkApplySummary>,
    pub selected: HashSet<BulkFileKey>,
    pub selection_anchor: Option<BulkFileKey>,
    pub progress_done: usize,
    pub progress_total: usize,
    pub progress_fraction: f32,
    pub progress_label: String,
    pub progress_detail: String,
}

impl BulkAutoTagState {
    pub fn open(&mut self) {
        *self = Self {
            phase: Some(BulkAutoTagPhase::PickDirectory),
            ..Self::default()
        };
    }

    pub fn close(&mut self) {
        *self = Self::default();
    }

    pub fn is_open(&self) -> bool {
        self.phase.is_some()
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
        self.selection_anchor = None;
    }

    fn actionable_keys(&self) -> Vec<BulkFileKey> {
        let mut keys = Vec::new();
        for (dir_idx, group) in self.groups.iter().enumerate() {
            for (file_idx, file) in group.files.iter().enumerate() {
                if file.suggested.is_some() && file.error.is_none() {
                    keys.push(BulkFileKey { dir_idx, file_idx });
                }
            }
        }
        keys
    }

    pub fn is_selected(&self, dir_idx: usize, file_idx: usize) -> bool {
        self.selected.contains(&BulkFileKey { dir_idx, file_idx })
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub fn select_file(&mut self, dir_idx: usize, file_idx: usize, shift: bool, control: bool) {
        let key = BulkFileKey { dir_idx, file_idx };
        if !self.is_actionable(dir_idx, file_idx) {
            return;
        }

        if control {
            if self.selected.contains(&key) {
                self.selected.remove(&key);
            } else {
                self.selected.insert(key.clone());
            }
            self.selection_anchor = Some(key);
            return;
        }

        if shift {
            if let Some(anchor) = self.selection_anchor.clone() {
                let keys = self.actionable_keys();
                let Some(start) = keys.iter().position(|entry| entry == &anchor) else {
                    self.selected.clear();
                    self.selected.insert(key.clone());
                    self.selection_anchor = Some(key);
                    return;
                };
                let Some(end) = keys.iter().position(|entry| entry == &key) else {
                    return;
                };
                let (lo, hi) = if start <= end { (start, end) } else { (end, start) };
                self.selected.clear();
                for entry in &keys[lo..=hi] {
                    self.selected.insert(entry.clone());
                }
                return;
            }
        }

        self.selected.clear();
        self.selected.insert(key.clone());
        self.selection_anchor = Some(key);
    }

    pub fn select_directory(&mut self, dir_idx: usize, shift: bool, control: bool) {
        let dir_keys: Vec<BulkFileKey> = self.groups.get(dir_idx).map(|group| {
            group
                .files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.suggested.is_some() && file.error.is_none())
                .map(|(file_idx, _)| BulkFileKey { dir_idx, file_idx })
                .collect()
        }).unwrap_or_default();

        if dir_keys.is_empty() {
            return;
        }

        if control {
            let all_selected = dir_keys.iter().all(|key| self.selected.contains(key));
            if all_selected {
                for key in dir_keys {
                    self.selected.remove(&key);
                }
            } else {
                for key in dir_keys {
                    self.selected.insert(key);
                }
            }
            return;
        }

        if shift {
            if let Some(anchor) = self.selection_anchor.clone() {
                let keys = self.actionable_keys();
                let Some(start) = keys.iter().position(|entry| entry == &anchor) else {
                    self.selected.clear();
                    for key in &dir_keys {
                        self.selected.insert(key.clone());
                    }
                    self.selection_anchor = dir_keys.first().cloned();
                    return;
                };
                let Some(last_in_dir) = keys.iter().rposition(|entry| entry.dir_idx == dir_idx) else {
                    return;
                };
                let (lo, hi) = if start <= last_in_dir {
                    (start, last_in_dir)
                } else {
                    (last_in_dir, start)
                };
                self.selected.clear();
                for entry in &keys[lo..=hi] {
                    self.selected.insert(entry.clone());
                }
                return;
            }
        }

        if !shift {
            self.selected.clear();
        }
        for key in dir_keys {
            self.selected.insert(key);
        }
        self.selection_anchor = self.selected.iter().next().cloned();
    }

    pub fn select_all_files(&mut self) {
        self.selected = self.actionable_keys().into_iter().collect();
        self.selection_anchor = self.selected.iter().next().cloned();
    }

    pub fn check_selected(&mut self) {
        for key in self.selected.clone() {
            if let Some(file) = self
                .groups
                .get_mut(key.dir_idx)
                .and_then(|group| group.files.get_mut(key.file_idx))
            {
                if file.suggested.is_some() && file.error.is_none() {
                    file.accepted = true;
                }
            }
        }
    }

    pub fn uncheck_selected(&mut self) {
        for key in self.selected.clone() {
            if let Some(file) = self
                .groups
                .get_mut(key.dir_idx)
                .and_then(|group| group.files.get_mut(key.file_idx))
            {
                if file.suggested.is_some() && file.error.is_none() {
                    file.accepted = false;
                }
            }
        }
    }

    fn is_actionable(&self, dir_idx: usize, file_idx: usize) -> bool {
        self.groups
            .get(dir_idx)
            .and_then(|group| group.files.get(file_idx))
            .is_some_and(|file| file.suggested.is_some() && file.error.is_none())
    }

    pub fn set_file_accepted(&mut self, dir_idx: usize, file_idx: usize, accepted: bool) {
        let Some(file) = self
            .groups
            .get_mut(dir_idx)
            .and_then(|group| group.files.get_mut(file_idx))
        else {
            return;
        };
        if file.suggested.is_none() || file.error.is_some() {
            return;
        }
        file.accepted = accepted;
    }

    pub fn start_running(&mut self, root: PathBuf) {
        self.root = Some(root);
        self.phase = Some(BulkAutoTagPhase::Running);
        self.status = String::new();
        self.progress_done = 0;
        self.progress_total = 0;
        self.progress_fraction = 0.0;
        self.progress_label = "Scanning folder…".into();
        self.progress_detail = "Starting…".into();
        self.error = None;
        self.groups.clear();
        self.clear_selection();
    }

    pub fn update_progress(&mut self, snapshot: BulkProgressSnapshot) {
        self.progress_done = snapshot.done();
        self.progress_total = snapshot.total();
        self.progress_fraction = snapshot.fraction();
        self.progress_label = snapshot.label().into();
        self.progress_detail = snapshot.detail();
    }

    pub fn set_no_untagged_files(&mut self, root: PathBuf, skipped_tagged: usize) {
        self.root = Some(root);
        self.skipped_tagged = skipped_tagged;
        self.phase = Some(BulkAutoTagPhase::PickDirectory);
        self.status = if skipped_tagged > 0 {
            format!("No untagged audio files found ({skipped_tagged} already tagged).")
        } else {
            "No untagged audio files found.".into()
        };
        self.error = None;
        self.groups.clear();
        self.failed = 0;
        self.apply_summary = None;
        self.clear_selection();
    }

    pub fn finish_scan(&mut self, summary: BulkScanSummary) {
        self.root = Some(summary.root);
        self.groups = summary.groups;
        self.skipped_tagged = summary.skipped_tagged;
        self.failed = summary.failed;
        self.apply_summary = None;
        self.error = None;
        self.phase = Some(BulkAutoTagPhase::Review);
        self.status = String::new();
        self.select_all_files();
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.phase = Some(BulkAutoTagPhase::PickDirectory);
        self.status.clear();
    }

    pub fn start_apply(&mut self) {
        self.phase = Some(BulkAutoTagPhase::Applying);
        self.progress_label = "Writing tags…".into();
        self.progress_done = 0;
        self.progress_total = self.accepted_count();
        self.progress_fraction = 0.0;
        self.progress_detail = if self.progress_total > 0 {
            format!("0 / {}", self.progress_total)
        } else {
            "Starting…".into()
        };
        self.error = None;
    }

    pub fn finish_apply(&mut self, summary: BulkApplySummary) {
        self.apply_summary = Some(summary);
        self.phase = Some(BulkAutoTagPhase::Done);
        self.status.clear();
        self.clear_selection();
    }

    pub fn actionable_count(&self) -> usize {
        crate::bulk_auto_tag::total_actionable(&self.groups)
    }

    pub fn accepted_count(&self) -> usize {
        crate::bulk_auto_tag::total_accepted(&self.groups)
    }
}

fn muted_text(theme: &theme::Theme) -> Color {
    theme
        .extended_palette()
        .background
        .base
        .text
        .scale_alpha(0.72)
}

fn modal_button_style(theme: &theme::Theme, status: ButtonStatus, primary: bool) -> ButtonStyle {
    let palette = theme.extended_palette();
    let accent = palette.primary.base.color;
    let mut style = ButtonStyle {
        text_color: palette.background.base.text,
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: palette.background.strong.color.scale_alpha(0.35),
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };
    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(
                if primary {
                    accent.scale_alpha(0.82)
                } else {
                    palette.background.weak.color.scale_alpha(0.45)
                }
                .into(),
            );
            if primary {
                style.text_color = Color::WHITE;
            }
        }
        ButtonStatus::Hovered => {
            style.background = Some(
                if primary {
                    accent.scale_alpha(0.92)
                } else {
                    accent.scale_alpha(0.16)
                }
                .into(),
            );
        }
        ButtonStatus::Pressed => {
            style.background = Some(accent.scale_alpha(0.72).into());
        }
    }
    style
}

fn list_row_button_style(
    theme: &theme::Theme,
    status: ButtonStatus,
    selected: bool,
    accepted: bool,
    zebra: bool,
) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: palette.background.base.text,
        border: Border {
            width: 0.0,
            radius: 0.0.into(),
            ..Default::default()
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };

    let zebra_bg = if zebra {
        palette.background.weak.color.scale_alpha(0.16)
    } else {
        Color::TRANSPARENT
    };

    let base_bg = if accepted {
        TUNDRA_ACCENT.scale_alpha(0.10)
    } else {
        zebra_bg
    };

    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(
                if selected {
                    TUNDRA_ACCENT.scale_alpha(0.22)
                } else {
                    base_bg
                }
                .into(),
            );
        }
        ButtonStatus::Hovered => {
            style.background = Some(TUNDRA_ACCENT.scale_alpha(if selected { 0.30 } else { 0.14 }).into());
        }
        ButtonStatus::Pressed => {
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.36).into());
        }
    }
    style
}

fn selection_stripe(selected: bool) -> Element<'static, Message> {
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fixed(SELECTION_STRIPE_WIDTH))
        .height(Length::Fixed(ROW_HEIGHT))
        .style(move |_theme| container::Style {
            background: Some(if selected {
                TUNDRA_ACCENT.into()
            } else {
                Color::TRANSPARENT.into()
            }),
            ..Default::default()
        })
        .into()
}

fn small_button(label: String, message: Message, primary: bool) -> Element<'static, Message> {
    button(text(label).size(11))
        .padding([4, 10])
        .on_press(message)
        .style(move |theme, status| modal_button_style(theme, status, primary))
        .into()
}

fn stat_chip(label: String, value: usize, accent: bool) -> Element<'static, Message> {
    container(
        row![
            text(label)
                .size(10)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                }),
            text(format!("{value}"))
                .size(11)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..iced::Font::default()
                })
                .style(move |theme: &Theme| iced::widget::text::Style {
                    color: Some(if accent {
                        TUNDRA_ACCENT.scale_alpha(0.95)
                    } else {
                        theme.extended_palette().background.base.text
                    }),
                }),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .style(move |theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(
                if accent {
                    TUNDRA_ACCENT.scale_alpha(0.12)
                } else {
                    palette.background.weak.color.scale_alpha(0.42)
                }
                .into(),
            ),
            border: Border {
                radius: 10.0.into(),
                width: 1.0,
                color: if accent {
                    TUNDRA_ACCENT.scale_alpha(0.28)
                } else {
                    palette.background.strong.color.scale_alpha(0.18)
                },
            },
            ..Default::default()
        }
    })
    .into()
}

fn instrument_chip(instrument: String) -> Element<'static, Message> {
    container(
        text(instrument)
            .size(11)
            .font(iced::Font {
                weight: iced::font::Weight::Semibold,
                ..iced::Font::default()
            })
            .style(|_theme: &Theme| iced::widget::text::Style {
                color: Some(TUNDRA_ACCENT.scale_alpha(0.95)),
            }),
    )
    .padding([3, 10])
    .style(|_theme: &Theme| container::Style {
        background: Some(TUNDRA_ACCENT.scale_alpha(0.14).into()),
        border: Border {
            radius: 12.0.into(),
            width: 1.0,
            color: TUNDRA_ACCENT.scale_alpha(0.32),
        },
        ..Default::default()
    })
    .into()
}

fn dir_count_badge(accepted: usize, total: usize) -> Element<'static, Message> {
    let all_checked = total > 0 && accepted == total;
    container(
        row![
            text(format!("{accepted}"))
                .size(10)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..iced::Font::default()
                })
                .style(move |_theme: &Theme| iced::widget::text::Style {
                    color: Some(if all_checked {
                        CONF_HIGH.scale_alpha(0.95)
                    } else if accepted > 0 {
                        TUNDRA_ACCENT.scale_alpha(0.95)
                    } else {
                        CONF_LOW.scale_alpha(0.95)
                    }),
                }),
            text(format!("/ {total}"))
                .size(10)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                }),
        ]
        .spacing(2)
        .align_y(Alignment::Center),
    )
    .padding([3, 8])
    .style(move |_theme: &Theme| container::Style {
        background: Some(
            if all_checked {
                CONF_HIGH.scale_alpha(0.12)
            } else {
                TUNDRA_ACCENT.scale_alpha(0.10)
            }
            .into(),
        ),
        border: Border {
            radius: 10.0.into(),
            width: 1.0,
            color: if all_checked {
                CONF_HIGH.scale_alpha(0.30)
            } else {
                TUNDRA_ACCENT.scale_alpha(0.22)
            },
        },
        ..Default::default()
    })
    .into()
}

fn expand_toggle_button_style(theme: &theme::Theme, status: ButtonStatus, expanded: bool) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: if expanded {
            Color::WHITE
        } else {
            TUNDRA_ACCENT.scale_alpha(0.92)
        },
        border: Border {
            radius: 0.0.into(),
            width: 1.0,
            color: if expanded {
                TUNDRA_ACCENT.scale_alpha(0.55)
            } else {
                palette.background.strong.color.scale_alpha(0.28)
            },
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };
    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(
                if expanded {
                    TUNDRA_ACCENT.scale_alpha(0.72)
                } else {
                    palette.background.weak.color.scale_alpha(0.50)
                }
                .into(),
            );
        }
        ButtonStatus::Hovered => {
            style.text_color = Color::WHITE;
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.88).into());
            style.border.color = TUNDRA_ACCENT.scale_alpha(0.70);
        }
        ButtonStatus::Pressed => {
            style.text_color = Color::WHITE;
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.95).into());
            style.border.color = TUNDRA_ACCENT;
        }
    }
    style
}

fn expand_toggle_button(expanded: bool, dir_idx: usize) -> Element<'static, Message> {
    let chevron = if expanded { "▾" } else { "▸" };
    container(
        button(
            container(
                text(chevron)
                    .size(17)
                    .font(iced::Font {
                        weight: iced::font::Weight::Semibold,
                        ..iced::Font::default()
                    }),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(0)
        .on_press(Message::BulkAutoTagToggleDirectoryExpanded(dir_idx))
        .style(move |theme, status| expand_toggle_button_style(theme, status, expanded)),
    )
    .width(Length::Fixed(EXPAND_TOGGLE_WIDTH))
    .height(Length::Fixed(ROW_HEIGHT))
    .padding(EXPAND_TOGGLE_MARGIN)
    .align_x(iced::alignment::Horizontal::Center)
    .align_y(Alignment::Center)
    .into()
}

fn checkbox_cell(checkbox: Element<'static, Message>) -> Element<'static, Message> {
    container(checkbox)
        .width(Length::Fixed(CHECKBOX_COLUMN_WIDTH))
        .height(Length::Fixed(ROW_HEIGHT))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(Alignment::Center)
        .into()
}

fn confidence_badge(confidence: Option<f64>) -> Element<'static, Message> {
    let (label, tone) = match confidence {
        Some(value) if value >= 0.85 => (format!("{:.0}%", value * 100.0), CONF_HIGH),
        Some(value) if value >= 0.65 => (format!("{:.0}%", value * 100.0), CONF_MED),
        Some(value) => (format!("{:.0}%", value * 100.0), CONF_LOW),
        None => ("—".into(), CONF_LOW),
    };
    container(
        text(label)
            .size(10)
            .font(iced::Font {
                weight: iced::font::Weight::Medium,
                ..iced::Font::default()
            })
            .style(move |_theme: &Theme| iced::widget::text::Style {
                color: Some(tone.scale_alpha(0.95)),
            }),
    )
    .padding([2, 7])
    .style(move |_theme: &Theme| container::Style {
        background: Some(tone.scale_alpha(0.14).into()),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: tone.scale_alpha(0.35),
        },
        ..Default::default()
    })
    .into()
}

fn table_header() -> Element<'static, Message> {
    container(
        row![
            Space::new().width(Length::Fixed(FILE_INDENT)),
            Space::new().width(Length::Fixed(SELECTION_STRIPE_WIDTH)),
            Space::new().width(Length::Fixed(CHECKBOX_COLUMN_WIDTH)),
            text("File")
                .size(10)
                .width(Length::FillPortion(2))
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                }),
            text("Suggested tag")
                .size(10)
                .width(Length::FillPortion(1))
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                }),
            text("Confidence")
                .size(10)
                .width(Length::Fixed(72.0))
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                }),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .padding([8, 10])
    .width(Length::Fill)
    .style(|theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.weak.color.scale_alpha(0.28).into()),
            border: Border {
                radius: 0.0.into(),
                width: 1.0,
                color: palette.background.strong.color.scale_alpha(0.16),
            },
            ..Default::default()
        }
    })
    .into()
}

fn dir_label(root: &PathBuf, dir: &PathBuf) -> String {
    dir.strip_prefix(root)
        .map(|relative| {
            let text = relative.to_string_lossy();
            if text.is_empty() {
                ".".to_string()
            } else {
                text.into_owned()
            }
        })
        .unwrap_or_else(|_| truncate_path(dir, 48))
}

fn file_row(
    state: &BulkAutoTagState,
    modifiers: Modifiers,
    dir_idx: usize,
    file_idx: usize,
    file: &crate::bulk_auto_tag::BulkFileProposal,
    zebra: bool,
) -> Element<'static, Message> {
    let name = file
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file.path.display().to_string());
    let selected = state.is_selected(dir_idx, file_idx);
    let shift = modifiers.shift();
    let control = modifiers.control() || modifiers.logo();

    if let Some(error) = file.error.clone() {
        return container(
            row![
                Space::new().width(Length::Fixed(FILE_INDENT)),
                Space::new().width(Length::Fixed(3.0)),
                text("✕").size(11).style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.95, 0.62, 0.62)),
                }),
                text(name).size(11).width(Length::Fill),
                text(error).size(10).style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                }),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        )
        .padding([6, 10])
        .width(Length::Fill)
        .style(move |theme: &Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(
                    if zebra {
                        Color::from_rgb8(0xff, 0x66, 0x66).scale_alpha(0.06)
                    } else {
                        Color::from_rgb8(0xff, 0x66, 0x66).scale_alpha(0.08)
                    }
                    .into(),
                ),
                border: Border {
                    radius: 0.0.into(),
                    width: 1.0,
                    color: palette.background.strong.color.scale_alpha(0.10),
                },
                ..Default::default()
            }
        })
        .into();
    }

    let suggested = file.suggested.clone().unwrap_or_default();
    let confidence = file.confidence;
    let accepted = file.accepted;

    let select_message = Message::BulkAutoTagSelectFile {
        dir_idx,
        file_idx,
        shift,
        control,
    };

    let label_row = row![
        Svg::from_path(resource_path("music-solid.svg"))
            .width(Length::Fixed(13.0))
            .height(Length::Fixed(13.0))
            .style(move |theme, _status| iced::widget::svg::Style {
                color: Some(if selected || accepted {
                    TUNDRA_ACCENT.scale_alpha(0.9)
                } else {
                    muted_text(theme)
                }),
            }),
        text(name)
            .size(12)
            .width(Length::FillPortion(2))
            .style(move |theme: &Theme| iced::widget::text::Style {
                color: Some(if selected {
                    theme.extended_palette().background.base.text
                } else {
                    muted_text(theme)
                }),
            }),
        instrument_chip(suggested),
        confidence_badge(confidence),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    container(
        row![
            Space::new().width(Length::Fixed(FILE_INDENT)),
            Row::new()
                .align_y(Alignment::Center)
                .height(Length::Fixed(ROW_HEIGHT))
                .push(selection_stripe(selected))
                .push(checkbox_cell(
                    checkbox(accepted)
                        .on_toggle(move |checked| Message::BulkAutoTagSetFileAccepted {
                            dir_idx,
                            file_idx,
                            accepted: checked,
                        })
                        .into(),
                ))
                .push(
                    Button::new(label_row)
                        .width(Length::Fill)
                        .padding([7, 10])
                        .on_press(select_message)
                        .style(move |theme, status| {
                            list_row_button_style(theme, status, selected, accepted, zebra)
                        }),
                )
                .width(Length::Fill),
        ]
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .into()
}

fn directory_group(
    state: &BulkAutoTagState,
    modifiers: Modifiers,
    root: &PathBuf,
    dir_idx: usize,
    group: &BulkDirGroup,
) -> Element<'static, Message> {
    let label = dir_label(root, &group.path);
    let count = group.actionable_count();
    let accepted = group.accepted_count();
    let expanded = group.expanded;
    let dir_selected = group
        .files
        .iter()
        .enumerate()
        .filter(|(_, file)| file.suggested.is_some() && file.error.is_none())
        .all(|(file_idx, _)| state.is_selected(dir_idx, file_idx))
        && count > 0;
    let shift = modifiers.shift();
    let control = modifiers.control() || modifiers.logo();

    let header = container(
        Row::new()
            .align_y(Alignment::Center)
            .height(Length::Fixed(ROW_HEIGHT))
            .push(selection_stripe(dir_selected))
            .push(expand_toggle_button(expanded, dir_idx))
            .push(
                Button::new(
                    row![
                        Svg::from_path(resource_path("folder-solid.svg"))
                            .width(Length::Fixed(14.0))
                            .height(Length::Fixed(14.0))
                            .style(move |theme, _status| iced::widget::svg::Style {
                                color: Some(if dir_selected || accepted > 0 {
                                    TUNDRA_ACCENT.scale_alpha(0.95)
                                } else {
                                    muted_text(theme)
                                }),
                            }),
                        text(label)
                            .size(12)
                            .font(iced::Font {
                                weight: iced::font::Weight::Semibold,
                                ..iced::Font::default()
                            })
                            .width(Length::Fill),
                        dir_count_badge(accepted, count),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
                )
                .width(Length::Fill)
                .height(Length::Fixed(ROW_HEIGHT))
                .padding([0, 10])
                .on_press(Message::BulkAutoTagSelectDirectory {
                    dir_idx,
                    shift,
                    control,
                })
                .style(move |theme, status| {
                    list_row_button_style(theme, status, dir_selected, accepted > 0, false)
                }),
            )
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .style(move |theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.weak.color.scale_alpha(0.32).into()),
            border: Border {
                radius: if expanded {
                    8.0.into()
                } else {
                    6.0.into()
                },
                width: 1.0,
                color: if expanded {
                    TUNDRA_ACCENT.scale_alpha(0.22)
                } else {
                    palette.background.strong.color.scale_alpha(0.18)
                },
            },
            ..Default::default()
        }
    });

    let mut column = Column::new().spacing(0).width(Length::Fill).push(header);

    if expanded {
        let file_rows: Vec<Element<Message>> = group
            .files
            .iter()
            .enumerate()
            .map(|(file_idx, file)| {
                file_row(
                    state,
                    modifiers,
                    dir_idx,
                    file_idx,
                    file,
                    file_idx % 2 == 1,
                )
            })
            .collect();

        column = column.push(
            container(Column::with_children(file_rows).spacing(0))
                .width(Length::Fill)
                .style(|theme: &Theme| {
                    let palette = theme.extended_palette();
                    container::Style {
                        background: Some(palette.background.base.color.scale_alpha(0.35).into()),
                        border: Border {
                            radius: 0.0.into(),
                            width: 1.0,
                            color: TUNDRA_ACCENT.scale_alpha(0.12),
                        },
                        ..Default::default()
                    }
                }),
        );
    }

    container(column)
        .width(Length::Fill)
        .into()
}

fn review_body(state: &BulkAutoTagState, modifiers: Modifiers) -> Element<'static, Message> {
    let root = state.root.clone().unwrap_or_default();
    let root_label = truncate_path(&root, 64);
    let dir_count = state.groups.len();
    let file_count: usize = state.groups.iter().map(|group| group.files.len()).sum();
    let all_collapsed = dir_count > 0 && state.groups.iter().all(|group| !group.expanded);
    let many_groups = dir_count > 1;

    let groups: Vec<Element<Message>> = state
        .groups
        .iter()
        .enumerate()
        .map(|(dir_idx, group)| directory_group(state, modifiers, &root, dir_idx, group))
        .collect();

    let mut body = Column::new().spacing(10).push(
        row![
            stat_chip("Ready".to_string(), state.actionable_count(), true),
            stat_chip("Checked".to_string(), state.accepted_count(), true),
            stat_chip("Selected".to_string(), state.selected_count(), false),
            stat_chip("Skipped".to_string(), state.skipped_tagged, false),
            stat_chip("Failed".to_string(), state.failed, state.failed > 0),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    );

    body = body.push(
        row![
            Svg::from_path(resource_path("folder-solid.svg"))
                .width(Length::Fixed(12.0))
                .height(Length::Fixed(12.0))
                .style(|_theme, _status| iced::widget::svg::Style {
                    color: Some(TUNDRA_ACCENT.scale_alpha(0.75)),
                }),
            text(format!("{root_label}  ·  {dir_count} folders · {file_count} files"))
                .size(11)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                })
                .width(Length::Fill),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    );

    if all_collapsed {
        body = body.push(
            text("Folders start collapsed for large scans — expand one or use Expand all.")
                .size(10)
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(TUNDRA_ACCENT.scale_alpha(0.80)),
                }),
        );
    }

    let mut toolbar = row![
        small_button("Select all".to_string(), Message::BulkAutoTagSelectAll, false),
        small_button("Clear selection".to_string(), Message::BulkAutoTagClearSelection, false),
        small_button("Check selected".to_string(), Message::BulkAutoTagCheckSelected, false),
        small_button("Uncheck selected".to_string(), Message::BulkAutoTagUncheckSelected, false),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    if many_groups {
        toolbar = toolbar.push(small_button(
            "Expand all".to_string(),
            Message::BulkAutoTagExpandAllDirectories,
            false,
        ));
        toolbar = toolbar.push(small_button(
            "Collapse all".to_string(),
            Message::BulkAutoTagCollapseAllDirectories,
            false,
        ));
    }

    toolbar = toolbar
        .push(Space::new().width(Length::Fill))
        .push(small_button(
            "Check all".to_string(),
            Message::BulkAutoTagAcceptAll,
            false,
        ))
        .push(small_button(
            "Uncheck all".to_string(),
            Message::BulkAutoTagRejectAll,
            false,
        ))
        .width(Length::Fill);

    body = body.push(toolbar).push(
        container(
            Column::new()
                .spacing(0)
                .height(Length::Fill)
                .push(table_header())
                .push(
                    scrollable(Column::with_children(groups).spacing(4))
                        .id(Id::new(BULK_LIST_SCROLL_ID))
                        .height(Length::Fill),
                ),
        )
        .height(Length::Fill)
        .width(Length::Fill)
        .style(|theme: &Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette.background.weak.color.scale_alpha(0.18).into()),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: palette.background.strong.color.scale_alpha(0.22),
                },
                ..Default::default()
            }
        }),
    );

    body.height(Length::Fill).into()
}

pub fn bulk_auto_tag_view<'a>(
    state: &'a BulkAutoTagState,
    modifiers: Modifiers,
) -> Element<'a, Message> {
    let phase = state.phase.clone().unwrap_or(BulkAutoTagPhase::PickDirectory);
    let busy = matches!(
        phase,
        BulkAutoTagPhase::Running | BulkAutoTagPhase::Applying
    );
    let is_review = matches!(phase, BulkAutoTagPhase::Review);

    let header = Column::new()
        .spacing(12)
        .push(
            row![
                text("Bulk Auto Tag").size(20),
                Space::new().width(Length::Fill),
                if is_review {
                    text("Shift/Ctrl+click to multi-select")
                        .size(10)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(muted_text(theme)),
                        })
                } else {
                    text("").size(10)
                },
            ]
            .align_y(Alignment::Center)
            .width(Length::Fill),
        )
        .push(
            text("Pick a folder, analyze untagged audio, then review and apply instrument tags.")
                .size(13)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(muted_text(theme)),
                })
                .width(Length::Fill),
        );
    let header = if is_review {
        header.push(
            text("Apply writes tags permanently. There is no undo. Untagged files may get a new tag container (for example ID3 on WAV).")
                .size(11)
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.88, 0.78, 0.52)),
                })
                .width(Length::Fill),
        )
    } else {
        header
    };

    let content: Element<Message> = match phase {
        BulkAutoTagPhase::PickDirectory => {
            let root_label = state
                .root
                .as_ref()
                .map(|path| truncate_path(path, 56))
                .unwrap_or_else(|| "No folder selected".to_string());
            let mut pick = Column::new()
                .spacing(10)
                .push(
                    container(
                        row![
                            Svg::from_path(resource_path("folder-solid.svg"))
                                .width(Length::Fixed(14.0))
                                .height(Length::Fixed(14.0))
                                .style(|_theme, _status| iced::widget::svg::Style {
                                    color: Some(TUNDRA_ACCENT.scale_alpha(0.9)),
                                }),
                            text(root_label).size(12).width(Length::Fill),
                        ]
                        .spacing(10)
                        .align_y(Alignment::Center),
                    )
                    .padding([8, 10])
                    .width(Length::Fill)
                    .style(|theme: &Theme| {
                        let palette = theme.extended_palette();
                        container::Style {
                            background: Some(palette.background.weak.color.scale_alpha(0.35).into()),
                            border: Border {
                                radius: 6.0.into(),
                                width: 1.0,
                                color: palette.background.strong.color.scale_alpha(0.22),
                            },
                            ..Default::default()
                        }
                    }),
                );
            if !state.status.is_empty() {
                pick = pick.push(text(&state.status).size(12));
            }
            if let Some(error) = &state.error {
                pick = pick.push(
                    text(error)
                        .size(12)
                        .style(|_theme: &Theme| iced::widget::text::Style {
                            color: Some(Color::from_rgb(0.95, 0.62, 0.62)),
                        }),
                );
            }
            pick.into()
        }
        BulkAutoTagPhase::Running | BulkAutoTagPhase::Applying => container(
            Column::new()
                .spacing(8)
                .push(
                    text(&state.progress_label)
                        .size(12)
                        .width(Length::Fill),
                )
                .push(progress_bar(0.0..=1.0, state.progress_fraction))
                .push(
                    text(&state.progress_detail)
                        .size(11)
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(muted_text(theme)),
                        }),
                ),
        )
        .padding([10, 12])
        .width(Length::Fill)
        .style(|_theme: &Theme| container::Style {
            background: Some(TUNDRA_ACCENT.scale_alpha(0.10).into()),
            border: Border {
                radius: 6.0.into(),
                width: 1.0,
                color: TUNDRA_ACCENT.scale_alpha(0.22),
            },
            ..Default::default()
        })
        .into(),
        BulkAutoTagPhase::Review => container(review_body(state, modifiers))
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        BulkAutoTagPhase::Done => {
            let mut done = Column::new().spacing(8);
            if let Some(summary) = &state.apply_summary {
                done = done.push(
                    container(
                        text(format!(
                            "Applied {} tags. {} failed.",
                            summary.applied,
                            summary.failed.len()
                        ))
                        .size(13)
                        .font(iced::Font {
                            weight: iced::font::Weight::Medium,
                            ..iced::Font::default()
                        })
                        .style(|_theme: &Theme| iced::widget::text::Style {
                            color: Some(Color::from_rgb(0.62, 0.88, 0.68)),
                        }),
                    )
                    .padding([10, 12])
                    .width(Length::Fill)
                    .style(|_theme: &Theme| container::Style {
                        background: Some(Color::from_rgb8(0x5c, 0xb8, 0x85).scale_alpha(0.12).into()),
                        border: Border {
                            radius: 6.0.into(),
                            width: 1.0,
                            color: Color::from_rgb8(0x5c, 0xb8, 0x85).scale_alpha(0.35),
                        },
                        ..Default::default()
                    }),
                );
                if !summary.failed.is_empty() {
                    let lines: Vec<Element<Message>> = summary
                        .failed
                        .iter()
                        .take(8)
                        .map(|(path, err)| {
                            text(format!("{} — {err}", truncate_path(path, 42)))
                                .size(11)
                                .into()
                        })
                        .collect();
                    done = done.push(
                        scrollable(Column::with_children(lines).spacing(4))
                            .height(Length::Fixed(120.0)),
                    );
                }
            }
            done.into()
        }
    };

    let can_scan = matches!(phase, BulkAutoTagPhase::PickDirectory) && !busy;
    let can_apply = matches!(phase, BulkAutoTagPhase::Review) && state.accepted_count() > 0;

    let footer = row![
        button(text("Choose folder…").size(12))
            .padding([6, 12])
            .on_press_maybe(if can_scan {
                Some(Message::BulkAutoTagPickDirectory)
            } else {
                None
            })
            .style(|theme, status| modal_button_style(theme, status, false)),
        button(text("Scan folder").size(12))
            .padding([6, 12])
            .on_press_maybe(if can_scan && state.root.is_some() {
                Some(Message::BulkAutoTagRunScan)
            } else {
                None
            })
            .style(|theme, status| modal_button_style(theme, status, false)),
        button(text("Apply checked").size(12))
            .padding([6, 12])
            .on_press_maybe(if can_apply {
                Some(Message::BulkAutoTagApply)
            } else {
                None
            })
            .style(|theme, status| modal_button_style(theme, status, true)),
        Space::new().width(Length::Fill),
        button(text(if busy { "Cancel" } else { "Close" }).size(12))
            .padding([6, 14])
            .on_press(Message::CloseBulkAutoTag)
            .style(|theme, status| modal_button_style(theme, status, false)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    let mut layout = Column::new()
        .spacing(12)
        .push(header)
        .push(content)
        .push(footer);
    if is_review {
        layout = layout.height(Length::Fill);
    }

    container(layout.padding(20))
        .width(Length::Fixed(820.0))
        .height(if is_review {
            Length::Fixed(REVIEW_MODAL_HEIGHT)
        } else {
            Length::Shrink
        })
        .style(|theme: &Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette.background.base.color.into()),
                border: Border {
                    width: 1.0,
                    color: palette.background.strong.color,
                    radius: 0.0.into(),
                },
                shadow: Shadow {
                    color: palette.background.base.text.scale_alpha(0.25),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 16.0,
                },
                ..Default::default()
            }
        })
        .into()
}
