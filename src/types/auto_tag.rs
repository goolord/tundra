use super::common::{
    modal_button_style, modal_error_style, modal_info_row, modal_ok_style, modal_shell,
    modal_warn_style, truncate_path, ui_muted_text, Message,
};
use crate::metadata::{AUTO_TAG_ALREADY_COMPLETE, AUTO_TAG_INSTRUMENT_PRESENT};
use super::settings::NO_AUDIO_SELECTED;
use crate::auto_tag::{ClassificationResult, ClassifyError};
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

    pub fn clear_error(&mut self) {
        self.error = None;
        self.error_details = None;
    }

    pub fn begin_run(&mut self) {
        self.running = true;
        self.clear_error();
        self.result = None;
        self.applied = false;
    }

    pub fn finish_run(&mut self, result: Result<ClassificationResult, ClassifyError>) {
        self.running = false;
        match result {
            Ok(classification) => {
                self.clear_error();
                self.result = Some(classification);
            }
            Err(err) => {
                self.result = None;
                self.error = Some(err.message);
                self.error_details = Some(err.details);
            }
        }
    }

    fn has_technical_details(&self) -> bool {
        self.result.is_some() || self.error_details.is_some()
    }
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

fn muted_text(content: &str) -> Element<'_, Message> {
    text(content)
        .size(11)
        .width(Length::Fill)
        .style(|theme: &Theme| iced::widget::text::Style {
            color: Some(ui_muted_text(theme)),
        })
        .into()
}

fn technical_details_panel<'a>(state: &'a AutoTagState) -> Element<'a, Message> {
    let mut details = Column::new().spacing(6).width(Length::Fill);

    if let Some(result) = &state.result {
        details = details
            .push(modal_info_row("Tier", format!("{}", result.tier)))
            .push(modal_info_row("Pipeline", &result.summary));
        if let Some(zcr) = result.zcr {
            details = details.push(modal_info_row("ZCR", format!("{zcr:.4}")));
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

pub fn auto_tag_view<'a>(
    state: &'a AutoTagState,
    path_status: Option<crate::metadata::AutoTagFieldStatus>,
) -> Element<'a, Message> {
    let needs_any = path_status.is_some_and(|status| status.needs_any());
    let needs_new_instrument = path_status.is_some_and(|status| status.needs_instrument);
    let allows_instrument_work = path_status.is_some_and(|status| status.allows_instrument_work());
    let can_retag = path_status.is_some_and(|status| status.can_retag_instrument);

    let mut body = Column::new()
        .spacing(12)
        .push(text("Auto Tag").size(18))
        .push(
            text("Fill missing tags, or replace instrument labels Tundra wrote earlier. Other metadata is left alone.")
                .size(13)
                .width(Length::Fill),
        );

    let target_label = state
        .target
        .as_ref()
        .map(|path| truncate_path(path, 56))
        .unwrap_or_else(|| NO_AUDIO_SELECTED.to_string());
    body = body.push(modal_info_row("File", target_label));

    let instrument_label = state
        .existing_instrument
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("(none)");
    body = body.push(modal_info_row("Current tag", instrument_label));

    if !needs_any && !allows_instrument_work && state.target.is_some() {
        body = body.push(
            text(AUTO_TAG_ALREADY_COMPLETE)
                .size(12)
                .style(modal_warn_style),
        );
    } else if !allows_instrument_work && state.target.is_some() {
        body = body.push(
            text(AUTO_TAG_INSTRUMENT_PRESENT)
                .size(12)
                .style(modal_warn_style),
        );
    } else if path_status.is_some_and(|status| status.can_retag_instrument) && state.target.is_some() {
        body = body.push(
            text("This file has an older Tundra tag. Detect again to upgrade it.")
                .size(12)
                .style(modal_warn_style),
        );
    }

    if state.running {
        body = body.push(text("Analyzing…").size(12).width(Length::Fill));
    } else if let Some(error) = &state.error {
        body = body.push(
            text(error)
                .size(12)
                .style(modal_error_style),
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
                .style(modal_ok_style),
        );
    } else if let Some(result) = &state.result {
        body = body.push(modal_info_row("Suggested", &result.instrument));
        if let Some(confidence) = result.confidence {
            body = body.push(modal_info_row(
                "Confidence",
                crate::auto_tag::confidence_percent(Some(confidence)),
            ));
        }
    } else if !state.status.is_empty() {
        body = body.push(text(&state.status).size(12));
    }

    if state.has_technical_details() {
        body = body.push(details_disclosure(state));
    }

    let can_run = state.target.is_some() && allows_instrument_work && !state.running;
    let can_apply = state.target.is_some()
        && (needs_any || (can_retag && state.result.is_some()))
        && !state.running
        && !state.applied
        && (!needs_new_instrument || state.result.is_some());

    if can_apply {
        body = body.push(
            text("Apply writes missing tags permanently. There is no undo. Files with other metadata keep their existing values.")
                .size(11)
                .style(modal_warn_style)
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

    modal_shell(body.padding(18), 560.0).into()
}
