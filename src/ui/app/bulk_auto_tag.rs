//! The Bulk Auto Tag modal's handlers. The scan and the writes run on
//! background threads; see `crate::bulk_auto_tag`.

use super::{App, Modal, background, pick_folder};
use crate::bulk_auto_tag::{self, BulkApplySummary, BulkPhase, ScanError};
use crate::ui::bulk_auto_tag::BulkAutoTagPhase;
use crate::ui::message::{BulkAutoTagMsg, Message};
use crate::ui::settings::FOLDER_OUTSIDE_ALLOWED;
use iced::Task;
use std::path::PathBuf;

impl App {
    pub(super) fn update_bulk_auto_tag(&mut self, message: BulkAutoTagMsg) -> Task<Message> {
        let (shift, control) = self.click_modifiers();
        let state = &mut self.bulk_auto_tag;
        match message {
            BulkAutoTagMsg::Open => {
                if !self.first_run_settings_open() {
                    self.dialog = None;
                    self.modal = Modal::None;
                    self.bulk_auto_tag.open();
                }
            }
            BulkAutoTagMsg::Close => {
                let applying = state.phase == Some(BulkAutoTagPhase::Applying) && state.job.is_some();
                // The first Close while writing asks the writes to stop; a second closes anyway.
                if applying && !state.apply_stop_requested {
                    state.request_stop_apply();
                } else {
                    state.close();
                }
            }
            BulkAutoTagMsg::PickDirectory => {
                let start_dir = state.root.clone().unwrap_or_else(|| self.file_selector.current_dir.clone());
                return pick_folder(start_dir, |picked| BulkAutoTagMsg::DirectoryPicked(picked).into());
            }
            BulkAutoTagMsg::DirectoryPicked(Some(dir)) => {
                if self.allowed_directories.contains_path(&dir) {
                    state.root = Some(dir);
                    state.error = None;
                } else {
                    state.set_error(FOLDER_OUTSIDE_ALLOWED);
                }
            }
            BulkAutoTagMsg::DirectoryPicked(None) => {}
            BulkAutoTagMsg::RunScan => return self.start_bulk_scan(),
            // The view reads progress from the job; the tick only redraws it.
            BulkAutoTagMsg::ProgressTick => {}
            BulkAutoTagMsg::ScanCompleted(generation, result) => {
                if state.finish_job(generation) {
                    match result {
                        Ok(summary) => state.finish_scan(summary),
                        Err(ScanError::Cancelled) => {}
                        Err(ScanError::Failed(message)) => state.set_error(message),
                    }
                }
            }
            BulkAutoTagMsg::SetFileAccepted(row, accepted) => state.set_accepted([row], accepted),
            BulkAutoTagMsg::SelectFile(row) => state.select_file(row, shift, control),
            BulkAutoTagMsg::SelectDirectory(dir_idx) => state.select_directory(dir_idx, shift, control),
            BulkAutoTagMsg::SelectAll => state.select_all_files(),
            BulkAutoTagMsg::ClearSelection => state.selection.clear(),
            BulkAutoTagMsg::CheckSelected(accepted) => {
                state.set_accepted(state.selection.iter().collect::<Vec<_>>(), accepted)
            }
            BulkAutoTagMsg::CheckAll(accepted) => state.set_accepted(0..state.files.len(), accepted),
            BulkAutoTagMsg::ToggleDirectoryExpanded(dir_idx) => {
                if let Some(group) = state.groups.get_mut(dir_idx) {
                    group.expanded = !group.expanded;
                }
            }
            BulkAutoTagMsg::ExpandAll(expanded) => state.groups.iter_mut().for_each(|group| group.expanded = expanded),
            BulkAutoTagMsg::Apply => return self.start_bulk_apply(),
            BulkAutoTagMsg::ApplyCompleted(generation, mut summary) => {
                // The files were written whether or not anyone is still watching.
                self.metadata_cache.merge(std::mem::take(&mut summary.refreshed));
                if !state.finish_job(generation) {
                    return Task::none();
                }
                if state.is_open() {
                    state.finish_apply(summary);
                }
                return self.refresh_search_if_active();
            }
        }
        Task::none()
    }

    fn start_bulk_scan(&mut self) -> Task<Message> {
        let state = &mut self.bulk_auto_tag;
        let root = match state.root.clone() {
            None => Err("Choose a folder before scanning."),
            Some(root) if !root.is_dir() => Err("That folder no longer exists."),
            Some(root) if !self.allowed_directories.contains_path(&root) => Err(FOLDER_OUTSIDE_ALLOWED),
            Some(root) => Ok(root),
        };
        let Ok(root) = root.map_err(|err| state.set_error(err)) else {
            return Task::none();
        };

        let job = state.start_job();
        state.start_running(root.clone());
        let metadata = self.metadata_cache.snapshot();
        let generation = job.generation;
        background(
            move || bulk_auto_tag::scan_and_classify(root, metadata, job.progress, job.cancel),
            || Err(ScanError::Failed("Scan failed unexpectedly.".into())),
            move |result| BulkAutoTagMsg::ScanCompleted(generation, result).into(),
        )
    }

    fn start_bulk_apply(&mut self) -> Task<Message> {
        let state = &mut self.bulk_auto_tag;
        let items = bulk_auto_tag::collect_accepted(&state.files);
        if items.is_empty() {
            return Task::none();
        }
        let job = state.start_job();
        job.progress.begin(BulkPhase::Applying, items.len());
        state.start_apply();
        let generation = job.generation;
        background(
            move || bulk_auto_tag::apply_items(&items, &job.progress, &job.cancel),
            || BulkApplySummary {
                failed: vec![(
                    PathBuf::new(),
                    "Apply stopped unexpectedly; some files may not have been tagged.".into(),
                )],
                ..BulkApplySummary::default()
            },
            move |summary| BulkAutoTagMsg::ApplyCompleted(generation, summary).into(),
        )
    }
}
