//! The one-button message dialog (errors, notices, About, help tables).

use super::message::Message;
use super::style;
use super::widgets::spacer;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, stack, text};
use iced::{Alignment, Color, Element, Length};

#[derive(Debug, Clone)]
pub struct Dialog {
    pub title: String,
    pub body: String,
    /// Two-column table shown under the body, e.g. input/action pairs.
    pub rows: Vec<(&'static str, &'static str)>,
}

impl Dialog {
    fn titled(title: &str, body: String) -> Self {
        Self {
            title: title.into(),
            body,
            rows: Vec::new(),
        }
    }

    pub fn about(body: String) -> Self {
        Self::titled("About Tundra", body)
    }

    pub fn notice(body: String) -> Self {
        Self::titled("Notice", body)
    }

    pub fn error(body: String) -> Self {
        Self::titled("Error", body)
    }

    pub fn waveform_help() -> Self {
        Self {
            rows: vec![
                ("Drag", "Seek"),
                ("Ctrl+drag", "Drag file"),
                ("Scroll", "Zoom"),
                ("Shift+scroll", "Pan"),
                ("Shift+drag", "Pan"),
                ("Space", "Play/pause"),
                ("+ / −", "Zoom"),
                ("Left / Right", "Pan"),
            ],
            ..Self::titled("Waveform controls", "Hover the waveform for +/− and arrow keys.".into())
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let mut body = column![text(&self.title).size(18)]
            .spacing(12)
            .padding(16)
            .width(Length::Fixed(440.0));
        if !self.body.is_empty() {
            body = body.push(text(&self.body).size(14).width(Length::Fill));
        }
        if !self.rows.is_empty() {
            body = body.push(table(&self.rows));
        }
        body = body.push(row![
            spacer(Length::Fill, Length::Shrink),
            button(text("OK")).on_press(Message::DismissDialog),
        ]);
        opaque(container(body).style(style::card(8.0)))
    }
}

fn table(rows: &[(&'static str, &'static str)]) -> Element<'static, Message> {
    let line = |input, action, size, alpha| {
        let cell = move |label| {
            text(label)
                .size(size)
                .width(Length::FillPortion(1))
                .style(style::faded_text(alpha))
        };
        row![cell(input), cell(action)]
            .spacing(12)
            .align_y(Alignment::Center)
            .width(Length::Fill)
    };
    let header = container(line("Input", "Action", 10, 0.62))
        .padding([6, 10])
        .width(Length::Fill)
        .style(style::panel(0.32, 0.18, 0.0));
    let body = rows.iter().enumerate().map(|(index, &(input, action))| {
        let zebra = index % 2 == 1;
        container(line(input, action, 13, 1.0))
            .padding([7, 10])
            .width(Length::Fill)
            .style(move |theme: &iced::Theme| {
                let palette = theme.extended_palette();
                container::background(if zebra {
                    palette.background.weak.color.scale_alpha(0.18)
                } else {
                    Color::TRANSPARENT
                })
            })
            .into()
    });
    container(column![header].extend(body).width(Length::Fill))
        .width(Length::Fill)
        .style(|theme: &iced::Theme| {
            let strong = theme.extended_palette().background.strong.color;
            container::Style::default().border(style::outline(strong.scale_alpha(0.22), 6.0))
        })
        .into()
}

/// Dims `base` and centers `overlay` on top; clicks never reach `base`.
pub fn with_dim_overlay<'a>(base: Element<'a, Message>, overlay: Element<'a, Message>) -> Element<'a, Message> {
    stack![base, opaque(container(center(overlay)).style(dim_scrim))]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Shows `dialog` over `base`; clicking outside it dismisses.
pub fn with_dialog<'a>(base: Element<'a, Message>, dialog: &'a Dialog) -> Element<'a, Message> {
    stack![
        base,
        opaque(mouse_area(center(dialog.view()).style(dim_scrim)).on_press(Message::DismissDialog))
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn dim_scrim(_theme: &iced::Theme) -> container::Style {
    container::background(Color::from_rgba(0.0, 0.0, 0.0, 0.55))
}
