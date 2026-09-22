//! The one-button message dialog (errors, notices, About, help tables).

use super::message::Message;
use super::style;
use super::widgets::spacer;
use iced::widget::{button, center, column, container, mouse_area, opaque, row, stack, text};
use iced::{Alignment, Color, Element, Length, Theme};

#[derive(Debug, Clone)]
pub struct Dialog {
    pub title: &'static str,
    pub body: String,
    /// Two-column table shown under the body, e.g. input/action pairs.
    pub rows: &'static [(&'static str, &'static str)],
}

impl Dialog {
    pub fn new(title: &'static str, body: String) -> Self {
        Self { title, body, rows: &[] }
    }

    pub fn waveform_help() -> Self {
        Self {
            rows: &[
                ("Drag", "Seek"),
                ("Ctrl+drag", "Drag file"),
                ("Scroll", "Zoom"),
                ("Shift+scroll", "Pan"),
                ("Shift+drag", "Pan"),
                ("Space", "Play/pause"),
                ("+ / −", "Zoom"),
                ("Left / Right", "Pan"),
            ],
            ..Self::new("Waveform controls", "Hover the waveform for +/− and arrow keys.".into())
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let body = column![
            text(self.title).size(18),
            (!self.body.is_empty()).then(|| text(&self.body).size(14).width(Length::Fill)),
            (!self.rows.is_empty()).then(|| table(self.rows)),
            row![spacer(Length::Fill, Length::Shrink), button(text("OK")).on_press(Message::DismissDialog)],
        ];
        opaque(container(body.spacing(12).padding(16).width(Length::Fixed(440.0))).style(style::card(8.0)))
    }
}

fn table(rows: &'static [(&'static str, &'static str)]) -> Element<'static, Message> {
    let line = |input, action, size, alpha| {
        let cell = move |label| text(label).size(size).width(Length::FillPortion(1)).style(style::faded_text(alpha));
        row![cell(input), cell(action)].spacing(12).align_y(Alignment::Center).width(Length::Fill)
    };
    let header = container(line("Input", "Action", 10, 0.62))
        .padding([6, 10])
        .width(Length::Fill)
        .style(style::panel(0.32, 0.18, 0.0));
    let body = rows.iter().enumerate().map(|(index, &(input, action))| {
        // Zebra stripes on every other row.
        let fill = if index % 2 == 1 { 0.18 } else { 0.0 };
        let cells = container(line(input, action, 13, 1.0)).padding([7, 10]).width(Length::Fill);
        cells
            .style(move |theme: &Theme| {
                container::background(theme.extended_palette().background.weak.color.scale_alpha(fill))
            })
            .into()
    });
    container(column![header].extend(body).width(Length::Fill))
        .width(Length::Fill)
        .style(|theme: &Theme| {
            container::Style::default()
                .border(style::outline(theme.extended_palette().background.strong.color.scale_alpha(0.22), 6.0))
        })
        .into()
}

/// Dims `base` and centers `overlay` on top. Clicks never reach `base`; with `dismiss`, one outside `overlay` closes the dialog.
pub fn with_overlay<'a>(
    base: Element<'a, Message>,
    overlay: Element<'a, Message>,
    dismiss: bool,
) -> Element<'a, Message> {
    let scrim = mouse_area(center(overlay).style(|_| container::background(Color::from_rgba(0.0, 0.0, 0.0, 0.55))));
    let scrim = if dismiss { scrim.on_press(Message::DismissDialog) } else { scrim };
    stack![base, opaque(scrim)].width(Length::Fill).height(Length::Fill).into()
}
