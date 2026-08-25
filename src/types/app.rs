use super::*;
use super::common::debounce;
use futures::future::{AbortHandle, Abortable};
use futures::*;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

use iced::{Element, Length, Task};
use std::sync::Arc;
use std::{collections::hash_map::HashMap, path::PathBuf};
use walkdir::WalkDir;

pub struct App {
    pub file_selector: FileSelector,
    pub menu: MainMenu,
    pub player: Player,
    pub search_thread: AbortHandle,
    pub dir_cache: DirCache,
    player_msgs: Option<futures::channel::mpsc::UnboundedReceiver<super::PlayerMsg>>,
    player_events_started: bool,
}

pub struct DirCache(HashMap<PathBuf, Vec<PathBuf>>);

impl DirCache {
    fn new() -> DirCache {
        DirCache(HashMap::new())
    }

    fn insert(&mut self, k: PathBuf, v: Vec<PathBuf>) -> Option<Vec<PathBuf>> {
        self.0.insert(k, v)
    }

    fn get(&self, k: &PathBuf) -> Option<&Vec<PathBuf>> {
        self.0.get(k)
    }

    fn contains_key(&self, k: &PathBuf) -> bool {
        self.0.contains_key(k)
    }

    fn get_path() -> Option<std::path::PathBuf> {
        match dirs::cache_dir() {
            Some(mut cache_dir) => {
                cache_dir.push("tundra");
                let _ = std::fs::create_dir(&cache_dir);
                cache_dir.push("dir_cache");
                cache_dir.set_extension("bin");
                Some(cache_dir)
            }
            None => None,
        }
    }

    fn get_dir_cache() -> DirCache {
        match DirCache::get_path() {
            Some(dir_cache) => match std::fs::read(dir_cache) {
                Ok(s) => bincode::deserialize(&s).map_or(DirCache::new(), DirCache),
                Err(_) => DirCache::new(),
            },
            None => DirCache::new(),
        }
    }

    fn persist(&self) {
        let Some(dir_cache) = DirCache::get_path() else {
            return;
        };
        let Ok(bytes) = bincode::serialize(&self.0) else {
            eprintln!("Failed to serialize directory cache");
            return;
        };
        if let Err(err) = std::fs::write(dir_cache, bytes) {
            eprintln!("Failed to write directory cache: {err}");
        }
    }
}

pub fn app() {
    iced::application(App::default, App::update, App::view)
        .title(App::title)
        .antialiasing(true)
        .run()
        .unwrap()
}

impl Default for App {
    fn default() -> App {
        let current_dir = startup_directory();
        let file_selector = FileSelector::new(&current_dir);
        let menu = MainMenu::new();
        let (player, player_msgs) = Player::new();
        let search_thread = AbortHandle::new_pair().0;
        let dir_cache = DirCache::get_dir_cache();
        App {
            file_selector,
            menu,
            player,
            search_thread,
            dir_cache,
            player_msgs: Some(player_msgs),
            player_events_started: false,
        }
    }
}

impl App {
    pub fn title(&self) -> String {
        String::from("Tundra Sample Browser")
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectedFile(selected_file) => {
                match &selected_file {
                    Some(file_path) => {
                        if file_path.is_dir() {
                            self.file_selector = FileSelector::new(file_path);
                        } else {
                            match self.player.play_file(file_path) {
                                Ok(()) => {
                                    self.file_selector.selected_file =
                                        self.file_selector.file_list.iter().position(|x| {
                                            selected_file
                                                .as_ref()
                                                .is_some_and(|y| y == &x.file_path)
                                        });
                                    if !self.player_events_started {
                                        self.player_events_started = true;
                                        if let Some(recv) = self.player_msgs.take() {
                                            return Task::perform(recv.into_future(), |x| {
                                                Message::PlayerMsg((x.0, Arc::new(x.1)))
                                            });
                                        }
                                    }
                                }
                                Err(err) => {
                                    self.player.set_error(err);
                                    self.file_selector.selected_file = None;
                                }
                            }
                        }
                    }
                    None => {
                        self.player.waveform = None;
                    }
                }

                Task::none()
            }

            Message::ChangeDirectory(parent_dir) => {
                self.file_selector = FileSelector::new(&parent_dir);
                if !self.dir_cache.contains_key(&self.file_selector.current_dir) {
                    let walker = future::lazy(|_| {
                        let children: Vec<PathBuf> = WalkDir::new(&parent_dir)
                            .max_depth(100)
                            .max_open(100)
                            .follow_links(true)
                            .into_iter()
                            .filter_entry(|e| FileList::file_filter(e.path()))
                            .filter_map(|e| match e {
                                Ok(e) => Some(e.path().to_path_buf()),
                                Err(_) => None,
                            })
                            .collect();
                        (parent_dir, children)
                    });
                    Task::perform(walker, Message::InsertDircache)
                } else {
                    Task::none()
                }
            }

            Message::Search(search_str) => {
                self.search_thread.abort();
                match self.dir_cache.get(&self.file_selector.current_dir) {
                    Some(children) => {
                        let (abort_handle, abort_reg) = AbortHandle::new_pair();
                        self.search_thread = abort_handle;
                        self.file_selector.search_value = search_str.clone();
                        if search_str.len() > 2 {
                            let matcher = SkimMatcherV2::default();
                            let children_clone = children.clone();
                            let file_list = Abortable::new(
                                async move {
                                    debounce(std::time::Duration::from_millis(200)).await;
                                    children_clone
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
                                        .collect()
                                },
                                abort_reg,
                            );
                            Task::perform(file_list, Message::SearchCompleted)
                        } else {
                            let (file_list, list_error) =
                                FileList::list_buttons(&self.file_selector.current_dir);
                            self.file_selector.file_list = file_list;
                            self.file_selector.list_error = list_error;
                            Task::none()
                        }
                    }
                    None => {
                        let (abort_handle, abort_reg) = AbortHandle::new_pair();
                        self.search_thread = abort_handle;
                        self.file_selector.search_value = search_str.clone();
                        let current_dir = self.file_selector.current_dir.clone();
                        if search_str.len() > 2 {
                            let matcher = SkimMatcherV2::default();
                            let file_list = Abortable::new(
                                async move {
                                    debounce(std::time::Duration::from_millis(300)).await;
                                    WalkDir::new(&current_dir)
                                        .max_depth(100)
                                        .max_open(100)
                                        .follow_links(true)
                                        .into_iter()
                                        .filter_entry(|e| FileList::file_filter(e.path()))
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
                            Task::perform(file_list, Message::SearchCompleted)
                        } else {
                            let (file_list, list_error) =
                                FileList::list_buttons(&self.file_selector.current_dir);
                            self.file_selector.file_list = file_list;
                            self.file_selector.list_error = list_error;
                            Task::none()
                        }
                    }
                }
            }

            Message::SearchCompleted(file_list_res) => {
                if let Ok(file_list) = file_list_res {
                    self.file_selector.file_list = file_list
                        .iter()
                        .map(|x| FileButton::new(x.to_path_buf(), &self.file_selector.current_dir))
                        .collect();
                    self.file_selector.list_error = None;
                }
                Task::none()
            }

            Message::TogglePlaying => {
                if self
                    .player
                    .controls
                    .is_playing
                    .load(std::sync::atomic::Ordering::SeqCst)
                {
                    self.player.pause();
                } else {
                    self.player.play();
                }
                Task::none()
            }

            Message::StopPlayback => {
                self.player.stop();
                Task::none()
            }

            Message::InsertDircache((parent_dir, children)) => {
                self.dir_cache.insert(parent_dir, children);
                self.dir_cache.persist();
                Task::none()
            }

            Message::InvalidateDircache => {
                self.dir_cache = DirCache::new();
                self.dir_cache.persist();
                Task::none()
            }

            Message::PlayerMsg((msg, recv)) => {
                match msg {
                    Some(PlayerMsg::PlayingStored) => (),
                    Some(PlayerMsg::SinkEmpty) => self.player.pause(),
                    Some(PlayerMsg::StreamFailed) => self.player.set_error(
                        "Audio output unavailable. Check your sound device.".into(),
                    ),
                    None => return Task::none(),
                }
                match Arc::into_inner(recv) {
                    None => {
                        eprintln!("Message::PlayerMsg Arc::into_inner failed");
                        Task::none()
                    }
                    Some(recv) => Task::perform(recv.into_future(), |x| {
                        Message::PlayerMsg((x.0, Arc::new(x.1)))
                    }),
                }
            }

            Message::DismissError => {
                self.player.error = None;
                Task::none()
            }

            Message::Seek(p) => {
                self.player.controls.seeking(p);
                Task::none()
            }

            Message::SeekCommit => {
                if let Some(seekbar) = &self.player.controls.seekbar {
                    self.player.seek(seekbar.seeking);
                }
                Task::none()
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let menu = self.menu.view();
        let file_selector = iced::widget::container(self.file_selector.view())
            .width(Length::FillPortion(1))
            .max_width(320.0)
            .height(Length::Fill)
            .padding(4);

        let player = self.player.view();

        let workspace = iced::widget::row![file_selector, player]
            .spacing(4)
            .height(Length::Fill)
            .width(Length::Fill);

        iced::widget::column![menu, workspace]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}
