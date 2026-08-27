use super::common::{modal_button_style, truncate_path, Message};
use crate::path_util::{cache_key, canonical_path};
use iced::widget::{button, container, row, scrollable, text, Column, Space};
use iced::{Alignment, Border, Color, Element, Length, Shadow, Theme};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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
        match std::fs::read(&path) {
            Ok(bytes) => match bincode::deserialize(&bytes) {
                Ok(settings) => settings,
                Err(err) => {
                    eprintln!(
                        "Failed to deserialize allowed directories ({}): {err}",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!("Failed to read allowed directories ({}): {err}", path.display());
                Self::default()
            }
        }
    }

    pub fn persist(&self) {
        let Some(path) = settings_file_path() else {
            return;
        };
        let Ok(bytes) = bincode::serialize(self) else {
            eprintln!("Failed to serialize allowed directories");
            return;
        };
        if let Err(err) = crate::path_util::write_atomic(&path, &bytes) {
            eprintln!("Failed to write allowed directories: {err}");
        }
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
        self.roots.iter().any(|root| path.starts_with(root))
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
        match std::fs::read(&path) {
            Ok(bytes) => match bincode::deserialize(&bytes) {
                Ok(store) => store,
                Err(err) => {
                    eprintln!(
                        "Failed to deserialize favorites ({}): {err}",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => {
                eprintln!("Failed to read favorites ({}): {err}", path.display());
                Self::default()
            }
        }
    }

    pub fn persist(&self) {
        let Some(path) = favorites_file_path() else {
            return;
        };
        let Ok(bytes) = bincode::serialize(self) else {
            eprintln!("Failed to serialize favorites");
            return;
        };
        if let Err(err) = crate::path_util::write_atomic(&path, &bytes) {
            eprintln!("Failed to write favorites: {err}");
        }
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

    /// Returns `true` when the path was added, `false` when removed.
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
    let mut config_dir = dirs::config_dir()?;
    config_dir.push("tundra");
    let _ = std::fs::create_dir_all(&config_dir);
    config_dir.push("favorites.bin");
    Some(config_dir)
}

fn try_resolve_path(path: &Path) -> Option<PathBuf> {
    canonical_path(path).ok()
}

fn settings_file_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(path) = test_settings_path() {
        return Some(path);
    }
    let mut config_dir = dirs::config_dir()?;
    config_dir.push("tundra");
    let _ = std::fs::create_dir_all(&config_dir);
    config_dir.push("allowed_directories.bin");
    migrate_settings_from_cache(&config_dir);
    Some(config_dir)
}

fn migrate_settings_from_cache(config_path: &Path) {
    if config_path.exists() {
        return;
    }
    let Some(mut cache_path) = dirs::cache_dir() else {
        return;
    };
    cache_path.push("tundra");
    cache_path.push("allowed_directories.bin");
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
            .style(|theme: &Theme| {
                let palette = theme.extended_palette();
                container::Style {
                    background: Some(palette.background.weak.color.scale_alpha(0.35).into()),
                    border: Border {
                        radius: 0.0.into(),
                        width: 1.0,
                        color: palette.background.strong.color.scale_alpha(0.28),
                    },
                    ..Default::default()
                }
            }),
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
                .style(|_theme: &Theme| iced::widget::text::Style {
                    color: Some(Color::from_rgb(0.95, 0.62, 0.62)),
                }),
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

    container(body.padding(18))
        .width(Length::Fixed(520.0))
        .style(|theme: &Theme| {
            let palette = theme.extended_palette();
            container::Style {
                background: Some(palette.background.base.color.into()),
                border: Border {
                    width: 1.0,
                    color: palette.background.strong.color,
                    radius: 0.0.into(),
                },
                shadow: Shadow {
                    color: palette.background.base.text.scale_alpha(0.25),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 16.0,
                },
                ..Default::default()
            }
        })
        .into()
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
