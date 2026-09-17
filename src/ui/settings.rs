//! The settings modal, and messages about the allowed directories.

use super::message::{Message, SettingsMsg};
use super::style;
use super::widgets::{modal_button, modal_shell, spacer};
use iced::widget::{Column, button, container, row, scrollable, text};
use iced::{Alignment, Element, Length};
use std::path::{Path, PathBuf};

pub const FILE_OUTSIDE_ALLOWED: &str = "File must be inside allowed directories.";
pub const FOLDER_OUTSIDE_ALLOWED: &str = "Folder must be inside allowed directories.";
pub const UNSUPPORTED_AUDIO: &str = "Choose a supported audio file.";
pub const SELECT_AUDIO_FIRST: &str = "Select an audio file first.";
pub const NO_AUDIO_SELECTED: &str = "No audio file selected";

fn directory_row(path: &Path) -> Element<'static, Message> {
    container(
        row![
            text(crate::path_util::truncate_path(path, 52)).size(12).width(Length::Fill),
            button(text("Remove").size(11))
                .padding([4, 8])
                .on_press(SettingsMsg::RemoveDirectory(path.to_path_buf()).into())
                .style(style::modal_button(false)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(style::panel(0.42, 0.24, 0.0))
    .into()
}

pub fn settings_view(allowed: &[PathBuf], first_run: bool, error: Option<&str>) -> Element<'static, Message> {
    let (title, intro, close_label) = if first_run {
        (
            "Choose search directories",
            "Tundra only searches and caches audio inside directories you allow. Add one or more folders to get started.",
            "Start",
        )
    } else {
        (
            "Settings",
            "Directories Tundra may search and cache. Changes apply immediately and prune data outside these folders.",
            "Done",
        )
    };

    let mut body = Column::new()
        .spacing(12)
        .push(text(title).size(18))
        .push(text(intro).size(13).width(Length::Fill));

    body = if allowed.is_empty() {
        body.push(
            container(text("No directories configured yet.").size(12).width(Length::Fill))
                .padding([8, 10])
                .width(Length::Fill)
                .style(style::panel(0.35, 0.22, 0.0)),
        )
    } else {
        body.push(
            scrollable(Column::with_children(allowed.iter().map(|path| directory_row(path))).spacing(6))
                .width(Length::Fill)
                .height(Length::Fixed(220.0)),
        )
    };

    if let Some(error) = error {
        body = body.push(text(error.to_owned()).size(12).color(style::ERROR));
    }

    body = body.push(
        row![
            modal_button("Add directory…", Some(SettingsMsg::PickDirectory.into()), false),
            spacer(Length::Fill, Length::Shrink),
            modal_button(close_label, Some(SettingsMsg::Close.into()), true).padding([6, 14]),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    );

    modal_shell(body.padding(18), 520.0).into()
}

