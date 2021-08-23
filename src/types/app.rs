use super::*;
use futures::future::{AbortHandle, Abortable};
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use iced::{executor, Application, Clipboard, Command, Container, Element, Length, Row};
use walkdir::WalkDir;
use std::{collections::hash_map::HashMap, path::PathBuf};

pub struct App {
    pub file_selector: FileSelector,
    pub player: Player,
    pub search_thread: AbortHandle,
    pub dir_cache: HashMap<PathBuf, Vec<PathBuf>>,
}

impl Application for App {
    type Message = Message;
    type Executor = executor::Default;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let current_dir = std::env::current_dir().unwrap();
        let file_selector = FileSelector::new(&current_dir);
        let player = Player::new();
        let search_thread = AbortHandle::new_pair().0;
        let dir_cache = HashMap::new();
        let app = App {
            file_selector,
            player,
            search_thread,
            dir_cache,
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
                let children: Vec<PathBuf> = WalkDir::new(&parent_dir)
                    .max_depth(100)
                    .max_open(100)
                    .follow_links(true)
                    .into_iter()
                    .filter_entry(|e| FileList::file_filter(e.path().into()))
                    .filter_map(|e| match e {
                        Ok(e) => {
                            let epath = e.path();
                            Some(epath.to_path_buf())
                        }
                        Err(_) => None,
                    })
                    .collect();
                self.dir_cache.insert(parent_dir, children);
                Command::none()
            }
            Message::Search(search_str) => {
                self.search_thread.abort();
                match self.dir_cache.get(&self.file_selector.current_dir) {
                    // dir is cached
                    Some(children) => {
                        self.file_selector.search_value = search_str.clone();
                        if search_str.len() > 2 {
                            let matcher = SkimMatcherV2::default();
                            let file_list: Vec<PathBuf> = children
                                    .iter()
                                    .filter_map(|e| {
                                        if matcher
                                            .fuzzy_match(
                                                e.to_string_lossy().as_ref(),
                                                &search_str,
                                            )
                                            .is_some()
                                        {
                                            Some(e.to_owned())
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();
                            Command::perform(std::future::ready(Ok(file_list)), Message::SearchCompleted)
                        } else {
                            self.file_selector.file_list = FileList::new(&self.file_selector.current_dir);
                            Command::none()
                        }
                    },
                    // dir not cached
                    None => {
                        let (abort_handle, abort_reg) = AbortHandle::new_pair();
                        self.search_thread = abort_handle;
                        self.file_selector.search_value = search_str.clone();
                        let current_dir = self.file_selector.current_dir.clone();
                        if search_str.len() > 2 {
                            let matcher = SkimMatcherV2::default();
                            let file_list = Abortable::new(
                                async move {
                                    WalkDir::new(&current_dir)
                                        .max_depth(100)
                                        .max_open(100)
                                        .follow_links(true)
                                        .into_iter()
                                        .filter_entry(|e| FileList::file_filter(e.path().into()))
                                        .filter_map(|e| match e {
                                            Ok(e) => {
                                                let epath = e.path();
                                                if matcher
                                                    .fuzzy_match(
                                                        epath.to_string_lossy().as_ref(),
                                                        &search_str,
                                                    )
                                                    .is_some()
                                                {
                                                    Some(epath.to_path_buf())
                                                } else {
                                                    None
                                                }
                                            }
                                            Err(_) => None,
                                        })
                                        .collect()
                                },
                                abort_reg,
                            );
                            Command::perform(file_list, Message::SearchCompleted)
                        } else {
                            self.file_selector.file_list = FileList::new(&self.file_selector.current_dir);
                            Command::none()
                        }
                    }
                }
            }
            Message::SearchCompleted(file_list_res) => {
                if let Ok(file_list) = file_list_res {
                    self.file_selector.file_list = file_list
                        .iter()
                        .map(|x| FileButton::new(x.to_path_buf()))
                        .collect();
                }
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
