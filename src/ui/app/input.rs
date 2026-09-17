//! Pointer and keyboard input that outlives a single widget: dragging files
//! out of the app, the sidebar resizer, the file list scrollbar, the custom
//! title bar, and keyboard shortcuts.

use super::prefs::{self, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, on_window};
use super::{App, Modal};
use crate::ui::FILE_DRAG_THRESHOLD;
use crate::ui::file_selector::{FILE_LIST_SCROLL_ID, FilterFocus};
use crate::ui::message::{FilterMsg, Message, WindowMsg};
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::widget::Id;
use iced::widget::operation::{self, AbsoluteOffset};
use iced::{Point, Task, window};
use std::path::PathBuf;

/// How far a title bar press must move before it drags the window.
const TITLE_DRAG_THRESHOLD: f32 = 4.0;

/// A press on a file that may turn into dragging it to another app.
pub(super) enum FileDrag {
    /// Shift+drag in the file list scrolls it instead.
    Scroll { last_y: f32 },
    File {
        path: PathBuf,
        /// Set by the first move after the press; the press itself carries no position.
        origin: Option<Point>,
        stage: DragStage,
        /// A release without dragging opens the file, as a click would.
        open_on_click: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DragStage {
    Pressed,
    /// Past the threshold, but drag-out needs the X11 window id first.
    WaitingForWindow,
    Started,
}

impl FileDrag {
    pub(super) fn file(path: PathBuf, open_on_click: bool) -> Self {
        Self::File {
            path,
            origin: None,
            stage: DragStage::Pressed,
            open_on_click,
        }
    }
}

pub(super) struct SidebarResize {
    /// Pointer x when the drag started; `None` until the first move reports it.
    pub origin_x: Option<f32>,
    pub origin_width: f32,
}

pub(super) struct ScrollbarDrag {
    track_top: f32,
    track_height: f32,
    /// Where on the thumb it was grabbed.
    grab_offset: f32,
}

/// Explains a failed drag-out and how to drag from the file manager instead.
pub(super) fn drag_out_notice(intro: &str) -> String {
    let gesture = if cfg!(target_os = "macos") {
        "Control-click or use a two-finger click"
    } else {
        "Right-click"
    };
    format!(
        "{intro} {gesture} the file and choose \"{}\", then drag it from there.",
        crate::platform::file_manager_label()
    )
}

fn scroll_file_list_to(offset: f32) -> Task<Message> {
    operation::scroll_to(
        Id::new(FILE_LIST_SCROLL_ID),
        AbsoluteOffset {
            x: Some(0.0),
            y: Some(offset),
        },
    )
}

impl App {
    pub(super) fn cursor_moved(&mut self, point: Point) -> Task<Message> {
        self.last_cursor = point;
        let mut tasks = Vec::new();

        if let Some(origin) = self.title_bar_press
            && point.distance(origin) >= TITLE_DRAG_THRESHOLD
        {
            self.title_bar_press = None;
            tasks.push(on_window(window::drag));
        }
        if let Some(resize) = &mut self.sidebar_resize {
            match resize.origin_x {
                None => resize.origin_x = Some(point.x),
                Some(origin_x) => {
                    self.sidebar_width =
                        (resize.origin_width + point.x - origin_x).clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                }
            }
        }
        if let Some(drag) = &self.file_list_scrollbar_drag {
            let metrics = self.file_selector.scroll_metrics();
            if metrics.max_scroll > 0.0 {
                let track_y = (point.y - drag.track_top).clamp(0.0, drag.track_height);
                tasks.push(scroll_file_list_to(
                    metrics.offset_for_track_y(track_y, drag.grab_offset),
                ));
            }
        }
        if self.sidebar_resize.is_none() && self.file_list_scrollbar_drag.is_none() {
            tasks.push(self.move_file_drag(point));
        }
        Task::batch(tasks)
    }

    pub(super) fn mouse_released(&mut self) -> Task<Message> {
        let resized = self.sidebar_resize.take().is_some();
        if resized {
            prefs::persist_sidebar_width(self.sidebar_width);
        }
        let scrolled = self.file_list_scrollbar_drag.take().is_some();
        self.finish_scrub(self.last_scrub_progress);
        if resized || scrolled {
            return Task::none();
        }
        self.release_file_drag()
    }

    /// Unmodified key presses no widget used.
    pub(super) fn key_pressed(&mut self, key: Key, modifiers: Modifiers) -> Task<Message> {
        let command = modifiers.control() || modifiers.logo() || modifiers.alt();
        let space = key == Key::Named(Named::Space);
        let focus = self.file_selector.filter_focus;

        if space && !command && !modifiers.shift() && self.transport_keys_enabled() {
            self.player.toggle_playing();
        }
        if !space && !command && self.waveform_hovered {
            self.edit_waveform_view(|view, sample_count| {
                view.apply_key(&key, sample_count);
            });
        }
        if key != Key::Named(Named::Tab) || focus == FilterFocus::None {
            return Task::none();
        }
        // Tab completes a tag field name, or else walks file search -> tag search -> file list.
        if focus == FilterFocus::TagSearch && self.file_selector.tag_search_can_autocomplete() {
            if command || modifiers.shift() {
                return Task::none();
            }
            return self.autocomplete_tag_field();
        }
        if command {
            return Task::none();
        }
        let shift = modifiers.shift();
        operation::is_focused(Id::new(crate::ui::file_selector::TAG_SEARCH_INPUT_ID)).map(move |on_tag| {
            let message = match (on_tag, shift) {
                (false, false) => FilterMsg::TagSearchFocused(true),
                (true, false) => FilterMsg::TagSearchFocused(false),
                // Shift+Tab from the top holds focus rather than falling through.
                (_, true) => FilterMsg::SearchFocused(true),
            };
            message.into()
        })
    }

    /// Space plays and pauses unless a modal or a text input has the keyboard.
    fn transport_keys_enabled(&self) -> bool {
        let focus = self.file_selector.filter_focus;
        self.player.waveform.is_some()
            && self.dialog.is_none()
            && self.modal == Modal::None
            && !self.bulk_auto_tag.is_open()
            && focus != FilterFocus::FileSearch
            && (focus != FilterFocus::TagSearch || self.file_list_focused)
    }

    pub(super) fn press_file(&mut self, path: PathBuf, from_file_list: bool) -> Task<Message> {
        if from_file_list {
            self.file_selector.filter_focus = FilterFocus::None;
            self.file_list_focused = true;
        }
        self.file_drag = Some(if from_file_list && self.modifiers.shift() {
            FileDrag::Scroll {
                last_y: self.last_cursor.y,
            }
        } else {
            FileDrag::file(path, from_file_list)
        });
        Task::none()
    }

    fn move_file_drag(&mut self, point: Point) -> Task<Message> {
        match &mut self.file_drag {
            None => Task::none(),
            Some(FileDrag::Scroll { last_y }) => {
                let dy = point.y - std::mem::replace(last_y, point.y);
                if dy.abs() < 0.5 {
                    return Task::none();
                }
                operation::scroll_by(FILE_LIST_SCROLL_ID, AbsoluteOffset { x: 0.0, y: dy })
            }
            Some(FileDrag::File {
                path, origin, stage, ..
            }) => {
                let Some(origin) = *origin else {
                    *origin = Some(point);
                    return Task::none();
                };
                if *stage != DragStage::Pressed || point.distance(origin) < FILE_DRAG_THRESHOLD {
                    return Task::none();
                }
                if self.drag_ready {
                    *stage = DragStage::Started;
                    let path = path.clone();
                    self.start_file_drag(path)
                } else {
                    *stage = DragStage::WaitingForWindow;
                    on_window(|id| {
                        window::run(id, |window| crate::drag_out::x11_window_id(window)).map(Message::DragWindowId)
                    })
                }
            }
        }
    }

    fn release_file_drag(&mut self) -> Task<Message> {
        let native_active = self.native_drag.is_active();
        let click_to_open = match &self.file_drag {
            Some(FileDrag::File {
                path,
                stage: DragStage::Pressed,
                open_on_click: true,
                ..
            }) if !native_active => Some(path.clone()),
            _ => None,
        };
        if native_active {
            self.native_drag.update(true, true);
        }
        if !self.native_drag.is_active() {
            self.file_drag = None;
        }
        click_to_open.map_or_else(Task::none, |path| self.open_path(&path))
    }

    /// Advances an X11 drag-out, which is driven by polling.
    pub(super) fn tick_native_drag(&mut self) -> Task<Message> {
        if self.native_drag.is_active() {
            self.native_drag.update(true, false);
            if !self.native_drag.is_active() {
                self.file_drag = None;
            }
        }
        Task::none()
    }

    pub(super) fn drag_window_ready(&mut self, window_id: Option<u32>) -> Task<Message> {
        let init = match window_id {
            Some(id) => self
                .native_drag
                .init_with_window_id(id)
                .map_err(|err| format!("Could not initialize drag-out: {err}.")),
            None if cfg!(all(unix, not(target_os = "macos"))) => {
                Err("Drag-out from the file list requires X11 and is unavailable on native Wayland.".into())
            }
            None => Err("Could not initialize drag-out.".into()),
        };
        self.drag_ready = init.is_ok();
        if let Err(intro) = init {
            self.show_notice(drag_out_notice(&intro));
            self.file_drag = None;
            return Task::none();
        }
        match &mut self.file_drag {
            Some(FileDrag::File { path, stage, .. }) if *stage == DragStage::WaitingForWindow => {
                *stage = DragStage::Started;
                let path = path.clone();
                self.start_file_drag(path)
            }
            _ => Task::none(),
        }
    }

    fn start_file_drag(&mut self, path: PathBuf) -> Task<Message> {
        let canonical = match crate::path_util::canonical_path(&path) {
            Ok(path) => path,
            Err(err) => {
                self.show_notice(drag_out_notice(&format!("Cannot drag {}: {err}.", path.display())));
                self.file_drag = None;
                return Task::none();
            }
        };

        #[cfg(any(windows, target_os = "macos"))]
        {
            self.file_drag = None;
            on_window(move |id| {
                let path = canonical.clone();
                window::run(id, move |window| crate::drag_out::start_blocking(window, path))
                    .map(Message::FileDragCompleted)
            })
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Err(err) = self.native_drag.start(canonical) {
                self.show_notice(drag_out_notice(&format!("Drag failed: {err}.")));
                self.file_drag = None;
            }
            Task::none()
        }
    }

    pub(super) fn press_scrollbar(&mut self, track_y: f32, track_top: f32, track_height: f32) -> Task<Message> {
        let metrics = self.file_selector.scroll_metrics();
        if metrics.max_scroll <= 0.0 {
            return Task::none();
        }
        let grab_offset = metrics.grab_offset(track_y);
        self.file_list_scrollbar_drag = Some(ScrollbarDrag {
            track_top,
            track_height,
            grab_offset,
        });
        scroll_file_list_to(metrics.offset_for_track_y(track_y, grab_offset))
    }

    pub(super) fn update_window(&mut self, message: WindowMsg) -> Task<Message> {
        let sync_maximized =
            |id: window::Id| window::is_maximized(id).map(|maximized| WindowMsg::MaximizedChanged(maximized).into());
        match message {
            WindowMsg::TitleBarPress => {
                self.title_bar_press = Some(self.last_cursor);
                Task::none()
            }
            WindowMsg::TitleBarRelease => {
                self.title_bar_press = None;
                Task::none()
            }
            WindowMsg::Minimize => on_window(|id| window::minimize(id, true)),
            WindowMsg::ToggleMaximize => {
                self.title_bar_press = None;
                on_window(move |id| window::toggle_maximize(id).chain(sync_maximized(id)))
            }
            WindowMsg::MaximizedChanged(maximized) => {
                self.window_maximized = maximized;
                Task::none()
            }
            WindowMsg::SyncMaximized => on_window(sync_maximized),
            WindowMsg::Resize(direction) => on_window(move |id| window::drag_resize(id, direction)),
        }
    }
}
