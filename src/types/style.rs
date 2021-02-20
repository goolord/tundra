use iced::{button, container, Background, Color};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    // Dark,
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::Light
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct SelectedContainer;

impl container::StyleSheet for SelectedContainer {
    fn style(&self) -> container::Style {
        container::Style {
            text_color: Some(Color::from_rgb8(0xff as u8, 0xff as u8, 0xff as u8)),
            background: Some(Background::Color(Color::from_rgb8(
                0x25 as u8, 0x7a as u8, 0xfd as u8,
            ))),
            ..container::Style::default()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct FileButton_;

impl button::StyleSheet for FileButton_ {
    fn active(&self) -> button::Style {
        button::Style {
            background: None,
            ..button::Style::default()
        }
    }

    fn hovered(&self) -> button::Style {
        button::Style {
            background: Some(Background::Color(Color::from_rgb8(
                0xee as u8, 0xee as u8, 0xee as u8,
            ))),
            ..button::Style::default()
        }
    }
}
