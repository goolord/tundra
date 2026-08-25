use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use lofty::file::TaggedFileExt;
use lofty::read_from_path;
use lofty::tag::{Accessor, ItemKey};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::types::is_audio;

fn push_part(parts: &mut Vec<String>, value: Option<impl AsRef<str>>) {
    if let Some(text) = value
        .as_ref()
        .map(AsRef::as_ref)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        parts.push(text.to_owned());
    }
}

/// Concatenated tag text used for fuzzy search (title, artist, album, etc.).
pub fn read_search_text(path: &Path) -> String {
    if !is_audio(path) {
        return String::new();
    }

    let Ok(tagged_file) = read_from_path(path) else {
        return String::new();
    };

    let Some(tag) = tagged_file.primary_tag().or_else(|| tagged_file.first_tag()) else {
        return String::new();
    };

    let mut parts = Vec::new();
    push_part(&mut parts, tag.title());
    push_part(&mut parts, tag.artist());
    push_part(&mut parts, tag.album());
    push_part(&mut parts, tag.genre());
    push_part(&mut parts, tag.comment());

    for key in [
        ItemKey::AlbumArtist,
        ItemKey::Composer,
        ItemKey::Label,
        ItemKey::TrackTitle,
        ItemKey::TrackArtist,
    ] {
        push_part(&mut parts, tag.get_string(&key));
    }

    for item in tag.items() {
        push_part(&mut parts, item.value().text());
    }

    parts.join(" ")
}

pub fn path_matches(matcher: &SkimMatcherV2, path: &Path, query: &str) -> bool {
    matcher
        .fuzzy_match(path.to_string_lossy().as_ref(), query)
        .is_some()
}

pub fn metadata_matches(
    matcher: &SkimMatcherV2,
    path: &Path,
    query: &str,
    cache: &mut HashMap<PathBuf, String>,
) -> bool {
    if !is_audio(path) {
        return false;
    }
    let text = cache
        .entry(path.to_path_buf())
        .or_insert_with(|| read_search_text(path));
    !text.is_empty() && matcher.fuzzy_match(text.as_str(), query).is_some()
}

pub fn file_matches_search(
    matcher: &SkimMatcherV2,
    path: &Path,
    query: &str,
    cache: &mut HashMap<PathBuf, String>,
) -> bool {
    path_matches(matcher, path, query) || metadata_matches(matcher, path, query, cache)
}
