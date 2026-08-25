pub use super::common::*;
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::widget::{container, mouse_area, row, scrollable, text, Button, Column, Row, Space, Svg, TextInput};
use iced::widget::Id;
use iced::{Alignment, Border, Color, Element, Length, Shadow, theme};
use iced_aw::ContextMenu;

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

const TUNDRA_ACCENT: Color = Color::from_rgb8(0x50, 0x7a, 0xe0);
pub const FILE_LIST_SCROLL_ID: &str = "file-list-scroll";

#[derive(Debug, Clone)]
pub struct FileSelector {
    pub current_dir: PathBuf,
    pub file_list: Vec<FileButton>,
    pub selected_file: Option<usize>,
    pub hovered_file: Option<usize>,
    pub search_value: String,
    pub list_error: Option<String>,
}

pub struct FileList;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileButton {
    pub file_path: PathBuf,
    pub label: String,
    pub is_dir: bool,
}

pub struct DirUp;

fn sidebar_panel(theme: &theme::Theme) -> Color {
    let base = theme.extended_palette().background.base.color;
    Color::from_rgb(base.r * 0.52, base.g * 0.52, base.b * 0.54)
}

fn muted_text(theme: &theme::Theme) -> Color {
    theme
        .extended_palette()
        .background
        .base
        .text
        .scale_alpha(0.72)
}

fn tree_icon_color(theme: &theme::Theme, emphasized: bool) -> Color {
    let palette = theme.extended_palette();
    if emphasized {
        palette.primary.base.color.scale_alpha(0.85)
    } else {
        palette.background.base.text.scale_alpha(0.62)
    }
}

fn file_tree_button_style(
    theme: &theme::Theme,
    status: ButtonStatus,
    selected: bool,
) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: if selected {
            palette.background.base.text
        } else {
            muted_text(theme)
        },
        border: Border {
            width: 0.0,
            radius: 0.0.into(),
            ..Default::default()
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };

    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(
                if selected {
                    TUNDRA_ACCENT.scale_alpha(0.28)
                } else {
                    Color::TRANSPARENT
                }
                .into(),
            );
        }
        ButtonStatus::Hovered => {
            style.text_color = palette.background.base.text;
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.55).into());
        }
        ButtonStatus::Pressed => {
            style.text_color = palette.background.base.text;
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.72).into());
        }
    }

    style
}

fn file_tree_row_container_style(
    theme: &theme::Theme,
    status: ButtonStatus,
    selected: bool,
) -> container::Style {
    let button = file_tree_button_style(theme, status, selected);
    container::Style {
        background: button.background,
        ..Default::default()
    }
}

fn selection_stripe(selected: bool) -> Element<'static, Message> {
    container(
        Space::new()
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fixed(3.0))
    .height(Length::Fill)
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

fn file_tree_row(content: Element<'_, Message>, selected: bool) -> Element<'_, Message> {
    Row::new()
        .push(selection_stripe(selected))
        .push(content)
        .width(Length::Fill)
        .into()
}

fn section_divider(theme: &theme::Theme) -> container::Style {
    container::Style {
        border: Border {
            width: 1.0,
            color: theme
                .extended_palette()
                .background
                .strong
                .color
                .scale_alpha(0.35),
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

impl DirUp {
    pub fn view(&self, cwd: PathBuf) -> Element<'_, Message> {
        let path_label = truncate_path(&cwd, 32);
        let content = Button::new(
            row![
                Svg::from_path(resource_path("up_chevron.svg"))
                    .height(Length::Fixed(14.0))
                    .width(Length::Fixed(14.0))
                    .style(|theme, _status| iced::widget::svg::Style {
                        color: Some(tree_icon_color(theme, false)),
                    }),
                text(path_label)
                    .size(11)
                    .color(Color::from_rgb(0.62, 0.66, 0.72)),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .on_press(Message::ChangeDirectory(match cwd.parent() {
            Some(x) => x.to_path_buf(),
            None => cwd,
        }))
        .width(Length::Fill)
        .padding([8, 10])
        .style(|theme, status| file_tree_button_style(theme, status, false));

        container(content)
            .width(Length::Fill)
            .style(move |theme| {
                let mut style = section_divider(theme);
                style.background = Some(sidebar_panel(theme).into());
                style
            })
            .into()
    }
}

impl FileList {
    pub fn file_filter(x: &Path) -> bool {
        (x.is_dir() && !is_hidden(x)) || is_audio(x)
    }

    pub fn list_buttons(dir: &Path) -> (Vec<FileButton>, Option<String>) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                return (
                    Vec::new(),
                    Some(format!("Cannot read {}: {err}", dir.display())),
                );
            }
        };

        let mut buttons: Vec<FileButton> = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                if Self::file_filter(&entry.path()) {
                    Some(FileButton::new(entry.path(), dir))
                } else {
                    None
                }
            })
            .collect();
        buttons.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a.label.to_lowercase().cmp(&b.label.to_lowercase()),
        });
        (buttons, None)
    }
}

impl FileSelector {
    pub fn new(dir: &Path) -> Self {
        let (file_list, list_error) = FileList::list_buttons(dir);
        FileSelector {
            current_dir: dir.to_owned(),
            file_list,
            selected_file: None,
            hovered_file: None,
            search_value: String::new(),
            list_error,
        }
    }

    pub fn view(&self) -> Column<'_, Message> {
        let mut column = Column::new()
            .push(DirUp.view(self.current_dir.to_owned()))
            .spacing(0)
            .height(Length::Fill);

        if let Some(error) = &self.list_error {
            column = column.push(
                container(
                    text(error)
                        .size(12)
                        .color(Color::from_rgb(0.92, 0.55, 0.55)),
                )
                .padding([8, 12])
                .width(Length::Fill),
            );
        }

        let new_col: Vec<Element<Message>> = self
            .file_list
            .iter()
            .enumerate()
            .map(|(index, button)| {
                button.view(
                    index,
                    self.selected_file == Some(index),
                    self.hovered_file == Some(index),
                )
            })
            .collect();

        let fs = scrollable(Column::with_children(new_col).spacing(0))
            .id(Id::new(FILE_LIST_SCROLL_ID))
            .height(Length::Fill);

        let search = container(
            TextInput::new("Search files…", &self.search_value)
                .on_input(Message::Search)
                .size(13)
                .padding([8, 10])
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(|theme| {
            let mut style = section_divider(theme);
            style.background = Some(sidebar_panel(theme).into());
            style
        });

        column.push(fs).push(search)
    }
}

impl FileButton {
    pub fn new(path: PathBuf, base_path: &Path) -> Self {
        let label = path
            .strip_prefix(base_path)
            .ok()
            .and_then(|relative| relative.file_name())
            .or_else(|| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let is_dir = path.is_dir();
        FileButton {
            file_path: path,
            label,
            is_dir,
        }
    }

    pub fn view(&self, index: usize, selected: bool, hovered: bool) -> Element<'_, Message> {
        let selected_copy = selected;
        let hovered_copy = hovered;
        let label_text = text(&self.label)
            .size(13)
            .style(move |theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if selected_copy || hovered_copy {
                    theme.extended_palette().background.base.text
                } else {
                    muted_text(theme)
                }),
            })
            .font(iced::Font {
                weight: if self.is_dir {
                    iced::font::Weight::Medium
                } else {
                    iced::font::Weight::Normal
                },
                ..iced::Font::default()
            });

        let label = if self.is_dir {
            row![
                Svg::from_path(resource_path("folder-solid.svg"))
                    .width(Length::Fixed(16.0))
                    .height(Length::Fixed(16.0))
                    .style(move |theme, _status| iced::widget::svg::Style {
                        color: Some(tree_icon_color(theme, selected_copy || hovered_copy)),
                    }),
                label_text,
            ]
            .spacing(10)
            .align_y(Alignment::Center)
        } else if is_audio(&self.file_path) {
            row![
                Svg::from_path(resource_path("music-solid.svg"))
                    .width(Length::Fixed(16.0))
                    .height(Length::Fixed(16.0))
                    .style(move |theme, _status| iced::widget::svg::Style {
                        color: Some(tree_icon_color(theme, selected_copy || hovered_copy)),
                    }),
                label_text,
            ]
            .spacing(10)
            .align_y(Alignment::Center)
        } else {
            row![label_text].align_y(Alignment::Center)
        };

        let row_status = if hovered {
            ButtonStatus::Hovered
        } else {
            ButtonStatus::Active
        };

        if self.is_dir {
            let path = self.file_path.clone();
            let button = Button::new(label)
                .on_press(Message::SelectedFile(Some(self.file_path.to_owned())))
                .width(Length::Fill)
                .padding([7, 10])
                .style(move |theme, button_status| {
                    file_tree_button_style(theme, button_status, selected)
                });

            return file_tree_row(
                ContextMenu::new(button, move || {
                    file_context_menu(
                        Message::FileCopyName(path.clone()),
                        Message::FileCopyPath(path.clone()),
                        Message::FileRevealInFileManager(path.clone()),
                    )
                })
                .style(context_menu_style)
                .into(),
                selected,
            );
        }

        let row_content = container(label)
            .width(Length::Fill)
            .padding([7, 10])
            .style(move |theme| file_tree_row_container_style(theme, row_status, selected));

        let path = self.file_path.clone();

        if !is_audio(&self.file_path) {
            let button = Button::new(row_content)
                .on_press(Message::SelectedFile(Some(self.file_path.to_owned())))
                .width(Length::Fill)
                .padding(0)
                .style(move |theme, button_status| {
                    file_tree_button_style(theme, button_status, selected)
                });

            return file_tree_row(
                ContextMenu::new(button, move || {
                    file_context_menu(
                        Message::FileCopyName(path.clone()),
                        Message::FileCopyPath(path.clone()),
                        Message::FileRevealInFileManager(path.clone()),
                    )
                })
                .style(context_menu_style)
                .into(),
                selected,
            );
        }

        let draggable = mouse_area(row_content)
            .on_press(Message::FileDragPress(self.file_path.to_owned()))
            .on_move(|point| Message::CursorMoved(point))
            .on_enter(Message::FileRowHover(index))
            .on_exit(Message::FileRowLeave)
            .interaction(iced::mouse::Interaction::Grab);

        file_tree_row(
            ContextMenu::new(draggable, move || {
                file_context_menu(
                    Message::FileCopyName(path.clone()),
                    Message::FileCopyPath(path.clone()),
                    Message::FileRevealInFileManager(path.clone()),
                )
            })
            .style(context_menu_style)
            .into(),
            selected,
        )
    }
}

impl PartialOrd for FileButton {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileButton {
    fn cmp(&self, other: &Self) -> Ordering {
        self.file_path.cmp(&other.file_path)
    }
}
