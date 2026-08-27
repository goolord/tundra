use super::common::{modal_button_style, truncate_path, Message};
use crate::metadata::{ManualTagEdits, TagFields};
use iced::widget::{button, container, row, text, text_input, Column, Space};
use iced::{Alignment, Border, Color, Element, Length, Shadow, Theme};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagEditorField {
    Instrument,
    Artist,
    Title,
    Bpm,
    Key,
    Genre,
    Comment,
}

impl TagEditorField {
    fn label(self) -> &'static str {
        match self {
            Self::Instrument => "Instrument",
            Self::Artist => "Artist",
            Self::Title => "Title",
            Self::Bpm => "BPM",
            Self::Key => "Key",
            Self::Genre => "Genre",
            Self::Comment => "Comment",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TagEditorState {
    pub target: Option<PathBuf>,
    pub instrument: String,
    pub artist: String,
    pub title: String,
    pub bpm: String,
    pub key: String,
    pub genre: String,
    pub comment: String,
    pub error: Option<String>,
    pub status: Option<String>,
}

impl TagEditorState {
    pub fn reset_for_path(&mut self, path: PathBuf, fields: TagFields) {
        let edits = ManualTagEdits::from_tag_fields(&fields);
        self.target = Some(path);
        self.instrument = edits.instrument;
        self.artist = edits.artist;
        self.title = edits.title;
        self.bpm = edits.bpm;
        self.key = edits.key;
        self.genre = edits.genre;
        self.comment = edits.comment;
        self.error = None;
        self.status = None;
    }

    pub fn set_field(&mut self, field: TagEditorField, value: String) {
        match field {
            TagEditorField::Instrument => self.instrument = value,
            TagEditorField::Artist => self.artist = value,
            TagEditorField::Title => self.title = value,
            TagEditorField::Bpm => self.bpm = value,
            TagEditorField::Key => self.key = value,
            TagEditorField::Genre => self.genre = value,
            TagEditorField::Comment => self.comment = value,
        }
        self.error = None;
        self.status = None;
    }

    pub fn edits(&self) -> ManualTagEdits {
        ManualTagEdits {
            instrument: self.instrument.clone(),
            artist: self.artist.clone(),
            title: self.title.clone(),
            bpm: self.bpm.clone(),
            key: self.key.clone(),
            genre: self.genre.clone(),
            comment: self.comment.clone(),
        }
    }
}

fn field_input<'a>(
    field: TagEditorField,
    value: &'a str,
) -> Element<'a, Message> {
    row![
        text(field.label())
            .size(11)
            .width(Length::Fixed(88.0))
            .style(|theme: &Theme| iced::widget::text::Style {
                color: Some(theme.extended_palette().background.base.text.scale_alpha(0.65)),
            }),
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
        .unwrap_or_else(|| "No audio file selected".to_string());

    let mut body = Column::new()
        .spacing(12)
        .push(text("Edit Tags").size(18))
        .push(
            text("Edit metadata directly. Blank instrument, artist, or comment leaves those unchanged; other empty fields clear stored values.")
                .size(13)
                .width(Length::Fill),
        )
        .push(
            container(
                row![
                    text("File")
                        .size(11)
                        .width(Length::Fixed(88.0))
                        .style(|theme: &Theme| iced::widget::text::Style {
                            color: Some(
                                theme
                                    .extended_palette()
                                    .background
                                    .base
                                    .text
                                    .scale_alpha(0.65),
                            ),
                        }),
                    text(target_label)
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
            }),
        )
        .push(field_input(TagEditorField::Instrument, &state.instrument))
        .push(field_input(TagEditorField::Artist, &state.artist))
        .push(field_input(TagEditorField::Title, &state.title))
        .push(field_input(TagEditorField::Bpm, &state.bpm))
        .push(field_input(TagEditorField::Key, &state.key))
        .push(field_input(TagEditorField::Genre, &state.genre))
        .push(field_input(TagEditorField::Comment, &state.comment));

    if let Some(error) = &state.error {
        body = body.push(
            text(error)
                .size(12)
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.95, 0.62, 0.62)),
                }),
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
