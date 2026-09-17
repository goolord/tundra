//! The Bulk Auto Tag modal's handlers. The scan and the writes run on
//! background threads; see `crate::bulk_auto_tag`.

use super::{App, Modal, pick_folder, run_blocking};
use crate::bulk_auto_tag::{self, BulkApplySummary, BulkPhase, ScanError};
use crate::ui::bulk_auto_tag::BulkAutoTagPhase;
use crate::ui::message::{BulkAutoTagMsg, Message};
use crate::ui::settings::FOLDER_OUTSIDE_ALLOWED;
use iced::Task;
use std::path::PathBuf;

impl App {
    pub(super) fn update_bulk_auto_tag(&mut self, message: BulkAutoTagMsg) -> Task<Message> {
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
                let start_dir = state
                    .root
                    .clone()
                    .unwrap_or_else(|| self.file_selector.current_dir.clone());
                return Task::perform(pick_folder(start_dir), |picked| {
                    BulkAutoTagMsg::DirectoryPicked(picked).into()
                });
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
            BulkAutoTagMsg::ProgressTick => state.update_progress(),
            BulkAutoTagMsg::ScanCompleted { generation, result } => {
                if state.finish_job(generation) {
                    match result {
                        Ok(summary) => state.finish_scan(summary),
                        Err(ScanError::Cancelled) => {}
                        Err(ScanError::Failed(message)) => state.set_error(message),
                    }
                }
            }
            BulkAutoTagMsg::SetFileAccepted { key, accepted } => state.set_file_accepted(key, accepted),
            BulkAutoTagMsg::SelectFile { key, shift, control } => state.select_file(key, shift, control),
            BulkAutoTagMsg::SelectDirectory {
                dir_idx,
                shift,
                control,
            } => state.select_directory(dir_idx, shift, control),
            BulkAutoTagMsg::SelectAll => state.select_all_files(),
            BulkAutoTagMsg::ClearSelection => state.selection.clear(),
            BulkAutoTagMsg::CheckSelected => state.set_selected_accepted(true),
            BulkAutoTagMsg::UncheckSelected => state.set_selected_accepted(false),
            BulkAutoTagMsg::AcceptAll => state.set_all_accepted(true),
            BulkAutoTagMsg::RejectAll => state.set_all_accepted(false),
            BulkAutoTagMsg::ToggleDirectoryExpanded(dir_idx) => {
                if let Some(group) = state.groups.get_mut(dir_idx) {
                    group.expanded = !group.expanded;
                }
            }
            BulkAutoTagMsg::ExpandAllDirectories => state.set_all_expanded(true),
            BulkAutoTagMsg::CollapseAllDirectories => state.set_all_expanded(false),
            BulkAutoTagMsg::Apply => return self.start_bulk_apply(),
            BulkAutoTagMsg::ApplyCompleted {
                generation,
                mut summary,
            } => {
                // The files were written whether or not anyone is still watching.
                self.metadata_cache.merge(std::mem::take(&mut summary.refreshed));
                if !self.bulk_auto_tag.finish_job(generation) {
                    return Task::none();
                }
                if self.bulk_auto_tag.is_open() {
                    self.bulk_auto_tag.finish_apply(summary);
                }
                return self.refresh_search_if_active();
            }
        }
        Task::none()
    }

    fn start_bulk_scan(&mut self) -> Task<Message> {
        let state = &mut self.bulk_auto_tag;
        let Some(root) = state.root.clone() else {
            state.set_error("Choose a folder before scanning.");
            return Task::none();
        };
        if !root.is_dir() {
            state.set_error("That folder no longer exists.");
            return Task::none();
        }
        if !self.allowed_directories.contains_path(&root) {
            state.set_error(FOLDER_OUTSIDE_ALLOWED);
            return Task::none();
        }

        let job = state.start_job();
        state.start_running(root.clone());
        let metadata = self.metadata_cache.snapshot();
        let generation = job.generation;
        Task::perform(
            run_blocking(move || bulk_auto_tag::scan_and_classify(root, metadata, job.progress, job.cancel)),
            move |result| {
                let result = result.unwrap_or_else(|()| Err(ScanError::Failed("Scan failed unexpectedly.".into())));
                BulkAutoTagMsg::ScanCompleted { generation, result }.into()
            },
        )
    }

    fn start_bulk_apply(&mut self) -> Task<Message> {
        let state = &mut self.bulk_auto_tag;
        let items = bulk_auto_tag::collect_accepted(&state.groups);
        if items.is_empty() {
            return Task::none();
        }
        let job = state.start_job();
        job.progress.begin(BulkPhase::Applying, items.len());
        state.start_apply();
        let generation = job.generation;
        Task::perform(
            run_blocking(move || bulk_auto_tag::apply_items(&items, Some(&job.progress), &job.cancel)),
            move |summary| {
                let summary = summary.unwrap_or_else(|()| BulkApplySummary {
                    failed: vec![(
                        PathBuf::new(),
                        "Apply stopped unexpectedly; some files may not have been tagged.".into(),
                    )],
                    ..BulkApplySummary::default()
                });
                BulkAutoTagMsg::ApplyCompleted { generation, summary }.into()
            },
        )
    }
}
