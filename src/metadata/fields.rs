use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TagField {
    Title,
    Artist,
    Album,
    Genre,
    Comment,
    AlbumArtist,
    Composer,
    Label,
    Bpm,
    Key,
    Instrument,
}

impl TagField {
    pub const ALL: [TagField; 11] = [
        TagField::Bpm,
        TagField::Key,
        TagField::Instrument,
        TagField::Title,
        TagField::Artist,
        TagField::Genre,
        TagField::Comment,
        TagField::Album,
        TagField::AlbumArtist,
        TagField::Composer,
        TagField::Label,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TagField::Title => "title",
            TagField::Artist => "artist",
            TagField::Album => "album",
            TagField::Genre => "genre",
            TagField::Comment => "comment",
            TagField::AlbumArtist => "albumartist",
            TagField::Composer => "composer",
            TagField::Label => "label",
            TagField::Bpm => "bpm",
            TagField::Key => "key",
            TagField::Instrument => "instrument",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TagField::Title => "Title",
            TagField::Artist => "Artist",
            TagField::Album => "Album",
            TagField::Genre => "Genre",
            TagField::Comment => "Comment",
            TagField::AlbumArtist => "Album artist",
            TagField::Composer => "Composer",
            TagField::Label => "Label",
            TagField::Bpm => "BPM",
            TagField::Key => "Key",
            TagField::Instrument => "Instrument",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        match key.trim().to_ascii_lowercase().as_str() {
            "title" | "track" | "name" => Some(TagField::Title),
            "artist" | "trackartist" => Some(TagField::Artist),
            "album" => Some(TagField::Album),
            "genre" => Some(TagField::Genre),
            "comment" => Some(TagField::Comment),
            "albumartist" | "album_artist" => Some(TagField::AlbumArtist),
            "composer" => Some(TagField::Composer),
            "label" => Some(TagField::Label),
            "bpm" | "tempo" => Some(TagField::Bpm),
            "key" | "initialkey" | "initial_key" => Some(TagField::Key),
            "instrument" | "inst" => Some(TagField::Instrument),
            _ => None,
        }
    }

    fn matches_query(self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        self.match_score(needle) >= 700
    }

    fn match_score(self, needle: &str) -> i32 {
        if needle.is_empty() {
            return 0;
        }
        let key = self.as_str();
        let label = self.label().to_ascii_lowercase();
        if key == needle {
            1_000
        } else if label == needle {
            900
        } else if key.starts_with(needle) {
            800
        } else if label.starts_with(needle) {
            700
        } else if key.contains(needle) {
            400
        } else if label.contains(needle) {
            300
        } else {
            0
        }
    }
}

pub fn tag_field_match_score(field: TagField, input: &str) -> i32 {
    field.match_score(&input.trim().to_ascii_lowercase())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagFilter {
    pub field: TagField,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagFields {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub comment: String,
    pub album_artist: String,
    pub composer: String,
    pub label: String,
    pub bpm: String,
    pub key: String,
    pub instrument: String,
    #[serde(default)]
    pub explicit_instrument: String,
    /// Artist under the container's canonical key, with no path hint applied.
    #[serde(default)]
    pub file_artist: String,
    /// Comment under the container's canonical key. Kept separate from
    /// `comment` so auto-tag status is judged from the same values whether it
    /// is computed from a cached entry or read fresh from disk.
    #[serde(default)]
    pub file_comment: String,
}

impl TagFields {
    pub fn field_value(&self, field: TagField) -> &str {
        match field {
            TagField::Instrument if !self.explicit_instrument.is_empty() => {
                &self.explicit_instrument
            }
            TagField::Title => &self.title,
            TagField::Artist => &self.artist,
            TagField::Album => &self.album,
            TagField::Genre => &self.genre,
            TagField::Comment => &self.comment,
            TagField::AlbumArtist => &self.album_artist,
            TagField::Composer => &self.composer,
            TagField::Label => &self.label,
            TagField::Bpm => &self.bpm,
            TagField::Key => &self.key,
            TagField::Instrument => &self.instrument,
        }
    }
}

/// Compact tags for the waveform toolbar (instrument, bpm, key, genre).
pub fn control_bar_tags(fields: &TagFields) -> Vec<(TagField, String)> {
    let mut tags = Vec::new();
    let instrument = fields.field_value(TagField::Instrument);
    if !instrument.is_empty() {
        tags.push((TagField::Instrument, instrument.to_string()));
    }
    if !fields.bpm.is_empty() {
        tags.push((TagField::Bpm, fields.bpm.clone()));
    }
    if !fields.key.is_empty() {
        tags.push((TagField::Key, fields.key.clone()));
    }
    if !fields.genre.is_empty() {
        tags.push((TagField::Genre, fields.genre.clone()));
    }
    tags
}

/// Compact tag line for tests and text-only surfaces.
#[cfg(test)]
pub fn format_control_bar_tags(fields: &TagFields) -> Option<String> {
    let tags = control_bar_tags(fields);
    if tags.is_empty() {
        None
    } else {
        Some(
            tags.iter()
                .map(|(field, value)| format!("{}: {value}", field.label()))
                .collect::<Vec<_>>()
                .join(" · "),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagParseError {
    MissingSeparator,
    UnknownField,
    EmptyValue,
    UnclosedQuote,
}

pub fn tag_parse_message(err: TagParseError) -> &'static str {
    match err {
        TagParseError::MissingSeparator => {
            "Use field:value (example: title:My Song)."
        }
        TagParseError::UnknownField => {
            "Unknown tag field. Try bpm, key, instrument, title, genre…"
        }
        TagParseError::EmptyValue => "Tag value cannot be empty.",
        TagParseError::UnclosedQuote => "Closing quote missing in tag value.",
    }
}

fn parse_tag_value(raw: &str) -> Result<String, TagParseError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(TagParseError::EmptyValue);
    }

    if raw.starts_with('"') {
        let rest = &raw[1..];
        let end = rest.find('"').ok_or(TagParseError::UnclosedQuote)?;
        let value = rest[..end].trim();
        if value.is_empty() {
            return Err(TagParseError::EmptyValue);
        }
        return Ok(value.to_owned());
    }

    Ok(raw.to_owned())
}

pub fn parse_tag_filter(input: &str) -> Result<TagFilter, TagParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(TagParseError::EmptyValue);
    }
    let Some((key, value)) = input.split_once(':') else {
        return Err(TagParseError::MissingSeparator);
    };
    let Some(field) = TagField::parse(key) else {
        return Err(TagParseError::UnknownField);
    };
    let value = parse_tag_value(value)?;
    Ok(TagFilter { field, value })
}

pub fn tag_field_suggestions(input: &str) -> Vec<TagField> {
    if input.contains(':') {
        return Vec::new();
    }
    let needle = input.trim().to_ascii_lowercase();
    TagField::ALL
        .into_iter()
        .filter(|field| field.matches_query(&needle))
        .collect()
}

pub fn tag_field_best_match(input: &str) -> Option<TagField> {
    if input.contains(':') {
        return None;
    }
    let needle = input.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    TagField::ALL
        .iter()
        .filter(|field| field.matches_query(&needle))
        .max_by_key(|field| field.match_score(&needle))
        .copied()
}

/// User-editable tag values for the manual tag editor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManualTagEdits {
    pub instrument: String,
    pub artist: String,
    pub title: String,
    pub bpm: String,
    pub key: String,
    pub genre: String,
    pub comment: String,
}

impl ManualTagEdits {
    pub fn from_tag_fields(fields: &TagFields) -> Self {
        Self {
            instrument: fields.field_value(TagField::Instrument).to_string(),
            artist: fields.artist.clone(),
            title: fields.title.clone(),
            bpm: fields.bpm.clone(),
            key: fields.key.clone(),
            genre: fields.genre.clone(),
            comment: fields.comment.clone(),
        }
    }
}
