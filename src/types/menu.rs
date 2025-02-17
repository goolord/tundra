use iced::widget::{button, text};
use iced::{Element, Length, alignment};
use iced_aw::menu::{Item, Menu};
use iced_aw::{menu_bar, menu_items};

pub use super::common::*;

#[derive(Clone)]
enum MenuMessage {}

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
        debug_button_s("Menu"),
        menu_tpl_1(menu_items!(
            (debug_button_s("Invalidate cache").on_press(Message::InvalidateDircache()))
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

fn labeled_button(
    label: &str,
    msg: Option<Message>,
) -> button::Button<Message, iced::Theme, iced::Renderer> {
    base_button(text(label).align_y(alignment::Vertical::Center), msg)
}

fn debug_button(label: &str) -> button::Button<Message, iced::Theme, iced::Renderer> {
    labeled_button(label, None).width(Length::Fill)
}

fn debug_button_s(label: &str) -> button::Button<Message, iced::Theme, iced::Renderer> {
    labeled_button(label, None).width(Length::Shrink)
}

// fn sub_menu<'a>(
//     label: &str,
//     msg: Message,
//     children: Vec<Menu<'a, Message>>,
// ) -> Menu<'a, Message> {
//     let handle = svg::Handle::from_path(format!(
//         "{}/caret-right-fill.svg",
//         env!("CARGO_MANIFEST_DIR")
//     ));
//     let arrow = svg(handle).width(Length::Shrink);
//
//     Menu::with_children(
//         base_button(
//             row![
//                 text(label)
//                     .width(Length::Fill)
//                     .height(Length::Fill)
//                     .vertical_alignment(alignment::Vertical::Center),
//                 arrow
//             ],
//         )
//         .on_press(msg)
//         .width(Length::Fill)
//         .height(Length::Fill),
//         children,
//     )
// }
