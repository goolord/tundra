use iced::widget::{button, container, mouse_area, row, stack, text, Space};
use iced::{Alignment, Border, Element, Length, Padding, Shadow, alignment, theme};
use iced_aw::menu::{self, Menu};
use iced_aw::style::{Status, menu_bar::primary};
use iced_aw::{menu_bar, menu_items};
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};

pub use super::common::*;

const TITLE_BAR_HEIGHT: f32 = 24.0;
const WINDOW_BUTTON_WIDTH: f32 = 28.0;
const MENU_DROPDOWN_PADDING: f32 = 4.0;

pub struct MainMenu {}

impl MainMenu {
    pub fn new() -> Self {
        MainMenu {}
    }

    pub fn view(&self, always_on_top: bool, active_file: Option<&str>) -> Element<'_, Message> {
        let palette_bg = move |theme: &theme::Theme| {
            theme.extended_palette().background.weak.color
        };
        let title = window_title(active_file);
        let title_style = move |theme: &theme::Theme| iced::widget::text::Style {
            color: Some(
                theme
                    .extended_palette()
                    .background
                    .base
                    .text
                    .scale_alpha(0.55),
            ),
        };

        container(
            row![
                container(
                    stack![
                        title_bar_drag_area(
                            container(Space::new())
                                .width(Length::Fill)
                                .height(Length::Fill),
                        ),
                        container(menu_bar_widget(always_on_top))
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .align_x(alignment::Horizontal::Left),
                    ],
                )
                .width(Length::FillPortion(1))
                .height(Length::Fill),
                container(title_bar_drag_area(
                    container(
                        text(title)
                            .size(11)
                            .style(title_style)
                            .width(Length::Fill)
                            .align_x(alignment::Horizontal::Center)
                            .align_y(alignment::Vertical::Center),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(Alignment::Center),
                ))
                .width(Length::FillPortion(2))
                .height(Length::Fill),
                container(
                    stack![
                        title_bar_drag_area(
                            container(Space::new())
                                .width(Length::Fill)
                                .height(Length::Fill),
                        ),
                        container(window_controls())
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .align_x(alignment::Horizontal::Right),
                    ],
                )
                .width(Length::FillPortion(1))
                .height(Length::Fill),
            ]
            .spacing(0)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fixed(TITLE_BAR_HEIGHT))
        .padding(Padding::ZERO)
        .style(move |theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette_bg(theme).into()),
                border: Border {
                    width: 1.0,
                    color: palette.background.strong.color.scale_alpha(0.55),
                    radius: 0.0.into(),
                },
                ..Default::default()
            }
        })
        .into()
    }
}

fn title_bar_drag_area<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    mouse_area(content)
        .on_press(Message::WindowTitleBarPress)
        .on_release(Message::WindowTitleBarRelease)
        .on_double_click(Message::WindowToggleMaximize)
        .into()
}

fn window_controls() -> Element<'static, Message> {
    row![
        window_button("−", Message::WindowMinimize, WindowButtonKind::Minimize),
        window_button("□", Message::WindowToggleMaximize, WindowButtonKind::Maximize),
        window_button("×", Message::Quit, WindowButtonKind::Close),
    ]
    .spacing(0)
    .into()
}

#[derive(Clone, Copy)]
enum WindowButtonKind {
    Minimize,
    Maximize,
    Close,
}

fn window_button(
    label: &'static str,
    message: Message,
    kind: WindowButtonKind,
) -> Element<'static, Message> {
    let font_size = match kind {
        WindowButtonKind::Close => 14,
        _ => 12,
    };
    button(
        text(label)
            .size(font_size)
            .font(iced::Font {
                weight: iced::font::Weight::Medium,
                ..iced::Font::default()
            })
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center),
    )
    .width(Length::Fixed(WINDOW_BUTTON_WIDTH))
    .height(Length::Fixed(TITLE_BAR_HEIGHT))
    .padding(Padding::ZERO)
    .on_press(message)
    .style(move |theme, status| window_button_style(theme, status, kind))
    .into()
}

fn ghost_hover_fill(theme: &theme::Theme, status: ButtonStatus) -> iced::Color {
    let text = theme.extended_palette().background.base.text;
    match status {
        ButtonStatus::Hovered => text.scale_alpha(0.08),
        ButtonStatus::Pressed => text.scale_alpha(0.14),
        _ => iced::Color::TRANSPARENT,
    }
}

fn window_button_style(
    theme: &theme::Theme,
    status: ButtonStatus,
    kind: WindowButtonKind,
) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: palette.background.base.text.scale_alpha(0.82),
        border: Border {
            width: 0.0,
            radius: 0.0.into(),
            color: iced::Color::TRANSPARENT,
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };

    match (kind, status) {
        (WindowButtonKind::Close, ButtonStatus::Hovered) => {
            style.background = Some(iced::Color::from_rgb8(0xc4, 0x2b, 0x1c).into());
            style.text_color = iced::Color::WHITE;
        }
        (WindowButtonKind::Close, ButtonStatus::Pressed) => {
            style.background = Some(iced::Color::from_rgb8(0x9a, 0x1f, 0x12).into());
            style.text_color = iced::Color::WHITE;
        }
        (WindowButtonKind::Close, _) => {
            style.background = Some(iced::Color::TRANSPARENT.into());
        }
        _ => {
            style.background = Some(ghost_hover_fill(theme, status).into());
        }
    }
    style
}

fn menu_bar_widget<'a>(always_on_top: bool) -> Element<'a, Message> {
    let menu_tpl = |items| {
        Menu::new(items)
            .width(220.0)
            .max_width(260.0)
            .padding(Padding::from([MENU_DROPDOWN_PADDING, 6.0]))
            .offset(MENU_DROPDOWN_PADDING)
            .spacing(2.0)
    };

    menu_bar!(
        (menu_root("File"), menu_tpl(menu_items!(
            (menu_item("Open File…", Message::OpenFile)),
            (menu_item("Open Folder…", Message::OpenFolder)),
            (menu_item("Go to Home", Message::GoHome)),
            (menu_item("Refresh", Message::RefreshDirectory)),
            (menu_item("Settings…", Message::OpenSettings)),
            (menu_item("Auto Tag (untagged)…", Message::OpenAutoTag)),
            (menu_item("Bulk Auto Tag…", Message::OpenBulkAutoTag)),
            (menu_item("Invalidate Cache", Message::InvalidateDircache)),
            (menu_item("Quit", Message::Quit)),
        ))),
        (menu_root("View"), menu_tpl(menu_items!(
            (menu_toggle_item(
                "Always On Top",
                always_on_top,
                Message::SetAlwaysOnTop(!always_on_top),
            )),
        ))),
        (menu_root("Help"), menu_tpl(menu_items!(
            (menu_item("About Tundra", Message::About)),
        ))),
    )
    .height(Length::Fill)
    .padding(Padding::from([0.0, 2.0]))
    .spacing(0.0)
    .draw_path(menu::DrawPath::FakeHovering)
    .close_on_item_click_global(true)
    .style(menu_bar_style)
    .into()
}

fn menu_bar_style(theme: &theme::Theme, status: Status) -> iced_aw::style::menu_bar::Style {
    let palette = theme.extended_palette();
    let mut style = primary(theme, status);
    style.bar_background = palette.background.weak.color.into();
    style.bar_border = Border {
        width: 0.0,
        radius: 0.0.into(),
        ..Default::default()
    };
    style.bar_shadow = Shadow::default();
    style.menu_background = palette.background.base.color.into();
    style.menu_border = Border {
        width: 1.0,
        color: palette.background.strong.color,
        radius: 4.0.into(),
    };
    style.menu_shadow = Shadow {
        color: palette.background.base.text.scale_alpha(0.15),
        offset: iced::Vector::new(0.0, 2.0),
        blur_radius: 8.0,
    };
    style.path = palette.background.weak.color.into();
    style.path_border = Border {
        width: 0.0,
        radius: 0.0.into(),
        ..Default::default()
    };
    style
}

fn flat_button_style(theme: &theme::Theme, status: ButtonStatus) -> ButtonStyle {
    let palette = theme.extended_palette();
    ButtonStyle {
        text_color: palette.background.base.text,
        border: Border::default(),
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    }
    .with_background(ghost_hover_fill(theme, status))
}

fn menu_root_button_style(theme: &theme::Theme, status: ButtonStatus) -> ButtonStyle {
    let palette = theme.extended_palette();
    let mut style = ButtonStyle {
        text_color: palette.background.base.text,
        border: Border {
            radius: 2.0.into(),
            ..Border::default()
        },
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };

    style.background = Some(ghost_hover_fill(theme, status).into());
    style
}

fn menu_toggle_item<'a>(
    label: &'a str,
    checked: bool,
    message: Message,
) -> button::Button<'a, Message> {
    let mark = if checked { "✓" } else { " " };
    button(
        row![
            text(mark)
                .size(13)
                .width(Length::Fixed(12.0))
                .align_x(alignment::Horizontal::Center),
            text(label)
                .size(13)
                .width(Length::Fill)
                .align_y(alignment::Vertical::Center),
        ]
        .spacing(6)
        .align_y(alignment::Vertical::Center),
    )
    .width(Length::Fill)
    .padding([3, 10])
    .style(flat_button_style)
    .on_press(message)
}

fn menu_item<'a>(label: &'a str, message: Message) -> button::Button<'a, Message> {
    button(
        text(label)
            .size(13)
            .width(Length::Fill)
            .align_y(alignment::Vertical::Center),
    )
    .width(Length::Fill)
    .padding([3, 10])
    .style(flat_button_style)
    .on_press(message)
}

fn menu_root<'a>(label: &'a str) -> button::Button<'a, Message> {
    button(
        text(label)
            .size(12)
            .align_y(alignment::Vertical::Center),
    )
    .height(Length::Fixed(TITLE_BAR_HEIGHT))
    .padding([0, 8])
    .style(menu_root_button_style)
    .on_press(Message::NoOp)
}
