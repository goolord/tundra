//! Audio file tags.
//!
//! - `read`: tags from each container, and Tundra's marker comments.
//! - `write`: the only path that changes an audio file (copy, edit, verify, swap).
//!   `riff` edits WAV/AIFF chunks directly; `verify` checks the staged copy.
//! - `cache`: index entries, and reading tags through the index.
//! - `search`: ranking paths by filename and tag filters.
//! - `hints`: instruments and artists named by a file's path.
//! - `auto_tag`: which fields auto-tag may fill or replace.
//! - `fields`: the tag fields themselves, and tag filter parsing.

mod auto_tag;
mod cache;
mod fields;
mod hints;
mod read;
mod riff;
mod search;
mod verify;
mod write;

pub use auto_tag::{AutoTagFieldStatus, auto_tag_field_status, auto_tag_field_status_from_fields};
pub use cache::{CachedMetadata, MetadataLookup, index_paths, refresh_cached_metadata};
pub use fields::{
    ManualTagEdits, TagField, TagFields, TagFilter, TagParseError, parse_tag_filter, tag_field_best_match,
    tag_field_suggestions,
};
pub use hints::{instrument_hint_from_path, instruments_related};
pub use read::{AUDIO_EXTENSIONS, instrument_tag, is_audio};
pub use search::{FILE_SEARCH_MIN_QUERY_LEN, SearchQuery, SearchResult, file_search_active, search, tag_only_search};
pub use write::{SavedTo, write_auto_tags, write_manual_tags};

#[cfg(test)]
pub(crate) use read::{TUNDRA_TAG_VERSION, read_tag_fields};
#[cfg(test)]
pub(crate) use riff::parse_riff_wave_chunks;
#[cfg(test)]
pub(crate) use write::stage_and_replace;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
