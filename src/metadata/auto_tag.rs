use std::path::Path;

use crate::types::is_audio;

use super::fields::TagFields;
use super::hints::artist_hint_from_path;
use super::read::{
    durable_instrument, file_tundra_tag_version, instrument_from_marked_comment,
    parse_tundra_comment_version, read_native_tags, tundra_tagged_file, TUNDRA_TAG_VERSION,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoTagFieldStatus {
    pub needs_instrument: bool,
    pub needs_artist: bool,
    pub needs_comment: bool,
    /// Instrument is present but was written by Tundra, so classify/apply may
    /// replace it. User-owned instrument tags stay read-only.
    pub can_retag_instrument: bool,
}

impl AutoTagFieldStatus {
    pub fn needs_any(self) -> bool {
        self.needs_instrument || self.needs_artist || self.needs_comment
    }

    pub fn allows_instrument_work(self) -> bool {
        self.needs_instrument || self.can_retag_instrument
    }

    pub fn is_complete(self) -> bool {
        !self.needs_any() && !self.can_retag_instrument
    }

    pub fn from_parts(
        path: &Path,
        explicit_instrument: &str,
        native_instrument: &str,
        file_artist: &str,
        comment: &str,
        native_writable: bool,
    ) -> Self {
        let tundra_tagged = tundra_tagged_file(path, comment, native_instrument);
        let has_instrument = !explicit_instrument.trim().is_empty();
        let current_tag =
            file_tundra_tag_version(path, comment, native_instrument) == Some(TUNDRA_TAG_VERSION);
        Self {
            needs_instrument: !has_instrument,
            can_retag_instrument: tundra_tagged && has_instrument && !current_tag,
            needs_artist: native_writable
                && file_artist.trim().is_empty()
                && artist_hint_from_path(path).is_some(),
            needs_comment: native_writable && needs_auto_tag_comment(comment),
        }
    }
}

pub fn auto_tag_already_complete_message() -> &'static str {
    "This file already has the tags Tundra would add."
}

pub fn auto_tag_field_status(path: &Path) -> Option<AutoTagFieldStatus> {
    if !is_audio(path) {
        return None;
    }
    let native = read_native_tags(path);
    let native_writable = native.is_some();
    let native = native.unwrap_or_default();
    Some(AutoTagFieldStatus::from_parts(
        path,
        durable_instrument(path, &native, None).unwrap_or_default().as_str(),
        native.instrument.as_deref().unwrap_or_default(),
        native.artist.as_deref().unwrap_or_default(),
        native.comment.as_deref().unwrap_or_default(),
        native_writable,
    ))
}

pub fn auto_tag_field_status_from_fields(path: &Path, fields: &TagFields) -> AutoTagFieldStatus {
    let native = read_native_tags(path);
    AutoTagFieldStatus::from_parts(
        path,
        &fields.explicit_instrument,
        native
            .as_ref()
            .and_then(|tags| tags.instrument.as_deref())
            .unwrap_or(""),
        &fields.file_artist,
        &fields.file_comment,
        native.is_some(),
    )
}

fn needs_auto_tag_comment(comment: &str) -> bool {
    let comment = comment.trim();
    if comment.is_empty() {
        return true;
    }
    match parse_tundra_comment_version(comment) {
        Some(version) => version != TUNDRA_TAG_VERSION,
        None => instrument_from_marked_comment(comment).is_some(),
    }
}
