use super::common::{
    modal_button_style, modal_error_style, modal_panel_style, modal_shell, truncate_path, Message,
};
use crate::path_util::{
    cache_file, cache_key, canonical_path, config_file, read_bincode_or_default, write_bincode,
};
use iced::widget::{button, container, row, scrollable, text, Column, Space};
use iced::{Alignment, Border, Element, Length, Theme};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_OUTSIDE_ALLOWED: &str = "File must be inside allowed directories.";
pub const FOLDER_OUTSIDE_ALLOWED: &str = "Folder must be inside allowed directories.";
pub const UNSUPPORTED_AUDIO: &str = "Choose a supported audio file.";
pub const SELECT_AUDIO_FIRST: &str = "Select an audio file first.";
pub const NO_AUDIO_SELECTED: &str = "No audio file selected";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AllowedDirectories {
    roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddDirectoryResult {
    Added,
    Duplicate,
    Unresolved,
}

impl AllowedDirectories {
    pub fn load() -> Self {
        let Some(path) = settings_file_path() else {
            return Self::default();
        };
        read_bincode_or_default(&path, "allowed directories")
    }

    pub fn persist(&self) {
        let Some(path) = settings_file_path() else {
            return;
        };
        write_bincode(&path, self, "allowed directories");
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn add(&mut self, path: PathBuf) -> (AddDirectoryResult, Option<PathBuf>) {
        let Some(resolved) = try_resolve_path(&path) else {
            return (AddDirectoryResult::Unresolved, None);
        };
        if self.roots.iter().any(|root| root == &resolved) {
            return (AddDirectoryResult::Duplicate, None);
        }
        self.roots.push(resolved.clone());
        self.roots.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
        (AddDirectoryResult::Added, Some(resolved))
    }

    pub fn remove(&mut self, path: &Path) {
        self.roots.retain(|root| root.as_path() != path);
    }

    pub fn contains_path(&self, path: &Path) -> bool {
        let path = try_resolve_path(path).unwrap_or_else(|| path.to_path_buf());
        self.roots
            .iter()
            .any(|root| crate::path_util::is_under(&path, root))
    }

    pub fn startup_directory(&self) -> Option<PathBuf> {
        self.roots.first().cloned()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FavoritesStore {
    paths: Vec<PathBuf>,
}

impl FavoritesStore {
    pub fn load() -> Self {
        let Some(path) = favorites_file_path() else {
            return Self::default();
        };
        read_bincode_or_default(&path, "favorites")
    }

    pub fn persist(&self) {
        let Some(path) = favorites_file_path() else {
            return;
        };
        write_bincode(&path, self, "favorites");
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    fn stored_key(path: &Path) -> PathBuf {
        canonical_path(path)
            .map(cache_key)
            .unwrap_or_else(|_| cache_key(path.to_path_buf()))
    }

    pub fn contains(&self, path: &Path) -> bool {
        let key = Self::stored_key(path);
        self.paths.iter().any(|stored| *stored == key)
    }

    pub fn toggle(&mut self, path: PathBuf) -> bool {
        let key = Self::stored_key(&path);
        if let Some(index) = self.paths.iter().position(|stored| *stored == key) {
            self.paths.remove(index);
            false
        } else {
            self.paths.push(key);
            self.paths.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
            true
        }
    }

    pub fn retain<F>(&mut self, keep: F)
    where
        F: Fn(&Path) -> bool,
    {
        self.paths.retain(|path| keep(path));
    }
}

fn favorites_file_path() -> Option<PathBuf> {
    config_file("favorites.bin")
}

fn try_resolve_path(path: &Path) -> Option<PathBuf> {
    canonical_path(path).ok()
}

fn settings_file_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = test_settings_path() {
        return Some(path);
    }
    let path = config_file("allowed_directories.bin")?;
    migrate_settings_from_cache(&path);
    Some(path)
}

fn migrate_settings_from_cache(config_path: &Path) {
    if config_path.exists() {
        return;
    }
    let Some(cache_path) = cache_file("allowed_directories.bin") else {
        return;
    };
    if cache_path.exists() && std::fs::copy(&cache_path, config_path).is_ok() {
        let _ = std::fs::remove_file(cache_path);
    }
}

fn directory_row(path: PathBuf) -> Element<'static, Message> {
    let label = truncate_path(&path, 52);
    container(
        row![
            text(label)
                .size(12)
                .width(Length::Fill),
            button(text("Remove").size(11))
                .padding([4, 8])
                .on_press(Message::RemoveAllowedDirectory(path))
                .style(|theme, status| modal_button_style(theme, status, false)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .padding([6, 8])
    .width(Length::Fill)
    .style(|theme: &Theme| {
        let palette = theme.extended_palette();
        container::Style {
            background: Some(palette.background.weak.color.scale_alpha(0.42).into()),
            border: Border {
                radius: 0.0.into(),
                width: 1.0,
                color: palette.background.strong.color.scale_alpha(0.24),
            },
            ..Default::default()
        }
    })
    .into()
}

pub fn settings_view(
    allowed: &[PathBuf],
    first_run: bool,
    error: Option<String>,
) -> Element<'static, Message> {
    let title = if first_run {
        "Choose search directories"
    } else {
        "Settings"
    };

    let intro = if first_run {
        "Tundra only searches and caches audio inside directories you allow. Add one or more folders to get started."
    } else {
        "Directories Tundra may search and cache. Changes apply immediately and prune data outside these folders."
    };

    let mut body = Column::new()
        .spacing(12)
        .push(text(title).size(18))
        .push(
            text(intro)
                .size(13)
                .width(Length::Fill),
        );

    if allowed.is_empty() {
        body = body.push(
            container(
                text("No directories configured yet.")
                    .size(12)
                    .width(Length::Fill),
            )
            .padding([8, 10])
            .width(Length::Fill)
            .style(modal_panel_style),
        );
    } else {
        let rows: Vec<Element<Message>> = allowed.iter().cloned().map(directory_row).collect();
        body = body.push(
            scrollable(Column::with_children(rows).spacing(6))
                .width(Length::Fill)
                .height(Length::Fixed(220.0)),
        );
    }

    if let Some(error) = error {
        body = body.push(
            text(error)
                .size(12)
                .style(modal_error_style),
        );
    }

    let close_label = if first_run { "Start" } else { "Done" };
    body = body.push(
        row![
            button(text("Add directory…").size(12))
                .padding([6, 12])
                .on_press(Message::PickAllowedDirectory)
                .style(|theme, status| modal_button_style(theme, status, false)),
            Space::new().width(Length::Fill),
            button(text(close_label).size(12))
                .padding([6, 14])
                .on_press(Message::CloseSettings)
                .style(|theme, status| modal_button_style(theme, status, true)),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    );

    modal_shell(body.padding(18), 520.0).into()
}

#[cfg(test)]
use std::cell::RefCell;

#[cfg(test)]
thread_local! {
    static TEST_SETTINGS_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn test_settings_path() -> Option<PathBuf> {
    TEST_SETTINGS_PATH.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
pub(crate) fn with_test_settings_path<F, R>(path: PathBuf, f: F) -> R
where
    F: FnOnce() -> R,
{
    TEST_SETTINGS_PATH.with(|slot| {
        *slot.borrow_mut() = Some(path);
        let result = f();
        *slot.borrow_mut() = None;
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_util::{reclaim_write_sidecars, sidecar, REPLACE_OLD_SUFFIX};
    use crate::test_fixtures::ScratchDir;

    #[test]
    fn favorites_toggle_and_contains() {
        let dir = ScratchDir::new("favorites-store");
        let sample = dir.path().join("kick.wav");
        std::fs::write(&sample, b"wav").expect("sample");

        let mut favorites = FavoritesStore::default();
        assert!(favorites.toggle(sample.clone()));
        assert!(favorites.contains(&sample));
        assert!(!favorites.toggle(sample));
        assert!(favorites.paths().is_empty());
    }

    #[test]
    fn allowed_directories_persist_recovers_from_crash_aside() {
        let dir = ScratchDir::new("settings-persist");
        let path = dir.path().join("allowed_directories.bin");
        let samples = dir.path().join("samples");
        std::fs::create_dir_all(&samples).expect("samples dir");
        let mut allowed = AllowedDirectories::default();
        let (added, _) = allowed.add(samples);
        assert_eq!(added, AddDirectoryResult::Added);

        with_test_settings_path(path.clone(), || allowed.persist());
        let bytes = std::fs::read(&path).expect("persisted");

        std::fs::write(sidecar(&path, REPLACE_OLD_SUFFIX), &bytes).expect("crash aside");
        std::fs::remove_file(&path).expect("crash delete");

        reclaim_write_sidecars(dir.path());
        assert_eq!(std::fs::read(&path).expect("restored"), bytes);

        with_test_settings_path(path, || allowed.persist());
        assert_eq!(dir.sidecar_count(), 0);
    }
}
