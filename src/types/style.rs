use iced::{button, container, text_input, Background, Color};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::Light
    }
}

const ACTIVE: Color = Color::from_rgb(
    0x72 as f32 / 255.0,
    0x89 as f32 / 255.0,
    0xDA as f32 / 255.0,
);

///////////////////////////////////////////////////////////////////////////

pub struct SelectedContainer;

impl container::StyleSheet for SelectedContainer {
    fn style(&self) -> container::Style {
        container::Style {
            text_color: Some(Color::from_rgb8(0xff_u8, 0xff_u8, 0xff_u8)),
            background: Some(Background::Color(Color::from_rgb8(
                0x25_u8, 0x7a_u8, 0xfd_u8,
            ))),
            ..container::Style::default()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct FileButton_;

impl FileButton_ {
    fn default_style() -> button::Style {
        button::Style {
            text_color: Color::from_rgb8(0xff, 0xff, 0xff),
            ..button::Style::default()
        }
    }
}

impl button::StyleSheet for FileButton_ {
    fn active(&self) -> button::Style {
        button::Style {
            background: None,
            ..FileButton_::default_style()
        }
    }

    fn hovered(&self) -> button::Style {
        button::Style {
            background: Some(Background::Color(Color::from_rgb8(0x31, 0x34, 0x38))),
            ..FileButton_::default_style()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct ControlButton_;

impl ControlButton_ {
    fn default_style() -> button::Style {
        button::Style {
            text_color: Color::from_rgb8(0xff, 0xff, 0xff),
            border_width: 2.0,
            border_color: Color::from_rgb8(0x19, 0x1d, 0x20),
            ..button::Style::default()
        }
    }
}

impl button::StyleSheet for ControlButton_ {
    fn active(&self) -> button::Style {
        button::Style {
            background: Some(Background::Color(Color::from_rgb8(0x31, 0x34, 0x38))),
            ..ControlButton_::default_style()
        }
    }

    fn hovered(&self) -> button::Style {
        button::Style {
            background: Some(Background::Color(Color::from_rgb8(0x37, 0x3a, 0x3e))),
            ..ControlButton_::default_style()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct Container_;

impl container::StyleSheet for Container_ {
    fn style(&self) -> container::Style {
        container::Style {
            background: Some(Background::Color(Color::from_rgb8(0x23, 0x27, 0x2a))),
            ..container::Style::default()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct Controls_;

impl container::StyleSheet for Controls_ {
    fn style(&self) -> container::Style {
        container::Style {
            background: Some(Background::Color(Color::from_rgb8(0x37, 0x3a, 0x3e))),
            ..container::Style::default()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

///////////////////////////////////////////////////////////////////////////

pub struct PlayerContainer;

impl container::StyleSheet for PlayerContainer {
    fn style(&self) -> container::Style {
        container::Style {
            background: Some(Background::Color(Color::from_rgb8(0x19, 0x1d, 0x20))),
            ..container::Style::default()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct FileSearch;

impl FileSearch {
    pub fn default_style() -> text_input::Style {
        text_input::Style {
            background: Background::Color(Color::from_rgb8(0x2d, 0x31, 0x35)),
            ..text_input::Style::default()
        }
    }
}

impl text_input::StyleSheet for FileSearch {
    fn active(&self) -> text_input::Style {
        text_input::Style {
            ..FileSearch::default_style()
        }
    }

    fn focused(&self) -> text_input::Style {
        text_input::Style {
            ..FileSearch::default_style()
        }
    }

    fn hovered(&self) -> text_input::Style {
        text_input::Style {
            ..FileSearch::default_style()
        }
    }

    fn placeholder_color(&self) -> Color {
        Color::from_rgb(0.4, 0.4, 0.4)
    }

    fn value_color(&self) -> Color {
        Color::WHITE
    }

    fn selection_color(&self) -> Color {
        ACTIVE
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct DirUpButton;

impl DirUpButton {
    fn default_style() -> button::Style {
        button::Style {
            text_color: Color::from_rgb8(0xff, 0xff, 0xff),
            ..button::Style::default()
        }
    }
}

impl button::StyleSheet for DirUpButton {
    fn active(&self) -> button::Style {
        button::Style {
            background: Some(Background::Color(Color::from_rgb8(0x2d, 0x31, 0x35))),
            ..DirUpButton::default_style()
        }
    }

    fn hovered(&self) -> button::Style {
        button::Style {
            background: Some(Background::Color(Color::from_rgb8(0x37, 0x3c, 0x41))),
            ..DirUpButton::default_style()
        }
    }
}
