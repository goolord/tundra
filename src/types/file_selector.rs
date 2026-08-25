pub use super::common::*;
use iced::widget::scrollable;
use iced::widget::Button;
use iced::widget::Column;
use iced::widget::Container;
use iced::widget::Row;
use iced::widget::Svg;
use iced::widget::Text;
use iced::widget::TextInput;
use iced::Element;
use iced::Length;
use std::cmp::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct FileSelector {
    pub current_dir: PathBuf,
    pub file_list: Vec<FileButton>,
    pub selected_file: Option<usize>,
    pub search_value: String,
    pub list_error: Option<String>,
}

pub struct FileList;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileButton {
    pub file_path: PathBuf,
    pub label: String,
}

pub struct DirUp;

impl DirUp {
    pub fn view(&self, cwd: PathBuf) -> Button<'_, Message> {
        let label = format!("  {}", truncate_path(&cwd, 28));
        Button::new(
            Row::new()
                .push(
                    Svg::from_path(resource_path("up_chevron.svg"))
                        .height(Length::Fixed(16.0))
                        .width(Length::Shrink),
                )
                .push(Text::new(label).size(16)),
        )
        .on_press(Message::ChangeDirectory(match cwd.parent() {
            Some(x) => x.to_path_buf(),
            None => cwd,
        }))
        .width(Length::Fill)
    }
}

impl FileList {
    pub fn file_filter(x: &Path) -> bool {
        (x.is_dir() && !is_hidden(x)) || is_audio(x)
    }

    pub fn list_buttons(dir: &Path) -> (Vec<FileButton>, Option<String>) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                return (
                    Vec::new(),
                    Some(format!("Cannot read {}: {err}", dir.display())),
                );
            }
        };

        let mut buttons: Vec<FileButton> = entries
            .filter_map(|entry| {
                let entry = entry.ok()?;
                if Self::file_filter(&entry.path()) {
                    Some(FileButton::new(entry.path(), dir))
                } else {
                    None
                }
            })
            .collect();
        buttons.sort();
        (buttons, None)
    }
}

impl FileSelector {
    pub fn new(dir: &Path) -> Self {
        let (file_list, list_error) = FileList::list_buttons(dir);
        FileSelector {
            current_dir: dir.to_owned(),
            file_list,
            selected_file: None,
            search_value: String::new(),
            list_error,
        }
    }

    pub fn view(&self) -> Column<'_, Message> {
        let dir_up =
            Container::new(DirUp.view(self.current_dir.to_owned()).padding(5)).width(Length::Fill);

        let mut column = Column::new().push(dir_up);

        if let Some(error) = &self.list_error {
            column = column.push(
                Container::new(Text::new(error).size(16))
                    .padding(8)
                    .width(Length::Fill),
            );
        }

        let new_col: Vec<Element<Message>> = self
            .file_list
            .iter()
            .map(|button| {
                let element: Button<'_, Message> = button.view();
                Container::new(element.padding(5))
                    .width(Length::Fill)
                    .into()
            })
            .collect();

        let fs = scrollable(Column::with_children(new_col).spacing(0).padding(0))
            .height(Length::Fill);
        let search = TextInput::new("Search", &self.search_value)
            .on_input(Message::Search)
            .size(18)
            .padding(10);

        column.push(fs).push(search)
    }
}

impl FileButton {
    pub fn new(path: PathBuf, base_path: &Path) -> Self {
        let label = match path.strip_prefix(base_path) {
            Ok(relative) => format!("  {}", relative.display()),
            Err(_) => format!("  {}", path.display()),
        };
        FileButton {
            file_path: path,
            label,
        }
    }

    pub fn view(&self) -> Button<'_, Message> {
        let text = Text::new(&self.label).size(16);
        let label = Row::with_children(if self.file_path.is_dir() {
            vec![
                Svg::from_path(resource_path("folder-solid.svg"))
                    .width(Length::Fixed(24.0))
                    .into(),
                text.into(),
            ]
        } else if is_audio(&self.file_path) {
            vec![
                Svg::from_path(resource_path("music-solid.svg"))
                    .height(Length::Fixed(24.0))
                    .width(Length::Shrink)
                    .into(),
                text.into(),
            ]
        } else {
            vec![text.into()]
        });
        Button::new(label)
            .on_press(Message::SelectedFile(Some(self.file_path.to_owned())))
            .width(Length::Fill)
    }
}

impl PartialOrd for FileButton {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileButton {
    fn cmp(&self, other: &Self) -> Ordering {
        self.file_path.cmp(&other.file_path)
    }
}
