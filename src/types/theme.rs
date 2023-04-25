use iced::widget::{button, container, scrollable, slider, text_input};
use iced::{Background, Color};
use iced_aw::style::split;
use iced_native::application;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Container {
    #[default]
    Container,
    SelectedContainer,
    Controls,
    PlayerContainer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Button {
    #[default]
    Default,
    FileButton,
    ControlButton,
    DirUpButton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Split {
    #[default]
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextInput {
    #[default]
    Default,
    FileSearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeType {
    #[default]
    Dark,
    Light,
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

impl application::StyleSheet for Theme {
    type Style = ();

    fn appearance(&self, _style: &Self::Style) -> application::Appearance {
        iced::Theme::Dark.appearance(&iced_native::theme::Application::Default)
    }
}

impl container::StyleSheet for Theme {
    type Style = Container;

    fn appearance(&self, style: &Self::Style) -> container::Appearance {
        match style {
            Container::Container => container::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x23, 0x27, 0x2a))),
                ..Default::default()
            },
            Container::Controls => container::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x37, 0x3a, 0x3e))),
                ..container::Appearance::default()
            },
            Container::PlayerContainer => container::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x19, 0x1d, 0x20))),
                ..container::Appearance::default()
            },
            _ => Default::default(),
        }
    }
}

impl button::StyleSheet for Theme {
    type Style = Button;

    fn active(&self, style: &Self::Style) -> button::Appearance {
        match style {
            Button::FileButton => button::Appearance {
                background: None,
                ..default_filebutton_style()
            },
            Button::ControlButton => button::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x31, 0x34, 0x38))),
                ..default_controlbutton_style()
            },

            Button::DirUpButton => button::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x2d, 0x31, 0x35))),
                ..iced::Theme::Dark.active(&iced::theme::Button::default())
            },
            Button::Default => Default::default(),
        }
    }

    fn hovered(&self, style: &Self::Style) -> button::Appearance {
        match style {
            Button::FileButton => button::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x31, 0x34, 0x38))),
                ..default_filebutton_style()
            },
            Button::ControlButton => button::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x37, 0x3a, 0x3e))),
                ..default_controlbutton_style()
            },

            Button::DirUpButton => button::Appearance {
                background: Some(Background::Color(Color::from_rgb8(0x37, 0x3c, 0x41))),
                ..iced::Theme::Dark.hovered(&iced::theme::Button::default())
            },
            Button::Default => Default::default(),
        }
    }
}

impl split::StyleSheet for Theme {
    type Style = Split;

    fn active(&self, _style: Self::Style) -> iced_aw::split::Appearance {
        default_split_styles()
    }

    fn hovered(&self, _style: Self::Style) -> iced_aw::split::Appearance {
        default_split_styles()
    }

    fn dragged(&self, _style: Self::Style) -> iced_aw::split::Appearance {
        default_split_styles()
    }
}

impl iced::widget::svg::StyleSheet for Theme {
    type Style = ();

    fn appearance(&self, _style: &Self::Style) -> iced::widget::svg::Appearance {
        Default::default()
    }
}

impl iced::widget::text::StyleSheet for Theme {
    type Style = ();

    fn appearance(&self, _style: Self::Style) -> iced::widget::text::Appearance {
        Default::default()
    }
}

impl iced::widget::scrollable::StyleSheet for Theme {
    type Style = ();

    fn active(&self, _style: &Self::Style) -> iced::widget::scrollable::Scrollbar {
        scrollable::Scrollbar {
            ..iced::Theme::Dark.active(&iced::theme::Scrollable::Default)
        }
    }

    fn hovered(
        &self,
        _style: &Self::Style,
        is_mouse_over_scrollbar: bool,
    ) -> iced::widget::scrollable::Scrollbar {
        scrollable::Scrollbar {
            ..iced::Theme::Dark.hovered(&iced::theme::Scrollable::Default, is_mouse_over_scrollbar)
        }
    }
}

impl iced::widget::slider::StyleSheet for Theme {
    type Style = ();

    fn active(&self, _style: &Self::Style) -> iced::widget::vertical_slider::Appearance {
        slider::Appearance {
            ..iced::Theme::Dark.active(&iced::theme::Slider::Default)
        }
    }

    fn hovered(&self, _style: &Self::Style) -> iced::widget::vertical_slider::Appearance {
        slider::Appearance {
            ..iced::Theme::Dark.hovered(&iced::theme::Slider::Default)
        }
    }

    fn dragging(&self, _style: &Self::Style) -> iced::widget::vertical_slider::Appearance {
        slider::Appearance {
            ..iced::Theme::Dark.dragging(&iced::theme::Slider::Default)
        }
    }
}

fn default_split_styles() -> iced_aw::split::Appearance {
    iced_aw::split::Appearance {
        background: Some(Background::Color(Color::from_rgb8(0x23, 0x27, 0x2a))),
        first_background: None,
        second_background: None,
        border_width: 2.0,
        border_color: Color::from_rgb8(0x23, 0x27, 0x2a),
        divider_border_color: Color::from_rgb8(0x23, 0x27, 0x2a),
        ..Default::default()
    }
}

impl text_input::StyleSheet for Theme {
    type Style = TextInput;
    fn active(&self, style: &Self::Style) -> text_input::Appearance {
        match style {
            TextInput::FileSearch => text_input::Appearance {
                ..iced::Theme::Dark.active(&iced::theme::TextInput::Default)
            },
            TextInput::Default => iced::Theme::Dark.active(&iced::theme::TextInput::Default),
        }
    }

    fn focused(&self, style: &Self::Style) -> text_input::Appearance {
        match style {
            TextInput::FileSearch => text_input::Appearance {
                ..iced::Theme::Dark.focused(&iced::theme::TextInput::Default)
            },
            TextInput::Default => iced::Theme::Dark.focused(&iced::theme::TextInput::Default),
        }
    }

    fn hovered(&self, style: &Self::Style) -> text_input::Appearance {
        match style {
            TextInput::FileSearch => text_input::Appearance {
                ..iced::Theme::Dark.hovered(&iced::theme::TextInput::Default)
            },
            TextInput::Default => iced::Theme::Dark.hovered(&iced::theme::TextInput::Default),
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
            ..iced::Theme::Dark.disabled(&iced::theme::TextInput::Default)
        }
    }
}

impl iced_aw::menu::StyleSheet for Theme {
    type Style = ();

    fn appearance(&self, _style: &Self::Style) -> iced_aw::menu::Appearance {
        iced::Theme::Dark.appearance(&iced_aw::style::MenuBarStyle::Default)
    }
}

fn default_filebutton_style() -> button::Appearance {
    button::Appearance {
        text_color: Color::from_rgb8(0xff, 0xff, 0xff),
        ..button::Appearance::default()
    }
}

fn default_controlbutton_style() -> button::Appearance {
    button::Appearance {
        text_color: Color::from_rgb8(0xff, 0xff, 0xff),
        border_width: 2.0,
        border_color: Color::from_rgb8(0x19, 0x1d, 0x20),
        ..button::Appearance::default()
    }
}
