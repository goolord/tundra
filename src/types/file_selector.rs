pub use super::common::*;
pub use super::style::*;
use ::iced::pure::widget::{Button, Column, Container, Row, Svg, Text, TextInput};
use iced::pure::scrollable;
use iced::Length;
use std::cmp::*;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FileSelector {
    pub current_dir: PathBuf,
    pub file_list: Vec<FileButton>,
    pub selected_file: Option<PathBuf>,
    pub search_value: String,
}

#[derive(Debug, Clone)]
pub struct FileList {
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileButton {
    pub file_path: PathBuf,
}

pub struct DirUp;

impl DirUp {
    pub fn view(&self, cwd: PathBuf) -> Button<Message> {
        let mut text = String::new();
        text.push_str("  ");
        text.push_str(cwd.to_str().unwrap_or("Go up"));
        Button::new(
            Row::new()
                .push(Svg::from_path("resources/up_chevron.svg").height(Length::Units(16)))
                .push(Text::new(text).size(24)),
        )
        .on_press(Message::ChangeDirectory(match cwd.parent() {
            Some(x) => x.to_path_buf(),
            None => cwd,
        }))
        .style(DirUpButton)
        .width(Length::Fill)
    }
}

impl FileList {
    pub fn file_filter(x: PathBuf) -> bool {
        (x.is_dir() && !is_hidden(&x)) || x.extension().map_or(false, is_audio)
    }
    pub fn list_dir(
        dir: &Path,
    ) -> std::iter::FilterMap<
        std::fs::ReadDir,
        fn(x: std::io::Result<std::fs::DirEntry>) -> Option<std::fs::DirEntry>,
    > {
        fn the_filter(x: std::io::Result<std::fs::DirEntry>) -> Option<std::fs::DirEntry> {
            match x {
                Ok(x) => {
                    if FileList::file_filter(x.path()) {
                        Some(x)
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        }
        fs::read_dir(dir).unwrap().filter_map(the_filter)
    }

    pub fn new(dir: &Path) -> Vec<FileButton> {
        let mut buttons: Vec<FileButton> = FileList::list_dir(dir)
            .map(|x| FileButton::new(x.path()))
            .collect();
        buttons.sort();
        buttons
    }
}

impl FileSelector {
    pub fn new(dir: &Path) -> Self {
        FileSelector {
            current_dir: dir.to_owned(),
            file_list: FileList::new(dir),
            selected_file: None,
            search_value: String::new(),
        }
    }

    pub fn view(&self) -> Column<Message> {
        let selected_file = self.selected_file.as_ref();
        let dir_up =
            Container::new(DirUp.view(self.current_dir.to_owned()).padding(5)).width(Length::Fill);
        let mut new_col: Vec<iced::pure::Element<Message>> =
            Vec::with_capacity(self.file_list.len() + 1);
        new_col.push(dir_up.into());
        new_col.extend(self.file_list.iter().map(|button| {
            let path = button.file_path.to_owned();
            let element: Button<Message> = button.view(&self.current_dir);
            let mut container = Container::new(element.padding(10)).width(Length::Fill);
            if Some(path.canonicalize().unwrap())
                == selected_file.map(|x| x.canonicalize().unwrap())
            {
                container = container.style(SelectedContainer);
            }
            container.into()
        }));
        let fs_column = Column::with_children(new_col).spacing(0).padding(0);
        let fs = scrollable(fs_column).height(Length::Fill);
        let search = TextInput::new("Search", &self.search_value, Message::Search)
            .style(FileSearch)
            .size(32)
            .padding(10);

        Column::new().push(fs).push(search)
    }
}

impl FileButton {
    pub fn new(x: PathBuf) -> Self {
        FileButton { file_path: x }
    }

    pub fn view(&self, base_path: &Path) -> Button<Message> {
        let fp = remove_prefix(
            self.file_path.to_str().unwrap(),
            base_path.as_os_str().to_str().unwrap(),
        );
        let mut file_string = String::with_capacity(2 + fp.len());
        file_string.push_str("  ");
        file_string.push_str(fp);
        let text = Text::new(file_string).size(24);
        let label = Row::with_children(if self.file_path.is_dir() {
            vec![
                Svg::from_path("./resources/folder-solid.svg")
                    .width(Length::Units(24))
                    .into(),
                text.into(),
            ]
        } else if is_audio(self.file_path.as_os_str()) {
            vec![
                Svg::from_path("./resources/music-solid.svg")
                    .height(Length::Units(24))
                    .width(Length::Shrink)
                    .into(),
                text.into(),
            ]
        } else {
            vec![text.into()]
        });
        Button::new(label)
            .style(FileButton_)
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

fn remove_prefix<'a>(s: &'a str, prefix: &str) -> &'a str {
    match s.strip_prefix(prefix) {
        Some(s) => unsafe { s.get_unchecked(1..s.len()) },
        None => s,
    }
}
