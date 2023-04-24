use iced::theme::{Button, Container, TextInput};
use iced::widget::{button, container, text_input};
use iced::{Background, Color};

// TODO: Refactor all of the types into this and use it instead of the default iced stuff
pub struct Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum ThemeType {
    #[default]
    Light,
    Dark,
}



const ACTIVE: Color = Color::from_rgb(
    0x72 as f32 / 255.0,
    0x89 as f32 / 255.0,
    0xDA as f32 / 255.0,
);

const INACTIVE: Color = Color::from_rgb(
    0x72 as f32 / 255.0,
    0x89 as f32 / 255.0,
    0xDA as f32 / 255.0,
);

///////////////////////////////////////////////////////////////////////////

pub struct SelectedContainer;

impl container::StyleSheet for SelectedContainer {
    type Style = iced::Theme;

    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        container::Appearance {
            text_color: Some(Color::from_rgb8(0xff_u8, 0xff_u8, 0xff_u8)),
            background: Some(Background::Color(Color::from_rgb8(
                0x25_u8, 0x7a_u8, 0xfd_u8,
            ))),
            ..Default::default()
        }
    }
}

impl From<SelectedContainer> for iced::theme::Container {
    fn from(value: SelectedContainer) -> Self {
        Container::Custom(Box::new(value))
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct FileButton_;

impl FileButton_ {
    fn default_style() -> button::Appearance {
        button::Appearance {
            text_color: Color::from_rgb8(0xff, 0xff, 0xff),
            ..button::Appearance::default()
        }
    }
}

impl button::StyleSheet for FileButton_ {
    type Style = iced::Theme;

    fn active(&self, _style: &Self::Style) -> button::Appearance {
        button::Appearance {
            background: None,
            ..FileButton_::default_style()
        }
    }

    fn hovered(&self, _style: &Self::Style) -> button::Appearance {
        button::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x31, 0x34, 0x38))),
            ..FileButton_::default_style()
        }
    }
}

impl From<FileButton_> for iced::theme::Button {
    fn from(value: FileButton_) -> Self {
        Button::Custom(Box::new(value))
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct ControlButton_;

impl ControlButton_ {
    fn default_style() -> button::Appearance {
        button::Appearance {
            text_color: Color::from_rgb8(0xff, 0xff, 0xff),
            border_width: 2.0,
            border_color: Color::from_rgb8(0x19, 0x1d, 0x20),
            ..button::Appearance::default()
        }
    }
}

impl button::StyleSheet for ControlButton_ {
    type Style = iced::Theme;
    fn active(&self, _style: &Self::Style) -> button::Appearance {
        button::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x31, 0x34, 0x38))),
            ..ControlButton_::default_style()
        }
    }

    fn hovered(&self, _style: &Self::Style) -> button::Appearance {
        button::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x37, 0x3a, 0x3e))),
            ..ControlButton_::default_style()
        }
    }
}

impl From<ControlButton_> for iced::theme::Button {
    fn from(value: ControlButton_) -> Self {
        Button::Custom(Box::new(value))
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct Container_;

impl container::StyleSheet for Container_ {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        container::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x23, 0x27, 0x2a))),
            ..container::Appearance::default()
        }
    }
}

impl From<Container_> for iced::theme::Container {
    fn from(value: Container_) -> Self {
        Container::Custom(Box::new(value))
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct Controls_;

impl container::StyleSheet for Controls_ {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        container::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x37, 0x3a, 0x3e))),
            ..container::Appearance::default()
        }
    }
}

impl From<Controls_> for iced::theme::Container {
    fn from(value: Controls_) -> Self {
        Container::Custom(Box::new(value))
    }
}

///////////////////////////////////////////////////////////////////////////

///////////////////////////////////////////////////////////////////////////

pub struct PlayerContainer;

impl container::StyleSheet for PlayerContainer {
    type Style = iced::Theme;
    fn appearance(&self, _style: &Self::Style) -> container::Appearance {
        container::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x19, 0x1d, 0x20))),
            ..container::Appearance::default()
        }
    }
}

impl From<PlayerContainer> for iced::theme::Container {
    fn from(value: PlayerContainer) -> Self {
        Container::Custom(Box::new(value))
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct FileSearch;

impl text_input::StyleSheet for FileSearch {
    type Style = iced::Theme;
    fn active(&self, _style: &Self::Style) -> text_input::Appearance {
        text_input::Appearance {
            ..iced::Theme::Dark.active(&TextInput::Default)
        }
    }

    fn focused(&self, _style: &Self::Style) -> text_input::Appearance {
        text_input::Appearance {
            ..iced::Theme::Dark.focused(&TextInput::Default)
        }
    }

    fn hovered(&self, _style: &Self::Style) -> text_input::Appearance {
        text_input::Appearance {
            ..iced::Theme::Dark.hovered(&TextInput::Default)
        }
    }

    fn placeholder_color(&self, _style: &Self::Style) -> Color {
        Color::from_rgb(0.4, 0.4, 0.4)
    }

    fn value_color(&self, _style: &Self::Style) -> Color {
        Color::WHITE
    }

    fn selection_color(&self, _style: &Self::Style) -> Color {
        ACTIVE
    }

    fn disabled_color(&self, _style: &Self::Style) -> Color {
        INACTIVE
    }

    fn disabled(&self, _style: &Self::Style) -> text_input::Appearance {
        text_input::Appearance {
            ..iced::Theme::Dark.disabled(&TextInput::Default)
        }
    }
}

impl From<FileSearch> for iced::theme::TextInput {
    fn from(value: FileSearch) -> Self {
        TextInput::Custom(Box::new(value))
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct DirUpButton;

impl button::StyleSheet for DirUpButton {
    type Style = iced::Theme;
    fn active(&self, _style: &Self::Style) -> button::Appearance {
        button::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x2d, 0x31, 0x35))),
            ..iced::Theme::Dark.active(&Button::default())
        }
    }

    fn hovered(&self, _style: &Self::Style) -> button::Appearance {
        button::Appearance {
            background: Some(Background::Color(Color::from_rgb8(0x37, 0x3c, 0x41))),
            ..iced::Theme::Dark.hovered(&Button::default())
        }
    }
}

impl From<DirUpButton> for iced::theme::Button {
    fn from(value: DirUpButton) -> Self {
        Button::Custom(Box::new(value))
    }
}
