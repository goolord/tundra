//! The custom title bar: menus, window title, and window buttons.

use super::message::{AutoTagMsg, BulkAutoTagMsg, Message, SettingsMsg, WindowMsg};
use super::style;
use super::widgets::spacer;
use iced::widget::{button, container, mouse_area, row, stack, text};
use iced::{Alignment, Border, Color, Element, Length, Padding, Shadow, Theme, alignment};
use iced_aw::menu::{self, Item, Menu, MenuBar};

const TITLE_BAR_HEIGHT: f32 = 24.0;
const WINDOW_BUTTON_WIDTH: f32 = 28.0;
const MENU_DROPDOWN_PADDING: f32 = 4.0;

pub fn window_title(active_file: Option<&str>) -> String {
    active_file.map_or_else(|| "Tundra".into(), |name| format!("Tundra - {name}"))
}

pub fn title_bar(always_on_top: bool, active_file: Option<&str>) -> Element<'static, Message> {
    let fill = |content: Element<'static, Message>| container(content).width(Length::Fill).height(Length::Fill);
    // Blank space behind the menus and window buttons still drags the window.
    let over_drag_area = |content: Element<'static, Message>| {
        fill(stack![drag_area(spacer(Length::Fill, Length::Fill)), content].into()).width(Length::FillPortion(1))
    };
    let title = text(window_title(active_file))
        .size(11)
        .style(style::faded_text(0.55))
        .width(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center);
    let window_controls = row![
        window_button("−", WindowMsg::Minimize.into(), false),
        window_button("□", WindowMsg::ToggleMaximize.into(), false),
        window_button("×", Message::Quit, true),
    ];

    container(
        row![
            over_drag_area(fill(menu_bar_widget(always_on_top)).into()),
            fill(drag_area(fill(title.into()).align_y(Alignment::Center))).width(Length::FillPortion(2)),
            over_drag_area(fill(window_controls.into()).align_x(alignment::Horizontal::Right).into()),
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
        if !close {
            return ghost_button(0.82)(theme, status);
        }
        let red = style::by_status(
            status,
            None,
            Some(Color::from_rgb8(0xc4, 0x2b, 0x1c)),
            Some(Color::from_rgb8(0x9a, 0x1f, 0x12)),
        );
        let text_color = if red.is_some() { Color::WHITE } else { style::text_alpha(theme, 0.82) };
        style::solid_button(text_color, Border::default(), red.unwrap_or(Color::TRANSPARENT))
    })
    .into()
}

/// A borderless button in body text at `alpha` that only shows a fill on hover.
fn ghost_button(alpha: f32) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let fill = style::by_status(
            status,
            Color::TRANSPARENT,
            style::text_alpha(theme, 0.08),
            style::text_alpha(theme, 0.14),
        );
        style::solid_button(style::text_alpha(theme, alpha), Border::default(), fill)
    }
}

fn menu_bar_widget(always_on_top: bool) -> Element<'static, Message> {
    let menu = |label: &'static str, items: Vec<button::Button<'static, Message>>| {
        let items = Menu::new(items.into_iter().map(Item::new).collect())
            .width(220.0)
            .max_width(260.0)
            .padding(Padding::from([MENU_DROPDOWN_PADDING, 6.0]))
            .offset(MENU_DROPDOWN_PADDING)
            .spacing(2.0);
        // `NoOp` enables the hover style; the menu bar handles the click itself.
        let root = button(text(label).size(12).align_y(alignment::Vertical::Center))
            .height(Length::Fixed(TITLE_BAR_HEIGHT))
            .padding([0, 8])
            .style(|theme, status| button::Style {
                border: Border::default().rounded(2.0),
                ..ghost_button(1.0)(theme, status)
            })
            .on_press(Message::NoOp);
        Item::with_menu(root, items)
    };
    let menu_button = |content: Element<'static, Message>, message| {
        button(content).width(Length::Fill).padding([3, 10]).style(ghost_button(1.0)).on_press(message)
    };
    let menu_item = |label, message| menu_button(text(label).size(13).width(Length::Fill).into(), message);
    let file = [
        ("Open File…", Message::OpenFile),
        ("Open Folder…", Message::OpenFolder),
        ("Go to Home", Message::GoHome),
        ("Refresh", Message::RefreshDirectory),
        ("Settings…", SettingsMsg::Open.into()),
        ("Auto Tag (untagged)…", AutoTagMsg::Open.into()),
        ("Bulk Auto Tag…", BulkAutoTagMsg::Open.into()),
        ("Invalidate Cache", Message::InvalidateDircache),
        ("Quit", Message::Quit),
    ];
    let on_top_mark = text(if always_on_top { "✓" } else { " " })
        .size(13)
        .width(Length::Fixed(12.0))
        .align_x(alignment::Horizontal::Center);
    let on_top =
        row![on_top_mark, text("Always On Top").size(13).width(Length::Fill)].spacing(6).align_y(Alignment::Center);

    MenuBar::new(vec![
        menu("File", file.into_iter().map(|(label, message)| menu_item(label, message)).collect()),
        menu("View", vec![menu_button(on_top.into(), Message::SetAlwaysOnTop(!always_on_top))]),
        menu("Help", vec![menu_item("About Tundra", Message::About)]),
    ])
    .height(Length::Fill)
    .padding(Padding::from([0.0, 2.0]))
    .spacing(0.0)
    .draw_path(menu::DrawPath::FakeHovering)
    .close_on_item_click_global(true)
    .style(|theme: &Theme, _status| {
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
    })
    .into()
}
