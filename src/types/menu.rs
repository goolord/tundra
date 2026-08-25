use iced::widget::{button, container, text};
use iced::{Border, Element, Length, Padding, Shadow, alignment, theme};
use iced_aw::menu::{self, Menu};
use iced_aw::style::{Status, menu_bar::primary};
use iced_aw::{menu_bar, menu_items};
use iced::widget::button::{Status as ButtonStatus, Style as ButtonStyle};

pub use super::common::*;

pub struct MainMenu {}

impl MainMenu {
    pub fn new() -> Self {
        MainMenu {}
    }

    pub fn view(&self) -> Element<'_, Message> {
        let bar = menu_bar_widget();

        container(bar)
            .width(Length::Fill)
            .height(Length::Fixed(28.0))
            .padding(Padding::ZERO)
            .style(|theme| {
                let palette = theme.extended_palette();
                container::Style {
                    background: Some(palette.background.weak.color.into()),
                    border: Border {
                        width: 1.0,
                        color: palette.background.strong.color,
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

fn menu_bar_widget<'a>() -> Element<'a, Message> {
    let menu_tpl = |items| {
        Menu::new(items)
            .width(220.0)
            .max_width(260.0)
            .offset(0.0)
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
        (menu_root("Help"), menu_tpl(menu_items!(
            (menu_item("About Tundra", Message::About)),
        ))),
    )
    .height(Length::Fill)
    .width(Length::Fill)
    .padding(Padding::from([0.0, 4.0]))
    .spacing(0.0)
    .draw_path(menu::DrawPath::Backdrop)
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
    let base = ButtonStyle {
        text_color: palette.background.base.text,
        border: Border::default(),
        shadow: Shadow::default(),
        ..ButtonStyle::default()
    };

    match status {
        ButtonStatus::Active | ButtonStatus::Disabled => {
            base.with_background(iced::Color::TRANSPARENT)
        }
        ButtonStatus::Hovered => base.with_background(palette.primary.weak.color.scale_alpha(0.35)),
        ButtonStatus::Pressed => base.with_background(palette.primary.weak.color.scale_alpha(0.55)),
    }
}

fn menu_item<'a>(label: &'a str, message: Message) -> button::Button<'a, Message> {
    button(
        text(label)
            .size(14)
            .width(Length::Fill)
            .align_y(alignment::Vertical::Center),
    )
    .width(Length::Fill)
    .padding([4, 12])
    .style(flat_button_style)
    .on_press(message)
}

fn menu_root<'a>(label: &'a str) -> button::Button<'a, Message> {
    button(
        text(label)
            .size(14)
            .align_y(alignment::Vertical::Center),
    )
    .padding([4, 10])
    .style(flat_button_style)
}
