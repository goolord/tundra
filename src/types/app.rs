use super::*;
use super::common::debounce;
use futures::future::{AbortHandle, Abortable};
use futures::*;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

use iced::widget::{button, column, container, row, stack, text};
use iced::{Color, Element, Event, Length, Subscription, Task};
use iced::event;
use iced::futures::stream;
use iced::window;
use iced_aw::ICED_AW_FONT_BYTES;
use futures::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::collections::hash_map::HashMap;
use std::time::Duration;
use walkdir::WalkDir;

pub struct App {
    pub file_selector: FileSelector,
    pub menu: MainMenu,
    pub player: Player,
    pub search_thread: AbortHandle,
    pub dir_cache: DirCache,
    player_msgs: Option<futures::channel::mpsc::UnboundedReceiver<super::PlayerMsg>>,
    player_events_started: bool,
    drag_over: bool,
    notice: Option<String>,
    waveform_hovered: bool,
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
        .font(ICED_AW_FONT_BYTES)
        .subscription(App::subscription)
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
            drag_over: false,
            notice: None,
            waveform_hovered: false,
        }
    }
}

impl App {
    pub fn subscription(state: &App) -> Subscription<Message> {
        let file_events = event::listen_with(|event, _status, _window| match event {
            Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
            Event::Window(window::Event::FileHovered(path)) => Some(Message::FileHovered(path)),
            Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FilesHoverLeft),
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::ModifiersChanged(modifiers))
            }
            _ => None,
        });

        let playing = state.player.controls.is_playing.load(Ordering::Relaxed)
            && state.player.waveform.is_some();
        let playback_tick = if playing {
            Subscription::run_with((), |_| {
                stream::unfold((), |()| async {
                    async_io::Timer::after(Duration::from_millis(33)).await;
                    Some((Message::PlaybackTick, ()))
                })
            })
        } else {
            Subscription::none()
        };

        let waveform_keys = if state.waveform_hovered && state.player.waveform.is_some() {
            event::listen_with(|event, _status, _window| match event {
                Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. }) => {
                    Some(Message::WaveformKey(key))
                }
                _ => None,
            })
        } else {
            Subscription::none()
        };

        Subscription::batch([file_events, playback_tick, waveform_keys])
    }

    fn open_path(&mut self, path: &Path) -> Task<Message> {
        if path.is_dir() {
            self.file_selector = FileSelector::new(path);
            Task::none()
        } else if is_audio(path) {
            self.play_audio(path)
        } else {
            Task::none()
        }
    }

    fn play_audio(&mut self, file_path: &Path) -> Task<Message> {
        match self.player.play_file(file_path) {
            Ok(()) => {
                self.file_selector.selected_file = self
                    .file_selector
                    .file_list
                    .iter()
                    .position(|entry| entry.file_path == file_path);
                self.ensure_player_events()
            }
            Err(err) => {
                self.player.set_error(err);
                self.file_selector.selected_file = None;
                Task::none()
            }
        }
    }

    fn ensure_player_events(&mut self) -> Task<Message> {
        if !self.player_events_started {
            self.player_events_started = true;
            if let Some(recv) = self.player_msgs.take() {
                return Task::perform(recv.into_future(), |x| {
                    Message::PlayerMsg((x.0, Arc::new(x.1)))
                });
            }
        }
        Task::none()
    }

    pub fn title(&self) -> String {
        String::from("Tundra Sample Browser")
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectedFile(selected_file) => match &selected_file {
                Some(file_path) => self.open_path(file_path),
                None => {
                    self.player.waveform = None;
                    Task::none()
                }
            },

            Message::FileDropped(path) => {
                self.drag_over = false;
                self.open_path(&path)
            }

            Message::FileHovered(path) => {
                self.drag_over = is_audio(&path) || path.is_dir();
                Task::none()
            }

            Message::FilesHoverLeft => {
                self.drag_over = false;
                Task::none()
            }

            Message::OpenFolder => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .pick_folder()
                        .await
                        .map(|folder| folder.path().to_path_buf())
                },
                Message::FolderPicked,
            ),

            Message::OpenFile => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .add_filter(
                            "Audio",
                            &["flac", "wav", "mp3", "ogg"],
                        )
                        .pick_file()
                        .await
                        .map(|file| file.path().to_path_buf())
                },
                Message::FilePicked,
            ),

            Message::FilePicked(file) => {
                if let Some(path) = file {
                    return self.open_path(&path);
                }
                Task::none()
            }

            Message::FolderPicked(folder) => {
                if let Some(path) = folder {
                    self.file_selector = FileSelector::new(&path);
                }
                Task::none()
            }

            Message::GoHome => {
                if let Some(home) = dirs::home_dir() {
                    self.file_selector = FileSelector::new(&home);
                }
                Task::none()
            }

            Message::RefreshDirectory => {
                let current_dir = self.file_selector.current_dir.clone();
                self.file_selector = FileSelector::new(&current_dir);
                Task::none()
            }

            Message::Quit => iced::exit(),

            Message::About => {
                self.notice = Some(format!(
                    "Tundra {} — browse and preview audio samples (FLAC, WAV, MP3, OGG). \
                     Drag-and-drop works on Windows, macOS, and X11; on native Wayland use File → Open File.",
                    env!("CARGO_PKG_VERSION")
                ));
                Task::none()
            }

            Message::DismissNotice => {
                self.notice = None;
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

            Message::WaveformViewChanged(view) => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformZoomIn => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    view.zoom_in();
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformZoomOut => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    view.zoom_out();
                    waveform.set_view(view);
                }
                Task::none()
            }

            Message::WaveformHoverChanged(hovered) => {
                self.waveform_hovered = hovered;
                Task::none()
            }

            Message::WaveformKey(key) => {
                if let Some(waveform) = &mut self.player.waveform {
                    let mut view = waveform.view_state();
                    if WaveFormView::apply_key(&mut view, &key) {
                        waveform.set_view(view);
                    }
                }
                Task::none()
            }

            Message::PlaybackTick => {
                self.player.sync_playback_ui();
                Task::none()
            }

            Message::ModifiersChanged(modifiers) => {
                if let Some(waveform) = &mut self.player.waveform {
                    waveform.set_modifiers(modifiers);
                }
                Task::none()
            }

            Message::Seek(p) => {
                self.player.begin_scrub(p);
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

        let notice = self.notice.as_ref().map(|notice_text| {
            container(
                row![
                    notice_text.as_str(),
                    button(text("Dismiss")).on_press(Message::DismissNotice),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .padding(6)
            .width(Length::Fill)
        });

        let file_selector = container(self.file_selector.view())
            .width(Length::FillPortion(1))
            .height(Length::Fill)
            .padding(4)
            .style(|theme| {
                let base = theme.extended_palette().background.base.color;
                container::Style {
                    background: Some(
                        Color::from_rgb(base.r * 0.58, base.g * 0.58, base.b * 0.58).into(),
                    ),
                    ..Default::default()
                }
            });

        let player = container(if self.drag_over {
            stack![
                self.player.view(),
                container(
                    text("Drop audio file or folder")
                        .size(18)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_theme| container::Style {
                    background: Some(Color::from_rgba(0.08, 0.12, 0.18, 0.82).into()),
                    ..Default::default()
                }),
            ]
            .width(Length::Fill)
            .height(Length::Fill)
        } else {
            stack![self.player.view()]
                .width(Length::Fill)
                .height(Length::Fill)
        })
        .width(Length::FillPortion(3))
        .height(Length::Fill);

        let workspace = row![file_selector, player]
            .spacing(4)
            .height(Length::Fill)
            .width(Length::Fill);

        let mut layout = column![menu].width(Length::Fill).height(Length::Fill);
        if let Some(notice) = notice {
            layout = layout.push(notice);
        }
        layout.push(workspace).into()
    }
}
