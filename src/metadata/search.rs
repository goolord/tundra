use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::types::is_audio;

use super::cache::{CachedMetadata, MetadataLookup, SearchResult};
use super::fields::{TagField, TagFilter, TagFields};
use super::hints::{instrument_search_terms, instrument_terms_related};

pub const FILE_SEARCH_MIN_QUERY_LEN: usize = 2;
pub(crate) const FILE_SEARCH_DEBOUNCE_MS: u64 = 200;
pub const TAG_SEARCH_DEBOUNCE_MS: u64 = FILE_SEARCH_DEBOUNCE_MS;
pub(crate) const FILE_SEARCH_DEBOUNCE_MS_SHORT: u64 = 450;
const CONTAINS_NAME_MATCH_SCORE: i64 = 400;
const FILE_SEARCH_MIN_FUZZY_SCORE: i32 = 70;
pub(crate) const FILE_SEARCH_CONFIDENT_RESULT_CAP: usize = 2_000;
const FILE_SEARCH_MAX_RESULTS: usize = 10_000;

/// True when a search should run. Tag filters alone are enough; a filename
/// query needs two characters unless tag filters already narrowed the set.
pub fn file_search_active(file_query: &str, tag_filters: &[TagFilter]) -> bool {
    !tag_filters.is_empty() || file_query.trim().len() >= FILE_SEARCH_MIN_QUERY_LEN
}

pub fn file_search_debounce_ms(query_len: usize) -> u64 {
    if query_len <= FILE_SEARCH_MIN_QUERY_LEN {
        FILE_SEARCH_DEBOUNCE_MS_SHORT
    } else {
        FILE_SEARCH_DEBOUNCE_MS
    }
}

fn file_search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect()
}

fn text_contains(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    if case_sensitive {
        haystack.contains(needle)
    } else {
        haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    }
}

fn term_field_score(
    matcher: &SkimMatcherV2,
    text: &str,
    term: &str,
    case_sensitive: bool,
) -> i64 {
    if text_contains(text, term, case_sensitive) {
        return CONTAINS_NAME_MATCH_SCORE + term.len() as i64;
    }
    matcher
        .fuzzy_match(text, term)
        .filter(|&score| score >= FILE_SEARCH_MIN_FUZZY_SCORE as i64)
        .unwrap_or(0) as i64
}

fn path_name_fields(path: &Path) -> Vec<String> {
    let mut name_fields = Vec::new();
    if let Some(name) = crate::path_util::file_name_lossy(path) {
        name_fields.push(name);
    }
    if let Some(stem) = crate::path_util::file_stem_lossy(path) {
        if name_fields.last().is_none_or(|last| last != &stem) {
            name_fields.push(stem);
        }
    }
    name_fields
}

fn path_has_substring_match(
    path: &Path,
    query: &str,
    case_sensitive: bool,
    filename_only: bool,
) -> bool {
    let terms = file_search_terms(query);
    if terms.is_empty() {
        return false;
    }
    let name_fields = path_name_fields(path);
    let path_string = path.to_string_lossy();
    terms.iter().all(|term| {
        name_fields
            .iter()
            .any(|field| text_contains(field, term, case_sensitive))
            || (!filename_only && text_contains(&path_string, term, case_sensitive))
    })
}

fn is_confident_file_match(name_score: i64, path_score: i64) -> bool {
    name_score >= CONTAINS_NAME_MATCH_SCORE || path_score >= CONTAINS_NAME_MATCH_SCORE
}

pub(crate) fn path_match_scores(
    matcher: &SkimMatcherV2,
    path: &Path,
    query: &str,
    case_sensitive: bool,
    filename_only: bool,
) -> (i64, i64) {
    let terms = file_search_terms(query);
    if terms.is_empty() {
        return (0, 0);
    }

    let name_fields = path_name_fields(path);
    let path_string = path.to_string_lossy().into_owned();
    let mut name_score = 0i64;
    let mut path_score = 0i64;

    for term in &terms {
        let name_term_score = name_fields
            .iter()
            .map(|field| term_field_score(matcher, field, term, case_sensitive))
            .max()
            .unwrap_or(0);
        let path_term_score = if filename_only {
            0
        } else if text_contains(&path_string, term, case_sensitive) {
            CONTAINS_NAME_MATCH_SCORE + term.len() as i64
        } else {
            0
        };
        let term_score = if filename_only {
            name_term_score
        } else {
            name_term_score.max(path_term_score)
        };
        if term_score == 0 {
            return (0, 0);
        }
        name_score += name_term_score;
        path_score += path_term_score;
    }

    (name_score, path_score)
}

fn is_direct_name_match(name_score: i64) -> bool {
    name_score > 0
}

fn text_eq(a: &str, b: &str, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(b)
    }
}

fn text_starts_with(haystack: &str, needle: &str, case_sensitive: bool) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    if case_sensitive {
        haystack.starts_with(needle)
    } else {
        haystack[..needle.len()].eq_ignore_ascii_case(needle)
    }
}

const FILE_SEARCH_BONUS: i64 = 1_000;
const DIRECT_FOLDER_BONUS: i64 = 2_000;
const EXACT_STEM_BONUS: i64 = 50_000;
const PREFIX_STEM_BONUS: i64 = 10_000;

fn search_sort_score(
    path: &Path,
    query: &str,
    name_score: i64,
    path_score: i64,
    is_dir: bool,
    case_sensitive: bool,
) -> i64 {
    if is_dir {
        if is_direct_name_match(name_score) {
            name_score + DIRECT_FOLDER_BONUS
        } else {
            path_score
        }
    } else {
        let base = if name_score > 0 { name_score } else { path_score };
        let mut score = base + FILE_SEARCH_BONUS;
        if let Some(stem) = crate::path_util::file_stem_lossy(path) {
            if text_eq(&stem, query, case_sensitive) {
                score += EXACT_STEM_BONUS;
            } else if text_starts_with(&stem, query, case_sensitive) {
                score += PREFIX_STEM_BONUS;
            }
        }
        score
    }
}

#[derive(Eq, PartialEq)]
struct SearchRank {
    score: i64,
    path: PathBuf,
}

impl Ord for SearchRank {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .score
            .cmp(&self.score)
            .then_with(|| self.path.cmp(&other.path))
    }
}

impl PartialOrd for SearchRank {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl SearchRank {
    fn for_file_search(
        path: PathBuf,
        query: &str,
        name_score: i64,
        path_score: i64,
        case_sensitive: bool,
    ) -> Self {
        let is_dir = !is_audio(&path);
        Self {
            score: search_sort_score(&path, query, name_score, path_score, is_dir, case_sensitive),
            path,
        }
    }

    fn for_tag_search(path: PathBuf, tag_score: i64) -> Self {
        Self { score: tag_score, path }
    }

    fn for_combined_search(
        path: PathBuf,
        query: &str,
        name_score: i64,
        path_score: i64,
        tag_score: i64,
        case_sensitive: bool,
    ) -> Self {
        let file_score =
            search_sort_score(&path, query, name_score, path_score, !is_audio(&path), case_sensitive);
        Self {
            score: file_score.saturating_add(tag_score.saturating_mul(100)),
            path,
        }
    }
}

pub(crate) fn file_search_matcher(case_sensitive: bool) -> SkimMatcherV2 {
    if case_sensitive {
        SkimMatcherV2::default().respect_case()
    } else {
        SkimMatcherV2::default().ignore_case()
    }
}

pub(crate) fn tag_search_matcher() -> SkimMatcherV2 {
    file_search_matcher(false)
}

fn min_term_scores(
    terms: &[String],
    mut score_term: impl FnMut(&str) -> Option<i64>,
) -> Option<i64> {
    if terms.is_empty() {
        return None;
    }
    let mut min = i64::MAX;
    for term in terms {
        min = min.min(score_term(term)?);
    }
    Some(min)
}

fn instrument_field_score(
    matcher: &SkimMatcherV2,
    value: &str,
    query: &str,
) -> Option<i64> {
    min_term_scores(&file_search_terms(query), |term| {
        instrument_term_score(matcher, value, term)
    })
}

fn instrument_term_score(matcher: &SkimMatcherV2, value: &str, term: &str) -> Option<i64> {
    let direct = term_field_score(matcher, value, term, false);
    if direct > 0 {
        return Some(direct);
    }
    if instrument_terms_related(term, value) {
        return Some(80 + term.len() as i64);
    }
    instrument_search_terms(term)
        .iter()
        .filter_map(|alias| {
            let score = term_field_score(matcher, value, alias, false);
            if score > 0 {
                Some(score)
            } else if instrument_terms_related(alias, value) {
                Some(80 + alias.len() as i64)
            } else {
                None
            }
        })
        .max()
}

fn tag_value_score(matcher: &SkimMatcherV2, value: &str, query: &str) -> Option<i64> {
    min_term_scores(&file_search_terms(query), |term| {
        let score = term_field_score(matcher, value, term, false);
        (score > 0).then_some(score)
    })
}

pub(crate) fn tag_field_score(
    matcher: &SkimMatcherV2,
    fields: &TagFields,
    filter: &TagFilter,
) -> Option<i64> {
    let value = fields.field_value(filter.field);
    if value.is_empty() {
        None
    } else if filter.field == TagField::Instrument {
        instrument_field_score(matcher, value, &filter.value)
    } else {
        tag_value_score(matcher, value, &filter.value)
    }
}

fn tag_match_score(
    matcher: &SkimMatcherV2,
    path: &Path,
    filters: &[TagFilter],
    lookup: &mut MetadataLookup,
    allow_disk_read: bool,
) -> i64 {
    if filters.is_empty() || !is_audio(path) {
        return 0;
    }
    let fields = lookup.tag_fields_for_search(path, allow_disk_read);
    let mut scores = Vec::with_capacity(filters.len());
    for filter in filters {
        let Some(score) = tag_field_score(matcher, &fields, filter) else {
            return 0;
        };
        scores.push(score);
    }
    scores.into_iter().min().unwrap_or(0)
}

pub(crate) fn collect_tag_matches(
    paths: &[PathBuf],
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
    allow_disk_read: bool,
) -> SearchResult {
    let tag_matcher = tag_search_matcher();
    let mut lookup = MetadataLookup::new(metadata);
    let mut matches = Vec::new();

    for path in paths.iter().filter(|path| is_audio(path)) {
        let tag_score = tag_match_score(
            &tag_matcher,
            path,
            tag_filters,
            &mut lookup,
            allow_disk_read,
        );
        if tag_score > 0 {
            matches.push(SearchRank::for_tag_search(path.clone(), tag_score));
        }
    }

    matches.sort();
    let paths = matches.into_iter().map(|entry| entry.path).collect();
    SearchResult {
        paths,
        new_metadata: lookup.into_new_entries(),
        cached_roots: HashMap::new(),
    }
}

#[cfg(test)]
pub fn tag_search_paths(
    paths: &[PathBuf],
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    collect_tag_matches(paths, tag_filters, metadata, true)
}

/// Tag-only search over the metadata index. Unindexed files are skipped so a
/// library-wide `instrument:` query does not re-parse every audio file.
pub fn tag_search_cached_paths(
    paths: &[PathBuf],
    tag_filters: &[TagFilter],
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    collect_tag_matches(paths, tag_filters, metadata, false)
}

pub fn search_paths(
    paths: &[PathBuf],
    file_query: &str,
    tag_filters: &[TagFilter],
    case_sensitive: bool,
    show_directories: bool,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    let trimmed = file_query.trim();
    let tag_active = !tag_filters.is_empty();
    if tag_active && trimmed.is_empty() {
        return collect_tag_matches(paths, tag_filters, metadata, true);
    }

    collect_file_matches(
        paths,
        file_query,
        tag_filters,
        case_sensitive,
        show_directories,
        metadata,
    )
}

pub(crate) fn collect_file_matches(
    paths: &[PathBuf],
    file_query: &str,
    tag_filters: &[TagFilter],
    case_sensitive: bool,
    show_directories: bool,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> SearchResult {
    let file_matcher = file_search_matcher(case_sensitive);
    let tag_matcher = tag_search_matcher();
    let tag_active = !tag_filters.is_empty();
    let filename_only =
        tag_active && file_query.trim().len() < FILE_SEARCH_MIN_QUERY_LEN;
    let mut lookup = MetadataLookup::new(metadata);
    let mut matches = Vec::new();

    for path in paths {
        if !show_directories && !is_audio(path) {
            continue;
        }

        if matches.len() >= FILE_SEARCH_CONFIDENT_RESULT_CAP
            && !path_has_substring_match(path, file_query, case_sensitive, filename_only)
        {
            continue;
        }

        let (name_score, path_score) = path_match_scores(
            &file_matcher,
            path,
            file_query,
            case_sensitive,
            filename_only,
        );

        if name_score == 0 && path_score == 0 {
            continue;
        }

        if matches.len() >= FILE_SEARCH_CONFIDENT_RESULT_CAP
            && !is_confident_file_match(name_score, path_score)
        {
            continue;
        }

        let tag_score = if tag_active {
            tag_match_score(&tag_matcher, path, tag_filters, &mut lookup, true)
        } else {
            0
        };

        if tag_active && tag_score == 0 {
            continue;
        }

        let rank = if tag_active {
            SearchRank::for_combined_search(
                path.clone(),
                file_query,
                name_score,
                path_score,
                tag_score,
                case_sensitive,
            )
        } else {
            SearchRank::for_file_search(
                path.clone(),
                file_query,
                name_score,
                path_score,
                case_sensitive,
            )
        };
        matches.push(rank);
    }

    matches.sort();
    if matches.len() > FILE_SEARCH_MAX_RESULTS {
        matches.truncate(FILE_SEARCH_MAX_RESULTS);
    }
    let paths = matches.into_iter().map(|entry| entry.path).collect();
    SearchResult {
        paths,
        new_metadata: lookup.into_new_entries(),
        cached_roots: HashMap::new(),
    }
}
