//! Ranking paths against a filename query and tag filters.

use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::cache::{CachedMetadata, MetadataLookup};
use super::fields::{TagField, TagFields, TagFilter};
use super::hints::{instrument_group_mask, instrument_search_terms};
use super::read::is_audio;

/// Shorter filename queries only run alongside tag filters.
pub const FILE_SEARCH_MIN_QUERY_LEN: usize = 2;
const CONTAINS_NAME_MATCH_SCORE: i64 = 400;
const FILE_SEARCH_MIN_FUZZY_SCORE: i64 = 70;
/// Past this many matches, only substring matches are added.
const FILE_SEARCH_CONFIDENT_RESULT_CAP: usize = 2_000;
const FILE_SEARCH_MAX_RESULTS: usize = 10_000;
const RELATED_INSTRUMENT_SCORE: i64 = 80;

const FILE_SEARCH_BONUS: i64 = 1_000;
const DIRECT_FOLDER_BONUS: i64 = 2_000;
const EXACT_STEM_BONUS: i64 = 50_000;
const PREFIX_STEM_BONUS: i64 = 10_000;

/// What to search for.
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchQuery<'a> {
    /// Filename terms, all of which must match.
    pub text: &'a str,
    pub tag_filters: &'a [TagFilter],
    pub case_sensitive: bool,
    /// Include folders whose names match.
    pub show_directories: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SearchResult {
    /// Best match first.
    pub paths: Vec<PathBuf>,
    /// Tags read from disk during the search, for the caller's index.
    pub new_metadata: HashMap<PathBuf, CachedMetadata>,
}

/// True when a search should run. Tag filters alone are enough; a filename
/// query needs two characters unless tag filters already narrowed the set.
pub fn file_search_active(file_query: &str, tag_filters: &[TagFilter]) -> bool {
    !tag_filters.is_empty() || file_query.trim().len() >= FILE_SEARCH_MIN_QUERY_LEN
}

/// Ranks `paths` against `query`, reading tags from `lookup`, whose new entries
/// (freshly indexed files) overlay the shared index without copying it.
pub fn search(paths: &[PathBuf], query: &SearchQuery, lookup: MetadataLookup) -> SearchResult {
    if !query.tag_filters.is_empty() && query.text.trim().is_empty() {
        return tag_matches(paths, query.tag_filters, lookup);
    }
    file_matches(paths, query, lookup)
}

fn matcher(case_sensitive: bool) -> SkimMatcherV2 {
    if case_sensitive {
        SkimMatcherV2::default().respect_case()
    } else {
        SkimMatcherV2::default().ignore_case()
    }
}

/// Case folding applied once to both query terms and searched text.
fn fold(text: &str, case_sensitive: bool) -> Cow<'_, str> {
    if case_sensitive {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.to_lowercase())
    }
}

fn split_terms(query: &str, case_sensitive: bool) -> Vec<String> {
    query
        .split_whitespace()
        .map(|term| fold(term, case_sensitive).into_owned())
        .collect()
}

/// Substring hits outrank fuzzy ones; weak fuzzy hits count as no match.
/// `text` and `term` must already be folded the same way.
fn term_score(matcher: &SkimMatcherV2, text: &str, term: &str) -> i64 {
    if text.contains(term) {
        return CONTAINS_NAME_MATCH_SCORE + term.len() as i64;
    }
    matcher
        .fuzzy_match(text, term)
        .filter(|&score| score >= FILE_SEARCH_MIN_FUZZY_SCORE)
        .unwrap_or(0)
}

/// A filename query, split and folded once per search instead of per path.
struct FileQuery<'a> {
    matcher: &'a SkimMatcherV2,
    query: &'a str,
    terms: Vec<String>,
    case_sensitive: bool,
    filename_only: bool,
}

/// One path's searchable text, folded once.
struct PathText<'a> {
    name: Cow<'a, str>,
    stem: Option<Cow<'a, str>>,
    full: Cow<'a, str>,
}

impl<'a> FileQuery<'a> {
    fn new(matcher: &'a SkimMatcherV2, query: &'a str, case_sensitive: bool, filename_only: bool) -> Self {
        let query = query.trim();
        Self {
            matcher,
            query,
            terms: split_terms(query, case_sensitive),
            case_sensitive,
            filename_only,
        }
    }

    fn text<'p>(&self, path: &'p Path) -> PathText<'p> {
        let fold_owned = |text: Cow<'p, str>| match text {
            Cow::Borrowed(text) => fold(text, self.case_sensitive),
            Cow::Owned(text) => Cow::Owned(fold(&text, self.case_sensitive).into_owned()),
        };
        let name = path.file_name().map(|name| name.to_string_lossy());
        let stem = path.file_stem().map(|stem| stem.to_string_lossy());
        let stem = stem.filter(|stem| name.as_deref() != Some(&**stem));
        PathText {
            name: fold_owned(name.unwrap_or_default()),
            stem: stem.map(fold_owned),
            full: if self.filename_only {
                Cow::Borrowed("")
            } else {
                fold_owned(path.to_string_lossy())
            },
        }
    }

    fn name_fields<'t>(text: &'t PathText) -> impl Iterator<Item = &'t str> {
        std::iter::once(&*text.name)
            .filter(|name| !name.is_empty())
            .chain(text.stem.as_deref())
    }

    /// Every term appears as a substring somewhere. Cheaper than scoring, used
    /// to skip paths once enough confident matches exist.
    fn has_substring_match(&self, text: &PathText) -> bool {
        !self.terms.is_empty()
            && self.terms.iter().all(|term| {
                Self::name_fields(text).any(|field| field.contains(term.as_str()))
                    || (!self.filename_only && text.full.contains(term.as_str()))
            })
    }

    /// `(name_score, path_score)`, or zeros unless every term matches.
    fn scores(&self, text: &PathText) -> (i64, i64) {
        if self.terms.is_empty() {
            return (0, 0);
        }
        let mut name_score = 0i64;
        let mut path_score = 0i64;
        for term in &self.terms {
            let name_term = Self::name_fields(text)
                .map(|field| term_score(self.matcher, field, term))
                .max()
                .unwrap_or(0);
            let path_term = if !self.filename_only && text.full.contains(term.as_str()) {
                CONTAINS_NAME_MATCH_SCORE + term.len() as i64
            } else {
                0
            };
            if name_term.max(path_term) == 0 {
                return (0, 0);
            }
            name_score += name_term;
            path_score += path_term;
        }
        (name_score, path_score)
    }

    fn sort_score(&self, path: &Path, name_score: i64, path_score: i64) -> i64 {
        if !is_audio(path) {
            return if name_score > 0 {
                name_score + DIRECT_FOLDER_BONUS
            } else {
                path_score
            };
        }
        let base = if name_score > 0 { name_score } else { path_score };
        let mut score = base + FILE_SEARCH_BONUS;
        if let Some(stem) = path.file_stem().map(|stem| stem.to_string_lossy()) {
            if text_eq(&stem, self.query, self.case_sensitive) {
                score += EXACT_STEM_BONUS;
            } else if text_starts_with(&stem, self.query, self.case_sensitive) {
                score += PREFIX_STEM_BONUS;
            }
        }
        score
    }
}

// Both fold case the same way matching does, so a name that matched
// case-insensitively also earns its exact or prefix bonus.
fn text_eq(a: &str, b: &str, case_sensitive: bool) -> bool {
    fold(a, case_sensitive) == fold(b, case_sensitive)
}

fn text_starts_with(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    fold(haystack, case_sensitive).starts_with(&*fold(needle, case_sensitive))
}

/// A query term for instrument filters, with its alias expansion computed once.
struct InstrumentTerm {
    term: String,
    groups: u32,
    aliases: Vec<(String, u32)>,
}

impl InstrumentTerm {
    fn new(term: String) -> Self {
        let aliases = instrument_search_terms(&term)
            .into_iter()
            .map(|alias| {
                let groups = instrument_group_mask(&alias);
                (alias, groups)
            })
            .collect();
        Self {
            groups: instrument_group_mask(&term),
            term,
            aliases,
        }
    }

    fn score(&self, matcher: &SkimMatcherV2, value: &str, value_groups: u32) -> Option<i64> {
        let direct = term_score(matcher, value, &self.term);
        if direct > 0 {
            return Some(direct);
        }
        if self.groups & value_groups != 0 {
            return Some(RELATED_INSTRUMENT_SCORE + self.term.len() as i64);
        }
        self.aliases
            .iter()
            .filter_map(|(alias, groups)| {
                let score = term_score(matcher, value, alias);
                if score > 0 {
                    Some(score)
                } else {
                    (groups & value_groups != 0).then(|| RELATED_INSTRUMENT_SCORE + alias.len() as i64)
                }
            })
            .max()
    }
}

enum FilterTerms {
    Plain(Vec<String>),
    Instrument(Vec<InstrumentTerm>),
}

/// Tag filters compiled for one search. Scores are memoized per distinct tag
/// value, since libraries repeat the same instrument, key, and BPM values
/// across thousands of files.
struct TagQuery<'a> {
    matcher: &'a SkimMatcherV2,
    filters: Vec<(TagField, FilterTerms, HashMap<String, Option<i64>>)>,
}

impl<'a> TagQuery<'a> {
    fn new(matcher: &'a SkimMatcherV2, filters: &[TagFilter]) -> Self {
        let filters = filters
            .iter()
            .map(|filter| {
                let terms = split_terms(&filter.value, false);
                let terms = if filter.field == TagField::Instrument {
                    FilterTerms::Instrument(terms.into_iter().map(InstrumentTerm::new).collect())
                } else {
                    FilterTerms::Plain(terms)
                };
                (filter.field, terms, HashMap::new())
            })
            .collect();
        Self { matcher, filters }
    }

    /// Lowest per-term score, or `None` unless every term matches `value`.
    fn value_score(matcher: &SkimMatcherV2, terms: &FilterTerms, value: &str) -> Option<i64> {
        fn all_min(mut scores: impl Iterator<Item = Option<i64>>) -> Option<i64> {
            scores.try_fold(i64::MAX, |min, score| score.map(|score| min.min(score)))
        }
        let value = value.to_lowercase();
        match terms {
            FilterTerms::Plain(terms) if !terms.is_empty() => all_min(terms.iter().map(|term| {
                let score = term_score(matcher, &value, term);
                (score > 0).then_some(score)
            })),
            FilterTerms::Instrument(terms) if !terms.is_empty() => {
                let groups = instrument_group_mask(&value);
                all_min(terms.iter().map(|term| term.score(matcher, &value, groups)))
            }
            _ => None,
        }
    }

    /// Lowest score across filters, or `None` unless every filter matches.
    fn score(&mut self, fields: &TagFields) -> Option<i64> {
        let mut lowest = i64::MAX;
        for (field, terms, memo) in &mut self.filters {
            let value = fields.field_value(*field);
            if value.is_empty() {
                return None;
            }
            let score = match memo.get(value) {
                Some(score) => *score,
                None => {
                    let score = Self::value_score(self.matcher, terms, value);
                    memo.insert(value.to_string(), score);
                    score
                }
            };
            lowest = lowest.min(score?);
        }
        Some(lowest)
    }

    /// Tags come from the index; files missing from it are read from disk only
    /// when `allow_disk` (a filename query already narrowed the set).
    fn score_path(&mut self, path: &Path, lookup: &mut MetadataLookup, allow_disk: bool) -> i64 {
        if self.filters.is_empty() || !is_audio(path) {
            return 0;
        }
        let score = if let Some(fields) = lookup.indexed_tag_fields(path) {
            self.score(fields)
        } else if allow_disk && path.exists() {
            let fields = lookup.tag_fields(path);
            self.score(&fields)
        } else {
            None
        };
        score.unwrap_or(0)
    }
}

#[derive(Eq, PartialEq)]
struct SearchRank {
    score: i64,
    path: PathBuf,
}

impl Ord for SearchRank {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.score.cmp(&self.score).then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for SearchRank {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn into_result(mut matches: Vec<SearchRank>, lookup: MetadataLookup, limit: usize) -> SearchResult {
    matches.sort_unstable();
    matches.truncate(limit);
    SearchResult {
        paths: matches.into_iter().map(|entry| entry.path).collect(),
        new_metadata: lookup.into_new_entries(),
    }
}

/// Tag filters with no filename query: answered from the index alone.
fn tag_matches(paths: &[PathBuf], tag_filters: &[TagFilter], mut lookup: MetadataLookup) -> SearchResult {
    let matcher = matcher(false);
    let mut tags = TagQuery::new(&matcher, tag_filters);
    let matches = paths
        .iter()
        .filter_map(|path| {
            let score = tags.score_path(path, &mut lookup, false);
            (score > 0).then(|| SearchRank {
                score,
                path: path.clone(),
            })
        })
        .collect();
    into_result(matches, lookup, usize::MAX)
}

fn file_matches(paths: &[PathBuf], query: &SearchQuery, mut lookup: MetadataLookup) -> SearchResult {
    let file_matcher = matcher(query.case_sensitive);
    let tag_matcher = matcher(false);
    let tag_active = !query.tag_filters.is_empty();
    let filename_only = tag_active && query.text.trim().len() < FILE_SEARCH_MIN_QUERY_LEN;
    let file_query = FileQuery::new(&file_matcher, query.text, query.case_sensitive, filename_only);
    let mut tags = TagQuery::new(&tag_matcher, query.tag_filters);
    let mut matches = Vec::new();

    for path in paths {
        if !query.show_directories && !is_audio(path) {
            continue;
        }
        let text = file_query.text(path);
        let capped = matches.len() >= FILE_SEARCH_CONFIDENT_RESULT_CAP;
        if capped && !file_query.has_substring_match(&text) {
            continue;
        }

        let (name_score, path_score) = file_query.scores(&text);
        if name_score == 0 && path_score == 0 {
            continue;
        }
        let confident = name_score >= CONTAINS_NAME_MATCH_SCORE || path_score >= CONTAINS_NAME_MATCH_SCORE;
        if capped && !confident {
            continue;
        }

        let file_score = file_query.sort_score(path, name_score, path_score);
        let score = if tag_active {
            let tag_score = tags.score_path(path, &mut lookup, true);
            if tag_score == 0 {
                continue;
            }
            file_score.saturating_add(tag_score.saturating_mul(100))
        } else {
            file_score
        };
        matches.push(SearchRank {
            score,
            path: path.clone(),
        });
    }

    into_result(matches, lookup, FILE_SEARCH_MAX_RESULTS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn name_score(path: &str, query: &str) -> i64 {
        let matcher = matcher(false);
        let query = FileQuery::new(&matcher, query, false, false);
        let path = PathBuf::from(path);
        let (name, path_score) = query.scores(&query.text(&path));
        name.max(path_score)
    }

    fn tag_score(fields: &TagFields, field: TagField, value: &str) -> Option<i64> {
        let matcher = matcher(false);
        let filter = TagFilter {
            field,
            value: value.into(),
        };
        TagQuery::new(&matcher, &[filter]).score(fields)
    }

    fn file_search(paths: &[PathBuf], text: &str) -> Vec<PathBuf> {
        let query = SearchQuery {
            text,
            show_directories: true,
            ..SearchQuery::default()
        };
        search(paths, &query, MetadataLookup::new(Arc::default())).paths
    }

    #[test]
    fn file_search_requires_every_term_and_rejects_weak_fuzzy_matches() {
        assert!(name_score("/Samples/Snare Drum 01.wav", "snare") > 0);
        assert!(name_score("/Samples/Snare Drum 01.wav", "snare drum") > 0);
        assert_eq!(name_score("/Samples/Snare Drum 01.wav", "snare kick"), 0);
        assert_eq!(
            name_score("/Samples/Synth Pad 01.wav", "snare"),
            0,
            "unrelated fuzzy matches should be rejected"
        );
    }

    #[test]
    fn file_search_confident_cap_skips_fuzzy_only_matches() {
        let mut paths: Vec<PathBuf> = (0..FILE_SEARCH_CONFIDENT_RESULT_CAP)
            .map(|index| PathBuf::from(format!("/Samples/snare-{index:04}.wav")))
            .collect();
        paths.push(PathBuf::from("/Samples/01 Snare.wav"));
        paths.push(PathBuf::from("/Samples/Synth Pad 01.wav"));

        let found = file_search(&paths, "snare");
        assert!(
            found.iter().any(|path| path.ends_with("01 Snare.wav")),
            "substring matches should remain"
        );
        assert!(
            !found.iter().any(|path| path.ends_with("Synth Pad 01.wav")),
            "fuzzy-only matches should be dropped once the confident cap is full"
        );
    }

    #[test]
    fn instrument_filter_uses_explicit_instrument() {
        let fields = TagFields {
            explicit_instrument: "Snare Drum".into(),
            instrument: "Drums".into(),
            ..TagFields::default()
        };
        assert!(tag_score(&fields, TagField::Instrument, "snare").is_some());
    }

    #[test]
    fn tag_filters_fuzzy_match_but_need_every_term() {
        let fields = TagFields {
            bpm: "120.00".into(),
            comment: "dark snare loop".into(),
            ..TagFields::default()
        };
        assert!(
            tag_score(&fields, TagField::Bpm, "120").is_some(),
            "partial values match"
        );
        assert!(tag_score(&fields, TagField::Comment, "snare loop").is_some());
        assert!(
            tag_score(&fields, TagField::Comment, "snare kick").is_none(),
            "every term must match"
        );
    }

    #[test]
    fn prefix_check_does_not_split_multibyte_characters() {
        assert!(!text_starts_with("ベースkick", "kick", false));
        assert!(!text_starts_with("éa", "e", false));
        assert!(text_starts_with("Kick 01", "kick", false));
        assert!(text_eq("ÉCLAT", "éclat", false));

        let paths = vec![PathBuf::from("/samples/ベースkick.wav")];
        assert_eq!(file_search(&paths, "kick"), paths);
    }

    #[test]
    fn case_insensitive_search_folds_non_ascii_text() {
        assert!(name_score("/samples/ÉCLAT Snare.wav", "éclat") > 0);
    }

    #[test]
    fn file_search_active_requires_two_chars_without_tags() {
        assert!(!file_search_active("2", &[]));
        assert!(!file_search_active(" ", &[]));
        assert!(file_search_active("ki", &[]));
        let kick = TagFilter {
            field: TagField::Instrument,
            value: "Kick".into(),
        };
        assert!(file_search_active("", &[kick]));
    }
}
