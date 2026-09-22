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

    /// Search key and display label.
    fn names(self) -> (&'static str, &'static str) {
        match self {
            TagField::Title => ("title", "Title"),
            TagField::Artist => ("artist", "Artist"),
            TagField::Album => ("album", "Album"),
            TagField::Genre => ("genre", "Genre"),
            TagField::Comment => ("comment", "Comment"),
            TagField::AlbumArtist => ("albumartist", "Album artist"),
            TagField::Composer => ("composer", "Composer"),
            TagField::Label => ("label", "Label"),
            TagField::Bpm => ("bpm", "BPM"),
            TagField::Key => ("key", "Key"),
            TagField::Instrument => ("instrument", "Instrument"),
        }
    }

    pub fn as_str(self) -> &'static str {
        self.names().0
    }

    pub fn label(self) -> &'static str {
        self.names().1
    }

    fn parse(key: &str) -> Option<Self> {
        let key = key.trim().to_ascii_lowercase();
        let key = match key.as_str() {
            "track" | "name" => "title",
            "trackartist" => "artist",
            "album_artist" => "albumartist",
            "tempo" => "bpm",
            "initialkey" | "initial_key" => "key",
            "inst" => "instrument",
            other => other,
        };
        Self::ALL.into_iter().find(|field| field.as_str() == key)
    }

    /// How well `needle` (lowercase, non-empty) names this field; 700 and up is a match.
    fn match_score(self, needle: &str) -> i32 {
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
        } else {
            0
        }
    }
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
            TagField::Instrument if !self.explicit_instrument.is_empty() => &self.explicit_instrument,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagParseError {
    MissingSeparator,
    UnknownField,
    EmptyValue,
    UnclosedQuote,
}

impl std::fmt::Display for TagParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            TagParseError::MissingSeparator => "Use field:value (example: title:My Song).",
            TagParseError::UnknownField => "Unknown tag field. Try bpm, key, instrument, title, genre…",
            TagParseError::EmptyValue => "Tag value cannot be empty.",
            TagParseError::UnclosedQuote => "Closing quote missing in tag value.",
        })
    }
}

pub fn parse_tag_filter(input: &str) -> Result<TagFilter, TagParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(TagParseError::EmptyValue);
    }
    let (key, value) = input.split_once(':').ok_or(TagParseError::MissingSeparator)?;
    let field = TagField::parse(key).ok_or(TagParseError::UnknownField)?;
    let value = value.trim();
    let value = match value.strip_prefix('"') {
        Some(rest) => rest[..rest.find('"').ok_or(TagParseError::UnclosedQuote)?].trim(),
        None => value,
    };
    if value.is_empty() {
        return Err(TagParseError::EmptyValue);
    }
    Ok(TagFilter {
        field,
        value: value.to_owned(),
    })
}

/// Fields whose key or label starts with what was typed (all of them when nothing was).
pub fn tag_field_suggestions(input: &str) -> Vec<TagField> {
    if input.contains(':') {
        return Vec::new();
    }
    let needle = input.trim().to_ascii_lowercase();
    TagField::ALL
        .into_iter()
        .filter(|field| needle.is_empty() || field.match_score(&needle) >= 700)
        .collect()
}

/// The best suggestion; ties go to the alphabetically first label.
pub fn tag_field_best_match(input: &str) -> Option<TagField> {
    let needle = input.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    tag_field_suggestions(input)
        .into_iter()
        .max_by_key(|field| (field.match_score(&needle), std::cmp::Reverse(field.label())))
}

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
    pub const EDITOR_FIELDS: [TagField; 7] = [
        TagField::Instrument,
        TagField::Artist,
        TagField::Title,
        TagField::Bpm,
        TagField::Key,
        TagField::Genre,
        TagField::Comment,
    ];

    fn from_fn(value: impl Fn(TagField) -> String) -> Self {
        let mut edits = Self::default();
        for field in Self::EDITOR_FIELDS {
            edits.set_field(field, value(field));
        }
        edits
    }

    pub fn from_tag_fields(fields: &TagFields) -> Self {
        Self::from_fn(|field| fields.field_value(field).to_string())
    }

    pub fn field_value(&self, field: TagField) -> &str {
        match field {
            TagField::Instrument => &self.instrument,
            TagField::Artist => &self.artist,
            TagField::Title => &self.title,
            TagField::Bpm => &self.bpm,
            TagField::Key => &self.key,
            TagField::Genre => &self.genre,
            TagField::Comment => &self.comment,
            TagField::Album | TagField::AlbumArtist | TagField::Composer | TagField::Label => "",
        }
    }

    pub fn set_field(&mut self, field: TagField, value: String) {
        let slot = match field {
            TagField::Instrument => &mut self.instrument,
            TagField::Artist => &mut self.artist,
            TagField::Title => &mut self.title,
            TagField::Bpm => &mut self.bpm,
            TagField::Key => &mut self.key,
            TagField::Genre => &mut self.genre,
            TagField::Comment => &mut self.comment,
            TagField::Album | TagField::AlbumArtist | TagField::Composer | TagField::Label => return,
        };
        *slot = value;
    }

    pub fn is_empty(&self) -> bool {
        Self::EDITOR_FIELDS
            .iter()
            .all(|field| self.field_value(*field).trim().is_empty())
    }

    /// Every field with surrounding whitespace removed.
    pub fn trimmed(&self) -> Self {
        Self::from_fn(|field| self.field_value(field).trim().to_string())
    }
}
