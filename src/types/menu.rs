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
    let sub_5 = debug_sub_menu(
        "SUB",
        vec![
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
        ],
    );
    let sub_4 = debug_sub_menu(
        "SUB",
        vec![
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
        ],
    )
    .width(180);
    let sub_3 = debug_sub_menu(
        "More sub menus",
        vec![
            debug_item("You can"),
            debug_item("nest menus"),
            sub_4,
            debug_item("how ever"),
            debug_item("You like"),
            sub_5,
        ],
    );
    let sub_2 = debug_sub_menu(
        "Another sub menu",
        vec![
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
            sub_3,
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
        ],
    )
    .width(140);
    let sub_1 = debug_sub_menu(
        "A sub menu",
        vec![
            debug_item("Item"),
            debug_item("Item"),
            sub_2,
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
        ],
    )
    .width(220);

    let root = MenuTree::with_children(
        debug_button("Nested Menus"),
        vec![
            debug_item("Item"),
            debug_item("Item"),
            sub_1,
            debug_item("Item"),
            debug_item("Item"),
            debug_item("Item"),
        ],
    )
    .width(110);

    root
}

fn base_button<'a>(content: impl Into<Element<'a, Message>>, msg: Message) -> Button<'a, Message> {
    Button::new(content)
        .padding([4, 8])
        .style(super::theme::Button::Default)
        .on_press(msg)
}

fn labeled_button(label: &str, msg: Message) -> Button<'_, Message> {
    base_button(
        Text::new(label)
            .width(Length::Fill)
            .height(Length::Fill)
            .vertical_alignment(alignment::Vertical::Center),
        msg,
    )
}

fn debug_button(label: &str) -> Button<'_, Message> {
    labeled_button(label, Message::Debug(label.into()))
}

fn debug_item(label: &str) -> MenuTree<'_, Message> {
    MenuTree::new(debug_button(label).width(Length::Fill).height(Length::Fill))
}

fn debug_sub_menu<'a>(label: &str, children: Vec<MenuTree<'a, Message>>) -> MenuTree<'a, Message> {
    sub_menu(label, Message::Debug(label.into()), children)
}

fn sub_menu<'a>(
    label: &str,
    msg: Message,
    children: Vec<MenuTree<'a, Message>>,
) -> MenuTree<'a, Message> {
    let handle = svg::Handle::from_path(format!(
        "{}/caret-right-fill.svg",
        env!("CARGO_MANIFEST_DIR")
    ));
    let arrow = svg(handle).width(Length::Shrink);

    MenuTree::with_children(
        base_button(
            row![
                text(label)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .vertical_alignment(alignment::Vertical::Center),
                arrow
            ],
            msg,
        )
        .width(Length::Fill)
        .height(Length::Fill),
        children,
    )
}
