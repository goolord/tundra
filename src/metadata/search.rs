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
fn fold<'a>(text: impl Into<Cow<'a, str>>, case_sensitive: bool) -> Cow<'a, str> {
    let text = text.into();
    if case_sensitive {
        text
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
    /// The whole query, folded, for the exact and prefix stem bonuses.
    query: String,
    terms: Vec<String>,
    case_sensitive: bool,
    filename_only: bool,
}

/// One path's searchable text, folded once. `full` is empty for filename-only queries.
struct PathText<'a> {
    name: Cow<'a, str>,
    stem: Option<Cow<'a, str>>,
    full: Cow<'a, str>,
}

impl PathText<'_> {
    fn name_fields(&self) -> impl Iterator<Item = &str> {
        std::iter::once(&*self.name)
            .filter(|name| !name.is_empty())
            .chain(self.stem.as_deref())
    }
}

impl<'a> FileQuery<'a> {
    fn new(matcher: &'a SkimMatcherV2, query: &str, case_sensitive: bool, filename_only: bool) -> Self {
        let query = query.trim();
        Self {
            matcher,
            query: fold(query, case_sensitive).into_owned(),
            terms: split_terms(query, case_sensitive),
            case_sensitive,
            filename_only,
        }
    }

    fn text<'p>(&self, path: &'p Path) -> PathText<'p> {
        let name = path.file_name().map(|name| name.to_string_lossy());
        let stem = path.file_stem().map(|stem| stem.to_string_lossy());
        let stem = stem.filter(|stem| name.as_deref() != Some(&**stem));
        PathText {
            name: fold(name.unwrap_or_default(), self.case_sensitive),
            stem: stem.map(|stem| fold(stem, self.case_sensitive)),
            full: if self.filename_only {
                Cow::Borrowed("")
            } else {
                fold(path.to_string_lossy(), self.case_sensitive)
            },
        }
    }

    /// Every term appears as a substring somewhere. Cheaper than scoring, used
    /// to skip paths once enough confident matches exist.
    fn has_substring_match(&self, text: &PathText) -> bool {
        !self.terms.is_empty()
            && self.terms.iter().all(|term| {
                text.name_fields().any(|field| field.contains(term.as_str())) || text.full.contains(term.as_str())
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
            let name_term = text
                .name_fields()
                .map(|field| term_score(self.matcher, field, term))
                .max()
                .unwrap_or(0);
            let path_term = if text.full.contains(term.as_str()) {
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

    fn sort_score(&self, path: &Path, text: &PathText, name_score: i64, path_score: i64) -> i64 {
        if !is_audio(path) {
            return if name_score > 0 {
                name_score + DIRECT_FOLDER_BONUS
            } else {
                path_score
            };
        }
        let base = if name_score > 0 { name_score } else { path_score };
        // The stem is folded like the query, so a name that matched
        // case-insensitively also earns its exact or prefix bonus.
        let stem = text.stem.as_deref().unwrap_or(&text.name);
        let bonus = if stem == self.query {
            EXACT_STEM_BONUS
        } else if stem.starts_with(self.query.as_str()) {
            PREFIX_STEM_BONUS
        } else {
            0
        };
        base + FILE_SEARCH_BONUS + bonus
    }
}

/// A tag filter term, then (for instrument filters) its aliases, each with its
/// instrument group mask. Expanded once per search.
struct FilterTerm(Vec<(String, u32)>);

impl FilterTerm {
    fn new(term: String, field: TagField) -> Self {
        if field != TagField::Instrument {
            return Self(vec![(term, 0)]);
        }
        let aliases = instrument_search_terms(&term);
        let with_groups = |term: String| {
            let groups = instrument_group_mask(&term);
            (term, groups)
        };
        Self(std::iter::once(term).chain(aliases).map(with_groups).collect())
    }

    /// A direct match, else a related instrument; the term itself first, then its best alias.
    fn score(&self, matcher: &SkimMatcherV2, value: &str, value_groups: u32) -> Option<i64> {
        let score = |(term, groups): &(String, u32)| match term_score(matcher, value, term) {
            0 => (groups & value_groups != 0).then(|| RELATED_INSTRUMENT_SCORE + term.len() as i64),
            direct => Some(direct),
        };
        let (term, aliases) = self.0.split_first()?;
        score(term).or_else(|| aliases.iter().filter_map(score).max())
    }
}

/// Tag filters compiled for one search. Scores are memoized per distinct tag
/// value, since libraries repeat the same instrument, key, and BPM values
/// across thousands of files.
struct TagQuery<'a> {
    matcher: &'a SkimMatcherV2,
    filters: Vec<(TagField, Vec<FilterTerm>, ScoreMemo)>,
}

type ScoreMemo = HashMap<String, Option<i64>>;

impl<'a> TagQuery<'a> {
    fn new(matcher: &'a SkimMatcherV2, filters: &[TagFilter]) -> Self {
        let filters = filters
            .iter()
            .map(|filter| {
                let terms = split_terms(&filter.value, false);
                let terms = terms
                    .into_iter()
                    .map(|term| FilterTerm::new(term, filter.field))
                    .collect();
                (filter.field, terms, HashMap::new())
            })
            .collect();
        Self { matcher, filters }
    }

    /// Lowest per-term score, or `None` unless every term matches `value`.
    fn value_score(matcher: &SkimMatcherV2, field: TagField, terms: &[FilterTerm], value: &str) -> Option<i64> {
        if terms.is_empty() {
            return None;
        }
        let value = value.to_lowercase();
        let groups = if field == TagField::Instrument {
            instrument_group_mask(&value)
        } else {
            0
        };
        terms.iter().try_fold(i64::MAX, |min, term| {
            term.score(matcher, &value, groups).map(|score| min.min(score))
        })
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
                    let score = Self::value_score(self.matcher, *field, terms, value);
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

/// Best score first, then by path; at most `limit` results.
fn into_result(mut matches: Vec<(i64, PathBuf)>, lookup: MetadataLookup, limit: usize) -> SearchResult {
    matches
        .sort_unstable_by(|(a_score, a_path), (b_score, b_path)| b_score.cmp(a_score).then_with(|| a_path.cmp(b_path)));
    matches.truncate(limit);
    SearchResult {
        paths: matches.into_iter().map(|(_, path)| path).collect(),
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
            (score > 0).then(|| (score, path.clone()))
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

        let file_score = file_query.sort_score(path, &text, name_score, path_score);
        let score = if tag_active {
            let tag_score = tags.score_path(path, &mut lookup, true);
            if tag_score == 0 {
                continue;
            }
            file_score.saturating_add(tag_score.saturating_mul(100))
        } else {
            file_score
        };
        matches.push((score, path.clone()));
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

    fn file_search(paths: &[&str], text: &str) -> Vec<PathBuf> {
        let paths: Vec<_> = paths.iter().map(PathBuf::from).collect();
        let query = SearchQuery {
            text,
            show_directories: true,
            ..SearchQuery::default()
        };
        search(&paths, &query, MetadataLookup::new(Arc::default())).paths
    }

    #[test]
    fn file_search_requires_every_term_and_rejects_weak_fuzzy_matches() {
        for (path, query, matches) in [
            ("/Samples/Snare Drum 01.wav", "snare", true),
            ("/Samples/Snare Drum 01.wav", "snare drum", true),
            ("/Samples/Snare Drum 01.wav", "snare kick", false),
            ("/Samples/Synth Pad 01.wav", "snare", false),
            ("/samples/ÉCLAT Snare.wav", "éclat", true),
        ] {
            assert_eq!(name_score(path, query) > 0, matches, "{path} / {query}");
        }
    }

    #[test]
    fn file_search_confident_cap_skips_fuzzy_only_matches() {
        let names: Vec<_> = (0..FILE_SEARCH_CONFIDENT_RESULT_CAP)
            .map(|index| format!("/Samples/snare-{index:04}.wav"))
            .chain(["/Samples/01 Snare.wav".into(), "/Samples/Synth Pad 01.wav".into()])
            .collect();
        let found = file_search(&names.iter().map(String::as_str).collect::<Vec<_>>(), "snare");
        assert!(
            found.iter().any(|path| path.ends_with("01 Snare.wav")),
            "substring matches remain"
        );
        assert!(
            !found.iter().any(|path| path.ends_with("Synth Pad 01.wav")),
            "fuzzy-only matches should be dropped once the confident cap is full"
        );
    }

    #[test]
    fn exact_and_prefix_stem_bonuses_fold_case_without_splitting_characters() {
        assert_eq!(
            file_search(&["/samples/ベースkick.wav"], "kick"),
            [PathBuf::from("/samples/ベースkick.wav")]
        );
        let ranked = file_search(
            &["/s/éa.wav", "/s/éclat extra.wav", "/s/ÉCLAT.wav", "/s/Kick 01.wav"],
            "éclat",
        );
        assert_eq!(
            ranked,
            [PathBuf::from("/s/ÉCLAT.wav"), PathBuf::from("/s/éclat extra.wav")]
        );
        let ranked = file_search(&["/s/a kick.wav", "/s/Kick 01.wav"], "kick");
        assert_eq!(ranked[0], PathBuf::from("/s/Kick 01.wav"), "prefix bonus");
    }

    #[test]
    fn tag_filters_fuzzy_match_but_need_every_term() {
        let fields = TagFields {
            bpm: "120.00".into(),
            comment: "dark snare loop".into(),
            explicit_instrument: "Snare Drum".into(),
            instrument: "Drums".into(),
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
        assert!(
            tag_score(&fields, TagField::Instrument, "snare").is_some(),
            "explicit instrument wins"
        );
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
