//! Lays out the window: title bar, sidebar, player, and whichever modal is open.

use super::{App, Modal};
use crate::ui::auto_tag::auto_tag_view;
use crate::ui::bulk_auto_tag::bulk_auto_tag_view;
use crate::ui::dialog::{with_dialog, with_dim_overlay};
use crate::ui::menu::title_bar;
use crate::ui::message::{Message, WindowMsg};
use crate::ui::settings::settings_view;
use crate::ui::style;
use crate::ui::tag_editor::tag_editor_view;
use crate::ui::widgets::{bar, spacer};
use iced::widget::{column, container, mouse_area, row, stack, text};
use iced::window::Direction;
use iced::{Color, Element, Length, Theme, mouse};

const SIDEBAR_RESIZER_HIT_WIDTH: f32 = 10.0;
const SIDEBAR_RESIZER_LINE_WIDTH: f32 = 2.0;
/// Width of the invisible frame that resizes the undecorated window.
const WINDOW_RESIZE_BORDER: f32 = 8.0;

impl App {
    pub fn view(&self) -> Element<'_, Message> {
        let sidebar = container(
            self.file_selector
                .view(self.search_enabled(), &self.favorites, self.modifiers),
        )
        .width(Length::Fixed(self.sidebar_width))
        .height(Length::Fill)
        .style(|theme: &Theme| {
            let palette = theme.extended_palette();
            let base = palette.background.base.color;
            container::background(Color::from_rgb(base.r * 0.56, base.g * 0.56, base.b * 0.58))
                .border(style::outline(palette.background.strong.color.scale_alpha(0.42), 0.0))
        });

        let mut player = stack![self.player.view(self.current_file_tags())];
        if self.drag_over {
            player = player.push(
                container(text("Drop audio file or folder").size(18))
                    .center(Length::Fill)
                    .style(|_theme| container::background(Color::from_rgba(0.08, 0.12, 0.18, 0.82))),
            );
        }

        let resizing = self.sidebar_resize.is_some();
        let mut workspace = stack![
            row![
                sidebar,
                sidebar_resizer(resizing),
                player.width(Length::Fill).height(Length::Fill)
            ]
            .height(Length::Fill)
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        if resizing {
            // Keeps the resize cursor, and blocks hover effects, while the pointer is off the handle.
            workspace = workspace
                .push(mouse_area(spacer(Length::Fill, Length::Fill)).interaction(mouse::Interaction::ResizingColumn));
        }

        let modal = match self.modal {
            Modal::Settings => Some(settings_view(
                self.allowed_directories.roots(),
                self.settings_first_run,
                self.settings_error.as_deref(),
            )),
            _ if self.bulk_auto_tag.is_open() => Some(bulk_auto_tag_view(&self.bulk_auto_tag)),
            Modal::TagEditor => Some(tag_editor_view(&self.tag_editor)),
            Modal::AutoTag => Some(auto_tag_view(&self.auto_tag)),
            Modal::None => None,
        };
        let workspace = match (modal, &self.dialog) {
            (Some(modal), _) => with_dim_overlay(workspace.into(), modal),
            (None, Some(dialog)) => with_dialog(workspace.into(), dialog),
            (None, None) => workspace.into(),
        };

        let layout = column![
            title_bar(self.always_on_top, self.current_file_name().as_deref()),
            workspace
        ]
        .width(Length::Fill)
        .height(Length::Fill);
        if self.window_maximized {
            layout.into()
        } else {
            window_resize_frame(layout.into())
        }
    }
}

/// The draggable line between the sidebar and the player.
fn sidebar_resizer(resizing: bool) -> Element<'static, Message> {
    let gutter = || {
        spacer(
            Length::Fixed((SIDEBAR_RESIZER_HIT_WIDTH - SIDEBAR_RESIZER_LINE_WIDTH) / 2.0),
            Length::Fill,
        )
    };
    let line = bar(Length::Fixed(SIDEBAR_RESIZER_LINE_WIDTH), Length::Fill, move |theme| {
        theme
            .extended_palette()
            .background
            .strong
            .color
            .scale_alpha(if resizing { 0.85 } else { 0.45 })
    });
    let hit_area = mouse_area(spacer(Length::Fixed(SIDEBAR_RESIZER_HIT_WIDTH), Length::Fill))
        .interaction(mouse::Interaction::ResizingColumn)
        .on_press(Message::SidebarResizeStart);
    stack![row![gutter(), line, gutter()], hit_area]
        .width(Length::Fixed(SIDEBAR_RESIZER_HIT_WIDTH))
        .height(Length::Fill)
        .into()
}

/// Edges and corners that resize the window, since it has no OS frame.
fn window_resize_frame(content: Element<'_, Message>) -> Element<'_, Message> {
    use mouse::Interaction::{ResizingDiagonallyDown, ResizingDiagonallyUp, ResizingHorizontally, ResizingVertically};
    let edge = Length::Fixed(WINDOW_RESIZE_BORDER);
    let handle = |width: Length,
                  height: Length,
                  direction: Direction,
                  cursor: mouse::Interaction|
     -> Element<'static, Message> {
        mouse_area(spacer(width, height))
            .on_press(WindowMsg::Resize(direction).into())
            .interaction(cursor)
            .into()
    };
    let fill = Length::Fill;

    let edges = column![
        // No north edge handle: the title bar drags the window there instead.
        row![
            handle(edge, edge, Direction::NorthWest, ResizingDiagonallyDown),
            spacer(fill, edge),
            handle(edge, edge, Direction::NorthEast, ResizingDiagonallyUp)
        ]
        .height(edge),
        row![
            handle(edge, fill, Direction::West, ResizingHorizontally),
            spacer(fill, fill),
            handle(edge, fill, Direction::East, ResizingHorizontally)
        ]
        .height(fill),
        row![
            handle(edge, edge, Direction::SouthWest, ResizingDiagonallyUp),
            handle(fill, edge, Direction::South, ResizingVertically),
            handle(edge, edge, Direction::SouthEast, ResizingDiagonallyDown)
        ]
        .height(edge),
    ];
    stack![
        container(content).width(fill).height(fill),
        edges.width(fill).height(fill)
    ]
    .width(fill)
    .height(fill)
    .into()
}
