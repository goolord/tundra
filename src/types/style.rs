use iced::{container, Background, Color};

pub const DEBUG_BORDER_BOUNDS: BoxedContainer = BoxedContainer;

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

pub struct BoxedContainer;

impl container::StyleSheet for BoxedContainer {
    fn style(&self) -> container::Style {
        container::Style {
            border_width: 3,
            border_color: Color::from_rgb(0xff as f32, 0x00 as f32, 0x00 as f32),
            ..container::Style::default()
        }
    }
}

///////////////////////////////////////////////////////////////////////////

pub struct SelectedContainer;

impl container::StyleSheet for SelectedContainer {
    fn style(&self) -> container::Style {
        container::Style {
            text_color: Some(Color::from_rgb(0xff as f32, 0xff as f32, 0xff as f32)),
            background: Some(Background::Color(Color::from_rgb(
                0x00 as f32,
                0x00 as f32,
                0xaa as f32,
            ))),
            ..container::Style::default()
        }
    }
}

///////////////////////////////////////////////////////////////////////////
