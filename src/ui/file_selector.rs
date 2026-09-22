//! The sidebar: the file list with its custom scrollbar, and the filter dock
//! (file search and tag filters) under it.

use super::message::{FilterMsg, Message};
use super::selection::Selection;
use super::style::{self, ACCENT, MUTED_ICON};
use super::widgets::{bar, file_context_menu, icon, selection_stripe, spacer};
use crate::library::FavoritesStore;
use crate::metadata::{TagFilter, is_audio, tag_field_best_match, tag_field_suggestions};
use crate::path_util::cache_key;
use iced::keyboard::Modifiers;
use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Action, Event, Frame, Program};
use iced::widget::scrollable::{self, Scrollbar};
use iced::widget::text::Wrapping;
use iced::widget::{Column, Id, Row, TextInput, button, column, container, mouse_area, row, stack, text};
use iced::{Alignment, Border, Color, Element, Length, Padding, Rectangle, Theme};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const FILE_LIST_SCROLL_ID: &str = "file-list-scroll";
pub const TAG_SEARCH_INPUT_ID: &str = "tag-search-input";
pub const FILE_SEARCH_INPUT_ID: &str = "file-search-input";
/// Fixed height of every file row; windowed rendering relies on this being exact.
const FILE_ROW_HEIGHT: f32 = 31.0;
/// Extra rows rendered above and below the viewport to absorb fast scrolling.
const FILE_ROW_OVERDRAW: usize = 12;
/// Rows to render until the first scroll event reports the real viewport height.
const FALLBACK_VIEWPORT_HEIGHT: f32 = 2400.0;
const SCROLLBAR_WIDTH: f32 = 10.0;
const SCROLLBAR_MIN_THUMB: f32 = 36.0;
const FILTER_CLEAR_TEXT_SIZE: f32 = 13.0;
const FILTER_CLEAR_PAD: [f32; 2] = [4.0, 8.0];
/// Right padding inside a filter input that the × button sits in.
const FILTER_CLEAR_INSET: f32 = FILTER_CLEAR_TEXT_SIZE + 2.0 * FILTER_CLEAR_PAD[1];

/// Which filter-bar text field owns keyboard focus. At most one is active; the other gets a
/// click-catcher overlay so two TextInputs cannot stay focused together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterFocus {
    #[default]
    None,
    FileSearch,
    TagSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileButton {
    pub file_path: PathBuf,
    pub label: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FileSelector {
    pub current_dir: PathBuf,
    pub file_list: Vec<FileButton>,
    selection: Selection<usize>,
    pub hovered_file: Option<usize>,
    pub filter_focus: FilterFocus,
    pub search_value: String,
    pub search_case_sensitive: bool,
    pub search_show_directories: bool,
    pub favorites_only: bool,
    pub tag_search_value: String,
    pub tag_filters: Vec<TagFilter>,
    pub tag_search_error: Option<String>,
    pub list_error: Option<String>,
    pub list_scroll_offset: f32,
    pub list_viewport_height: f32,
}

/// Where the scrollbar thumb sits for a given list length and scroll offset.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrollMetrics {
    pub max_scroll: f32,
    thumb_height: f32,
    thumb_top: f32,
    scroll_range: f32,
}

impl ScrollMetrics {
    fn new(total_rows: usize, scroll_offset: f32, track_height: f32) -> Self {
        let track_height = track_height.max(0.0);
        let content_height = total_rows as f32 * FILE_ROW_HEIGHT;
        let max_scroll = (content_height - track_height).max(0.0);
        if track_height <= 0.0 || max_scroll <= 0.0 {
            return Self { thumb_height: track_height, ..Self::default() };
        }
        let min_thumb = SCROLLBAR_MIN_THUMB.min(track_height * 0.9);
        let mut thumb_height = (track_height * track_height / content_height).clamp(min_thumb, track_height);
        if track_height - thumb_height <= 0.0 {
            thumb_height = track_height * 0.25;
        }
        let scroll_range = track_height - thumb_height;
        Self { max_scroll, thumb_height, thumb_top: scroll_offset / max_scroll * scroll_range, scroll_range }
    }

    fn on_thumb(&self, track_y: f32) -> bool {
        (self.thumb_top..=self.thumb_top + self.thumb_height).contains(&track_y)
    }

    /// Where on the thumb a press at `track_y` grabbed it; its middle when the press missed.
    pub fn grab_offset(&self, track_y: f32) -> f32 {
        if self.on_thumb(track_y) { track_y - self.thumb_top } else { self.thumb_height / 2.0 }
    }

    /// The scroll offset that puts the grabbed point of the thumb under `track_y`.
    pub fn offset_for_track_y(&self, track_y: f32, grab_offset: f32) -> f32 {
        if self.scroll_range <= 0.0 || self.max_scroll <= 0.0 {
            return 0.0;
        }
        let thumb_top = (track_y - grab_offset).clamp(0.0, self.scroll_range);
        thumb_top / self.scroll_range * self.max_scroll
    }
}

/// The scrollable's own scrollbar is hidden; this canvas draws a thin one
/// that stays visible and supports click-to-jump and dragging.
struct FileListScrollbar(ScrollMetrics);

impl Program<Message> for FileListScrollbar {
    type State = ();

    fn update(&self, _state: &mut (), event: &Event, bounds: Rectangle, cursor: Cursor) -> Option<Action<Message>> {
        let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else {
            return None;
        };
        let position = cursor.position_in(bounds).filter(|_| self.0.max_scroll > 0.0)?;
        let press =
            Message::FileListScrollbarPress { track_y: position.y, track_top: bounds.y, track_height: bounds.height };
        Some(Action::publish(press).and_capture())
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let metrics = self.0;
        if metrics.max_scroll > 0.0 {
            let hovered = cursor.position_in(bounds).is_some_and(|point| metrics.on_thumb(point.y));
            frame.fill_rectangle(
                iced::Point::new(1.0, metrics.thumb_top),
                iced::Size::new(bounds.width - 2.0, metrics.thumb_height),
                style::muted(theme).scale_alpha(if hovered { 0.72 } else { 0.48 }),
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, _state: &(), bounds: Rectangle, cursor: Cursor) -> mouse::Interaction {
        match cursor.position_in(bounds) {
            Some(point) if self.0.max_scroll > 0.0 && self.0.on_thumb(point.y) => mouse::Interaction::Grab,
            Some(_) if self.0.max_scroll > 0.0 => mouse::Interaction::Pointer,
            _ => mouse::Interaction::default(),
        }
    }
}

impl FileButton {
    pub fn new(path: PathBuf, base_path: &Path, is_dir: bool) -> Self {
        let label = path
            .strip_prefix(base_path)
            .ok()
            .and_then(crate::path_util::file_name_lossy)
            .unwrap_or_else(|| crate::path_util::file_label(&path));
        FileButton { file_path: path, label, is_dir }
    }
}

/// Folders first, then files, each alphabetically; or an error row's text.
pub fn list_buttons(dir: &Path) -> (Vec<FileButton>, Option<String>) {
    match crate::library::list_directory(dir) {
        Ok(entries) => {
            let mut buttons: Vec<FileButton> =
                entries.into_iter().map(|entry| FileButton::new(entry.path, dir, entry.is_dir)).collect();
            buttons.sort_by_cached_key(|button| (!button.is_dir, button.label.to_lowercase()));
            (buttons, None)
        }
        Err(err) => (Vec::new(), Some(err)),
    }
}

impl FileSelector {
    pub fn new(dir: &Path) -> Self {
        let (file_list, list_error) = list_buttons(dir);
        FileSelector {
            current_dir: dir.to_owned(),
            file_list,
            list_error,
            search_show_directories: true,
            ..Self::default()
        }
    }

    /// Moves to `dir`. An active search keeps its results; otherwise the list is reloaded.
    pub fn reload_directory(&mut self, dir: &Path) {
        self.current_dir = dir.to_owned();
        if !self.search_active() {
            (self.file_list, self.list_error) = list_buttons(dir);
        }
        self.selection.clear();
        self.hovered_file = None;
    }

    pub fn search_active(&self) -> bool {
        crate::metadata::file_search_active(&self.search_value, &self.tag_filters)
    }

    pub fn tag_only_search(&self) -> bool {
        !self.tag_filters.is_empty() && self.search_value.trim().is_empty()
    }

    /// Tab completes the tag field name only while the input is a bare, matching field prefix.
    pub fn tag_search_can_autocomplete(&self) -> bool {
        let input = &self.tag_search_value;
        !input.contains(':') && !input.trim().is_empty() && tag_field_best_match(input).is_some()
    }

    pub fn scroll_metrics(&self) -> ScrollMetrics {
        ScrollMetrics::new(self.file_list.len(), self.list_scroll_offset, self.list_viewport_height)
    }

    pub fn select_row(&mut self, index: usize, shift: bool, control: bool) {
        if index < self.file_list.len() {
            let order: Vec<usize> = (0..self.file_list.len()).collect();
            self.selection.click(index, shift, control, &order);
        }
    }

    /// Swap in a new listing. Selection is carried over by path: row indices
    /// from the old listing would otherwise point at different files, and
    /// actions on "the selected file" (auto-tag, tag editor) would hit them.
    pub fn set_file_list(&mut self, file_list: Vec<FileButton>, list_error: Option<String>) {
        let key_at = |&index: &usize| self.file_list.get(index).map(|entry| cache_key(&entry.file_path));
        let selected: HashSet<PathBuf> = self.selection.iter().filter_map(key_at).collect();
        let anchor = self.selection.anchor().and_then(key_at);

        self.file_list = file_list;
        self.list_error = list_error;
        self.hovered_file = None;
        self.selection.clear();
        if selected.is_empty() {
            return;
        }
        let mut new_anchor = None;
        let mut indices = Vec::new();
        for (index, entry) in self.file_list.iter().enumerate() {
            let key = cache_key(&entry.file_path);
            if selected.contains(&key) {
                indices.push(index);
                if anchor.as_ref() == Some(&key) {
                    new_anchor = Some(index);
                }
            }
        }
        self.selection.set(indices, new_anchor);
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    pub fn sync_selection_for_path(&mut self, path: &Path) {
        let key = cache_key(path);
        if let Some(index) = self.file_list.iter().position(|entry| cache_key(&entry.file_path) == key) {
            self.selection.select_only(index);
        }
    }

    pub fn selected_audio_path(&self) -> Option<PathBuf> {
        self.selection
            .anchor()
            .or_else(|| self.selection.iter().min())
            .and_then(|&index| self.file_list.get(index))
            .filter(|entry| !entry.is_dir && is_audio(&entry.file_path))
            .map(|entry| entry.file_path.clone())
    }

    /// Adds `filter`, replacing any filter on the same field.
    pub fn add_tag_filter(&mut self, filter: TagFilter) {
        match self.tag_filters.iter_mut().find(|entry| entry.field == filter.field) {
            Some(existing) => *existing = filter,
            None => self.tag_filters.push(filter),
        }
    }

    pub fn view(&self, search_enabled: bool, favorites: &FavoritesStore, modifiers: Modifiers) -> Column<'_, Message> {
        let header =
            if self.favorites_only { favorites_list_header() } else { parent_directory_button(&self.current_dir) };
        let selected_hint = (self.selection.len() > 1).then(|| {
            let hint = format!("{} selected · Shift/Ctrl+click to extend", self.selection.len());
            container(text(hint).size(10).style(style::faded_text(0.58))).padding([4, 10]).width(Length::Fill)
        });
        let error = self.list_error.as_ref().map(|error| {
            container(text(error).size(12).color(Color::from_rgb(0.92, 0.55, 0.55)))
                .padding([8, 12])
                .width(Length::Fill)
        });
        let list = mouse_area(
            row![
                self.rows_view(search_enabled, favorites, modifiers),
                canvas::Canvas::new(FileListScrollbar(self.scroll_metrics()))
                    .width(Length::Fixed(SCROLLBAR_WIDTH))
                    .height(Length::Fill),
            ]
            .height(Length::Fill),
        )
        .on_enter(Message::FileListHoverChanged(true))
        .on_exit(Message::FileListHoverChanged(false));

        column![header, selected_hint, error, list, search_enabled.then(|| self.filter_dock())].height(Length::Fill)
    }

    /// Windowed rendering: only rows near the viewport become widgets;
    /// spacers stand in for the rest so the scroll range stays correct.
    fn rows_view(
        &self,
        search_enabled: bool,
        favorites: &FavoritesStore,
        modifiers: Modifiers,
    ) -> Element<'_, Message> {
        let total = self.file_list.len();
        let viewport_height =
            if self.list_viewport_height > 0.0 { self.list_viewport_height } else { FALLBACK_VIEWPORT_HEIGHT };
        let rows_in_view = (viewport_height / FILE_ROW_HEIGHT).ceil() as usize + 1;
        let first_in_view =
            ((self.list_scroll_offset / FILE_ROW_HEIGHT).floor() as usize).min(total.saturating_sub(rows_in_view));
        let start = first_in_view.saturating_sub(FILE_ROW_OVERDRAW);
        let end = (first_in_view + rows_in_view + FILE_ROW_OVERDRAW).min(total);
        let gap = |rows: usize| -> Element<'_, Message> {
            spacer(Length::Fill, Length::Fixed(rows as f32 * FILE_ROW_HEIGHT)).into()
        };

        let rows = self.file_list[start..end].iter().enumerate().map(|(offset, entry)| {
            let index = start + offset;
            file_row(
                entry,
                index,
                self.selection.contains(&index),
                self.hovered_file == Some(index),
                search_enabled,
                favorites.contains_listed(&entry.file_path),
                modifiers,
            )
        });
        let children =
            (start > 0).then(|| gap(start)).into_iter().chain(rows).chain((end < total).then(|| gap(total - end)));

        iced::widget::scrollable(Column::with_children(children))
            .id(Id::new(FILE_LIST_SCROLL_ID))
            .direction(scrollable::Direction::Vertical(Scrollbar::hidden()))
            .style(hidden_scrollbar_style)
            .on_scroll(Message::FileListScrolled)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn filter_dock(&self) -> Element<'_, Message> {
        let tag_chips = (!self.tag_filters.is_empty()).then(|| {
            container(
                Row::with_children(self.tag_filters.iter().map(tag_chip))
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .wrap()
                    .vertical_spacing(8),
            )
            .width(Length::Fill)
            .padding([4, 10])
        });
        let hint = text("Add filters like bpm:120, key:Am, or instrument:Kick")
            .size(10)
            .style(style::text_color(|theme| style::muted(theme).scale_alpha(0.85)));
        let error = self.tag_search_error.as_ref().map(|error| {
            container(text(error).size(11).color(style::ERROR))
                .padding([6, 10])
                .width(Length::Fill)
                .style(style::tinted(style::DANGER, 0.12, 0.28, 0.0))
        });
        let typing_field = !self.tag_search_value.is_empty() && !self.tag_search_value.contains(':');
        let suggestions = typing_field.then(|| tag_suggestions_panel(&self.tag_search_value)).flatten();

        let body = column![
            filter_label_row(file_search_header(self)),
            filter_input(
                TextInput::new("Search files…", &self.search_value)
                    .id(Id::new(FILE_SEARCH_INPUT_ID))
                    .on_input(|value| FilterMsg::Search(value).into())
                    .size(13),
                !self.search_value.is_empty(),
                FilterMsg::Search(String::new()),
                FilterMsg::SearchFocused(true),
                self.filter_focus == FilterFocus::FileSearch,
            ),
            bar(Length::Fill, Length::Fixed(1.0), |theme| {
                theme.extended_palette().background.strong.color.scale_alpha(0.22)
            }),
            filter_label_row(tag_section_header(self.tag_filters.len())),
            tag_chips,
            filter_label_row(hint),
            error,
            suggestions,
            filter_input(
                TextInput::new("title:value — Enter or Tab", &self.tag_search_value)
                    .id(Id::new(TAG_SEARCH_INPUT_ID))
                    .on_input(|value| FilterMsg::TagSearchInput(value).into())
                    .on_submit(FilterMsg::TagSearchSubmit.into())
                    .size(12),
                !self.tag_search_value.is_empty(),
                FilterMsg::TagSearchInput(String::new()),
                FilterMsg::TagSearchFocused(true),
                self.filter_focus == FilterFocus::TagSearch,
            ),
        ];

        container(column![bar(Length::Fill, Length::Fixed(2.0), |_| ACCENT.scale_alpha(0.42)), body])
            .width(Length::Fill)
            .style(|theme: &Theme| {
                let palette = theme.extended_palette();
                container::Style::default()
                    .background(sidebar_panel(theme).scale_alpha(0.98))
                    .border(style::outline(palette.background.strong.color.scale_alpha(0.30), 0.0))
                    .shadow(style::drop_shadow(palette.background.base.color.scale_alpha(0.55), -4.0, 14.0))
            })
            .into()
    }
}

/// Hides the scrollable's own scrollbar and middle-click autoscroll marker.
fn hidden_scrollbar_style(_theme: &Theme, _status: scrollable::Status) -> scrollable::Style {
    let clear = Color::TRANSPARENT.into();
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller { background: clear, border: Border::default() },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        auto_scroll: scrollable::AutoScroll {
            background: clear,
            border: Border::default(),
            shadow: iced::Shadow::default(),
            icon: Color::TRANSPARENT,
        },
    }
}

fn sidebar_panel(theme: &Theme) -> Color {
    let base = theme.extended_palette().background.base.color;
    Color::from_rgb(base.r * 0.52, base.g * 0.52, base.b * 0.54)
}

fn sidebar_section_style(theme: &Theme) -> container::Style {
    let strong = theme.extended_palette().background.strong.color;
    container::Style::default().background(sidebar_panel(theme)).border(style::outline(strong.scale_alpha(0.35), 0.0))
}

fn tree_icon_color(theme: &Theme, emphasized: bool) -> Color {
    let palette = theme.extended_palette();
    if emphasized {
        palette.primary.base.color.scale_alpha(0.85)
    } else {
        palette.background.base.text.scale_alpha(0.62)
    }
}

fn file_tree_button_style(theme: &Theme, status: button::Status, selected: bool) -> button::Style {
    let idle = if selected { ACCENT.scale_alpha(0.20) } else { Color::TRANSPARENT };
    let hovered = ACCENT.scale_alpha(if selected { 0.28 } else { 0.12 });
    let text_color = if selected || style::by_status(status, false, true, true) {
        theme.extended_palette().background.base.text
    } else {
        style::muted(theme)
    };
    let background = style::by_status(status, idle, hovered, ACCENT.scale_alpha(0.34));
    style::solid_button(text_color, Border::default(), background)
}

fn parent_directory_button(cwd: &Path) -> Element<'static, Message> {
    let parent = cwd.parent().unwrap_or(cwd).to_path_buf();
    container(
        button(
            row![
                icon("up_chevron.svg", 14.0, |theme| tree_icon_color(theme, false)),
                text(crate::path_util::truncate_path(cwd, 32)).size(11).style(style::muted_text),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .on_press(Message::ChangeDirectory(parent))
        .width(Length::Fill)
        .padding([8, 10])
        .style(|theme, status| file_tree_button_style(theme, status, false)),
    )
    .width(Length::Fill)
    .style(sidebar_section_style)
    .into()
}

fn favorites_list_header() -> Element<'static, Message> {
    container(
        row![
            text("★").size(12).color(ACCENT.scale_alpha(0.95)),
            text("Favorites").size(11).font(style::SEMIBOLD).style(style::muted_text),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding([8, 10])
    .style(sidebar_section_style)
    .into()
}

fn file_row(
    entry: &FileButton,
    index: usize,
    selected: bool,
    hovered: bool,
    search_enabled: bool,
    favorite: bool,
    modifiers: Modifiers,
) -> Element<'_, Message> {
    let audio = !entry.is_dir && is_audio(&entry.file_path);
    let highlight = selected || hovered;
    let label = container(
        text(&entry.label)
            .size(13)
            .wrapping(Wrapping::None)
            .width(Length::Fill)
            .font(if entry.is_dir { style::MEDIUM } else { iced::Font::DEFAULT })
            .style(style::highlight_text(highlight)),
    )
    .width(Length::Fill)
    .clip(true);
    let row_icon = |name, size| icon(name, size, move |theme| tree_icon_color(theme, highlight));

    let content: Row<'_, Message> = if entry.is_dir {
        row![row_icon("folder-solid.svg", 16.0), label].spacing(10)
    } else if audio {
        row![
            favorite_star_button(entry.file_path.clone(), favorite),
            row![row_icon("music-solid.svg", 12.0), label].spacing(8).align_y(Alignment::Center),
        ]
        .spacing(1)
    } else {
        row![label]
    };
    let content = content.align_y(Alignment::Center).width(Length::Fill);

    let select = Message::FileListSelect(index);
    let row_style = move |theme: &Theme, status| file_tree_button_style(theme, status, selected);
    let multi_select = modifiers.shift() || modifiers.control() || modifiers.logo();

    let clickable: Element<'_, Message> = if entry.is_dir {
        button(content).on_press(select).width(Length::Fill).padding([7, 10]).style(row_style).into()
    } else {
        let status = if hovered { button::Status::Hovered } else { button::Status::Active };
        let body = container(content).width(Length::Fill).padding([7, 10]).style(move |theme| container::Style {
            background: row_style(theme, status).background,
            ..container::Style::default()
        });
        if audio && !multi_select {
            // A press may become a drag out of the app; `app/input.rs` decides on release.
            mouse_area(body)
                .on_press(Message::FileDragPress { path: entry.file_path.clone(), from_file_list: true })
                .on_enter(Message::FileRowHover(index))
                .on_exit(Message::FileRowLeave)
                .interaction(mouse::Interaction::Grab)
                .into()
        } else {
            button(body).on_press(select).width(Length::Fill).padding(0).style(row_style).into()
        }
    };

    let path = entry.file_path.clone();
    let menu = iced_aw::ContextMenu::new(clickable, move || {
        file_context_menu(&path, audio && search_enabled, audio, audio.then_some(favorite))
    })
    .style(super::widgets::context_menu_style);

    row![selection_stripe(selected, 3.0, Length::Fill), menu]
        .width(Length::Fill)
        .height(Length::Fixed(FILE_ROW_HEIGHT))
        .into()
}

fn favorite_star_button(path: PathBuf, favorite: bool) -> Element<'static, Message> {
    let star =
        icon(
            "star-solid.svg",
            11.0,
            move |_| {
                if favorite { ACCENT.scale_alpha(0.95) } else { MUTED_ICON.scale_alpha(0.42) }
            },
        );
    button(container(star).center(Length::Fill))
        .padding(0)
        .width(Length::Fixed(15.0))
        .height(Length::Fixed(15.0))
        .on_press(Message::ToggleFavorite(path))
        .style(move |_theme, status| {
            let border = if favorite || style::by_status(status, false, true, true) {
                ACCENT.scale_alpha(if favorite { 0.28 } else { 0.16 })
            } else {
                Color::TRANSPARENT
            };
            let idle = if favorite { ACCENT.scale_alpha(0.10) } else { Color::TRANSPARENT };
            let background = style::by_status(status, idle, ACCENT.scale_alpha(0.15), ACCENT.scale_alpha(0.22));
            style::solid_button(MUTED_ICON, style::outline(border, 4.0), background)
        })
        .into()
}

fn filter_label_row<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content).width(Length::Fill).padding([8, 10]).into()
}

fn filter_section_header(icon_name: &'static str, title: &'static str) -> Row<'static, Message> {
    row![
        icon(icon_name, 12.0, |_| ACCENT.scale_alpha(0.85)),
        text(title).size(11).font(style::SEMIBOLD).style(style::muted_text),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
}

fn accent_badge(label: impl text::IntoFragment<'static>, size: u32) -> Element<'static, Message> {
    container(text(label).size(size).font(style::SEMIBOLD).color(ACCENT.scale_alpha(0.95)))
        .padding([2, 6])
        .style(style::tinted(ACCENT, 0.18, 0.28, 8.0))
        .into()
}

fn file_search_header(selector: &FileSelector) -> Element<'static, Message> {
    let on_off = move |on: bool| if on { ACCENT.scale_alpha(0.95) } else { MUTED_ICON };
    let (favorites, directories, case) =
        (selector.favorites_only, selector.search_show_directories, selector.search_case_sensitive);
    // "a" lights up when matching ignores case, "A" when it respects it.
    let case_letter = move |letter: &'static str, lit: bool| {
        text(letter).size(11).style(style::text_color(move |theme| {
            if lit { ACCENT.scale_alpha(0.95) } else { style::muted(theme).scale_alpha(0.72) }
        }))
    };
    filter_section_header("search-solid.svg", "File search")
        .push(selector.search_active().then(|| accent_badge("active", 9)))
        .push(spacer(Length::Fill, Length::Shrink))
        .push(toggle_chip(
            text(if favorites { "★" } else { "☆" }).size(13).color(on_off(favorites)),
            favorites,
            FilterMsg::ToggleFavoritesOnly,
        ))
        .push(toggle_chip(
            icon("folder-solid.svg", 12.0, move |_| on_off(directories)),
            directories,
            FilterMsg::ToggleShowDirectories,
        ))
        .push(toggle_chip(
            row![case_letter("a", !case), case_letter("A", case).font(style::SEMIBOLD)].align_y(Alignment::Center),
            case,
            FilterMsg::ToggleCaseSensitive,
        ))
        .into()
}

fn toggle_chip<'a>(content: impl Into<Element<'a, Message>>, active: bool, message: FilterMsg) -> Element<'a, Message> {
    button(content)
        .padding([2, 6])
        .on_press(message.into())
        .style(move |theme: &Theme, status| {
            let palette = theme.extended_palette();
            let border =
                if active { ACCENT.scale_alpha(0.35) } else { palette.background.strong.color.scale_alpha(0.24) };
            let idle = if active { ACCENT.scale_alpha(0.14) } else { Color::TRANSPARENT };
            let background = style::by_status(status, idle, ACCENT.scale_alpha(0.18), ACCENT.scale_alpha(0.26));
            style::solid_button(palette.background.base.text, style::outline(border, 6.0), background)
        })
        .into()
}

fn tag_section_header(filter_count: usize) -> Element<'static, Message> {
    let badges = (filter_count > 0).then(|| [accent_badge(filter_count.to_string(), 10), accent_badge("active", 9)]);
    filter_section_header("music-solid.svg", "Tag filters").extend(badges.into_iter().flatten()).into()
}

fn tag_chip(filter: &TagFilter) -> Element<'static, Message> {
    let field = filter.field;
    let accent = style::tag_field_color(field);
    let close = button(text("×").size(13)).on_press(FilterMsg::TagFilterRemove(field).into()).padding([2, 4]).style(
        |theme: &Theme, status| {
            let text_color = if style::by_status(status, true, false, false) {
                style::text_alpha(theme, 0.55)
            } else {
                theme.extended_palette().background.base.text
            };
            let background = style::by_status(
                status,
                Color::TRANSPARENT,
                style::DANGER.scale_alpha(0.22),
                style::DANGER.scale_alpha(0.38),
            );
            style::solid_button(text_color, iced::border::rounded(8.0), background)
        },
    );

    container(
        row![
            container(text(field.as_str()).size(10).font(style::SEMIBOLD).style(style::faded_text(0.95)))
                .padding([3, 7])
                .style(move |_theme| {
                    container::background(accent.scale_alpha(0.55)).border(iced::border::rounded(6.0))
                }),
            text(filter.value.clone()).size(12).font(style::MEDIUM),
            close,
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([5, 8])
    .style(move |theme| {
        let palette = theme.extended_palette();
        container::Style::default()
            .background(palette.background.base.color.scale_alpha(0.55))
            .border(style::outline(accent.scale_alpha(0.35), 16.0))
            .shadow(style::drop_shadow(palette.background.base.color.scale_alpha(0.35), 2.0, 6.0))
    })
    .into()
}

/// Field names matching what is typed in the tag search, best match highlighted.
fn tag_suggestions_panel(input: &str) -> Option<Element<'static, Message>> {
    let best_match = tag_field_best_match(input);
    let suggestions = tag_field_suggestions(input);
    if suggestions.is_empty() {
        return None;
    }
    let rows = suggestions.into_iter().map(|field| {
        let highlighted = best_match == Some(field);
        button(
            row![
                container(
                    text(field.as_str())
                        .size(11)
                        .font(style::SEMIBOLD)
                        .color(style::tag_field_color(field).scale_alpha(0.95)),
                )
                .padding([2, 0]),
                text(":").size(11).style(style::muted_text),
                spacer(Length::Fill, Length::Shrink),
                text(field.label()).size(10).style(style::muted_text),
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .width(Length::Fill),
        )
        .on_press(FilterMsg::TagSuggestionSelect(field).into())
        .width(Length::Fill)
        .padding([6, 10])
        .style(move |theme, status| file_tree_button_style(theme, status, highlighted))
        .into()
    });
    let panel = container(Column::with_children(rows).padding([4, 0])).width(Length::Fill).style(|theme| {
        let palette = theme.extended_palette();
        container::Style::default()
            .background(palette.background.base.color.scale_alpha(0.94))
            .border(style::outline(palette.background.strong.color.scale_alpha(0.40), 0.0))
            .shadow(style::drop_shadow(palette.background.base.color.scale_alpha(0.65), 4.0, 10.0))
    });
    Some(panel.into())
}

/// A filter text input with a × clear button inside its right edge.
///
/// While inactive, a click-catcher covers the input so focus moves through
/// `on_activate` instead of letting two inputs hold focus at once.
fn filter_input<'a>(
    input: TextInput<'a, Message>,
    show_clear: bool,
    on_clear: FilterMsg,
    on_activate: FilterMsg,
    active: bool,
) -> Element<'a, Message> {
    let on_activate: Message = on_activate.into();
    let input = mouse_area(input.padding(Padding::from([8.0, 10.0]).right(FILTER_CLEAR_INSET)).width(Length::Fill))
        .on_press(on_activate.clone());

    let clear_slot: Element<'a, Message> = if show_clear {
        button(text("×").size(FILTER_CLEAR_TEXT_SIZE))
            .on_press(on_clear.into())
            .padding(FILTER_CLEAR_PAD)
            .style(|theme: &Theme, status| {
                let text_color = style::text_alpha(theme, style::by_status(status, 0.45, 0.85, 0.85));
                style::solid_button(text_color, iced::border::rounded(4.0), Color::TRANSPARENT)
            })
            .into()
    } else {
        spacer(Length::Fixed(FILTER_CLEAR_INSET), Length::Fixed(0.0)).into()
    };

    // The overlay spans the whole input so the button can sit inside its right padding.
    // It must stay transparent to the mouse everywhere except the button itself: `Space` and
    // `button` report `Interaction::None` outside their own bounds, which is what lets clicks
    // reach the TextInput underneath. Giving this layer a background, a `mouse_area`, or any
    // other widget that claims an interaction will silently break click-to-focus.
    // When inactive, the click-catcher sits under this overlay so × still clears.
    let clear_overlay = container(clear_slot)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Center);

    let layers = if active {
        stack![input, clear_overlay]
    } else {
        let click_catcher = mouse_area(spacer(Length::Fill, Length::Fill)).on_press(on_activate);
        stack![input, click_catcher, clear_overlay]
    };
    container(layers.width(Length::Fill)).width(Length::Fill).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrollbar_round_trips_offsets_through_the_track() {
        let metrics = ScrollMetrics::new(1_000, 0.0, 600.0);
        assert!(metrics.max_scroll > 0.0);
        let grab = metrics.grab_offset(metrics.thumb_height / 2.0);
        let middle = metrics.offset_for_track_y(metrics.scroll_range / 2.0 + grab, grab);
        assert!((middle - metrics.max_scroll / 2.0).abs() < 1.0);
        assert_eq!(metrics.offset_for_track_y(-50.0, grab), 0.0);
        assert_eq!(metrics.offset_for_track_y(10_000.0, grab), metrics.max_scroll);
    }

    #[test]
    fn short_lists_do_not_scroll() {
        let metrics = ScrollMetrics::new(3, 0.0, 600.0);
        assert_eq!(metrics.max_scroll, 0.0);
        assert_eq!(metrics.offset_for_track_y(300.0, 0.0), 0.0);
    }
}
