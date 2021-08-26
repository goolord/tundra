pub use super::common::*;
pub use super::style::*;
use iced::Row;
use iced::{
    button, scrollable, text_input, Button, Column, Container, Length, Scrollable, Text, TextInput,
};
use std::cmp::*;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FileSelector {
    pub scroll_state: scrollable::State,
    pub current_dir: PathBuf,
    pub dir_up: DirUp,
    pub file_list: Vec<FileButton>,
    pub selected_file: Option<PathBuf>,
    pub search: text_input::State,
    pub search_value: String,
}

#[derive(Debug, Clone)]
pub struct FileList {
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileButton {
    pub file_button: button::State,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct DirUp {
    pub button: button::State,
}

impl DirUp {
    pub fn view(&mut self, cwd: PathBuf) -> Button<Message> {
        let mut text = String::new();
        text.push_str("  ");
        text.push_str(cwd.to_str().unwrap_or("Go up"));
        Button::new(
            &mut self.button,
            Row::new()
                .push(iced::Svg::from_path("resources/up_chevron.svg").height(Length::Units(16)))
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
            scroll_state: scrollable::State::new(),
            current_dir: dir.to_owned(),
            dir_up: DirUp {
                button: button::State::new(),
            },
            file_list: FileList::new(dir),
            selected_file: None,
            search: text_input::State::new(),
            search_value: String::new(),
        }
    }

    pub fn view(&mut self) -> Column<Message> {
        let selected_file = self.selected_file.as_ref();
        let dir_up = Container::new(self.dir_up.view(self.current_dir.to_owned()))
            .padding(5)
            .width(Length::Fill);
        let mut new_col: Vec<iced::Element<Message>> = Vec::with_capacity(self.file_list.len() + 1);
        new_col.push(dir_up.into());
        new_col.extend( self.file_list.iter_mut().map(|button| {
            let path = button.file_path.to_owned();
            let element: Button<Message> = button.view(&self.current_dir);
            let mut container = Container::new(element).padding(5).width(Length::Fill);
            if Some(path.canonicalize().unwrap())
                == selected_file.map(|x| x.canonicalize().unwrap())
            {
                container = container.style(SelectedContainer);
            }
            container.into()
        }));
        let fs_column = Column::with_children(new_col);
        let fs = Scrollable::new(&mut self.scroll_state)
            .push(fs_column)
            .width(Length::Fill)
            .height(Length::Fill);
        let search = TextInput::new(
            &mut self.search,
            "Search",
            &self.search_value,
            Message::Search,
        )
        .style(FileSearch)
        .size(32)
        .padding(10);

        Column::new().push(fs).push(search)
    }
}

impl FileButton {
    pub fn new(x: PathBuf) -> Self {
        FileButton {
            file_button: button::State::new(),
            file_path: x,
        }
    }

    pub fn view(&mut self, base_path: &Path) -> Button<Message> {
        let mut file_string = String::new();
        file_string.push_str("  ");
        file_string.push_str(self.file_path.to_str().unwrap());
        let text = Text::new(remove_prefix(
            &file_string,
            base_path.as_os_str().to_str().unwrap(),
        ))
        .size(24);
        let label = Row::new();
        let label_2 = if self.file_path.is_dir() {
            label
                .push(iced::Svg::from_path("./resources/folder-solid.svg").width(Length::Units(24)))
        } else {
            label
        };
        let label_3 = if is_audio(self.file_path.as_os_str()) {
            label_2
                .push(iced::Svg::from_path("./resources/music-solid.svg").height(Length::Units(24)))
        } else {
            label_2
        }
        .push(text);
        Button::new(&mut self.file_button, label_3)
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
