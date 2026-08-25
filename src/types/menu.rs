use iced::widget::{button, text};
use iced::{Element, Length, alignment};
use iced_aw::menu::Menu;
use iced_aw::{menu_bar, menu_items};

pub use super::common::*;

pub struct MainMenu {}

impl MainMenu {
    pub fn new() -> Self {
        MainMenu {}
    }

    pub fn view(&self) -> Element<'_, Message> {
        menu_1()
    }
}

fn menu_1<'a>() -> Element<'a, Message> {
    let menu_tpl_1 = |items| Menu::new(items).max_width(180.0).offset(15.0).spacing(5.0);
    let root = menu_bar!((
        menu_button("Menu"),
        menu_tpl_1(menu_items!(
            (menu_button("Invalidate cache").on_press(Message::InvalidateDircache))
        ))
    ))
    .width(110);

    root.into()
}

fn base_button<'a>(
    content: impl Into<Element<'a, Message>>,
    msg: Option<Message>,
) -> button::Button<'a, Message> {
    let button = button(content)
        .padding([4, 8])
        .style(iced::widget::button::primary);
    match msg {
        None => button,
        Some(m) => button.on_press(m),
    }
}

fn menu_button<'a>(label: &'a str) -> button::Button<'a, Message> {
    base_button(
        text(label).align_y(alignment::Vertical::Center),
        None,
    )
    .width(Length::Shrink)
}
