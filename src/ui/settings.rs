//! The settings modal, and user-facing messages shared by several views.

use super::message::{Message, SettingsMsg};
use super::style;
use super::widgets::{modal_button, modal_footer, modal_heading, modal_shell};
use iced::widget::{Column, button, container, row, scrollable, text};
use iced::{Alignment, Element, Length};
use std::path::{Path, PathBuf};

pub const FILE_OUTSIDE_ALLOWED: &str = "File must be inside allowed directories.";
pub const FOLDER_OUTSIDE_ALLOWED: &str = "Folder must be inside allowed directories.";
pub const UNSUPPORTED_AUDIO: &str = "Choose a supported audio file.";
pub const SELECT_AUDIO_FIRST: &str = "Select an audio file first.";
pub const NO_AUDIO_SELECTED: &str = "No audio file selected";
pub const AUTO_TAG_ALREADY_COMPLETE: &str = "This file already has the tags Tundra would add.";
pub const AUTO_TAG_INSTRUMENT_PRESENT: &str =
    "Instrument tag already present. Apply to fill any missing artist or comment tags.";

fn directory_row(path: &Path) -> Element<'static, Message> {
    container(
        row![
            text(crate::path_util::truncate_path(path, 52))
                .size(12)
                .width(Length::Fill),
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
    let list: Element<'static, Message> = if allowed.is_empty() {
        container(text("No directories configured yet.").size(12).width(Length::Fill))
            .padding([8, 10])
            .width(Length::Fill)
            .style(style::panel(0.35, 0.22, 0.0))
            .into()
    } else {
        scrollable(Column::with_children(allowed.iter().map(|path| directory_row(path))).spacing(6))
            .width(Length::Fill)
            .height(Length::Fixed(220.0))
            .into()
    };
    let body = modal_heading(title, intro)
        .push(list)
        .push(error.map(|error| text(error.to_owned()).size(12).color(style::ERROR)))
        .push(modal_footer(
            [modal_button(
                "Add directory…",
                Some(SettingsMsg::PickDirectory.into()),
                false,
            )],
            modal_button(close_label, Some(SettingsMsg::Close.into()), true),
        ));
    modal_shell(body.padding(18), 520.0).into()
}
