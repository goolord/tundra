//! The single-file Auto Tag modal.

use super::message::{AutoTagMsg, Message};
use super::settings::{AUTO_TAG_ALREADY_COMPLETE, AUTO_TAG_INSTRUMENT_PRESENT, NO_AUDIO_SELECTED};
use super::style;
use super::widgets::{modal_button, modal_footer, modal_heading, modal_info_row, modal_shell};
use crate::auto_tag::{ClassificationResult, ClassifyError};
use crate::metadata::AutoTagFieldStatus;
use iced::widget::{button, column, container, text};
use iced::{Color, Element, Length, Theme};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct AutoTagState {
    pub target: Option<PathBuf>,
    pub existing_instrument: Option<String>,
    /// Read when the target changes or is written, not on every frame.
    pub path_status: Option<AutoTagFieldStatus>,
    pub running: bool,
    pub applying: bool,
    pub status: String,
    pub result: Option<ClassificationResult>,
    pub error: Option<String>,
    pub error_details: Option<String>,
    pub details_open: bool,
    pub applied: bool,
}

impl AutoTagState {
    pub fn for_target(target: Option<PathBuf>) -> Self {
        let mut state = Self { target, ..Self::default() };
        state.refresh_from_disk();
        state
    }

    /// Re-read the target's current instrument and auto-tag status.
    pub fn refresh_from_disk(&mut self) {
        self.existing_instrument = self.target.as_deref().and_then(crate::metadata::instrument_tag);
        self.path_status = self.target.as_deref().and_then(crate::metadata::auto_tag_field_status);
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
        self.clear_error();
        match result {
            Ok(classification) => self.result = Some(classification),
            Err(err) => {
                self.result = None;
                self.error = Some(err.message);
                self.error_details = Some(err.details);
            }
        }
    }
}

fn muted(content: &str) -> Element<'_, Message> {
    text(content).size(11).width(Length::Fill).style(style::muted_text).into()
}

fn details_disclosure(state: &AutoTagState) -> Element<'_, Message> {
    let arrow = if state.details_open { "▼" } else { "▶" };
    let toggle = button(text(format!("Technical details {arrow}")).size(12))
        .padding(0)
        .on_press(AutoTagMsg::ToggleDetails.into())
        .style(|theme: &Theme, status| {
            let palette = theme.extended_palette();
            let accent = palette.primary.base.color;
            button::Style {
                text_color: style::by_status(status, palette.background.base.text, accent, accent.scale_alpha(0.85)),
                ..button::Style::default()
            }
        });
    if !state.details_open {
        return toggle.into();
    }

    let result = state.result.as_ref();
    let details = column![
        result.map(|result| modal_info_row("Tier", result.tier.to_string())),
        result.map(|result| modal_info_row("Pipeline", &result.summary)),
        result.and_then(|result| result.zcr).map(|zcr| modal_info_row("ZCR", format!("{zcr:.4}"))),
        state.error_details.as_deref().map(muted),
        muted("Setup: cargo xtask setup"),
    ]
    .spacing(6)
    .width(Length::Fill);
    column![toggle, container(details.padding([8, 10])).width(Length::Fill).style(style::panel(0.22, 0.18, 0.0)),]
        .spacing(6)
        .width(Length::Fill)
        .into()
}

pub fn auto_tag_view(state: &AutoTagState) -> Element<'_, Message> {
    let status = state.path_status;
    let has_target = state.target.is_some();
    let needs_any = status.is_some_and(|status| status.needs_any());
    let allows_instrument_work = status.is_some_and(|status| status.allows_instrument_work());
    let can_retag = status.is_some_and(|status| status.can_retag_instrument);
    let colored = |message: String, color: Color| text(message).size(12).color(color);

    let mut body = modal_heading(
        "Auto Tag",
        "Fill missing tags, or replace instrument labels Tundra wrote earlier. Other metadata is left alone.",
    )
    .push(modal_info_row(
        "File",
        state
            .target
            .as_deref()
            .map_or_else(|| NO_AUDIO_SELECTED.to_string(), |path| crate::path_util::truncate_path(path, 56)),
    ))
    .push(modal_info_row(
        "Current tag",
        state.existing_instrument.as_deref().filter(|value| !value.trim().is_empty()).unwrap_or("(none)"),
    ));

    let status_warning = if allows_instrument_work {
        can_retag.then_some("This file has an older Tundra tag. Detect again to upgrade it.")
    } else if needs_any {
        Some(AUTO_TAG_INSTRUMENT_PRESENT)
    } else {
        Some(AUTO_TAG_ALREADY_COMPLETE)
    };
    body = body.push(status_warning.filter(|_| has_target).map(|warning| colored(warning.into(), style::WARN)));

    if state.running {
        body = body.push(text("Analyzing…").size(12).width(Length::Fill));
    } else if let Some(error) = &state.error {
        body = body.push(colored(error.clone(), style::ERROR));
    } else if state.applied {
        let message = if state.status.is_empty() {
            "Instrument tag written. You can now find this file with tag search.".to_string()
        } else {
            state.status.clone()
        };
        body = body.push(colored(message, style::OK));
    } else if let Some(result) = &state.result {
        body = body.push(modal_info_row("Suggested", &result.instrument)).push(
            result
                .confidence
                .map(|confidence| modal_info_row("Confidence", crate::auto_tag::confidence_percent(Some(confidence)))),
        );
    } else if !state.status.is_empty() {
        body = body.push(text(&state.status).size(12));
    }

    let can_run = has_target && allows_instrument_work && !state.running;
    let can_apply = has_target
        && (needs_any || can_retag)
        && !state.running
        && !state.applying
        && !state.applied
        && (!allows_instrument_work || state.result.is_some());

    let body = body
        .push((state.result.is_some() || state.error_details.is_some()).then(|| details_disclosure(state)))
        .push(can_apply.then(|| {
            text("Apply writes missing tags permanently. There is no undo. Files with other metadata keep their existing values.")
                .size(11)
                .color(style::WARN)
                .width(Length::Fill)
        }))
        .push(modal_footer(
            [
                modal_button("Choose file…", (!state.running).then(|| AutoTagMsg::PickFile.into()), false),
                modal_button("Detect instrument", can_run.then(|| AutoTagMsg::Run.into()), false),
                modal_button("Apply tag", can_apply.then(|| AutoTagMsg::Apply.into()), true),
            ],
            modal_button("Close", Some(AutoTagMsg::Close.into()), false),
        ));
    modal_shell(body.padding(18), 560.0).into()
}
