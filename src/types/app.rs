use super::*;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use iced::{Application, Clipboard, Command, Container, Element, Length, Row, executor};
use walkdir::WalkDir;
use std::thread;

pub struct App {
    pub file_selector: FileSelector,
    pub player: Player,
}

impl Application for App {
    type Message = Message;
    type Executor = executor::Default;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let current_dir = std::env::current_dir().unwrap();
        let file_selector = FileSelector::new(&current_dir);
        let player = Player::new();
        let app = App {
            file_selector,
            player,
        };
        (app, Command::none())
    }

    fn title(&self) -> String {
        String::from("Tundra Sample Browser")
    }

    fn update(&mut self, message: Message, _clipboard: &mut Clipboard) -> Command<Message> {
        match message {
            Message::SelectedFile(selected_file) => {
                match &selected_file {
                    Some(file_path) => {
                        if file_path.is_dir() {
                            self.file_selector = FileSelector::new(file_path);
                        } else {
                            self.player.play_file(file_path.to_owned());
                            self.file_selector.selected_file = selected_file;
                        }
                    }
                    None => {
                        self.player.waveform = None;
                    }
                }

                Command::none()
            }
            Message::ChangeDirectory(parent_dir) => {
                self.file_selector = FileSelector::new(&parent_dir);
                Command::none()
            }
            Message::Search(search_str) => {
                if search_str.len() > 2 {
                    let matcher = SkimMatcherV2::default();
                    let file_list = WalkDir::new(&self.file_selector.current_dir)
                        .max_depth(100)
                        .max_open(100)
                        .follow_links(true)
                        .into_iter()
                        .filter_entry(|e| !is_hidden(e))
                        .filter_map(|e| match e {
                            Ok(e) => {
                                let epath = e.path();
                                if matcher
                                    .fuzzy_match(epath.to_string_lossy().as_ref(), &search_str)
                                    .is_some()
                                {
                                    Some(FileButton::new(epath.to_path_buf()))
                                } else {
                                    None
                                }
                            }
                            Err(_) => None,
                        })
                        .collect();
                    self.file_selector.file_list = file_list;
                } else {
                    self.file_selector.file_list = FileList::new(&self.file_selector.current_dir)
                }
                self.file_selector.search_value = search_str;
                Command::none()
            }
        }
    }

    fn view(&mut self) -> Element<Message> {
        let player = self.player.view();
        let file_selector_container = Container::new(self.file_selector.view())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(Container_)
            .center_x();

        Row::new().push(file_selector_container).push(player).into()
    }
}
