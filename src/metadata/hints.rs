use std::collections::HashSet;
use std::path::Path;

const INSTRUMENT_ALIAS_GROUPS: &[&[&str]] = &[
    &[
        "kick", "bd", "bassdrum", "bass drum", "kick drum", "kickdrum", "808 kick", "808kick",
        "kik",
    ],
    &["snare", "sd", "sn", "snr"],
    &["rim", "rimshot", "rim shot", "side stick", "sidestick"],
    &[
        "hihat", "hi-hat", "hi hat", "hh", "hat", "hats", "open hat", "closed hat", "openhat",
        "closedhat", "op hat", "cl hat",
    ],
    &["clap", "handclap", "hand clap"],
    &["tom", "toms", "floor tom", "floortom", "rack tom", "racktom"],
    &[
        "perc", "percs", "percussion", "conga", "bongo", "bongos", "shaker", "tamb",
        "tambourine", "cowbell", "triangle", "woodblock", "cabasa",
    ],
    &["crash", "cymbal", "cymbals", "ride", "splash", "china"],
    &["bass", "sub", "subbass", "sub bass", "808 bass", "808bass", "reese", "wobble"],
    &["synth", "lead", "pad", "pluck", "stab", "arp", "arpeggio", "keys"],
    &[
        "fx", "sfx", "effect", "impact", "riser", "sweep", "noise", "atm", "atmosphere",
        "ambient", "transition", "downlifter", "uplifter",
    ],
    &["vocal", "vox", "voice", "acapella", "aca", "phrase", "adlib"],
    &["piano", "rhodes", "organ", "electric piano", "ep"],
    &["guitar", "gtr", "acoustic guitar", "acousticguitar"],
    &["loop", "loops", "top loop", "toploop", "drum loop", "drumloop"],
    &["oneshot", "one shot", "one-shot"],
    &["brass", "horn", "trumpet", "sax", "saxophone", "flute", "strings", "string"],
];

const INSTRUMENT_HINT_LABELS: &[&str] = &[
    "Kick",
    "Snare",
    "Rim",
    "Hi-Hat",
    "Clap",
    "Tom",
    "Percussion",
    "Cymbal",
    "Bass",
    "Synth",
    "FX",
    "Vocal",
    "Piano",
    "Guitar",
    "Loop",
    "One-shot",
    "Brass",
];

const _: () = assert!(INSTRUMENT_ALIAS_GROUPS.len() == INSTRUMENT_HINT_LABELS.len());

const MIN_INSTRUMENT_SUBSTRING_LEN: usize = 4;

fn instrument_alias_matches(needle: &str, alias_norm: &str) -> bool {
    if needle.is_empty() || alias_norm.is_empty() {
        return false;
    }
    if needle == alias_norm || is_simple_plural(needle, alias_norm) || is_simple_plural(alias_norm, needle)
    {
        return true;
    }
    // Short tokens only: "hat"/"hh". Prefix on longer names made `bass` hit
    // `bassdrum` (Kick) and `shot` hit `rimshot`.
    needle.len() < MIN_INSTRUMENT_SUBSTRING_LEN
        && alias_norm.len() < MIN_INSTRUMENT_SUBSTRING_LEN
        && (alias_norm.starts_with(needle) || needle.starts_with(alias_norm))
}

fn is_simple_plural(plural: &str, singular: &str) -> bool {
    plural.len() == singular.len() + 1 && plural.starts_with(singular) && plural.ends_with('s')
}

fn normalize_instrument_term(term: &str) -> String {
    term.to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn instrument_alias_groups_for(term: &str) -> Vec<&'static [&'static str]> {
    let needle = normalize_instrument_term(term);
    if needle.is_empty() {
        return Vec::new();
    }
    INSTRUMENT_ALIAS_GROUPS
        .iter()
        .copied()
        .filter(|group| {
            group.iter().any(|alias| {
                instrument_alias_matches(&needle, &normalize_instrument_term(alias))
            })
        })
        .collect()
}

pub(crate) fn instrument_search_terms(query: &str) -> Vec<String> {
    let mut terms = HashSet::new();
    terms.insert(normalize_instrument_term(query));
    for group in instrument_alias_groups_for(query) {
        for alias in group {
            terms.insert(normalize_instrument_term(alias));
        }
    }
    terms.into_iter().filter(|term| !term.is_empty()).collect()
}

pub fn instruments_related(left: &str, right: &str) -> bool {
    instrument_terms_related(left, right)
}

pub(crate) fn instrument_terms_related(query: &str, value: &str) -> bool {
    let query_groups = instrument_alias_groups_for(query);
    let value_groups = instrument_alias_groups_for(value);
    query_groups
        .iter()
        .any(|group| value_groups.iter().any(|other| std::ptr::eq(*group, *other)))
}

pub fn instrument_hint_from_path(path: &Path) -> Option<String> {
    let mut best: Option<(i32, &'static str)> = None;
    let mut consider = |score: i32, label: &'static str| {
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, label));
        }
    };

    if let Some(stem) = crate::path_util::file_stem_lossy(path) {
        for token in hint_name_tokens(&stem) {
            if let Some(label) = hint_label_for_term(&token) {
                consider(1_000 + token.len() as i32, label);
            }
        }
    }

    for (depth, ancestor) in path.ancestors().skip(1).enumerate() {
        let score_base = 500_i32.saturating_sub(depth as i32 * 50);
        if score_base == 0 {
            break;
        }
        let Some(name) = ancestor.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        for token in hint_name_tokens(name) {
            if let Some(label) = hint_label_for_term(&token) {
                consider(score_base + token.len() as i32, label);
            }
        }
    }

    best.map(|(_, label)| label.to_string())
}

/// Where an instrument hint came from, so the classifier can say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintSource {
    Path,
    #[allow(dead_code)]
    Tags,
}

impl HintSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Path => "Path hint",
            Self::Tags => "Tag hint",
        }
    }
}

const GENERIC_PATH_SEGMENTS: &[&str] = &[
    "samples",
    "sample",
    "packs",
    "pack",
    "library",
    "libraries",
    "lib",
    "audio",
    "music",
    "sound",
    "sounds",
    "assets",
    "content",
    "download",
    "downloads",
    "documents",
    "desktop",
    "users",
    "user",
    "home",
    "wav",
    "wavs",
    "aiff",
    "aif",
    "flac",
    "mp3",
    "splice",
    "loopcloud",
    "loopmasters",
    "producerloops",
    "one shots",
    "oneshots",
    "one-shots",
    "one_shots",
    "loops",
    "loop",
    "stems",
    "stem",
    "multis",
    "multitracks",
    "presets",
    "preset",
    "projects",
    "project",
    "favorites",
    "favourites",
    "recent",
    "temp",
    "tmp",
    "data",
    "media",
    "files",
    "file",
    "drive",
    "storage",
];

/// Best-effort artist label from parent folders (e.g. pack or label folder names).
pub fn artist_hint_from_path(path: &Path) -> Option<String> {
    let parent = path.parent()?;
    let mut candidates = Vec::new();

    for ancestor in parent.ancestors() {
        if should_stop_artist_walk(ancestor) {
            break;
        }
        let Some(name) = ancestor.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_generic_path_segment(name)
            || path_segment_is_instrument_category(name)
            || path_segment_is_pack_metadata(name)
            || path_segment_is_ephemeral_temp(name)
        {
            continue;
        }
        let formatted = format_path_segment_as_artist(name);
        if formatted.is_empty() {
            continue;
        }
        candidates.push(formatted);
    }

    match candidates.len() {
        0 => None,
        1 => Some(candidates[0].clone()),
        _ => Some(candidates[1].clone()),
    }
}

fn should_stop_artist_walk(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    std::env::temp_dir()
        .canonicalize()
        .ok()
        .and_then(|temp| path.canonicalize().ok().map(|canonical| canonical == temp))
        .unwrap_or(false)
}

fn path_segment_is_ephemeral_temp(name: &str) -> bool {
    name.split('_').any(|part| {
        part.len() >= 8 && part.chars().all(|ch| ch.is_ascii_digit())
    })
}

fn is_generic_path_segment(name: &str) -> bool {
    let norm = normalize_instrument_term(name);
    if norm.is_empty() {
        return true;
    }
    GENERIC_PATH_SEGMENTS
        .iter()
        .any(|segment| norm == normalize_instrument_term(segment))
}

fn path_segment_is_instrument_category(name: &str) -> bool {
    hint_name_tokens(name)
        .iter()
        .any(|token| hint_label_for_term(token).is_some())
}

fn path_segment_is_pack_metadata(name: &str) -> bool {
    let norm = normalize_instrument_term(name);
    norm.starts_with("vol") && norm.len() <= 8
        || norm.starts_with("volume")
        || (norm.starts_with("pt") && norm.len() <= 5)
        || norm.starts_with("part")
        || norm.starts_with("disc") && norm.len() <= 6
}

fn format_path_segment_as_artist(name: &str) -> String {
    let primary = name.split(" - ").next().unwrap_or(name).trim();
    primary
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn hint_name_tokens(name: &str) -> Vec<String> {
    let mut tokens = vec![normalize_instrument_term(name)];
    let mut current = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(normalize_instrument_term(&current));
            current.clear();
        }
    }
    if !current.is_empty() {
        tokens.push(normalize_instrument_term(&current));
    }
    tokens.retain(|token| !token.is_empty());
    tokens
}

pub(crate) fn hint_label_for_term(term: &str) -> Option<&'static str> {
    let needle = normalize_instrument_term(term);
    if needle.is_empty() {
        return None;
    }
    INSTRUMENT_ALIAS_GROUPS
        .iter()
        .zip(INSTRUMENT_HINT_LABELS.iter())
        .find(|(group, _)| {
            group.iter().any(|alias| {
                instrument_alias_matches(&needle, &normalize_instrument_term(alias))
            })
        })
        .map(|(_, label)| *label)
}
