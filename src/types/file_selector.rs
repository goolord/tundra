pub use super::common::*;
use crate::metadata::{tag_field_best_match, tag_field_suggestions, TagField, TagFilter};
use super::settings::FavoritesStore;
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};
use iced::widget::canvas::{self, Action, Event, Frame, Program};
use iced::widget::canvas::Path as CanvasPath;
use iced::widget::scrollable::{self, Scrollbar, Status as ScrollableStatus};
use iced::widget::text::Wrapping;
use iced::widget::{button, container, mouse_area, row, scrollable as scrollable_widget, stack, text, Button, Column, Row, Space, TextInput};
use iced::widget::Id;
use iced::{Alignment, Background, Border, Color, Element, Length, Rectangle, Shadow, theme};
use iced::keyboard::Modifiers;
use iced::mouse::{self, Cursor};
use iced_aw::ContextMenu;

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const TUNDRA_MUTED_ICON: Color = Color::from_rgb8(0x52, 0x56, 0x5c);
const TAG_CHIP_RADIUS: f32 = 16.0;
const FILTER_INPUT_RADIUS: f32 = 0.0;
pub const FILE_LIST_SCROLL_ID: &str = "file-list-scroll";
/// Fixed height of every file row; windowed rendering relies on this being exact.
pub const FILE_ROW_HEIGHT: f32 = 31.0;
/// Extra rows rendered above and below the viewport to absorb fast scrolling.
const FILE_ROW_OVERDRAW: usize = 12;
/// Fallback for windowed row rendering until the first scroll event reports height.
const FILE_LIST_RENDER_VIEWPORT_FALLBACK: f32 = 2400.0;
pub const FILE_LIST_SCROLLBAR_WIDTH: f32 = 10.0;
pub const FILE_LIST_SCROLLBAR_MIN_THUMB: f32 = 36.0;
pub const TAG_SEARCH_INPUT_ID: &str = "tag-search-input";
pub const FILE_SEARCH_INPUT_ID: &str = "file-search-input";

fn file_list_scrollable_style(_theme: &theme::Theme, _status: ScrollableStatus) -> scrollable::Style {
    let transparent_scroller = scrollable::Scroller {
        background: Background::Color(Color::TRANSPARENT),
        border: Border {
            width: 0.0,
            color: Color::TRANSPARENT,
            radius: 0.0.into(),
        },
    };
    let transparent_rail = scrollable::Rail {
        background: None,
        border: Border {
            width: 0.0,
            color: Color::TRANSPARENT,
            radius: 0.0.into(),
        },
        scroller: transparent_scroller,
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: transparent_rail,
        horizontal_rail: transparent_rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            shadow: Shadow::default(),
            icon: Color::TRANSPARENT,
        },
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FileListScrollMetrics {
    pub max_scroll: f32,
    pub thumb_height: f32,
    pub thumb_top: f32,
    pub scroll_range: f32,
}

pub fn file_list_render_viewport_height(list_viewport_height: f32) -> f32 {
    if list_viewport_height > 0.0 {
        list_viewport_height
    } else {
        FILE_LIST_RENDER_VIEWPORT_FALLBACK
    }
}

pub fn file_list_scroll_metrics(
    total_rows: usize,
    scroll_offset: f32,
    track_height: f32,
) -> FileListScrollMetrics {
    if track_height <= 0.0 {
        return FileListScrollMetrics {
            max_scroll: 0.0,
            thumb_height: 0.0,
            thumb_top: 0.0,
            scroll_range: 0.0,
        };
    }

    let content_height = total_rows as f32 * FILE_ROW_HEIGHT;
    let max_scroll = (content_height - track_height).max(0.0);
    if max_scroll <= 0.0 {
        return FileListScrollMetrics {
            max_scroll: 0.0,
            thumb_height: track_height,
            thumb_top: 0.0,
            scroll_range: 0.0,
        };
    }

    let ratio = track_height / content_height;
    let min_thumb = FILE_LIST_SCROLLBAR_MIN_THUMB.min(track_height * 0.9);
    let mut thumb_height = (track_height * ratio).clamp(min_thumb, track_height);
    let mut scroll_range = track_height - thumb_height;
    if scroll_range <= 0.0 {
        thumb_height = track_height * 0.25;
        scroll_range = track_height - thumb_height;
    }
    let thumb_top = (scroll_offset / max_scroll) * scroll_range;
    FileListScrollMetrics {
        max_scroll,
        thumb_height,
        thumb_top,
        scroll_range,
    }
}

pub fn file_list_scroll_metrics_for(selector: &FileSelector) -> FileListScrollMetrics {
    let track_height = selector.list_viewport_height.max(0.0);
    file_list_scroll_metrics(
        selector.file_list.len(),
        selector.list_scroll_offset,
        track_height,
    )
}

pub fn file_list_scrollbar_grab_offset(metrics: &FileListScrollMetrics, track_y: f32) -> f32 {
    if track_y >= metrics.thumb_top && track_y <= metrics.thumb_top + metrics.thumb_height {
        track_y - metrics.thumb_top
    } else {
        metrics.thumb_height / 2.0
    }
}

pub fn file_list_scroll_offset_for_track_y(
    metrics: &FileListScrollMetrics,
    track_y: f32,
    grab_offset: f32,
) -> f32 {
    if metrics.scroll_range <= 0.0 || metrics.max_scroll <= 0.0 {
        return 0.0;
    }
    let thumb_top = (track_y - grab_offset).clamp(0.0, metrics.scroll_range);
    (thumb_top / metrics.scroll_range) * metrics.max_scroll
}

#[derive(Debug, Clone)]
struct FileListScrollbar {
    total_rows: usize,
    scroll_offset: f32,
    list_viewport_height: f32,
}

impl FileListScrollbar {
    fn metrics(&self) -> FileListScrollMetrics {
        file_list_scroll_metrics(
            self.total_rows,
            self.scroll_offset,
            self.list_viewport_height.max(0.0),
        )
    }
}

impl Program<Message> for FileListScrollbar {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Option<Action<Message>> {
        let metrics = self.metrics();
        if metrics.max_scroll <= 0.0 {
            return None;
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                cursor.position_in(bounds).map(|position| {
                    Action::publish(Message::FileListScrollbarPress {
                        track_y: position.y,
                        track_top: bounds.y,
                        track_height: bounds.height,
                    })
                    .and_capture()
                })
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        theme: &theme::Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        let metrics = self.metrics();
        let mut frame = Frame::new(renderer, bounds.size());
        if metrics.max_scroll <= 0.0 {
            return vec![frame.into_geometry()];
        }

        let thumb_bounds = Rectangle {
            x: 1.0,
            y: metrics.thumb_top,
            width: bounds.width - 2.0,
            height: metrics.thumb_height,
        };
        let hovered = cursor
            .position_in(bounds)
            .is_some_and(|point| {
                point.y >= thumb_bounds.y
                    && point.y <= thumb_bounds.y + thumb_bounds.height
            });

        let thumb_color = ui_muted_text(theme).scale_alpha(if hovered { 0.72 } else { 0.48 });
        let thumb = CanvasPath::rectangle(thumb_bounds.position(), thumb_bounds.size());
        frame.fill(&thumb, thumb_color);
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> mouse::Interaction {
        let metrics = self.metrics();
        if metrics.max_scroll <= 0.0 {
            return mouse::Interaction::default();
        }
        let Some(point) = cursor.position_in(bounds) else {
            return mouse::Interaction::default();
        };
        let on_thumb =
            point.y >= metrics.thumb_top && point.y <= metrics.thumb_top + metrics.thumb_height;
        if on_thumb {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::Pointer
        }
    }
}

fn file_list_scrollbar(
    total_rows: usize,
    scroll_offset: f32,
    list_viewport_height: f32,
) -> Element<'static, Message> {
    iced::widget::canvas(FileListScrollbar {
        total_rows,
        scroll_offset,
        list_viewport_height,
    })
    .width(Length::Fixed(FILE_LIST_SCROLLBAR_WIDTH))
    .height(Length::Fill)
    .into()
}

#[derive(Debug, Clone)]
pub struct FileSelector {
    pub current_dir: PathBuf,
    pub file_list: Vec<FileButton>,
    selected: HashSet<usize>,
    selection_anchor: Option<usize>,
    pub hovered_file: Option<usize>,
    pub search_value: String,
    pub search_case_sensitive: bool,
    pub search_show_directories: bool,
    pub favorites_only: bool,
    pub tag_search_value: String,
    pub tag_filters: Vec<TagFilter>,
    pub tag_search_error: Option<String>,
    pub list_error: Option<String>,
    pub list_scroll_offset: f32,
    pub list_viewport_height: f32,
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
            ui_muted_text(theme)
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
                    TUNDRA_ACCENT.scale_alpha(0.20)
                } else {
                    Color::TRANSPARENT
                }
                .into(),
            );
        }
        ButtonStatus::Hovered => {
            style.text_color = palette.background.base.text;
            style.background = Some(
                if selected {
                    TUNDRA_ACCENT.scale_alpha(0.28)
                } else {
                    TUNDRA_ACCENT.scale_alpha(0.12)
                }
                .into(),
            );
        }
        ButtonStatus::Pressed => {
            style.text_color = palette.background.base.text;
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.34).into());
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

fn file_tree_row(content: Element<'_, Message>, selected: bool) -> Element<'_, Message> {
    Row::new()
        .push(selection_stripe(selected, 3.0, Length::Fill))
        .push(content)
        .width(Length::Fill)
        .height(Length::Fixed(FILE_ROW_HEIGHT))
        .into()
}

fn file_row_menu<'a>(
    content: impl Into<Element<'a, Message>>,
    path: PathBuf,
    extras: bool,
    search_enabled: bool,
    favorite_label: Option<String>,
    selected: bool,
) -> Element<'a, Message> {
    file_tree_row(
        ContextMenu::new(content, move || {
            file_context_menu(
                Message::FileCopyName(path.clone()),
                Message::FileCopyPath(path.clone()),
                Message::FileRevealInFileManager(path.clone()),
                (extras && search_enabled).then(|| Message::OpenAutoTagFor(path.clone())),
                extras.then(|| {
                    (
                        favorite_label
                            .clone()
                            .unwrap_or_else(|| "Add to favorites".into()),
                        Message::ToggleFavorite(path.clone()),
                    )
                }),
                extras.then(|| Message::OpenTagEditorFor(path.clone())),
            )
        })
        .style(context_menu_style)
        .into(),
        selected,
    )
}

fn tag_chip_close_style(theme: &theme::Theme, status: ButtonStatus) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: palette.background.base.text.scale_alpha(0.55),
        border: Border {
            radius: 8.0.into(),
            ..Default::default()
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };
    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(Color::TRANSPARENT.into());
        }
        ButtonStatus::Hovered => {
            style.text_color = palette.background.base.text;
            style.background = Some(UI_DANGER.scale_alpha(0.22).into());
        }
        ButtonStatus::Pressed => {
            style.text_color = palette.background.base.text;
            style.background = Some(UI_DANGER.scale_alpha(0.38).into());
        }
    }
    style
}

fn tag_chip(filter: &TagFilter) -> Element<'static, Message> {
    let field = filter.field;
    let accent = tag_field_color(field);
    let value = filter.value.clone();

    container(
        row![
            container(
                text(field.as_str())
                    .size(10)
                    .font(iced::Font {
                        weight: iced::font::Weight::Semibold,
                        ..iced::Font::default()
                    })
                    .style(move |theme: &theme::Theme| {
                        let palette = theme.extended_palette();
                        iced::widget::text::Style {
                            color: Some(palette.background.base.text.scale_alpha(0.95)),
                        }
                    }),
            )
            .padding([3, 7])
            .style(move |_theme| container::Style {
                background: Some(accent.scale_alpha(0.55).into()),
                border: Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            text(value)
                .size(12)
                .font(iced::Font {
                    weight: iced::font::Weight::Medium,
                    ..iced::Font::default()
                })
                .style(|theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(theme.extended_palette().background.base.text),
                }),
            button(text("×").size(13))
                .on_press(Message::TagFilterRemove(field))
                .padding([2, 4])
                .style(tag_chip_close_style),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([5, 8])
    .style(move |theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.base.color.scale_alpha(0.55).into()),
            border: Border {
                radius: TAG_CHIP_RADIUS.into(),
                width: 1.0,
                color: accent.scale_alpha(0.35),
            },
            shadow: Shadow {
                color: palette.background.base.color.scale_alpha(0.35),
                offset: iced::Vector::new(0.0, 2.0),
                blur_radius: 6.0,
            },
            ..Default::default()
        }
    })
    .into()
}

fn accent_badge(label: impl Into<String>, size: u32) -> Element<'static, Message> {
    container(
        text(label.into())
            .size(size)
            .font(iced::Font {
                weight: iced::font::Weight::Semibold,
                ..iced::Font::default()
            })
            .style(|_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(TUNDRA_ACCENT.scale_alpha(0.95)),
            }),
    )
    .padding([2, 6])
    .style(|_theme| container::Style {
        background: Some(TUNDRA_ACCENT.scale_alpha(0.18).into()),
        border: Border {
            radius: 8.0.into(),
            width: 1.0,
            color: TUNDRA_ACCENT.scale_alpha(0.28),
        },
        ..Default::default()
    })
    .into()
}

fn filter_dock_accent_bar() -> Element<'static, Message> {
    container(
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(2.0)),
    )
    .width(Length::Fill)
    .style(|_theme| container::Style {
        background: Some(TUNDRA_ACCENT.scale_alpha(0.42).into()),
        ..Default::default()
    })
    .into()
}

fn filter_dock_style(theme: &theme::Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(sidebar_panel(theme).scale_alpha(0.98).into()),
        border: Border {
            width: 1.0,
            color: palette.background.strong.color.scale_alpha(0.30),
            radius: 0.0.into(),
        },
        shadow: Shadow {
            color: palette.background.base.color.scale_alpha(0.55),
            offset: iced::Vector::new(0.0, -4.0),
            blur_radius: 14.0,
        },
        ..Default::default()
    }
}

fn filter_section_divider() -> Element<'static, Message> {
    container(
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(1.0)),
    )
    .width(Length::Fill)
    .style(|theme: &theme::Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.strong.color.scale_alpha(0.22).into()),
            ..Default::default()
        }
    })
    .into()
}

fn file_list_select_message(index: usize, modifiers: Modifiers) -> Message {
    Message::FileListSelect {
        index,
        shift: modifiers.shift(),
        control: modifiers.control() || modifiers.logo(),
    }
}

fn selection_status_label(count: usize) -> Element<'static, Message> {
    container(
        text(format!("{count} selected · Shift/Ctrl+click to extend"))
            .size(10)
            .style(|theme: &theme::Theme| iced::widget::text::Style {
                color: Some(
                    theme
                        .extended_palette()
                        .background
                        .base
                        .text
                        .scale_alpha(0.58),
                ),
            }),
    )
    .padding([4, 10])
    .width(Length::Fill)
    .into()
}

fn filter_section_header(icon: &'static str, title: &'static str) -> Row<'static, Message> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            resource_svg(icon)
                .width(Length::Fixed(12.0))
                .height(Length::Fixed(12.0))
                .style(|_theme, _status| iced::widget::svg::Style {
                    color: Some(TUNDRA_ACCENT.scale_alpha(0.85)),
                }),
        )
        .push(
            text(title)
                .size(11)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..iced::Font::default()
                })
                .style(|theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(ui_muted_text(theme)),
                }),
        )
}

fn file_search_header(
    active: bool,
    case_sensitive: bool,
    show_directories: bool,
    favorites_only: bool,
) -> Element<'static, Message> {
    let mut header = filter_section_header("search-solid.svg", "File search");

    if active {
        header = header.push(accent_badge("active", 9));
    }

    header
        .push(Space::new().width(Length::Fill))
        .push(file_search_favorites_button(favorites_only))
        .push(file_search_directories_button(show_directories))
        .push(file_search_case_button(case_sensitive))
        .into()
}

fn toggle_chip_style(theme: &theme::Theme, status: ButtonStatus, active: bool) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: palette.background.base.text,
        border: Border {
            radius: 6.0.into(),
            width: 1.0,
            color: if active {
                TUNDRA_ACCENT.scale_alpha(0.35)
            } else {
                palette.background.strong.color.scale_alpha(0.24)
            },
        },
        ..ButtonStyle::default()
    };
    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            style.background = Some(
                if active {
                    TUNDRA_ACCENT.scale_alpha(0.14)
                } else {
                    Color::TRANSPARENT
                }
                .into(),
            );
        }
        ButtonStatus::Hovered => {
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.18).into());
        }
        ButtonStatus::Pressed => {
            style.background = Some(TUNDRA_ACCENT.scale_alpha(0.26).into());
        }
    }
    style
}

fn toggle_chip<'a>(
    content: impl Into<Element<'a, Message>>,
    active: bool,
    message: Message,
) -> Element<'a, Message> {
    button(content)
        .padding([2, 6])
        .on_press(message)
        .style(move |theme, status| toggle_chip_style(theme, status, active))
        .into()
}

fn file_search_favorites_button(favorites_only: bool) -> Element<'static, Message> {
    toggle_chip(
        text(if favorites_only { "★" } else { "☆" })
            .size(13)
            .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if favorites_only {
                    TUNDRA_ACCENT.scale_alpha(0.95)
                } else {
                    TUNDRA_MUTED_ICON
                }),
            }),
        favorites_only,
        Message::ToggleFavoritesOnly,
    )
}

fn favorites_list_header() -> Element<'static, Message> {
    container(
        row![
            text("★")
                .size(12)
                .style(|_theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(TUNDRA_ACCENT.scale_alpha(0.95)),
                }),
            text("Favorites")
                .size(11)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..iced::Font::default()
                })
                .style(|theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(ui_muted_text(theme)),
                }),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([8, 10])
    .style(sidebar_section_style)
    .into()
}

fn favorite_star_button(path: PathBuf, favorite: bool) -> Element<'static, Message> {
    const STAR_ICON: f32 = 11.0;
    const STAR_BTN: f32 = 15.0;
    let star = resource_svg("star-solid.svg")
        .width(Length::Fixed(STAR_ICON))
        .height(Length::Fixed(STAR_ICON))
        .style(move |_theme, _status| iced::widget::svg::Style {
            color: Some(if favorite {
                TUNDRA_ACCENT.scale_alpha(0.95)
            } else {
                TUNDRA_MUTED_ICON.scale_alpha(0.42)
            }),
        });
    button(
        container(star)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    )
    .padding(0)
    .width(Length::Fixed(STAR_BTN))
    .height(Length::Fixed(STAR_BTN))
    .on_press(Message::ToggleFavorite(path))
    .style(move |_theme: &theme::Theme, status| {
        let show_border = favorite
            || matches!(status, ButtonStatus::Hovered | ButtonStatus::Pressed);
        let mut style = ButtonStyle {
            text_color: TUNDRA_MUTED_ICON,
            border: Border {
                radius: 4.0.into(),
                width: 1.0,
                color: if show_border {
                    TUNDRA_ACCENT.scale_alpha(if favorite { 0.28 } else { 0.16 })
                } else {
                    Color::TRANSPARENT
                },
            },
            ..ButtonStyle::default()
        };
        match status {
            ButtonStatus::Active | ButtonStatus::Disabled => {
                style.background = Some(
                    if favorite {
                        TUNDRA_ACCENT.scale_alpha(0.10)
                    } else {
                        Color::TRANSPARENT
                    }
                    .into(),
                );
            }
            ButtonStatus::Hovered => {
                style.background = Some(TUNDRA_ACCENT.scale_alpha(0.15).into());
            }
            ButtonStatus::Pressed => {
                style.background = Some(TUNDRA_ACCENT.scale_alpha(0.22).into());
            }
        }
        style
    })
    .into()
}

fn file_search_directories_button(show_directories: bool) -> Element<'static, Message> {
    toggle_chip(
        resource_svg("folder-solid.svg")
            .width(Length::Fixed(12.0))
            .height(Length::Fixed(12.0))
            .style(move |_theme, _status| iced::widget::svg::Style {
                color: Some(if show_directories {
                    TUNDRA_ACCENT.scale_alpha(0.95)
                } else {
                    TUNDRA_MUTED_ICON
                }),
            }),
        show_directories,
        Message::ToggleSearchShowDirectories,
    )
}

fn file_search_case_button(case_sensitive: bool) -> Element<'static, Message> {
    toggle_chip(
        row![
            text("a")
                .size(11)
                .style(move |theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if case_sensitive {
                        ui_muted_text(theme).scale_alpha(0.72)
                    } else {
                        TUNDRA_ACCENT.scale_alpha(0.95)
                    }),
                }),
            text("A")
                .size(11)
                .font(iced::Font {
                    weight: iced::font::Weight::Semibold,
                    ..iced::Font::default()
                })
                .style(move |theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(if case_sensitive {
                        TUNDRA_ACCENT.scale_alpha(0.95)
                    } else {
                        ui_muted_text(theme).scale_alpha(0.72)
                    }),
                }),
        ]
        .spacing(0)
        .align_y(Alignment::Center),
        case_sensitive,
        Message::ToggleSearchCaseSensitive,
    )
}

fn tag_section_header(filter_count: usize) -> Element<'static, Message> {
    let mut header = filter_section_header("music-solid.svg", "Tag filters");

    if filter_count > 0 {
        header = header
            .push(accent_badge(filter_count.to_string(), 10))
            .push(accent_badge("active", 9));
    }

    header.push(Space::new().width(Length::Fill)).into()
}

fn filter_label_row(content: Element<'_, Message>) -> Element<'_, Message> {
    container(content)
        .width(Length::Fill)
        .padding([8, 10])
        .into()
}

fn filter_helper_text(label: &str) -> Element<'_, Message> {
    filter_label_row(
        text(label)
            .size(10)
            .style(|theme: &theme::Theme| iced::widget::text::Style {
                color: Some(ui_muted_text(theme).scale_alpha(0.85)),
            })
            .into(),
    )
}

fn tag_suggestion_row(field: TagField, highlighted: bool) -> Element<'static, Message> {
    let name = field.as_str().to_owned();
    let hint = field.label().to_owned();
    button(
        row![
            container(
                text(name)
                    .size(11)
                    .font(iced::Font {
                        weight: iced::font::Weight::Semibold,
                        ..iced::Font::default()
                    })
                    .style(move |_theme: &theme::Theme| iced::widget::text::Style {
                        color: Some(tag_field_color(field).scale_alpha(0.95)),
                    }),
            )
            .padding([2, 0]),
            text(":")
                .size(11)
                .style(|theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(ui_muted_text(theme)),
                }),
            Space::new().width(Length::Fill),
            text(hint)
                .size(10)
                .style(|theme: &theme::Theme| iced::widget::text::Style {
                    color: Some(ui_muted_text(theme)),
                }),
        ]
        .spacing(4)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .on_press(Message::TagSuggestionSelect(field))
    .width(Length::Fill)
    .padding([6, 10])
    .style(move |theme, status| file_tree_button_style(theme, status, highlighted))
    .into()
}

fn tag_suggestions_panel(suggestions: Vec<Element<'static, Message>>) -> Element<'static, Message> {
    container(
        Column::with_children(suggestions)
            .spacing(0)
            .padding([4, 0]),
    )
    .width(Length::Fill)
    .style(|theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.base.color.scale_alpha(0.94).into()),
            border: Border {
                radius: FILTER_INPUT_RADIUS.into(),
                width: 1.0,
                color: palette.background.strong.color.scale_alpha(0.40),
            },
            shadow: Shadow {
                color: palette.background.base.color.scale_alpha(0.65),
                offset: iced::Vector::new(0.0, 4.0),
                blur_radius: 10.0,
            },
            ..Default::default()
        }
    })
    .into()
}

fn tag_suggestions_slot(suggestions: Vec<Element<'static, Message>>) -> Element<'static, Message> {
    if suggestions.is_empty() {
        container(Space::new())
            .height(Length::Fixed(0.0))
            .width(Length::Fill)
            .into()
    } else {
        tag_suggestions_panel(suggestions)
    }
}

const FILTER_CLEAR_TEXT_SIZE: f32 = 13.0;
const FILTER_CLEAR_PAD_X: f32 = 8.0;
const FILTER_CLEAR_PAD_Y: f32 = 4.0;
const FILTER_CLEAR_MIN_HIT_WIDTH: f32 = 24.0;

fn filter_clear_inset() -> f32 {
    (FILTER_CLEAR_TEXT_SIZE + FILTER_CLEAR_PAD_X * 2.0).max(FILTER_CLEAR_MIN_HIT_WIDTH)
}

fn filter_clear_button_style(theme: &theme::Theme, status: ButtonStatus) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: palette.background.base.text.scale_alpha(0.45),
        background: Some(Color::TRANSPARENT.into()),
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };
    match status {
        ButtonStatus::Hovered | ButtonStatus::Pressed => {
            style.text_color = palette.background.base.text.scale_alpha(0.85);
        }
        ButtonStatus::Active | ButtonStatus::Disabled => {}
    }
    style
}

fn filter_clear_button(on_press: Message) -> Element<'static, Message> {
    button(text("×").size(FILTER_CLEAR_TEXT_SIZE))
        .on_press(on_press)
        .padding([FILTER_CLEAR_PAD_Y, FILTER_CLEAR_PAD_X])
        .style(filter_clear_button_style)
        .into()
}

fn filter_input_with_clear(
    input: Element<'_, Message>,
    show_clear: bool,
    on_clear: Message,
    on_activate: Message,
    active: bool,
) -> Element<'_, Message> {
    let clear_slot: Element<'_, Message> = if show_clear {
        filter_clear_button(on_clear)
    } else {
        Space::new()
            .width(Length::Fixed(filter_clear_inset()))
            .height(Length::Fixed(0.0))
            .into()
    };

    // The overlay spans the whole input so the button can sit inside its right padding.
    // It must stay transparent to the mouse everywhere except the button itself: `Space` and
    // `button` report `Interaction::None` outside their own bounds, which is what lets clicks
    // reach the TextInput underneath. Giving this layer a background, a `mouse_area`, or any
    // other widget that claims an interaction will silently break click-to-focus.
    // When inactive, the click-catcher sits under this overlay so × still clears.
    let clear_overlay = container(clear_slot)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Center);

    if active {
        container(stack![input, clear_overlay].width(Length::Fill))
            .width(Length::Fill)
            .into()
    } else {
        container(
            stack![
                input,
                mouse_area(
                    Space::new()
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .on_press(on_activate),
                clear_overlay,
            ]
            .width(Length::Fill),
        )
        .width(Length::Fill)
        .into()
    }
}

fn filter_input_padding() -> iced::Padding {
    iced::Padding::from([8.0, 10.0]).right(filter_clear_inset())
}

fn file_search_input(search_value: &str, active: bool) -> Element<'_, Message> {
    let show_clear = !search_value.is_empty();

    let input = mouse_area(
        TextInput::new("Search files…", search_value)
            .id(Id::new(FILE_SEARCH_INPUT_ID))
            .on_input(Message::Search)
            .size(13)
            .padding(filter_input_padding())
            .width(Length::Fill),
    )
    .on_press(Message::SearchFocused(true))
    .into();

    filter_input_with_clear(
        input,
        show_clear,
        Message::Search(String::new()),
        Message::SearchFocused(true),
        active,
    )
}

fn tag_search_input(tag_search_value: &str, active: bool) -> Element<'_, Message> {
    let show_clear = !tag_search_value.is_empty();

    let input = mouse_area(
        TextInput::new("title:value — Enter or Tab", tag_search_value)
            .id(Id::new(TAG_SEARCH_INPUT_ID))
            .on_input(Message::TagSearchInput)
            .on_submit(Message::TagSearchSubmit)
            .size(12)
            .padding(filter_input_padding())
            .width(Length::Fill),
    )
    .on_press(Message::TagSearchFocused(true))
    .into();

    filter_input_with_clear(
        input,
        show_clear,
        Message::TagSearchInput(String::new()),
        Message::TagSearchFocused(true),
        active,
    )
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

fn sidebar_section_style(theme: &theme::Theme) -> container::Style {
    let mut style = section_divider(theme);
    style.background = Some(sidebar_panel(theme).into());
    style
}

impl DirUp {
    pub fn view(&self, cwd: PathBuf) -> Element<'_, Message> {
        let path_label = truncate_path(&cwd, 32);
        let content = Button::new(
            row![
                resource_svg("up_chevron.svg")
                    .height(Length::Fixed(14.0))
                    .width(Length::Fixed(14.0))
                    .style(|theme, _status| iced::widget::svg::Style {
                        color: Some(tree_icon_color(theme, false)),
                    }),
                text(path_label)
                    .size(11)
                    .style(|theme: &theme::Theme| iced::widget::text::Style {
                        color: Some(ui_muted_text(theme)),
                    }),
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
            .style(sidebar_section_style)
            .into()
    }
}

impl FileList {
    pub fn file_filter(x: &Path) -> bool {
        (x.is_dir() && !is_hidden(x)) || is_audio(x)
    }

    pub fn list_buttons(dir: &Path) -> (Vec<FileButton>, Option<String>) {
        crate::path_util::reclaim_write_sidecars(dir);
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
                let file_type = entry.file_type().ok()?;
                let path = entry.path();
                let is_dir = if file_type.is_symlink() {
                    path.is_dir()
                } else {
                    file_type.is_dir()
                };
                Self::file_filter(&path).then(|| FileButton::with_kind(path, dir, is_dir))
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
            selected: HashSet::new(),
            selection_anchor: None,
            hovered_file: None,
            search_value: String::new(),
            search_case_sensitive: false,
            search_show_directories: true,
            favorites_only: false,
            tag_search_value: String::new(),
            tag_filters: Vec::new(),
            tag_search_error: None,
            list_error,
            list_scroll_offset: 0.0,
            list_viewport_height: 0.0,
        }
    }

    pub fn reload_directory(&mut self, dir: &Path) {
        let search_value = self.search_value.clone();
        let search_case_sensitive = self.search_case_sensitive;
        let search_show_directories = self.search_show_directories;
        let favorites_only = self.favorites_only;
        let tag_search_value = self.tag_search_value.clone();
        let tag_filters = self.tag_filters.clone();
        let tag_search_error = self.tag_search_error.clone();
        let search_active = self.search_active();
        self.current_dir = dir.to_owned();
        if !search_active {
            let (file_list, list_error) = FileList::list_buttons(dir);
            self.file_list = file_list;
            self.list_error = list_error;
        }
        self.selected.clear();
        self.selection_anchor = None;
        self.hovered_file = None;
        self.search_value = search_value;
        self.search_case_sensitive = search_case_sensitive;
        self.search_show_directories = search_show_directories;
        self.favorites_only = favorites_only;
        self.tag_search_value = tag_search_value;
        self.tag_filters = tag_filters;
        self.tag_search_error = tag_search_error;
    }

    pub fn search_active(&self) -> bool {
        crate::metadata::file_search_active(&self.search_value, &self.tag_filters)
    }

    pub fn tag_only_search(&self) -> bool {
        !self.tag_filters.is_empty() && self.search_value.trim().is_empty()
    }

    pub fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    pub fn select_row(&mut self, index: usize, shift: bool, control: bool) {
        if index >= self.file_list.len() {
            return;
        }

        if control {
            if self.selected.contains(&index) {
                self.selected.remove(&index);
            } else {
                self.selected.insert(index);
            }
            self.selection_anchor = Some(index);
            return;
        }

        if shift {
            if let Some(anchor) = self.selection_anchor {
                let lo = anchor.min(index);
                let hi = anchor.max(index);
                self.selected.clear();
                for i in lo..=hi {
                    self.selected.insert(i);
                }
                return;
            }
        }

        self.selected.clear();
        self.selected.insert(index);
        self.selection_anchor = Some(index);
    }

    pub fn clear_selection(&mut self) {
        self.selected.clear();
        self.selection_anchor = None;
    }

    pub fn sync_selection_for_path(&mut self, path: &Path) {
        let key = crate::path_util::cache_key(path.to_path_buf());
        let Some(index) = self.file_list.iter().position(|entry| {
            crate::path_util::cache_key(entry.file_path.clone()) == key
        }) else {
            return;
        };
        self.selected.clear();
        self.selected.insert(index);
        self.selection_anchor = Some(index);
    }

    fn primary_index(&self) -> Option<usize> {
        self.selection_anchor
            .or_else(|| self.selected.iter().copied().min())
    }

    pub fn selected_audio_path(&self) -> Option<PathBuf> {
        self.primary_index()
            .and_then(|index| self.file_list.get(index))
            .filter(|entry| !entry.is_dir && is_audio(&entry.file_path))
            .map(|entry| entry.file_path.clone())
    }

    pub fn add_tag_filter(&mut self, filter: TagFilter) {
        if let Some(existing) = self
            .tag_filters
            .iter()
            .position(|entry| entry.field == filter.field)
        {
            self.tag_filters[existing] = filter;
        } else {
            self.tag_filters.push(filter);
        }
    }

    fn tag_suggestions(&self) -> Vec<Element<'static, Message>> {
        let best_match = tag_field_best_match(&self.tag_search_value);
        tag_field_suggestions(&self.tag_search_value)
            .into_iter()
            .map(|field| {
                tag_suggestion_row(field, best_match == Some(field))
            })
            .collect()
    }

    pub fn view(
        &self,
        search_enabled: bool,
        favorites: &FavoritesStore,
        modifiers: Modifiers,
        filter_focus: FilterFocus,
    ) -> Column<'_, Message> {
        let mut column = Column::new().spacing(0).height(Length::Fill);

        if self.favorites_only {
            column = column.push(favorites_list_header());
        } else {
            column = column.push(DirUp.view(self.current_dir.to_owned()));
        }

        if self.selected_count() > 1 {
            column = column.push(selection_status_label(self.selected_count()));
        }

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

        // Windowed rendering: only build widgets for rows near the viewport;
        // spacers stand in for the rest so scrollbar geometry stays correct.
        let total = self.file_list.len();
        let viewport_height = file_list_render_viewport_height(self.list_viewport_height);
        let rows_in_view = (viewport_height / FILE_ROW_HEIGHT).ceil() as usize + 1;
        let first_in_view = ((self.list_scroll_offset / FILE_ROW_HEIGHT).floor() as usize)
            .min(total.saturating_sub(rows_in_view));
        let start = first_in_view.saturating_sub(FILE_ROW_OVERDRAW);
        let end = (first_in_view + rows_in_view + FILE_ROW_OVERDRAW).min(total);

        let mut new_col: Vec<Element<Message>> = Vec::with_capacity(end - start + 2);
        if start > 0 {
            new_col.push(
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(start as f32 * FILE_ROW_HEIGHT))
                    .into(),
            );
        }
        for (index, button) in self.file_list[start..end].iter().enumerate() {
            let index = start + index;
            new_col.push(button.view(
                index,
                self.is_selected(index),
                self.hovered_file == Some(index),
                search_enabled,
                favorites.contains(&button.file_path),
                modifiers,
            ));
        }
        if end < total {
            new_col.push(
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed((total - end) as f32 * FILE_ROW_HEIGHT))
                    .into(),
            );
        }
        let fs = scrollable_widget(Column::with_children(new_col).spacing(0))
            .id(Id::new(FILE_LIST_SCROLL_ID))
            .direction(scrollable::Direction::Vertical(Scrollbar::hidden()))
            .style(file_list_scrollable_style)
            .on_scroll(Message::FileListScrolled)
            .height(Length::Fill);

        let list_with_scrollbar = mouse_area(
            row![
                fs.width(Length::Fill),
                file_list_scrollbar(total, self.list_scroll_offset, self.list_viewport_height),
            ]
            .spacing(0)
            .height(Length::Fill),
        )
        .on_enter(Message::FileListHoverChanged(true))
        .on_exit(Message::FileListHoverChanged(false));

        let file_search_active = self.search_active();

        let mut filter_body = Column::new()
            .spacing(0)
            .push(filter_label_row(file_search_header(
                file_search_active,
                self.search_case_sensitive,
                self.search_show_directories,
                self.favorites_only,
            )))
            .push(file_search_input(
                &self.search_value,
                filter_focus == FilterFocus::FileSearch,
            ))
            .push(filter_section_divider())
            .push(filter_label_row(tag_section_header(self.tag_filters.len())));

        if !self.tag_filters.is_empty() {
            let chips: Vec<Element<Message>> = self
                .tag_filters
                .iter()
                .map(tag_chip)
                .collect();
            filter_body = filter_body.push(
                container(
                    Row::with_children(chips)
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .width(Length::Fill)
                        .wrap()
                        .vertical_spacing(8),
                )
                .width(Length::Fill)
                .padding([4, 10]),
            );
        }

        filter_body = filter_body.push(filter_helper_text(
            "Add filters like bpm:120, key:Am, or instrument:Kick",
        ));

        if let Some(error) = &self.tag_search_error {
            filter_body = filter_body.push(
                container(
                    text(error)
                        .size(11)
                        .style(modal_error_style),
                )
                .padding([6, 10])
                .width(Length::Fill)
                .style(|_theme| container::Style {
                    background: Some(UI_DANGER.scale_alpha(0.12).into()),
                    border: Border {
                        radius: 0.0.into(),
                        width: 1.0,
                        color: UI_DANGER.scale_alpha(0.28),
                    },
                    ..Default::default()
                }),
            );
        }

        let suggestions = if !self.tag_search_value.is_empty() && !self.tag_search_value.contains(':') {
            self.tag_suggestions()
        } else {
            Vec::new()
        };
        filter_body = filter_body.push(tag_suggestions_slot(suggestions));
        filter_body = filter_body.push(tag_search_input(
            &self.tag_search_value,
            filter_focus == FilterFocus::TagSearch,
        ));

        let filter_dock = container(
            Column::new()
                .push(filter_dock_accent_bar())
                .push(filter_body),
        )
        .width(Length::Fill)
        .style(filter_dock_style);

        if !search_enabled {
            return column.push(list_with_scrollbar);
        }

        column.push(list_with_scrollbar).push(filter_dock)
    }
}

fn file_tree_label(
    label: &str,
    selected: bool,
    hovered: bool,
    is_dir: bool,
) -> Element<'_, Message> {
    let selected_copy = selected;
    let hovered_copy = hovered;
    container(
        text(label)
            .size(13)
            .wrapping(Wrapping::None)
            .width(Length::Fill)
            .style(move |theme: &theme::Theme| iced::widget::text::Style {
                color: Some(if selected_copy || hovered_copy {
                    theme.extended_palette().background.base.text
                } else {
                    ui_muted_text(theme)
                }),
            })
            .font(iced::Font {
                weight: if is_dir {
                    iced::font::Weight::Medium
                } else {
                    iced::font::Weight::Normal
                },
                ..iced::Font::default()
            }),
    )
    .width(Length::Fill)
    .clip(true)
    .into()
}

impl FileButton {
    pub fn with_kind(path: PathBuf, base_path: &Path, is_dir: bool) -> Self {
        let label = path
            .strip_prefix(base_path)
            .ok()
            .and_then(crate::path_util::file_name_lossy)
            .unwrap_or_else(|| crate::path_util::file_label(&path));
        FileButton {
            file_path: path,
            label,
            is_dir,
        }
    }

    pub fn view(
        &self,
        index: usize,
        selected: bool,
        hovered: bool,
        search_enabled: bool,
        is_favorite: bool,
        modifiers: Modifiers,
    ) -> Element<'_, Message> {
        let multi_select = modifiers.shift() || modifiers.control() || modifiers.logo();
        let select_message = file_list_select_message(index, modifiers);
        let selected_copy = selected;
        let hovered_copy = hovered;
        let label = file_tree_label(&self.label, selected, hovered, self.is_dir);

        let label = if self.is_dir {
            row![
                resource_svg("folder-solid.svg")
                    .width(Length::Fixed(16.0))
                    .height(Length::Fixed(16.0))
                    .style(move |theme, _status| iced::widget::svg::Style {
                        color: Some(tree_icon_color(theme, selected_copy || hovered_copy)),
                    }),
                label,
            ]
            .spacing(10)
            .align_y(Alignment::Center)
            .width(Length::Fill)
        } else if is_audio(&self.file_path) {
            row![
                favorite_star_button(self.file_path.clone(), is_favorite),
                row![
                    resource_svg("music-solid.svg")
                        .width(Length::Fixed(12.0))
                        .height(Length::Fixed(12.0))
                        .style(move |theme, _status| iced::widget::svg::Style {
                            color: Some(tree_icon_color(theme, selected_copy || hovered_copy)),
                        }),
                    label,
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            ]
            .spacing(1)
            .align_y(Alignment::Center)
            .width(Length::Fill)
        } else {
            row![label].align_y(Alignment::Center).width(Length::Fill)
        };

        let row_status = if hovered {
            ButtonStatus::Hovered
        } else {
            ButtonStatus::Active
        };

        if self.is_dir {
            let path = self.file_path.clone();
            let button = Button::new(label)
                .on_press(select_message)
                .width(Length::Fill)
                .padding([7, 10])
                .style(move |theme, button_status| {
                    file_tree_button_style(theme, button_status, selected)
                });

            return file_row_menu(button, path, false, search_enabled, None, selected);
        }

        let row_content = container(label)
            .width(Length::Fill)
            .padding([7, 10])
            .style(move |theme| file_tree_row_container_style(theme, row_status, selected));

        let path = self.file_path.clone();
        let favorite_label = if is_favorite {
            "Remove from favorites"
        } else {
            "Add to favorites"
        }
        .to_string();

        if !is_audio(&self.file_path) {
            let button = Button::new(row_content)
                .on_press(select_message)
                .width(Length::Fill)
                .padding(0)
                .style(move |theme, button_status| {
                    file_tree_button_style(theme, button_status, selected)
                });

            return file_row_menu(button, path, false, search_enabled, None, selected);
        }

        if multi_select {
            let button = Button::new(row_content)
                .on_press(select_message)
                .width(Length::Fill)
                .padding(0)
                .style(move |theme, button_status| {
                    file_tree_button_style(theme, button_status, selected)
                });

            return file_row_menu(
                button,
                path,
                true,
                search_enabled,
                Some(favorite_label),
                selected,
            );
        }

        let draggable = mouse_area(row_content)
            .on_press(Message::FileDragPress {
                path: self.file_path.to_owned(),
                from_file_list: true,
            })
            .on_move(|point| Message::CursorMoved(point))
            .on_enter(Message::FileRowHover(index))
            .on_exit(Message::FileRowLeave)
            .interaction(iced::mouse::Interaction::Grab);

        file_row_menu(
            draggable,
            path,
            true,
            search_enabled,
            Some(favorite_label),
            selected,
        )
    }
}
