use crate::types::theme::Theme;

pub type Renderer = iced::Renderer<Theme>;
pub type Element<'a, Message> = iced::Element<'a, Message, Renderer>;
pub type Container<'a, Message> = iced::widget::Container<'a, Message, Renderer>;
pub type Button<'a, Message> = iced::widget::Button<'a, Message, Renderer>;
pub type TextInput<'a, Message> = iced::widget::TextInput<'a, Message, Renderer>;
pub type Row<'a, Message> = iced::widget::Row<'a, Message, Renderer>;
pub type Column<'a, Message> = iced::widget::Column<'a, Message, Renderer>;
pub type Canvas<Message, P> = iced::widget::Canvas<Message, Theme, P>;
pub type Svg = iced::widget::Svg<Renderer>;
pub type Slider<'a, T, Message> = iced::widget::Slider<'a, T, Message, Renderer>;
pub type Space = iced::widget::Space;
pub type Text<'a> = iced::widget::Text<'a, Renderer>;
pub type MenuBar<'a, Message> = iced_aw::menu::MenuBar<'a, Message, Renderer>;
pub type MenuTree<'a, Message> = iced_aw::menu::MenuTree<'a, Message, Renderer>;
