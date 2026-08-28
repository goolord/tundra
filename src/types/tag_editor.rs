use super::common::{
    modal_button_style, modal_error_style, modal_info_row, modal_label_style, modal_shell,
    truncate_path, Message,
};
use super::settings::NO_AUDIO_SELECTED;
use crate::metadata::{ManualTagEdits, TagField, TagFields};
use iced::widget::{button, row, text, text_input, Column, Space};
use iced::{Alignment, Element, Length, Theme};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct TagEditorState {
    pub target: Option<PathBuf>,
    pub edits: ManualTagEdits,
    pub error: Option<String>,
    pub status: Option<String>,
}

impl TagEditorState {
    pub fn reset_for_path(&mut self, path: PathBuf, fields: TagFields) {
        self.target = Some(path);
        self.edits = ManualTagEdits::from_tag_fields(&fields);
        self.error = None;
        self.status = None;
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

fn field_input<'a>(field: TagField, value: &'a str) -> Element<'a, Message> {
    row![
        text(field.label())
            .size(11)
            .width(Length::Fixed(88.0))
            .style(modal_label_style),
        text_input("", value)
            .on_input(move |input| Message::TagEditorInput(field, input))
            .padding([6, 8])
            .width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

pub fn tag_editor_view<'a>(state: &'a TagEditorState) -> Element<'a, Message> {
    let target_label = state
        .target
        .as_ref()
        .map(|path| truncate_path(path, 56))
        .unwrap_or_else(|| NO_AUDIO_SELECTED.to_string());

    let mut body = Column::new()
        .spacing(12)
        .push(text("Edit Tags").size(18))
        .push(
            text("Edit metadata directly. Blank instrument, artist, or comment leaves those unchanged; other empty fields clear stored values.")
                .size(13)
                .width(Length::Fill),
        )
        .push(modal_info_row("File", target_label));

    for field in ManualTagEdits::EDITOR_FIELDS {
        body = body.push(field_input(field, state.edits.field_value(field)));
    }

    if let Some(error) = &state.error {
        body = body.push(
            text(error)
                .size(12)
                .style(modal_error_style),
        );
    } else if let Some(status) = &state.status {
        body = body.push(
            text(status)
                .size(12)
                .style(|theme: &Theme| iced::widget::text::Style {
                    color: Some(theme.extended_palette().primary.base.color),
                }),
        );
    }

    body = body.push(
        row![
            button(text("Cancel").size(12))
                .padding([6, 12])
                .on_press(Message::CloseTagEditor)
                .style(|theme, status| modal_button_style(theme, status, false)),
            Space::new().width(Length::Fill),
            button(text("Save").size(12))
                .padding([6, 14])
                .on_press(Message::TagEditorSave)
                .style(|theme, status| modal_button_style(theme, status, true)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    );

    modal_shell(body.padding(18), 560.0).into()
}
