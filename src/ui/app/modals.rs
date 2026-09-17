//! The settings, auto-tag, and tag editor modals.

use super::{App, Modal, pick_audio_file, pick_folder, run_blocking};
use crate::auto_tag::{self, ClassifyError};
use crate::library::AddDirectory;
use crate::metadata::{auto_tag_field_status, instrument_tag, write_auto_tags, write_manual_tags};
use crate::ui::auto_tag::AutoTagState;
use crate::ui::message::{AutoTagMsg, Message, SettingsMsg, TagEditorMsg};
use crate::ui::settings::{AUTO_TAG_ALREADY_COMPLETE, AUTO_TAG_INSTRUMENT_PRESENT, SELECT_AUDIO_FIRST};
use crate::ui::tag_editor::TagEditorState;
use iced::Task;
use std::path::PathBuf;

impl App {
    /// Nothing else may open until the first-run settings are done.
    pub(super) fn first_run_settings_open(&self) -> bool {
        self.modal == Modal::Settings && self.settings_first_run
    }

    /// Opens `modal` in place of any dialog or bulk auto-tag. False if first-run settings block it.
    fn open_modal(&mut self, modal: Modal) -> bool {
        if self.first_run_settings_open() {
            return false;
        }
        self.dialog = None;
        self.bulk_auto_tag.close();
        self.modal = modal;
        true
    }

    /// The file an action may work on, or the reason it may not.
    fn tag_target(&self, target: Option<&PathBuf>) -> Result<PathBuf, &'static str> {
        let path = target.ok_or(SELECT_AUDIO_FIRST)?;
        match self.allowed_audio_error(path) {
            Some(err) => Err(err),
            None => Ok(path.clone()),
        }
    }

    pub(super) fn update_settings(&mut self, message: SettingsMsg) -> Task<Message> {
        match message {
            SettingsMsg::Open => {
                if self.open_modal(Modal::Settings) {
                    self.settings_error = None;
                }
                Task::none()
            }
            SettingsMsg::Close => self.close_settings(),
            SettingsMsg::PickDirectory => {
                let start_dir = self
                    .allowed_directories
                    .startup_directory()
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(super::startup_directory);
                Task::perform(pick_folder(start_dir), |picked| {
                    SettingsMsg::DirectoryPicked(picked).into()
                })
            }
            SettingsMsg::DirectoryPicked(Some(path)) => match self.allowed_directories.add(&path) {
                AddDirectory::Added(resolved) => {
                    self.allowed_directories.persist();
                    self.settings_error = None;
                    if self.dir_cache.contains_key(&resolved) {
                        self.refresh_search_if_active()
                    } else {
                        self.walk_task(resolved)
                    }
                }
                AddDirectory::Unresolved => {
                    self.settings_error = Some("Could not resolve that directory.".into());
                    Task::none()
                }
                AddDirectory::Duplicate => Task::none(),
            },
            SettingsMsg::DirectoryPicked(None) => Task::none(),
            SettingsMsg::RemoveDirectory(path) => {
                self.allowed_directories.remove(&path);
                self.allowed_directories.persist();
                self.refresh_search_if_active()
            }
        }
    }

    fn close_settings(&mut self) -> Task<Message> {
        if self.allowed_directories.is_empty() {
            self.settings_error = Some("Add at least one directory to search.".into());
            return Task::none();
        }
        self.modal = Modal::None;
        self.settings_first_run = false;
        self.settings_error = None;
        self.prune_caches();
        let mut tasks = vec![self.warm_allowed_caches(), self.refresh_search_if_active()];
        if self.pending_launch_path.is_some() && self.player.audio_ready() {
            tasks.push(self.open_pending_launch());
        } else if let Some(dir) = self.allowed_directories.startup_directory()
            && !self.allowed_directories.contains_path(&self.file_selector.current_dir)
        {
            // The folder on screen may be one the user just disallowed.
            tasks.push(self.navigate_directory(dir));
        }
        Task::batch(tasks)
    }

    pub(super) fn update_auto_tag(&mut self, message: AutoTagMsg) -> Task<Message> {
        match message {
            AutoTagMsg::Open => {
                if self.open_modal(Modal::AutoTag) {
                    self.auto_tag = AutoTagState::for_target(self.file_selector.selected_audio_path());
                }
            }
            AutoTagMsg::OpenFor(path) => {
                if self.open_modal(Modal::AutoTag) {
                    self.set_auto_tag_target(path);
                }
            }
            AutoTagMsg::Close => self.modal = Modal::None,
            AutoTagMsg::PickFile => {
                let start_dir = self
                    .auto_tag
                    .target
                    .as_deref()
                    .and_then(std::path::Path::parent)
                    .map_or_else(|| self.file_selector.current_dir.clone(), |dir| dir.to_path_buf());
                return Task::perform(pick_audio_file(start_dir), |picked| {
                    AutoTagMsg::FilePicked(picked).into()
                });
            }
            AutoTagMsg::FilePicked(Some(path)) => match self.allowed_audio_error(&path) {
                Some(err) => self.auto_tag.set_error(err),
                None => self.auto_tag = AutoTagState::for_target(Some(path)),
            },
            AutoTagMsg::FilePicked(None) => {}
            AutoTagMsg::Run => return self.run_auto_tag(),
            AutoTagMsg::Completed(path, result) => {
                // The modal may have been closed and reopened on another file while
                // this ran; a label for one file must never be offered for another.
                if self.auto_tag.running && self.auto_tag.target.as_ref() == Some(&path) {
                    self.auto_tag.finish_run(result);
                }
            }
            AutoTagMsg::ToggleDetails => self.auto_tag.details_open = !self.auto_tag.details_open,
            AutoTagMsg::Apply => return self.apply_auto_tag(),
            AutoTagMsg::Applied(path, instrument, result) => return self.auto_tag_applied(path, instrument, result),
        }
        Task::none()
    }

    fn set_auto_tag_target(&mut self, path: PathBuf) {
        match self.allowed_audio_error(&path) {
            Some(err) => {
                self.auto_tag = AutoTagState::default();
                self.auto_tag.set_error(err);
            }
            None => self.auto_tag = AutoTagState::for_target(Some(path)),
        }
    }

    fn run_auto_tag(&mut self) -> Task<Message> {
        let path = match self.tag_target(self.auto_tag.target.as_ref()) {
            Ok(path) => path,
            Err(err) => {
                self.auto_tag.set_error(err);
                return Task::none();
            }
        };
        if let Some(existing) = instrument_tag(&path) {
            self.auto_tag.existing_instrument = Some(existing);
        }
        if auto_tag_field_status(&path).is_some_and(|status| !status.allows_instrument_work()) {
            self.auto_tag.clear_error();
            self.auto_tag.status = AUTO_TAG_INSTRUMENT_PRESENT.into();
            return Task::none();
        }
        self.auto_tag.begin_run();
        let classify_path = path.clone();
        Task::perform(
            run_blocking(move || auto_tag::classify_file(&classify_path)),
            move |result| {
                let result = result.unwrap_or_else(|()| {
                    Err(ClassifyError::new(
                        "Couldn't analyze this file.",
                        "Classifier thread stopped unexpectedly.",
                    ))
                });
                AutoTagMsg::Completed(path.clone(), result).into()
            },
        )
    }

    fn apply_auto_tag(&mut self) -> Task<Message> {
        let path = match self.tag_target(self.auto_tag.target.as_ref()) {
            Ok(path) => path,
            Err(err) => {
                self.auto_tag.set_error(err);
                return Task::none();
            }
        };
        let Some(status) = self.auto_tag.path_status.filter(|status| !status.is_complete()) else {
            self.auto_tag.set_error(AUTO_TAG_ALREADY_COMPLETE);
            return Task::none();
        };
        // A missing or outdated Tundra instrument needs a fresh detection;
        // re-stamping the old label would mark it current without checking.
        let instrument = if status.allows_instrument_work() {
            match &self.auto_tag.result {
                Some(result) => result.instrument.clone(),
                None => {
                    self.auto_tag.set_error("Detect an instrument before applying tags.");
                    return Task::none();
                }
            }
        } else {
            instrument_tag(&path).unwrap_or_default()
        };
        self.auto_tag.applying = true;
        self.auto_tag.clear_error();
        let (write_path, write_instrument) = (path.clone(), instrument.clone());
        Task::perform(
            run_blocking(move || write_auto_tags(&write_path, &write_instrument)),
            move |result| {
                let result = result.unwrap_or_else(|()| Err("Applying tags stopped unexpectedly.".into()));
                AutoTagMsg::Applied(path.clone(), instrument.clone(), result).into()
            },
        )
    }

    fn auto_tag_applied(&mut self, path: PathBuf, instrument: String, result: Result<bool, String>) -> Task<Message> {
        // The file may have changed even if the modal moved on.
        let changed = matches!(result, Ok(true)) && self.merge_path_metadata(&path);
        if self.auto_tag.target.as_ref() == Some(&path) {
            let state = &mut self.auto_tag;
            state.applying = false;
            state.refresh_from_disk();
            match result {
                Ok(written) => {
                    state.applied = true;
                    state.result = None;
                    state.status = match (written, instrument.is_empty()) {
                        (false, _) => AUTO_TAG_ALREADY_COMPLETE.into(),
                        (true, true) => "Applied missing tags.".into(),
                        (true, false) => format!("Applied tags (instrument: {instrument})."),
                    };
                }
                Err(err) => {
                    state.applied = false;
                    state.set_error(err);
                }
            }
        }
        if changed {
            self.refresh_search_if_active()
        } else {
            Task::none()
        }
    }

    pub(super) fn update_tag_editor(&mut self, message: TagEditorMsg) -> Task<Message> {
        match message {
            TagEditorMsg::OpenFor(path) => {
                // Reopening a file whose save is still running shows that save rather
                // than reloading tags it is about to replace.
                if self.tag_editor.saving && self.tag_editor.target.as_ref() == Some(&path) {
                    self.modal = Modal::TagEditor;
                } else if self.open_modal(Modal::TagEditor) {
                    self.tag_editor = match self.allowed_audio_error(&path) {
                        Some(err) => {
                            let mut state = TagEditorState::default();
                            state.set_error(err);
                            state
                        }
                        None => TagEditorState::for_path(path.clone(), &self.metadata_cache.tag_fields_for(&path)),
                    };
                }
                Task::none()
            }
            TagEditorMsg::Close => {
                self.modal = Modal::None;
                if !self.tag_editor.saving {
                    self.tag_editor = TagEditorState::default();
                }
                Task::none()
            }
            TagEditorMsg::Input(field, value) => {
                self.tag_editor.set_field(field, value);
                Task::none()
            }
            TagEditorMsg::Save => {
                let path = match self.tag_target(self.tag_editor.target.as_ref()) {
                    Ok(path) => path,
                    Err(err) => {
                        self.tag_editor.set_error(err);
                        return Task::none();
                    }
                };
                let edits = self.tag_editor.edits.clone();
                self.tag_editor.begin_save();
                let write_path = path.clone();
                Task::perform(
                    run_blocking(move || write_manual_tags(&write_path, &edits)),
                    move |result| {
                        let result = result.unwrap_or_else(|()| Err("Saving stopped unexpectedly.".into()));
                        TagEditorMsg::Saved(path.clone(), result).into()
                    },
                )
            }
            TagEditorMsg::Saved(path, result) => {
                // The file may have changed even if the editor moved on.
                let changed = result.is_ok() && self.merge_path_metadata(&path);
                if self.tag_editor.target.as_ref() == Some(&path) {
                    self.tag_editor.finish_save(result);
                }
                if changed {
                    self.refresh_search_if_active()
                } else {
                    Task::none()
                }
            }
        }
    }
}
