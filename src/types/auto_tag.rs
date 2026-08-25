use super::common::{truncate_path, Message};
use crate::auto_tag::ClassificationResult;
use iced::widget::{button, container, row, text, Column, Space};
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::{Alignment, Border, Color, Element, Length, Shadow, Theme, theme};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct AutoTagState {
    pub target: Option<PathBuf>,
    pub existing_instrument: Option<String>,
    pub running: bool,
    pub status: String,
    pub result: Option<ClassificationResult>,
    pub error: Option<String>,
    pub error_details: Option<String>,
    pub details_open: bool,
    pub applied: bool,
}

impl AutoTagState {
    pub fn reset_for_target(
        &mut self,
        target: Option<PathBuf>,
        existing_instrument: Option<String>,
    ) {
        self.target = target;
        self.existing_instrument = existing_instrument;
        self.running = false;
        self.status = String::new();
        self.result = None;
        self.error = None;
        self.error_details = None;
        self.details_open = false;
        self.applied = false;
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.error_details = None;
    }

    pub fn is_untagged(&self) -> bool {
        self.existing_instrument.as_ref().is_none_or(|value| value.trim().is_empty())
    }

    fn has_technical_details(&self) -> bool {
        self.result.is_some() || self.error_details.is_some()
    }
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

fn text_toggle_style(theme: &theme::Theme, status: ButtonStatus) -> ButtonStyle {
    let palette = theme.extended_palette();
    let base = palette.background.base.text;
    let mut style = ButtonStyle {
        text_color: base,
        background: None,
        border: Border {
            radius: 0.0.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };
    match status {
        ButtonStatus::Hovered => {
            style.text_color = palette.primary.base.color;
        }
        ButtonStatus::Pressed => {
            style.text_color = palette.primary.base.color.scale_alpha(0.85);
        }
        ButtonStatus::Active | ButtonStatus::Disabled => {}
    }
    style
}

fn info_panel<'a>(label: &'a str, value: impl Into<std::borrow::Cow<'a, str>>) -> Element<'a, Message> {
    let value = value.into();
    container(
        row![
            text(label)
                .size(11)
                .width(Length::Fixed(88.0))
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(theme.extended_palette().background.base.text.scale_alpha(0.65)),
                }),
            text(value)
                .size(12)
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(|theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.weak.color.scale_alpha(0.35).into()),
            border: Border {
                radius: 0.0.into(),
                width: 1.0,
                color: palette.background.strong.color.scale_alpha(0.22),
            },
            ..Default::default()
        }
    })
    .into()
}

fn muted_text(content: &str) -> Element<'_, Message> {
    text(content)
        .size(11)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(theme.extended_palette().background.base.text.scale_alpha(0.72)),
        })
        .into()
}

fn technical_details_panel<'a>(state: &'a AutoTagState) -> Element<'a, Message> {
    let mut details = Column::new().spacing(6).width(Length::Fill);

    if let Some(result) = &state.result {
        details = details
            .push(info_panel("Tier", format!("{}", result.tier)))
            .push(info_panel("Pipeline", &result.summary));
        if let Some(zcr) = result.zcr {
            details = details.push(info_panel("ZCR", format!("{zcr:.4}")));
        }
    }

    if let Some(error_details) = &state.error_details {
        details = details.push(muted_text(error_details));
    }

    details = details.push(muted_text("Setup: cargo xtask setup"));

    container(details.padding([8, 10]))
        .width(Length::Fill)
        .style(|theme: &Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette.background.weak.color.scale_alpha(0.22).into()),
                border: Border {
                    radius: 0.0.into(),
                    width: 1.0,
                    color: palette.background.strong.color.scale_alpha(0.18),
                },
                ..Default::default()
            }
        })
        .into()
}

fn details_disclosure<'a>(state: &'a AutoTagState) -> Element<'a, Message> {
    let label = if state.details_open {
        "Technical details ▼"
    } else {
        "Technical details ▶"
    };

    let mut section = Column::new().spacing(6).width(Length::Fill).push(
        button(text(label).size(12))
            .padding(0)
            .on_press(Message::ToggleAutoTagDetails)
            .style(text_toggle_style),
    );

    if state.details_open {
        section = section.push(technical_details_panel(state));
    }

    section.into()
}

pub fn auto_tag_view<'a>(state: &'a AutoTagState) -> Element<'a, Message> {
    let untagged = state.is_untagged();

    let mut body = Column::new()
        .spacing(12)
        .push(text("Auto Tag").size(18))
        .push(
            text("Suggest an instrument tag for audio files that don't have one yet. Tag search is unchanged.")
                .size(13)
                .width(Length::Fill),
        );

    let target_label = state
        .target
        .as_ref()
        .map(|path| truncate_path(path, 56))
        .unwrap_or_else(|| "No audio file selected".to_string());
    body = body.push(info_panel("File", target_label));

    let instrument_label = state
        .existing_instrument
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("(none)");
    body = body.push(info_panel("Current tag", instrument_label));

    if !untagged && state.target.is_some() {
        body = body.push(
            text("This file is already tagged. Pick another file or use tag search to filter by instrument.")
                .size(12)
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.88, 0.78, 0.52)),
                }),
        );
    }

    if state.running {
        body = body.push(text("Analyzing…").size(12).width(Length::Fill));
    } else if let Some(error) = &state.error {
        body = body.push(
            text(error)
                .size(12)
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.95, 0.62, 0.62)),
                }),
        );
    } else if state.applied {
        let applied_message = if state.status.is_empty() {
            "Instrument tag written. You can now find this file with tag search.".to_string()
        } else {
            state.status.clone()
        };
        body = body.push(
            text(applied_message)
                .size(12)
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.62, 0.88, 0.68)),
                }),
        );
    } else if let Some(result) = &state.result {
        body = body.push(info_panel("Suggested", &result.instrument));
        if let Some(confidence) = result.confidence {
            body = body.push(info_panel(
                "Confidence",
                format!("{confidence:.0}%", confidence = confidence * 100.0),
            ));
        }
    } else if !state.status.is_empty() {
        body = body.push(text(&state.status).size(12));
    }

    if state.has_technical_details() {
        body = body.push(details_disclosure(state));
    }

    let can_run = state.target.is_some() && untagged && !state.running;
    let can_apply = state.result.is_some() && untagged && !state.running && !state.applied;

    if can_apply {
        body = body.push(
            text("Apply writes the instrument tag permanently. There is no undo. Untagged files may get a new tag container (for example ID3 on WAV).")
                .size(11)
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.88, 0.78, 0.52)),
                })
                .width(Length::Fill),
        );
    }

    body = body.push(
        row![
            button(text("Choose file…").size(12))
                .padding([6, 12])
                .on_press_maybe(if state.running {
                    None
                } else {
                    Some(Message::AutoTagPickFile)
                })
                .style(|theme, status| modal_button_style(theme, status, false)),
            button(text("Detect instrument").size(12))
                .padding([6, 12])
                .on_press_maybe(if can_run {
                    Some(Message::AutoTagRun)
                } else {
                    None
                })
                .style(|theme, status| modal_button_style(theme, status, false)),
            button(text("Apply tag").size(12))
                .padding([6, 12])
                .on_press_maybe(if can_apply {
                    Some(Message::AutoTagApply)
                } else {
                    None
                })
                .style(|theme, status| modal_button_style(theme, status, true)),
            Space::new().width(Length::Fill),
            button(text("Close").size(12))
                .padding([6, 14])
                .on_press(Message::CloseAutoTag)
                .style(|theme, status| modal_button_style(theme, status, false)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    );

    container(body.padding(18))
        .width(Length::Fixed(560.0))
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
