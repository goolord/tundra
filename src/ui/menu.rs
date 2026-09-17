//! The custom title bar: menus, window title, and window buttons.

use super::message::{AutoTagMsg, BulkAutoTagMsg, Message, SettingsMsg, WindowMsg};
use super::style;
use super::widgets::spacer;
use iced::widget::{button, container, mouse_area, row, stack, text};
use iced::{Alignment, Border, Color, Element, Length, Padding, Shadow, Theme, alignment};
use iced_aw::menu::{self, Menu};
use iced_aw::style::Status;
use iced_aw::{menu_bar, menu_items};

const TITLE_BAR_HEIGHT: f32 = 24.0;
const WINDOW_BUTTON_WIDTH: f32 = 28.0;
const MENU_DROPDOWN_PADDING: f32 = 4.0;

pub fn window_title(active_file: Option<&str>) -> String {
    match active_file {
        Some(name) => format!("Tundra - {name}"),
        None => "Tundra".into(),
    }
}

pub fn title_bar(always_on_top: bool, active_file: Option<&str>) -> Element<'static, Message> {
    let fill = |content: Element<'static, Message>| container(content).width(Length::Fill).height(Length::Fill);
    let blank_drag_area = || drag_area(spacer(Length::Fill, Length::Fill));
    let title = text(window_title(active_file))
        .size(11)
        .style(style::faded_text(0.55))
        .width(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center);

    container(
        row![
            fill(stack![blank_drag_area(), fill(menu_bar_widget(always_on_top))].into())
                .width(Length::FillPortion(1)),
            fill(drag_area(fill(title.into()).align_y(Alignment::Center)))
                .width(Length::FillPortion(2)),
            fill(
                stack![
                    blank_drag_area(),
                    fill(window_controls()).align_x(alignment::Horizontal::Right)
                ]
                .into()
            )
            .width(Length::FillPortion(1)),
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fixed(TITLE_BAR_HEIGHT))
    .style(|theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style::default()
            .background(palette.background.weak.color)
            .border(style::outline(palette.background.strong.color.scale_alpha(0.55), 0.0))
    })
    .into()
}

/// Pressing and dragging here moves the window; double-click maximizes.
fn drag_area(content: impl Into<Element<'static, Message>>) -> Element<'static, Message> {
    mouse_area(content)
        .on_press(WindowMsg::TitleBarPress.into())
        .on_release(WindowMsg::TitleBarRelease.into())
        .on_double_click(WindowMsg::ToggleMaximize.into())
        .into()
}

fn window_controls() -> Element<'static, Message> {
    row![
        window_button("−", WindowMsg::Minimize.into(), false),
        window_button("□", WindowMsg::ToggleMaximize.into(), false),
        window_button("×", Message::Quit, true),
    ]
    .into()
}

fn window_button(label: &'static str, message: Message, close: bool) -> Element<'static, Message> {
    button(
        text(label)
            .size(if close { 14 } else { 12 })
            .font(style::MEDIUM)
            .width(Length::Fill)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center),
    )
    .width(Length::Fixed(WINDOW_BUTTON_WIDTH))
    .height(Length::Fixed(TITLE_BAR_HEIGHT))
    .padding(Padding::ZERO)
    .on_press(message)
    .style(move |theme, status| {
        let base = button::Style {
            text_color: style::text_alpha(theme, 0.82),
            ..button::Style::default()
        };
        if !close {
            return base.with_background(ghost_hover_fill(theme, status));
        }
        let red = style::by_status(
            status,
            Color::TRANSPARENT,
            Color::from_rgb8(0xc4, 0x2b, 0x1c),
            Color::from_rgb8(0x9a, 0x1f, 0x12),
        );
        let text_color = if red == Color::TRANSPARENT { base.text_color } else { Color::WHITE };
        button::Style { text_color, ..base }.with_background(red)
    })
    .into()
}

fn ghost_hover_fill(theme: &Theme, status: button::Status) -> Color {
    style::by_status(
        status,
        Color::TRANSPARENT,
        style::text_alpha(theme, 0.08),
        style::text_alpha(theme, 0.14),
    )
}

fn menu_bar_widget(always_on_top: bool) -> Element<'static, Message> {
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
            (menu_item("Settings…", SettingsMsg::Open.into())),
            (menu_item("Auto Tag (untagged)…", AutoTagMsg::Open.into())),
            (menu_item("Bulk Auto Tag…", BulkAutoTagMsg::Open.into())),
            (menu_item("Invalidate Cache", Message::InvalidateDircache)),
            (menu_item("Quit", Message::Quit)),
        ))),
        (menu_root("View"), menu_tpl(menu_items!(
            (menu_toggle_item("Always On Top", always_on_top, Message::SetAlwaysOnTop(!always_on_top))),
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

fn menu_bar_style(theme: &Theme, _status: Status) -> iced_aw::style::menu_bar::Style {
    let palette = theme.extended_palette();
    iced_aw::style::menu_bar::Style {
        bar_background: palette.background.weak.color.into(),
        bar_border: Border::default(),
        bar_shadow: Shadow::default(),
        menu_background: palette.background.base.color.into(),
        menu_border: style::outline(palette.background.strong.color, 4.0),
        menu_shadow: style::drop_shadow(palette.background.base.text.scale_alpha(0.15), 2.0, 8.0),
        path: palette.background.weak.color.into(),
        path_border: Border::default(),
    }
}

fn flat_button_style(theme: &Theme, status: button::Status) -> button::Style {
    button::Style {
        text_color: theme.extended_palette().background.base.text,
        ..button::Style::default()
    }
    .with_background(ghost_hover_fill(theme, status))
}

fn menu_toggle_item(label: &'static str, checked: bool, message: Message) -> button::Button<'static, Message> {
    let mark = text(if checked { "✓" } else { " " })
        .size(13)
        .width(Length::Fixed(12.0))
        .align_x(alignment::Horizontal::Center);
    let label = text(label).size(13).width(Length::Fill);
    menu_button(row![mark, label].spacing(6).align_y(Alignment::Center), message)
}

fn menu_item(label: &'static str, message: Message) -> button::Button<'static, Message> {
    menu_button(text(label).size(13).width(Length::Fill), message)
}

fn menu_button(
    content: impl Into<Element<'static, Message>>,
    message: Message,
) -> button::Button<'static, Message> {
    button(content)
        .width(Length::Fill)
        .padding([3, 10])
        .style(flat_button_style)
        .on_press(message)
}

fn menu_root(label: &'static str) -> button::Button<'static, Message> {
    button(text(label).size(12).align_y(alignment::Vertical::Center))
        .height(Length::Fixed(TITLE_BAR_HEIGHT))
        .padding([0, 8])
        .style(|theme, status| button::Style {
            border: Border::default().rounded(2.0),
            ..flat_button_style(theme, status)
        })
        .on_press(Message::NoOp)
}
