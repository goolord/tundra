//! Small widget builders reused across views.

use super::message::Message;
use super::style;
use iced::widget::svg::Handle;
use iced::widget::{Button, Column, Row, Space, Svg, button, column, container, row, text};
use iced::{Alignment, Border, Color, Element, Length, Theme};

mod embedded_resources {
    include!(concat!(env!("OUT_DIR"), "/embedded_resources.rs"));
}

fn resource_handle(name: &str) -> Handle {
    embedded_resources::handle(name).unwrap_or_else(|| {
        debug_assert!(false, "unknown embedded resource: {name}");
        eprintln!("Unknown resource {name:?}; falling back to play.svg");
        embedded_resources::handle("play.svg").expect("play.svg must be embedded at build time")
    })
}

/// A square SVG from `resources/`, tinted by `color`.
pub fn icon<'a>(name: &str, size: f32, color: impl Fn(&Theme) -> Color + 'a) -> Svg<'a> {
    Svg::new(resource_handle(name))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(style::icon_color(color))
}

/// An empty box of the given size, for gaps and flexible fill.
pub fn spacer(width: Length, height: Length) -> Space {
    Space::new().width(width).height(height)
}

/// A solid bar: dividers, accent strips, the selection stripe.
pub fn bar<'a>(width: Length, height: Length, color: impl Fn(&Theme) -> Color + 'a) -> Element<'a, Message> {
    container(spacer(Length::Fill, Length::Fill))
        .width(width)
        .height(height)
        .style(move |theme| container::background(color(theme)))
        .into()
}

pub fn selection_stripe(selected: bool, width: f32, height: Length) -> Element<'static, Message> {
    let color = if selected { style::ACCENT } else { Color::TRANSPARENT };
    bar(Length::Fixed(width), height, move |_| color)
}

/// The card every modal sits in.
pub fn modal_shell<'a>(body: impl Into<Element<'a, Message>>, width: f32) -> container::Container<'a, Message> {
    container(body).width(Length::Fixed(width)).style(style::card(0.0))
}

/// A modal footer or toolbar button; `None` disables it.
pub fn modal_button<'a>(label: &'a str, message: Option<Message>, primary: bool) -> Button<'a, Message> {
    button(text(label).size(12))
        .padding([6, 12])
        .on_press_maybe(message)
        .style(style::modal_button(primary))
}

/// A modal's title and intro, the top of its body column.
pub fn modal_heading<'a>(title: &'a str, intro: &'a str) -> Column<'a, Message> {
    column![text(title).size(18), text(intro).size(13).width(Length::Fill)].spacing(12)
}

/// A modal's footer: `actions` on the left, `last` (close or confirm) on the right.
pub fn modal_footer<'a>(
    actions: impl IntoIterator<Item = Button<'a, Message>>,
    last: Button<'a, Message>,
) -> Element<'a, Message> {
    Row::with_children(actions.into_iter().map(Element::from))
        .push(spacer(Length::Fill, Length::Shrink))
        .push(last.padding([6, 14]))
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .into()
}

/// A label/value line in a modal.
pub fn modal_info_row<'a>(label: &'a str, value: impl Into<std::borrow::Cow<'a, str>>) -> Element<'a, Message> {
    container(
        row![
            text(label)
                .size(11)
                .width(Length::Fixed(88.0))
                .style(style::faded_text(0.65)),
            text(value.into()).size(12).width(Length::Fill),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(style::panel(0.35, 0.22, 0.0))
    .into()
}

fn context_menu_button(label: &str, message: Message) -> Element<'static, Message> {
    button(text(label.to_owned()).size(14).width(Length::Fill))
        .width(Length::Fill)
        .padding([4, 12])
        .style(|theme: &Theme, status| {
            let palette = theme.extended_palette();
            let hover = palette.primary.weak.color;
            let background = style::by_status(
                status,
                palette.background.base.color,
                hover.scale_alpha(0.35),
                hover.scale_alpha(0.55),
            );
            style::solid_button(palette.background.base.text, Border::default(), background)
        })
        .on_press(message)
        .into()
}

pub fn context_menu_style(theme: &Theme, _status: iced_aw::style::Status) -> iced_aw::style::context_menu::Style {
    iced_aw::style::context_menu::Style {
        background: theme.extended_palette().background.base.color.into(),
    }
}

/// Actions a file's right-click menu offers, beyond copy and reveal.
#[derive(Default)]
pub struct FileMenuExtras {
    pub auto_tag: Option<Message>,
    pub edit_tags: Option<Message>,
    pub favorite: Option<(&'static str, Message)>,
}

pub fn file_context_menu(
    copy_name: Message,
    copy_path: Message,
    reveal: Message,
    extras: FileMenuExtras,
) -> Element<'static, Message> {
    let optional = [
        extras.auto_tag.map(|message| ("Auto-tag", message)),
        extras.edit_tags.map(|message| ("Edit tags…", message)),
        extras.favorite,
    ];
    let always = [
        ("Copy name", copy_name),
        ("Copy full path", copy_path),
        (crate::platform::file_manager_label(), reveal),
    ];
    column(
        optional
            .into_iter()
            .flatten()
            .chain(always)
            .map(|(label, message)| context_menu_button(label, message)),
    )
    .spacing(2)
    .padding([4, 0])
    .width(220)
    .into()
}
