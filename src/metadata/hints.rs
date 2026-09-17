use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

/// Names for each instrument, matched against words in file and folder names
/// and used to widen instrument searches. English names and common sample-pack
/// abbreviations come first in each group, then other languages (Spanish,
/// Portuguese, French, Italian, German, Japanese, Chinese, Korean).
///
/// Non-ASCII names of two or more characters also match inside a longer word,
/// since Japanese and Chinese names are not separated by spaces
/// (`スネアドラム01`). Two-letter drum-machine codes that are also ordinary
/// words live in `WEAK_ALIASES` instead.
const INSTRUMENT_ALIAS_GROUPS: &[&[&str]] = &[
    &[
        "kick", "bd", "bassdrum", "bass drum", "kick drum", "kickdrum", "808 kick", "808kick",
        "kik", "kck", "bombo", "bumbo", "grosse caisse", "cassa", "basstrommel", "キック",
        "バスドラム", "バスドラ", "底鼓", "大鼓", "킥",
    ],
    &[
        "snare", "sd", "sn", "snr", "snare drum", "snaredrum", "caja", "redoblante", "caixa",
        "caisse claire", "rullante", "kleine trommel", "スネア", "军鼓", "小鼓", "스네어",
    ],
    &["rim", "rimshot", "rim shot", "side stick", "sidestick", "rim click", "rimclick"],
    &[
        "hihat", "hi-hat", "hi hat", "hh", "hat", "hats", "open hat", "closed hat", "openhat",
        "closedhat", "op hat", "cl hat", "hhat", "chh", "ohh", "phh", "hhc", "hho",
        "pedal hat", "pedalhat", "charleston", "chimbal", "ハイハット", "踩镲", "하이햇",
    ],
    &[
        "clap", "handclap", "hand clap", "clp", "snap", "fingersnap", "finger snap", "snp",
        "palmas", "aplauso", "battimani", "クラップ", "拍手", "클랩",
    ],
    &[
        "tom", "toms", "floor tom", "floortom", "rack tom", "racktom", "tomtom", "tom tom",
        "hi tom", "mid tom", "low tom", "タム", "通鼓", "탐탐",
    ],
    &[
        "perc", "percs", "percussion", "conga", "bongo", "bongos", "shaker", "tamb",
        "tambourine", "cowbell", "triangle", "woodblock", "cabasa", "prc", "clave", "claves",
        "guiro", "timbale", "timbales", "djembe", "cajon", "agogo", "maraca", "tabla",
        "tamborine", "percusion", "percusión", "percussioni", "perkussion", "パーカッション",
        "打击乐", "퍼커션",
    ],
    &[
        "crash", "cymbal", "cymbals", "ride", "splash", "china", "crsh", "crashes", "cym",
        "cymb", "platillo", "prato", "cymbale", "piatto", "piatti", "becken", "シンバル",
        "吊镲", "镲片", "심벌",
    ],
    &[
        "bass", "sub", "subbass", "sub bass", "808 bass", "808bass", "reese", "wobble", "bajo",
        "baixo", "basse", "basso", "ベース", "贝斯", "베이스",
    ],
    &[
        "synth", "lead", "pad", "pluck", "stab", "arp", "arpeggio", "keys", "synthesizer",
        "teclado", "clavier", "tastiera", "sintetizador", "シンセ", "合成器", "신스",
    ],
    &[
        "fx", "sfx", "effect", "impact", "riser", "sweep", "noise", "atm", "atmosphere",
        "ambient", "transition", "downlifter", "uplifter", "whoosh", "swoosh", "glitch",
        "efectos", "effets", "effetti", "エフェクト", "音效", "효과음",
    ],
    &[
        "vocal", "vox", "voice", "acapella", "aca", "phrase", "adlib", "voc", "vcl",
        "acappella", "a cappella", "choir", "chant", "voz", "voix", "voce", "stimme", "gesang",
        "ボーカル", "ボイス", "人声", "보컬",
    ],
    &[
        "piano", "rhodes", "organ", "electric piano", "epiano", "klavier", "ピアノ",
        "钢琴", "피아노",
    ],
    &[
        "guitar", "gtr", "acoustic guitar", "acousticguitar", "guit", "guitarra", "guitare",
        "chitarra", "gitarre", "violao", "violão", "ギター", "吉他",
    ],
    &[
        "loop", "loops", "top loop", "toploop", "drum loop", "drumloop", "bucle", "boucle",
        "ループ", "循环", "루프",
    ],
    &["oneshot", "one shot", "one-shot", "ワンショット"],
    &[
        "brass", "horn", "trumpet", "sax", "saxophone", "flute", "strings", "string", "metales",
        "cuivres", "ottoni", "blechbläser", "ブラス", "铜管",
    ],
];

/// Drum-machine codes (TR-808/909 pad names and common kit abbreviations) that
/// are also everyday words or initials: `Oh Yeah.wav` is a vocal, not an open
/// hat. They only name the instrument when nothing else in the path does, and
/// never widen searches.
const WEAK_ALIASES: &[(&str, &str)] = &[
    ("ch", "Hi-Hat"),
    ("oh", "Hi-Hat"),
    ("rd", "Cymbal"),
    ("cr", "Cymbal"),
    ("cy", "Cymbal"),
    ("cp", "Clap"),
    ("rs", "Rim"),
    ("lt", "Tom"),
    ("mt", "Tom"),
    ("ht", "Tom"),
    ("cb", "Percussion"),
    ("cl", "Percussion"),
    ("ma", "Percussion"),
    ("kd", "Kick"),
    // Also "extended play" in release folder names.
    ("ep", "Piano"),
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
const _: () = assert!(INSTRUMENT_ALIAS_GROUPS.len() <= u32::BITS as usize);

const MIN_INSTRUMENT_SUBSTRING_LEN: usize = 4;
/// Everyday loanwords that contain a katakana or Hangul instrument name
/// (`カスタム` "custom" contains `タム`, `グループ` "group" contains `ループ`).
/// They are removed from a word before substring matching.
const FALSE_FRIENDS: &[&str] = &[
    "カスタム",
    "グループ",
    "データベース",
    "ブラスト",
    "데이터베이스",
];
/// Shortest word that may match an alias by prefix. One- and two-letter words
/// are initials or key names (`C`, `A`), not instruments.
const MIN_PREFIX_NEEDLE_LEN: usize = 3;

/// `INSTRUMENT_ALIAS_GROUPS`, normalized once.
static NORMALIZED_ALIAS_GROUPS: LazyLock<Vec<Vec<String>>> = LazyLock::new(|| {
    INSTRUMENT_ALIAS_GROUPS
        .iter()
        .map(|group| group.iter().map(|alias| normalize_instrument_term(alias)).collect())
        .collect()
});

fn instrument_alias_matches(needle: &str, alias_norm: &str) -> bool {
    if needle.is_empty() || alias_norm.is_empty() {
        return false;
    }
    if needle == alias_norm || is_simple_plural(needle, alias_norm) || is_simple_plural(alias_norm, needle)
    {
        return true;
    }
    if !alias_norm.is_ascii() {
        return non_ascii_alias_within(needle, alias_norm);
    }
    // Short ASCII tokens only. Prefix on longer names made `bass` hit
    // `bassdrum` (Kick) and `shot` hit `rimshot`.
    needle.is_ascii()
        && needle.len() >= MIN_PREFIX_NEEDLE_LEN
        && needle.len() < MIN_INSTRUMENT_SUBSTRING_LEN
        && alias_norm.len() < MIN_INSTRUMENT_SUBSTRING_LEN
        && (alias_norm.starts_with(needle) || needle.starts_with(alias_norm))
}

/// Japanese and Chinese words run together without spaces, so non-ASCII
/// names match inside a longer word. Two-character kana names (`タム`) are too
/// common inside loanwords and must match a whole word.
fn non_ascii_alias_within(needle: &str, alias_norm: &str) -> bool {
    let length = alias_norm.chars().count();
    let kana = alias_norm
        .chars()
        .all(|ch| matches!(ch, '\u{3040}'..='\u{30FF}'));
    if length < 2 || (kana && length < 3) {
        return false;
    }
    let mut word = needle.to_string();
    for friend in FALSE_FRIENDS {
        word = word.replace(friend, " ");
    }
    word.contains(alias_norm)
}

fn is_simple_plural(plural: &str, singular: &str) -> bool {
    plural.len() == singular.len() + 1 && plural.starts_with(singular) && plural.ends_with('s')
}

fn normalize_instrument_term(term: &str) -> String {
    term.chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Bit `i` is set when `term` names an instrument in alias group `i`.
pub(crate) fn instrument_group_mask(term: &str) -> u32 {
    let needle = normalize_instrument_term(term);
    if needle.is_empty() {
        return 0;
    }
    NORMALIZED_ALIAS_GROUPS
        .iter()
        .enumerate()
        .filter(|(_, group)| group.iter().any(|alias| instrument_alias_matches(&needle, alias)))
        .fold(0, |mask, (index, _)| mask | 1 << index)
}

pub(crate) fn instrument_search_terms(query: &str) -> Vec<String> {
    let mask = instrument_group_mask(query);
    let mut terms: HashSet<String> = NORMALIZED_ALIAS_GROUPS
        .iter()
        .enumerate()
        .filter(|(index, _)| mask & 1 << index != 0)
        .flat_map(|(_, group)| group.iter().cloned())
        .collect();
    terms.insert(normalize_instrument_term(query));
    terms.into_iter().filter(|term| !term.is_empty()).collect()
}

pub fn instruments_related(left: &str, right: &str) -> bool {
    instrument_group_mask(left) & instrument_group_mask(right) != 0
}

pub fn instrument_hint_from_path(path: &Path) -> Option<String> {
    let mut best: Option<(i32, &'static str)> = None;
    let mut consider = |score: i32, label: &'static str| {
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, label));
        }
    };

    // A weak alias scores below any real name anywhere in the path, but a weak
    // alias in the file name still beats one in a folder. Kit codes only mean
    // something in the file name or the kit folder holding it; higher up, `MA`
    // or `CR` is far more likely a user or project name.
    let mut consider_token = |base: i32, token: &str, allow_weak: bool| {
        let length = token.chars().count() as i32;
        if let Some(label) = hint_label_for_term(token) {
            consider(base + length, label);
        } else if let Some(label) = weak_hint_label(token).filter(|_| allow_weak) {
            consider(base / 100 + length, label);
        }
    };

    if let Some(stem) = crate::path_util::file_stem_lossy(path) {
        for token in hint_name_tokens(&stem) {
            consider_token(1_000, &token, true);
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
            consider_token(score_base, &token, depth == 0);
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
    // Both spellings of the temp dir, resolved once: this runs for every
    // ancestor of every file read.
    static TEMP_DIRS: LazyLock<Vec<std::path::PathBuf>> = LazyLock::new(|| {
        let temp = std::env::temp_dir();
        let canonical = crate::path_util::canonical_path(&temp).ok();
        [Some(temp), canonical]
            .into_iter()
            .flatten()
            .map(|dir| crate::path_util::cache_key(&dir))
            .collect()
    });
    path.parent().is_none()
        || TEMP_DIRS.contains(&crate::path_util::cache_key(path))
}

fn path_segment_is_ephemeral_temp(name: &str) -> bool {
    name.split('_').any(|part| {
        part.len() >= 8 && part.chars().all(|ch| ch.is_ascii_digit())
    })
}

fn is_generic_path_segment(name: &str) -> bool {
    static NORMALIZED: LazyLock<HashSet<String>> = LazyLock::new(|| {
        GENERIC_PATH_SEGMENTS
            .iter()
            .map(|segment| normalize_instrument_term(segment))
            .collect()
    });
    let norm = normalize_instrument_term(name);
    norm.is_empty() || NORMALIZED.contains(&norm)
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

/// Candidate words in a file or folder name: the whole name, each word (split
/// at punctuation, spaces, and letter/digit boundaries, so `Kick01` yields
/// `kick`), and each pair of adjacent words joined (`Bass Drum` -> `bassdrum`).
pub(crate) fn hint_name_tokens(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in name.chars() {
        let boundary = current
            .chars()
            .last()
            .is_some_and(|last| last.is_numeric() != ch.is_numeric());
        if (!ch.is_alphanumeric() || boundary) && !current.is_empty() {
            words.push(normalize_instrument_term(&current));
            current.clear();
        }
        if ch.is_alphanumeric() {
            current.push(ch);
        }
    }
    if !current.is_empty() {
        words.push(normalize_instrument_term(&current));
    }

    let mut tokens = vec![normalize_instrument_term(name)];
    tokens.extend(words.iter().cloned());
    tokens.extend(words.windows(2).map(|pair| format!("{}{}", pair[0], pair[1])));
    tokens.retain(|token| !token.is_empty());
    tokens
}

fn weak_hint_label(term: &str) -> Option<&'static str> {
    let needle = normalize_instrument_term(term);
    WEAK_ALIASES
        .iter()
        .find(|(alias, _)| *alias == needle)
        .map(|(_, label)| *label)
}

pub(crate) fn hint_label_for_term(term: &str) -> Option<&'static str> {
    let mask = instrument_group_mask(term);
    (mask != 0).then(|| INSTRUMENT_HINT_LABELS[mask.trailing_zeros() as usize])
}
