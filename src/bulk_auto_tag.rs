use crate::auto_tag::{self, ClassificationResult, ClassifyError};
use crate::metadata::{index_paths, write_instrument_tag_if_untagged, CachedMetadata};
use crate::types::is_audio;
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};
use walkdir::WalkDir;

const SCAN_YIELD_INTERVAL: usize = 64;
const PHASE_SCAN: u8 = 0;
const PHASE_CLASSIFY: u8 = 1;
const PHASE_APPLY: u8 = 2;

static CLASSIFIER_POOL: LazyLock<rayon::ThreadPool> = LazyLock::new(|| {
    ThreadPoolBuilder::new()
        .num_threads(auto_tag::classifier_worker_count())
        .build()
        .expect("classifier thread pool")
});

#[derive(Debug, Clone, Copy)]
pub struct BulkProgressSnapshot {
    pub phase: u8,
    pub scan_done: usize,
    pub classify_done: usize,
    pub classify_total: usize,
    pub apply_done: usize,
    pub apply_total: usize,
}

impl BulkProgressSnapshot {
    pub fn label(&self) -> &'static str {
        match self.phase {
            PHASE_CLASSIFY => "Analyzing files…",
            PHASE_APPLY => "Writing tags…",
            _ => "Scanning folder…",
        }
    }

    pub fn done(&self) -> usize {
        match self.phase {
            PHASE_CLASSIFY => self.classify_done,
            PHASE_APPLY => self.apply_done,
            _ => self.scan_done,
        }
    }

    pub fn total(&self) -> usize {
        match self.phase {
            PHASE_CLASSIFY => self.classify_total,
            PHASE_APPLY => self.apply_total,
            _ => 0,
        }
    }

    pub fn fraction(&self) -> f32 {
        match self.phase {
            PHASE_SCAN => {
                if self.scan_done == 0 {
                    0.0
                } else {
                    // Unknown total during directory walk — asymptotic progress toward 90%.
                    (1.0 - 1.0 / (self.scan_done as f32 * 0.08 + 1.0)).min(0.90)
                }
            }
            _ => {
                let total = self.total();
                if total == 0 {
                    0.0
                } else {
                    (self.done() as f32 / total as f32).clamp(0.0, 1.0)
                }
            }
        }
    }

    pub fn detail(&self) -> String {
        let total = self.total();
        let done = self.done();
        if total > 0 {
            format!("{done} / {total}")
        } else if self.phase == PHASE_SCAN && done > 0 {
            format!("{done} audio files checked")
        } else {
            String::from("Starting…")
        }
    }
}

#[derive(Debug)]
pub struct BulkScanProgress {
    phase: AtomicU8,
    scan_done: AtomicUsize,
    classify_done: AtomicUsize,
    classify_total: AtomicUsize,
    apply_done: AtomicUsize,
    apply_total: AtomicUsize,
}

impl BulkScanProgress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            phase: AtomicU8::new(PHASE_SCAN),
            scan_done: AtomicUsize::new(0),
            classify_done: AtomicUsize::new(0),
            classify_total: AtomicUsize::new(0),
            apply_done: AtomicUsize::new(0),
            apply_total: AtomicUsize::new(0),
        })
    }

    pub fn set_scanning(&self) {
        self.phase.store(PHASE_SCAN, Ordering::Relaxed);
        self.scan_done.store(0, Ordering::Relaxed);
        self.classify_done.store(0, Ordering::Relaxed);
        self.classify_total.store(0, Ordering::Relaxed);
        self.apply_done.store(0, Ordering::Relaxed);
        self.apply_total.store(0, Ordering::Relaxed);
    }

    pub fn set_classifying(&self, total: usize) {
        self.phase.store(PHASE_CLASSIFY, Ordering::Relaxed);
        self.classify_done.store(0, Ordering::Relaxed);
        self.classify_total.store(total, Ordering::Relaxed);
    }

    pub fn set_applying(&self, total: usize) {
        self.phase.store(PHASE_APPLY, Ordering::Relaxed);
        self.apply_done.store(0, Ordering::Relaxed);
        self.apply_total.store(total, Ordering::Relaxed);
    }

    pub fn inc_scan(&self) {
        self.scan_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_classify(&self) {
        self.classify_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inc_apply(&self) {
        self.apply_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> BulkProgressSnapshot {
        BulkProgressSnapshot {
            phase: self.phase.load(Ordering::Relaxed),
            scan_done: self.scan_done.load(Ordering::Relaxed),
            classify_done: self.classify_done.load(Ordering::Relaxed),
            classify_total: self.classify_total.load(Ordering::Relaxed),
            apply_done: self.apply_done.load(Ordering::Relaxed),
            apply_total: self.apply_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BulkFileProposal {
    pub path: PathBuf,
    pub suggested: Option<String>,
    pub confidence: Option<f64>,
    pub accepted: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BulkDirGroup {
    pub path: PathBuf,
    pub files: Vec<BulkFileProposal>,
    pub expanded: bool,
}

impl BulkDirGroup {
    pub fn actionable_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.suggested.is_some() && file.error.is_none())
            .count()
    }

    pub fn accepted_count(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.accepted && file.suggested.is_some() && file.error.is_none())
            .count()
    }

    pub fn set_all_accepted(&mut self, accepted: bool) {
        for file in &mut self.files {
            if file.suggested.is_some() && file.error.is_none() {
                file.accepted = accepted;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct BulkScanSummary {
    pub root: PathBuf,
    pub groups: Vec<BulkDirGroup>,
    pub skipped_tagged: usize,
    pub failed: usize,
}

pub const COLLAPSE_DIR_THRESHOLD: usize = 6;
pub const COLLAPSE_FILE_THRESHOLD: usize = 40;

pub fn groups_start_collapsed(dir_count: usize, file_count: usize) -> bool {
    dir_count > COLLAPSE_DIR_THRESHOLD || file_count > COLLAPSE_FILE_THRESHOLD
}

#[derive(Debug, Clone)]
pub struct BulkApplySummary {
    pub applied: usize,
    pub applied_paths: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

fn has_explicit_instrument(path: &Path, metadata: &HashMap<PathBuf, CachedMetadata>) -> bool {
    metadata
        .get(path)
        .is_some_and(|cached| !cached.fields.explicit_instrument.is_empty())
}

fn enrich_metadata(
    paths: &[PathBuf],
    snapshot: Arc<HashMap<PathBuf, CachedMetadata>>,
) -> Arc<HashMap<PathBuf, CachedMetadata>> {
    let missing: Vec<PathBuf> = paths
        .iter()
        .filter(|path| !snapshot.contains_key(*path))
        .cloned()
        .collect();
    if missing.is_empty() {
        return snapshot;
    }
    let mut merged = (*snapshot).clone();
    merged.extend(index_paths(&missing, Arc::clone(&snapshot)));
    Arc::new(merged)
}

fn scan_cancelled(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

fn should_skip_walk_entry(entry: &walkdir::DirEntry) -> bool {
    entry.depth() > 0 && is_link_or_reparse(entry.path())
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

fn collect_audio_paths(
    root: &Path,
    progress: Option<&BulkScanProgress>,
    cancel: &AtomicBool,
) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    let mut seen = 0usize;

    crate::path_util::reclaim_write_sidecars_tree(root);

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !should_skip_walk_entry(entry))
        .filter_map(|entry| entry.ok())
    {
        if scan_cancelled(cancel) {
            return Err("Scan cancelled.".into());
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        if is_link_or_reparse(&path) || !is_audio(&path) {
            continue;
        }
        seen += 1;
        if let Some(progress) = progress {
            progress.inc_scan();
            if seen % SCAN_YIELD_INTERVAL == 0 {
                std::thread::yield_now();
            }
        }
        paths.push(path);
    }

    paths.sort();
    Ok(paths)
}

fn partition_untagged(
    paths: &[PathBuf],
    metadata: &HashMap<PathBuf, CachedMetadata>,
) -> (Vec<PathBuf>, usize) {
    let mut untagged = Vec::new();
    let mut skipped_tagged = 0usize;

    for path in paths {
        if has_explicit_instrument(path, metadata) {
            skipped_tagged += 1;
        } else {
            untagged.push(path.clone());
        }
    }

    (untagged, skipped_tagged)
}

pub fn classify_files_with_progress(
    paths: Vec<PathBuf>,
    progress: &BulkScanProgress,
    cancel: &AtomicBool,
) -> Result<Vec<(PathBuf, Result<ClassificationResult, ClassifyError>)>, String> {
    progress.set_classifying(paths.len());

    let results = CLASSIFIER_POOL.install(|| {
        paths
            .into_par_iter()
            .map(|path| {
                if scan_cancelled(cancel) {
                    return (
                        path,
                        Err(ClassifyError::new(
                            "Scan cancelled.",
                            "Bulk classify interrupted",
                        )),
                    );
                }
                let result = auto_tag::classify_file_bulk(&path);
                if scan_cancelled(cancel) {
                    return (
                        path,
                        Err(ClassifyError::new(
                            "Scan cancelled.",
                            "Bulk classify interrupted",
                        )),
                    );
                }
                progress.inc_classify();
                std::thread::yield_now();
                (path, result)
            })
            .collect::<Vec<_>>()
    });

    if scan_cancelled(cancel) {
        return Err("Scan cancelled.".into());
    }
    Ok(results)
}

pub fn scan_and_classify(
    root: PathBuf,
    metadata: Arc<HashMap<PathBuf, CachedMetadata>>,
    progress: Arc<BulkScanProgress>,
    cancel: Arc<AtomicBool>,
) -> Result<BulkScanSummary, String> {
    if scan_cancelled(&cancel) {
        return Err("Scan cancelled.".into());
    }
    auto_tag::warm_classifier_pool().map_err(|err| err.message)?;
    progress.set_scanning();
    let audio_paths = collect_audio_paths(&root, Some(&progress), &cancel)?;
    if scan_cancelled(&cancel) {
        return Err("Scan cancelled.".into());
    }
    let metadata_map = enrich_metadata(&audio_paths, metadata);
    let (paths, skipped_tagged) = partition_untagged(&audio_paths, metadata_map.as_ref());
    let results = if paths.is_empty() {
        Vec::new()
    } else {
        classify_files_with_progress(paths, &progress, &cancel)?
    };
    auto_tag::flush_classify_cache();
    Ok(build_scan_summary(root, skipped_tagged, results))
}

pub fn build_scan_summary(
    root: PathBuf,
    skipped_tagged: usize,
    results: Vec<(PathBuf, Result<ClassificationResult, ClassifyError>)>,
) -> BulkScanSummary {
    let mut grouped: BTreeMap<PathBuf, Vec<BulkFileProposal>> = BTreeMap::new();
    let mut failed = 0usize;

    for (path, result) in results {
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.clone());

        let proposal = match result {
            Ok(classification) => BulkFileProposal {
                path: path.clone(),
                suggested: Some(classification.instrument),
                confidence: classification.confidence,
                accepted: true,
                error: None,
            },
            Err(err) => {
                failed += 1;
                BulkFileProposal {
                    path: path.clone(),
                    suggested: None,
                    confidence: None,
                    accepted: false,
                    error: Some(err.message),
                }
            }
        };

        grouped.entry(parent).or_default().push(proposal);
    }

    for files in grouped.values_mut() {
        files.sort_by(|a, b| a.path.cmp(&b.path));
    }

    let dir_count = grouped.len();
    let file_count: usize = grouped.values().map(|files| files.len()).sum();
    let start_expanded = !groups_start_collapsed(dir_count, file_count);

    let groups = grouped
        .into_iter()
        .map(|(path, files)| BulkDirGroup {
            path,
            files,
            expanded: start_expanded,
        })
        .collect();

    BulkScanSummary {
        root,
        groups,
        skipped_tagged,
        failed,
    }
}

#[derive(Debug, Clone)]
pub struct BulkApplyItem {
    pub path: PathBuf,
    pub instrument: String,
}

pub fn collect_accepted(groups: &[BulkDirGroup]) -> Vec<BulkApplyItem> {
    groups
        .iter()
        .flat_map(|group| {
            group.files.iter().filter_map(|file| {
                if !file.accepted || file.error.is_some() {
                    return None;
                }
                Some(BulkApplyItem {
                    path: file.path.clone(),
                    instrument: file.suggested.clone()?,
                })
            })
        })
        .collect()
}

pub fn apply_items(
    items: &[BulkApplyItem],
    progress: Option<&BulkScanProgress>,
    cancel: &AtomicBool,
) -> BulkApplySummary {
    let mut applied = 0usize;
    let mut applied_paths = Vec::new();
    let mut failed = Vec::new();
    let mut cancelled = false;

    for item in items {
        if scan_cancelled(cancel) {
            cancelled = true;
            break;
        }
        match write_instrument_tag_if_untagged(&item.path, &item.instrument) {
            Ok(()) => {
                applied += 1;
                applied_paths.push(item.path.clone());
            }
            Err(err) => failed.push((item.path.clone(), err)),
        }
        if let Some(progress) = progress {
            progress.inc_apply();
            std::thread::yield_now();
        }
    }

    BulkApplySummary {
        applied,
        applied_paths,
        failed,
        cancelled,
    }
}

pub fn total_actionable(groups: &[BulkDirGroup]) -> usize {
    groups.iter().map(BulkDirGroup::actionable_count).sum()
}

pub fn total_accepted(groups: &[BulkDirGroup]) -> usize {
    groups.iter().map(BulkDirGroup::accepted_count).sum()
}

pub fn set_all_accepted(groups: &mut [BulkDirGroup], accepted: bool) {
    for group in groups {
        group.set_all_accepted(accepted);
    }
}

pub fn is_empty_scan(summary: &BulkScanSummary) -> bool {
    summary.groups.is_empty() && summary.failed == 0
}
