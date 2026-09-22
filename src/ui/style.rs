//! Colors, fonts, and style functions shared by every view.
//!
//! Reach for these before writing a `Style { .. }` literal inline, so the app
//! keeps one look and a palette tweak happens in one place.

use crate::metadata::TagField;
use iced::border;
use iced::font::Weight;
use iced::widget::{button, container, svg, text};
use iced::{Border, Color, Font, Shadow, Theme, Vector};

pub const ACCENT: Color = Color::from_rgb8(0x50, 0x7a, 0xe0);
pub const DANGER: Color = Color::from_rgb8(0xff, 0x66, 0x66);
pub const ERROR: Color = Color::from_rgb(0.95, 0.62, 0.62);
pub const WARN: Color = Color::from_rgb(0.88, 0.78, 0.52);
pub const OK: Color = Color::from_rgb(0.62, 0.88, 0.68);
pub const MUTED_ICON: Color = Color::from_rgb8(0x52, 0x56, 0x5c);

pub const MEDIUM: Font = Font { weight: Weight::Medium, ..Font::DEFAULT };
pub const SEMIBOLD: Font = Font { weight: Weight::Semibold, ..Font::DEFAULT };

/// Picks a value by button state. Disabled buttons look idle.
pub fn by_status<T>(status: button::Status, idle: T, hovered: T, pressed: T) -> T {
    match status {
        button::Status::Active | button::Status::Disabled => idle,
        button::Status::Hovered => hovered,
        button::Status::Pressed => pressed,
    }
}

/// The theme's body text color at `alpha`.
pub fn text_alpha(theme: &Theme, alpha: f32) -> Color {
    theme.extended_palette().background.base.text.scale_alpha(alpha)
}

/// Secondary text: labels, hints, counts.
pub fn muted(theme: &Theme) -> Color {
    text_alpha(theme, 0.72)
}

/// Text colored per theme.
pub fn text_color(color: impl Fn(&Theme) -> Color) -> impl Fn(&Theme) -> text::Style {
    move |theme| text::Style { color: Some(color(theme)) }
}

pub fn muted_text(theme: &Theme) -> text::Style {
    text_color(muted)(theme)
}

/// Text in the theme's body color at `alpha`.
pub fn faded_text(alpha: f32) -> impl Fn(&Theme) -> text::Style {
    text_color(move |theme| text_alpha(theme, alpha))
}

/// Text in the theme's primary color.
pub fn primary_text(theme: &Theme) -> text::Style {
    text_color(|theme| theme.extended_palette().primary.base.color)(theme)
}

/// Body text when `highlight`, muted otherwise.
pub fn highlight_text(highlight: bool) -> impl Fn(&Theme) -> text::Style {
    text_color(move |theme| if highlight { text_alpha(theme, 1.0) } else { muted(theme) })
}

/// A button with one `background`, usually picked with [`by_status`].
pub fn solid_button(text_color: Color, border: Border, background: Color) -> button::Style {
    button::Style { text_color, border, ..button::Style::default() }.with_background(background)
}

/// Tints a monochrome SVG icon.
pub fn icon_color(color: impl Fn(&Theme) -> Color) -> impl Fn(&Theme, svg::Status) -> svg::Style {
    move |theme, _status| svg::Style { color: Some(color(theme)) }
}

/// Theme background at `fill` alpha with a strong-color outline at `border` alpha.
pub fn panel(fill: f32, border: f32, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.extended_palette();
        container::Style::default()
            .background(palette.background.weak.color.scale_alpha(fill))
            .border(outline(palette.background.strong.color.scale_alpha(border), radius))
    }
}

/// A chip or banner filled and outlined with one `tone`.
pub fn tinted(tone: Color, fill: f32, border: f32, radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        container::Style::default().background(tone.scale_alpha(fill)).border(outline(tone.scale_alpha(border), radius))
    }
}

/// A one-pixel border.
pub fn outline(color: Color, radius: f32) -> Border {
    border::rounded(radius).width(1.0).color(color)
}

/// The drop shadow under floating surfaces.
pub fn drop_shadow(color: Color, offset_y: f32, blur_radius: f32) -> Shadow {
    Shadow { color, offset: Vector::new(0.0, offset_y), blur_radius }
}

/// Floating card behind modals and dialogs.
pub fn card(radius: f32) -> impl Fn(&Theme) -> container::Style {
    move |theme| {
        let palette = theme.extended_palette();
        container::Style::default()
            .background(palette.background.base.color)
            .border(outline(palette.background.strong.color, radius))
            .shadow(drop_shadow(palette.background.base.text.scale_alpha(0.25), 4.0, 16.0))
    }
}

/// Buttons in modal footers and toolbars. `primary` marks the main action.
pub fn modal_button(primary: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |theme, status| {
        let palette = theme.extended_palette();
        let accent = palette.primary.base.color;
        let idle = if primary { accent.scale_alpha(0.82) } else { palette.background.weak.color.scale_alpha(0.45) };
        let hovered = accent.scale_alpha(if primary { 0.92 } else { 0.16 });
        let text_color =
            if primary && by_status(status, true, false, false) { Color::WHITE } else { palette.background.base.text };
        let border = outline(palette.background.strong.color.scale_alpha(0.35), 6.0);
        solid_button(text_color, border, by_status(status, idle, hovered, accent.scale_alpha(0.72)))
    }
}

pub fn tag_field_color(field: TagField) -> Color {
    match field {
        TagField::Title => ACCENT,
        TagField::Artist => Color::from_rgb8(0x66, 0x72, 0xe8),
        TagField::Album => Color::from_rgb8(0x48, 0x96, 0xc8),
        TagField::Genre | TagField::Instrument => Color::from_rgb8(0x52, 0xa8, 0x86),
        TagField::Comment => Color::from_rgb8(0x78, 0x82, 0x98),
        TagField::AlbumArtist => Color::from_rgb8(0x62, 0x66, 0xd8),
        TagField::Composer => Color::from_rgb8(0x86, 0x70, 0xc0),
        TagField::Label => Color::from_rgb8(0x6a, 0x88, 0xb4),
        TagField::Bpm => Color::from_rgb8(0xc8, 0x72, 0x48),
        TagField::Key => Color::from_rgb8(0x9a, 0x68, 0xc0),
    }
}
