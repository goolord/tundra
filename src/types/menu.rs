use iced::widget::{row, svg, text};
use iced::{alignment, Length};
use iced_aw::menu::{ItemHeight, ItemWidth};

pub use super::common::*;
use super::widget::*;

#[derive(Clone)]
enum MenuMessage {}

pub struct MainMenu {}

impl MainMenu {
    pub fn new() -> Self {
        MainMenu {}
    }

    pub fn view(&self) -> MenuBar<Message> {
        MenuBar::new(vec![menu_1()])
            .item_width(ItemWidth::Uniform(180))
            .item_height(ItemHeight::Uniform(25))
    }
}

fn menu_1<'a>() -> MenuTree<'a, Message> {
    let root = MenuTree::with_children(
        labeled_button("Menu"),
        vec![item("Invalidate cache", Message::InvalidateDircache())],
    )
    .width(110);

    root
}

fn base_button<'a>(content: impl Into<Element<'a, Message>>) -> Button<'a, Message> {
    Button::new(content)
        .padding([4, 8])
        .style(super::theme::Button::MenuButton)
}

fn labeled_button(label: &str) -> Button<'_, Message> {
    base_button(
        Text::new(label)
            .width(Length::Fill)
            .height(Length::Fill)
            .vertical_alignment(alignment::Vertical::Center),
    )
}

fn item(label: &str, msg: Message) -> MenuTree<'_, Message> {
    MenuTree::new(
        labeled_button(label)
            .on_press(msg)
            .width(Length::Fill)
            .height(Length::Fill),
    )
}

// fn sub_menu<'a>(
//     label: &str,
//     msg: Message,
//     children: Vec<MenuTree<'a, Message>>,
// ) -> MenuTree<'a, Message> {
//     let handle = svg::Handle::from_path(format!(
//         "{}/caret-right-fill.svg",
//         env!("CARGO_MANIFEST_DIR")
//     ));
//     let arrow = svg(handle).width(Length::Shrink);
//
//     MenuTree::with_children(
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
