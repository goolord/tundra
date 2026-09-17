//! Bulk auto-tagging: walk a folder, classify every file that needs an
//! instrument, group the proposals by directory for review, then write the
//! accepted ones. Runs on background threads; the UI polls `BulkScanProgress`.

use crate::auto_tag::{self, ClassificationResult, ClassifyError};
use crate::metadata::{
    auto_tag_field_status, auto_tag_field_status_from_fields, index_paths, instrument_tag, is_audio,
    write_auto_tags, AutoTagFieldStatus, CachedMetadata,
};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use walkdir::WalkDir;

const SCAN_YIELD_INTERVAL: usize = 64;
/// Scans bigger than either limit start with their folders collapsed.
const COLLAPSE_DIR_THRESHOLD: usize = 6;
const COLLAPSE_FILE_THRESHOLD: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkPhase {
    Scanning,
    Classifying,
    Applying,
}

/// A scan that ended without a summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanError {
    Cancelled,
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
pub struct BulkProgressSnapshot {
    pub phase: BulkPhase,
    pub done: usize,
    /// Zero while scanning, when the total is not known yet.
    pub total: usize,
}

impl BulkProgressSnapshot {
    pub fn label(&self) -> &'static str {
        match self.phase {
            BulkPhase::Scanning => "Scanning folder…",
            BulkPhase::Classifying => "Analyzing files…",
            BulkPhase::Applying => "Writing tags…",
        }
    }

    pub fn fraction(&self) -> f32 {
        if self.phase == BulkPhase::Scanning {
            // Unknown total during the walk: creep toward 90%.
            return if self.done == 0 { 0.0 } else { (1.0 - 1.0 / (self.done as f32 * 0.08 + 1.0)).min(0.90) };
        }
        if self.total == 0 { 0.0 } else { (self.done as f32 / self.total as f32).clamp(0.0, 1.0) }
    }

    pub fn detail(&self) -> String {
        if self.total > 0 {
            format!("{} / {}", self.done, self.total)
        } else if self.phase == BulkPhase::Scanning && self.done > 0 {
            format!("{} audio files checked", self.done)
        } else {
            "Starting…".into()
        }
    }
}

/// Progress shared between a bulk job and the UI polling it.
#[derive(Debug)]
pub struct BulkScanProgress {
    phase: AtomicU8,
    done: AtomicUsize,
    total: AtomicUsize,
}

impl BulkScanProgress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            phase: AtomicU8::new(BulkPhase::Scanning as u8),
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(0),
        })
    }

    /// Starts `phase` with nothing done out of `total`.
    pub fn begin(&self, phase: BulkPhase, total: usize) {
        self.phase.store(phase as u8, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    pub fn advance(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> BulkProgressSnapshot {
        let phase = match self.phase.load(Ordering::Relaxed) {
            1 => BulkPhase::Classifying,
            2 => BulkPhase::Applying,
            _ => BulkPhase::Scanning,
        };
        BulkProgressSnapshot {
            phase,
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BulkFileProposal {
    pub path: PathBuf,
    pub suggested: Option<String>,
    pub confidence: Option<f64>,
    /// Checked for Apply.
    pub accepted: bool,
    pub error: Option<String>,
}

impl BulkFileProposal {
    /// Has a suggestion that Apply could write.
    pub fn is_actionable(&self) -> bool {
        self.suggested.is_some() && self.error.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct BulkDirGroup {
    pub path: PathBuf,
    pub files: Vec<BulkFileProposal>,
    pub expanded: bool,
}

impl BulkDirGroup {
    pub fn actionable_count(&self) -> usize {
        self.files.iter().filter(|file| file.is_actionable()).count()
    }

    pub fn accepted_count(&self) -> usize {
        self.files.iter().filter(|file| file.accepted && file.is_actionable()).count()
    }
}

#[derive(Debug, Clone)]
pub struct BulkScanSummary {
    pub root: PathBuf,
    pub groups: Vec<BulkDirGroup>,
    pub skipped_complete: usize,
    pub failed: usize,
}

impl BulkScanSummary {
    /// Nothing to review: every file was already tagged.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.failed == 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct BulkApplySummary {
    pub written: usize,
    pub unchanged: usize,
    /// Fresh index entries for written files, read on the apply thread so the
    /// UI merges them in one step.
    pub refreshed: HashMap<PathBuf, CachedMetadata>,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

#[derive(Debug, Clone)]
pub struct BulkApplyItem {
    pub path: PathBuf,
    pub instrument: String,
}

fn auto_tag_status(path: &Path, metadata: &HashMap<PathBuf, CachedMetadata>) -> Option<AutoTagFieldStatus> {
    metadata
        .get(path)
        .map(|cached| auto_tag_field_status_from_fields(path, &cached.fields))
        .or_else(|| auto_tag_field_status(path))
}

fn existing_instrument_label(path: &Path, metadata: &HashMap<PathBuf, CachedMetadata>) -> Result<String, ClassifyError> {
    let cached = metadata
        .get(path)
        .map(|cached| cached.fields.explicit_instrument.trim())
        .filter(|label| !label.is_empty());
    cached.map(str::to_string).or_else(|| instrument_tag(path)).ok_or_else(|| {
        ClassifyError::new(
            "Could not read existing instrument tag.",
            "Metadata-only auto tag requires an instrument label in the file.",
        )
    })
}

/// Files that need classifying, files whose instrument is set but that miss
/// other auto tags, and how many need nothing.
fn partition_auto_tag_candidates(
    paths: &[PathBuf],
    metadata: &HashMap<PathBuf, CachedMetadata>,
) -> (Vec<PathBuf>, Vec<PathBuf>, usize) {
    let mut to_classify = Vec::new();
    let mut metadata_only = Vec::new();
    let mut skipped_complete = 0;
    for path in paths {
        match auto_tag_status(path, metadata) {
            Some(status) if !status.allows_instrument_work() && status.needs_any() => metadata_only.push(path.clone()),
            Some(status) if !status.allows_instrument_work() => skipped_complete += 1,
            _ => to_classify.push(path.clone()),
        }
    }
    (to_classify, metadata_only, skipped_complete)
}

/// `snapshot` plus freshly read tags for any path it lacks.
fn enrich_metadata(
    paths: &[PathBuf],
    snapshot: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> Arc<HashMap<PathBuf, CachedMetadata>> {
    let missing: Vec<PathBuf> = paths.iter().filter(|path| !snapshot.contains_key(*path)).cloned().collect();
    if missing.is_empty() {
        return snapshot;
    }
    let mut merged = (*snapshot).clone();
    merged.extend(index_paths(&missing, Arc::clone(&snapshot)));
    Arc::new(merged)
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), ScanError> {
    if cancel.load(Ordering::Relaxed) { Err(ScanError::Cancelled) } else { Ok(()) }
}

fn is_link_or_reparse(path: &Path) -> bool {
    if path.is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            return meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
        }
    }
    false
}

/// Every audio file under `root`, sorted. Never follows links or junctions,
/// so a bulk write cannot escape the folder the user picked.
fn collect_audio_paths(root: &Path, progress: &BulkScanProgress, cancel: &AtomicBool) -> Result<Vec<PathBuf>, ScanError> {
    let mut paths = Vec::new();
    let mut sweep = crate::safe_write::SidecarSweep::default();
    let walk = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_link_or_reparse(entry.path()))
        .filter_map(Result::ok);
    for entry in walk {
        check_cancel(cancel)?;
        sweep.note(entry.path());
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if is_link_or_reparse(&path) || !is_audio(&path) {
            continue;
        }
        paths.push(path);
        progress.advance();
        if paths.len().is_multiple_of(SCAN_YIELD_INTERVAL) {
            std::thread::yield_now();
        }
    }
    paths.extend(sweep.finish().into_iter().filter(|path| is_audio(path)));
    paths.sort();
    Ok(paths)
}

type Classified = Vec<(PathBuf, Result<ClassificationResult, ClassifyError>)>;

fn classify_files(paths: Vec<PathBuf>, progress: &BulkScanProgress, cancel: &AtomicBool) -> Result<Classified, ScanError> {
    progress.begin(BulkPhase::Classifying, paths.len());
    // Tier 1 runs in Rust on every core; tier-2 requests queue for the few
    // Python workers inside the classifier pool.
    let results = paths
        .into_par_iter()
        .map(|path| {
            // After a cancel the remaining files are skipped, not analysed.
            let result = match check_cancel(cancel) {
                Ok(()) => auto_tag::classify_file_bulk(&path),
                Err(_) => Err(ClassifyError::new("Scan cancelled.", "Bulk classify interrupted")),
            };
            progress.advance();
            (path, result)
        })
        .collect();
    check_cancel(cancel)?;
    Ok(results)
}

pub fn scan_and_classify(
    root: PathBuf,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
    progress: Arc<BulkScanProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<BulkScanSummary, ScanError> {
    check_cancel(&cancel)?;
    // Load the models while the folder is walked. A missing Python setup only
    // fails the grey-zone files that need it; tier 1 still tags the rest.
    std::thread::spawn(|| {
        if let Err(err) = auto_tag::warm_classifier_pool() {
            eprintln!("{}: {}", err.message, err.details);
        }
    });
    progress.begin(BulkPhase::Scanning, 0);
    let audio_paths = collect_audio_paths(&root, &progress, &cancel)?;
    check_cancel(&cancel)?;
    let metadata = enrich_metadata(&audio_paths, metadata);
    let (to_classify, metadata_only, skipped_complete) = partition_auto_tag_candidates(&audio_paths, &metadata);
    let classified = classify_files(to_classify, &progress, &cancel);
    // Keep what was analysed even when the scan was cancelled part-way.
    auto_tag::flush_classify_cache();
    let mut results = classified?;
    for path in metadata_only {
        check_cancel(&cancel)?;
        let result = existing_instrument_label(&path, &metadata).map(|instrument| ClassificationResult {
            instrument,
            tier: 0,
            zcr: None,
            confidence: None,
            summary: "Existing instrument tag".into(),
        });
        results.push((path, result));
    }
    Ok(build_scan_summary(root, skipped_complete, results))
}

fn build_scan_summary(root: PathBuf, skipped_complete: usize, results: Classified) -> BulkScanSummary {
    let mut grouped: BTreeMap<PathBuf, Vec<BulkFileProposal>> = BTreeMap::new();
    let mut failed = 0;

    for (path, result) in results {
        let (suggested, confidence, error) = match result {
            Ok(classification) => (Some(classification.instrument), classification.confidence, None),
            Err(err) => {
                failed += 1;
                (None, None, Some(err.message))
            }
        };
        // Pre-check only suggestions worth trusting; a low-confidence guess
        // must be opted into before Apply writes it permanently.
        let accepted = suggested.is_some()
            && confidence.is_none_or(|confidence| confidence >= auto_tag::MEDIUM_CLASSIFIER_CONFIDENCE);
        let parent = path.parent().map_or_else(|| root.clone(), Path::to_path_buf);
        grouped.entry(parent).or_default().push(BulkFileProposal {
            path,
            suggested,
            confidence,
            accepted,
            error,
        });
    }

    let file_count: usize = grouped.values().map(Vec::len).sum();
    let expanded = grouped.len() <= COLLAPSE_DIR_THRESHOLD && file_count <= COLLAPSE_FILE_THRESHOLD;
    let groups = grouped
        .into_iter()
        .map(|(path, mut files)| {
            files.sort_by(|a, b| a.path.cmp(&b.path));
            BulkDirGroup { path, files, expanded }
        })
        .collect();

    BulkScanSummary {
        root,
        groups,
        skipped_complete,
        failed,
    }
}

/// The checked proposals, ready to write.
pub fn collect_accepted(groups: &[BulkDirGroup]) -> Vec<BulkApplyItem> {
    groups
        .iter()
        .flat_map(|group| &group.files)
        .filter(|file| file.accepted && file.error.is_none())
        .filter_map(|file| {
            Some(BulkApplyItem {
                path: file.path.clone(),
                instrument: file.suggested.clone()?,
            })
        })
        .collect()
}

/// Writes each item's tags, stopping early once `cancel` is set.
pub fn apply_items(items: &[BulkApplyItem], progress: Option<&BulkScanProgress>, cancel: &AtomicBool) -> BulkApplySummary {
    let mut summary = BulkApplySummary::default();
    if let Some(progress) = progress {
        progress.begin(BulkPhase::Applying, items.len());
    }
    for item in items {
        if check_cancel(cancel).is_err() {
            summary.cancelled = true;
            break;
        }
        match write_auto_tags(&item.path, &item.instrument) {
            Ok(true) => {
                summary.written += 1;
                if let Some(entry) = crate::metadata::refresh_cached_metadata(&item.path) {
                    summary.refreshed.insert(crate::path_util::cache_key(&item.path), entry);
                }
            }
            Ok(false) => summary.unchanged += 1,
            Err(err) => summary.failed.push((item.path.clone(), err)),
        }
        if let Some(progress) = progress {
            progress.advance();
            std::thread::yield_now();
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{search, MetadataLookup, SearchQuery, TagField, TagFilter};
    use crate::test_fixtures::{copy_asset, write_riff_info, ScratchDir, ASSET_FORMATS};

    /// A `Kicks` folder holding one untagged file per format.
    fn kick_folder(label: &str) -> (ScratchDir, Vec<PathBuf>) {
        let root = ScratchDir::new(label);
        let kicks = root.path().join("Kicks");
        std::fs::create_dir_all(&kicks).expect("create scratch dirs");
        let paths = ASSET_FORMATS.iter().map(|ext| copy_asset(&kicks, "sample", ext)).collect();
        (root, paths)
    }

    /// Mirrors the app: index the files, then search the index.
    fn instrument_hits(paths: &[PathBuf], value: &str) -> usize {
        let indexed = Arc::new(index_paths(paths, Arc::new(HashMap::new())));
        let filter = TagFilter {
            field: TagField::Instrument,
            value: value.to_string(),
        };
        let query = SearchQuery {
            tag_filters: &[filter],
            ..SearchQuery::default()
        };
        search(paths, &query, MetadataLookup::new(indexed)).paths.len()
    }

    /// The user-facing contract for the bulk tagger: every untagged file of
    /// every format is proposed, applying writes them all, and the result is
    /// what an `instrument:Kick` search returns.
    #[test]
    fn bulk_apply_makes_every_format_findable_by_instrument() {
        let (_root, paths) = kick_folder("bulk-apply");
        let metadata = HashMap::new();

        let (to_classify, metadata_only, skipped_complete) = partition_auto_tag_candidates(&paths, &metadata);
        assert_eq!(to_classify.len(), ASSET_FORMATS.len(), "every untagged file should be queued for classification");
        assert!(metadata_only.is_empty(), "nothing is already tagged yet");
        assert_eq!(skipped_complete, 0);
        assert_eq!(instrument_hits(&paths, "Kick"), 0, "nothing should match before tagging");

        let items: Vec<_> = paths
            .iter()
            .map(|path| BulkApplyItem {
                path: path.clone(),
                instrument: "Kick".to_string(),
            })
            .collect();
        let progress = BulkScanProgress::new();
        let summary = apply_items(&items, Some(&progress), &AtomicBool::new(false));

        assert_eq!(summary.failed, Vec::new(), "no format should fail to tag");
        assert_eq!(summary.written, ASSET_FORMATS.len());
        assert_eq!(summary.unchanged, 0);
        assert!(!summary.cancelled);
        assert_eq!(progress.snapshot().detail(), format!("{0} / {0}", ASSET_FORMATS.len()));
        assert_eq!(instrument_hits(&paths, "Kick"), ASSET_FORMATS.len(), "instrument:Kick must return every tagged file");

        // Same tag version: bulk scan should skip already-tagged files.
        let (to_classify, metadata_only, skipped_complete) = partition_auto_tag_candidates(&paths, &metadata);
        assert!(to_classify.is_empty(), "current-version tags should not be re-classified");
        assert!(metadata_only.is_empty());
        assert_eq!(skipped_complete, ASSET_FORMATS.len());
        assert_eq!(apply_items(&items, None, &AtomicBool::new(false)).written, 0);
    }

    #[test]
    fn cancelled_apply_writes_nothing_and_says_so() {
        let (_root, paths) = kick_folder("bulk-cancel");
        let items = vec![BulkApplyItem {
            path: paths[0].clone(),
            instrument: "Kick".into(),
        }];
        let summary = apply_items(&items, None, &AtomicBool::new(true));
        assert!(summary.cancelled);
        assert_eq!(summary.written, 0);
    }

    #[test]
    fn low_confidence_suggestions_start_unchecked_and_errors_count_as_failed() {
        let guess = |confidence| {
            Ok(ClassificationResult {
                instrument: "Kick".into(),
                tier: 2,
                zcr: None,
                confidence: Some(confidence),
                summary: String::new(),
            })
        };
        let root = PathBuf::from("/samples");
        let summary = build_scan_summary(
            root.clone(),
            3,
            vec![
                (root.join("a/sure.wav"), guess(0.99)),
                (root.join("a/unsure.wav"), guess(0.01)),
                (root.join("b/broken.wav"), Err(ClassifyError::new("Nope", ""))),
            ],
        );
        assert_eq!((summary.groups.len(), summary.failed, summary.skipped_complete), (2, 1, 3));
        let accepted: Vec<bool> = summary.groups[0].files.iter().map(|file| file.accepted).collect();
        assert_eq!(accepted, [true, false]);
        assert_eq!(summary.groups[0].actionable_count(), 2);
        assert_eq!(summary.groups[1].actionable_count(), 0);
        assert_eq!(collect_accepted(&summary.groups).len(), 1);
    }

    #[test]
    fn legacy_tundra_comment_is_queued_for_reclassify() {
        let (_root, paths) = kick_folder("bulk-legacy");
        let audio = paths.iter().find(|path| path.extension().is_some_and(|ext| ext == "wav")).cloned().expect("wav fixture");
        write_riff_info(&audio, &[("IKEY", "Snare"), ("ICMT", "Tundra")]);

        let (to_classify, metadata_only, skipped_complete) =
            partition_auto_tag_candidates(std::slice::from_ref(&audio), &HashMap::new());
        assert_eq!(to_classify, vec![audio.clone()]);
        assert!(metadata_only.is_empty(), "legacy Tundra v0 must reclassify, not stamp v1 over the old instrument");
        assert_eq!(skipped_complete, 0);
    }
}
