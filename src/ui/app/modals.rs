//! The settings, auto-tag, and tag editor modals.

use super::{App, Modal, background, pick_audio_file, pick_folder};
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
        self.allowed_audio_error(path).map_or_else(|| Ok(path.clone()), Err)
    }

    /// Re-runs the search when a tag write changed the index.
    fn tags_written(&mut self, changed: bool) -> Task<Message> {
        if changed { self.refresh_search_if_active() } else { Task::none() }
    }

    pub(super) fn update_settings(&mut self, message: SettingsMsg) -> Task<Message> {
        match message {
            SettingsMsg::Open => {
                if self.open_modal(Modal::Settings) {
                    self.settings_error = None;
                }
            }
            SettingsMsg::Close => return self.close_settings(),
            SettingsMsg::PickDirectory => {
                let start_dir = self
                    .allowed_directories
                    .startup_directory()
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(super::startup_directory);
                return pick_folder(start_dir, |picked| SettingsMsg::DirectoryPicked(picked).into());
            }
            SettingsMsg::DirectoryPicked(Some(path)) => match self.allowed_directories.add(&path) {
                AddDirectory::Added(resolved) => {
                    self.allowed_directories.persist();
                    self.settings_error = None;
                    return if self.dir_cache.contains_key(&resolved) {
                        self.refresh_search_if_active()
                    } else {
                        self.walk_task(resolved)
                    };
                }
                AddDirectory::Unresolved => self.settings_error = Some("Could not resolve that directory.".into()),
                AddDirectory::Duplicate => {}
            },
            SettingsMsg::DirectoryPicked(None) => {}
            SettingsMsg::RemoveDirectory(path) => {
                self.allowed_directories.remove(&path);
                self.allowed_directories.persist();
                return self.refresh_search_if_active();
            }
        }
        Task::none()
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
                    self.auto_tag = AutoTagState::default();
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
                return pick_audio_file(start_dir, |picked| AutoTagMsg::FilePicked(picked).into());
            }
            AutoTagMsg::FilePicked(Some(path)) => self.set_auto_tag_target(path),
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

    /// Retargets the modal at `path`, or shows why it cannot be tagged.
    fn set_auto_tag_target(&mut self, path: PathBuf) {
        match self.allowed_audio_error(&path) {
            Some(err) => self.auto_tag.set_error(err),
            None => self.auto_tag = AutoTagState::for_target(Some(path)),
        }
    }

    fn run_auto_tag(&mut self) -> Task<Message> {
        let target = self.tag_target(self.auto_tag.target.as_ref());
        let Ok(path) = target.map_err(|err| self.auto_tag.set_error(err)) else {
            return Task::none();
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
        background(
            move || auto_tag::classify_file(&classify_path),
            || {
                let details = "Classifier thread stopped unexpectedly.";
                Err(ClassifyError::new("Couldn't analyze this file.", details))
            },
            move |result| AutoTagMsg::Completed(path, result).into(),
        )
    }

    fn apply_auto_tag(&mut self) -> Task<Message> {
        let target = self.tag_target(self.auto_tag.target.as_ref());
        let Ok(path) = target.map_err(|err| self.auto_tag.set_error(err)) else {
            return Task::none();
        };
        let Some(status) = self.auto_tag.path_status.filter(|status| !status.is_complete()) else {
            self.auto_tag.set_error(AUTO_TAG_ALREADY_COMPLETE);
            return Task::none();
        };
        // A missing or outdated Tundra instrument needs a fresh detection;
        // re-stamping the old label would mark it current without checking.
        let instrument = if status.allows_instrument_work() {
            let Some(result) = &self.auto_tag.result else {
                self.auto_tag.set_error("Detect an instrument before applying tags.");
                return Task::none();
            };
            result.instrument.clone()
        } else {
            instrument_tag(&path).unwrap_or_default()
        };
        self.auto_tag.applying = true;
        self.auto_tag.clear_error();
        let (write_path, write_instrument) = (path.clone(), instrument.clone());
        background(
            move || write_auto_tags(&write_path, &write_instrument),
            || Err("Applying tags stopped unexpectedly.".into()),
            move |result| AutoTagMsg::Applied(path, instrument, result).into(),
        )
    }

    fn auto_tag_applied(&mut self, path: PathBuf, instrument: String, result: Result<bool, String>) -> Task<Message> {
        // The file may have changed even if the modal moved on.
        let changed = matches!(result, Ok(true)) && self.merge_path_metadata(&path);
        if self.auto_tag.target.as_ref() == Some(&path) {
            let state = &mut self.auto_tag;
            state.applying = false;
            state.refresh_from_disk();
            state.applied = result.is_ok();
            match result {
                Ok(written) => {
                    state.result = None;
                    state.status = match (written, instrument.is_empty()) {
                        (false, _) => AUTO_TAG_ALREADY_COMPLETE.into(),
                        (true, true) => "Applied missing tags.".into(),
                        (true, false) => format!("Applied tags (instrument: {instrument})."),
                    };
                }
                Err(err) => state.set_error(err),
            }
        }
        self.tags_written(changed)
    }

    pub(super) fn update_tag_editor(&mut self, message: TagEditorMsg) -> Task<Message> {
        match message {
            // Reopening a file whose save is still running shows that save rather
            // than reloading tags it is about to replace.
            TagEditorMsg::OpenFor(path) if self.tag_editor.saving && self.tag_editor.target.as_ref() == Some(&path) => {
                self.modal = Modal::TagEditor;
            }
            TagEditorMsg::OpenFor(path) => {
                if self.open_modal(Modal::TagEditor) {
                    self.tag_editor = match self.allowed_audio_error(&path) {
                        Some(err) => {
                            let mut state = TagEditorState::default();
                            state.set_error(err);
                            state
                        }
                        None => TagEditorState::for_path(path.clone(), &self.metadata_cache.tag_fields_for(&path)),
                    };
                }
            }
            TagEditorMsg::Close => {
                self.modal = Modal::None;
                if !self.tag_editor.saving {
                    self.tag_editor = TagEditorState::default();
                }
            }
            TagEditorMsg::Input(field, value) => self.tag_editor.set_field(field, value),
            TagEditorMsg::Save => {
                let target = self.tag_target(self.tag_editor.target.as_ref());
                let Ok(path) = target.map_err(|err| self.tag_editor.set_error(err)) else {
                    return Task::none();
                };
                let edits = self.tag_editor.edits.clone();
                self.tag_editor.begin_save();
                let write_path = path.clone();
                return background(
                    move || write_manual_tags(&write_path, &edits),
                    || Err("Saving stopped unexpectedly.".into()),
                    move |result| TagEditorMsg::Saved(path, result).into(),
                );
            }
            TagEditorMsg::Saved(path, result) => {
                // The file may have changed even if the editor moved on.
                let changed = result.is_ok() && self.merge_path_metadata(&path);
                if self.tag_editor.target.as_ref() == Some(&path) {
                    self.tag_editor.finish_save(result);
                }
                return self.tags_written(changed);
            }
        }
        Task::none()
    }
}
