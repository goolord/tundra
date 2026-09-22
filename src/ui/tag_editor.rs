//! The Edit Tags modal.

use super::message::{Message, TagEditorMsg};
use super::settings::NO_AUDIO_SELECTED;
use super::style;
use super::widgets::{modal_button, modal_footer, modal_heading, modal_info_row, modal_shell};
use crate::metadata::{ManualTagEdits, SavedTo, TagField, TagFields};
use iced::widget::{row, text, text_input};
use iced::{Alignment, Element, Length};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct TagEditorState {
    pub target: Option<PathBuf>,
    pub edits: ManualTagEdits,
    pub error: Option<String>,
    pub status: Option<String>,
    pub saving: bool,
}

impl TagEditorState {
    pub fn for_path(path: PathBuf, fields: &TagFields) -> Self {
        Self { target: Some(path), edits: ManualTagEdits::from_tag_fields(fields), ..Self::default() }
    }

    pub fn begin_save(&mut self) {
        self.saving = true;
        self.error = None;
        self.status = Some("Saving…".into());
    }

    pub fn finish_save(&mut self, result: Result<SavedTo, String>) {
        self.saving = false;
        match result {
            Ok(SavedTo::File) => self.set_status("Tags saved.".into()),
            Ok(SavedTo::Sidecar(reason)) => {
                self.set_status(format!("Saved in Tundra only; the file was left unchanged. {reason}"));
            }
            Err(err) => self.set_error(err),
        }
    }

    fn set_status(&mut self, status: String) {
        self.error = None;
        self.status = Some(status);
    }

    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.status = None;
    }

    pub fn set_field(&mut self, field: TagField, value: String) {
        self.edits.set_field(field, value);
        self.error = None;
        self.status = None;
    }
}

pub fn tag_editor_view(state: &TagEditorState) -> Element<'_, Message> {
    let target_label = state
        .target
        .as_deref()
        .map_or_else(|| NO_AUDIO_SELECTED.to_string(), |path| crate::path_util::truncate_path(path, 56));
    let fields = ManualTagEdits::EDITOR_FIELDS.into_iter().map(|field| {
        row![
            text(field.label()).size(11).width(Length::Fixed(88.0)).style(style::faded_text(0.65)),
            text_input("", state.edits.field_value(field))
                .on_input(move |input| TagEditorMsg::Input(field, input).into())
                .padding([6, 8])
                .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    });
    let feedback = match (&state.error, &state.status) {
        (Some(error), _) => Some(text(error).size(12).color(style::ERROR)),
        (None, Some(status)) => Some(text(status).size(12).style(style::primary_text)),
        (None, None) => None,
    };

    let body = modal_heading(
        "Edit Tags",
        "Edit metadata directly. Blank instrument, artist, or comment leaves those unchanged; other empty fields clear stored values.",
    )
    .push(modal_info_row("File", target_label))
    .extend(fields)
    .push(feedback)
    .push(modal_footer(
        [modal_button("Cancel", Some(TagEditorMsg::Close.into()), false)],
        modal_button("Save", (!state.saving).then(|| TagEditorMsg::Save.into()), true),
    ));
    modal_shell(body.padding(18), 560.0).into()
}
