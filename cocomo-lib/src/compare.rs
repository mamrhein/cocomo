// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Directory comparison logic.
//!
//! Merges two scanned directory trees into a unified [`DirComparison`] result.
//! Each entry is classified by its status: same, different, orphan, etc.

use std::{collections::HashMap, path::Path, sync::Arc};

use crate::{
    FileSystem, NodeFileSystem, Result,
    hash::{ContentCache, ContentId, hash_file, hash_file_node},
    identity::FileId,
    scan::{ScanConfig, ScanEntry, ScanResult, scan_directory_node},
};

/// The comparison status of a directory entry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialOrd, PartialEq)]
pub enum DirEntryStatus {
    /// Text files with equal content (equal by hash).
    Same,
    /// Binary files with equal content (equal by hash).
    SameBinary,
    /// Same name, size within tolerance but content differs.
    Similar,
    /// Same name, different content.
    Different,
    /// Entry exists only on the left side.
    LeftOnly,
    /// Entry exists only on the right side.
    RightOnly,
    /// Entry exists only on the center side (3-way).
    CenterOnly,
    /// Changes on both sides are non-conflicting and can be auto-merged.
    Mergeable,
    /// Conflicting changes in a 3-way merge.
    Conflict,
    /// Same name but different type (e.g., file vs. directory).
    IdenticalNameDifferentType,
}

impl std::fmt::Display for DirEntryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DirEntryStatus::Same => write!(f, "same"),
            DirEntryStatus::SameBinary => write!(f, "same (binary)"),
            DirEntryStatus::Similar => write!(f, "similar"),
            DirEntryStatus::Different => write!(f, "different"),
            DirEntryStatus::LeftOnly => write!(f, "left only"),
            DirEntryStatus::RightOnly => write!(f, "right only"),
            DirEntryStatus::CenterOnly => write!(f, "center only"),
            DirEntryStatus::Mergeable => write!(f, "mergeable"),
            DirEntryStatus::Conflict => write!(f, "conflict"),
            DirEntryStatus::IdenticalNameDifferentType => {
                write!(f, "type mismatch")
            }
        }
    }
}

/// The status symbol shown in the TUI gutter.
impl DirEntryStatus {
    pub fn symbol(&self) -> &'static str {
        match self {
            DirEntryStatus::Same => "=",
            DirEntryStatus::SameBinary => "=",
            DirEntryStatus::Similar => "~",
            DirEntryStatus::Different => "!",
            DirEntryStatus::LeftOnly => "<",
            DirEntryStatus::RightOnly => ">",
            DirEntryStatus::CenterOnly => "|",
            DirEntryStatus::Mergeable => "^",
            DirEntryStatus::Conflict => "X",
            DirEntryStatus::IdenticalNameDifferentType => "?",
        }
    }
}

/// Metadata captured for a single side of a comparison entry.
#[derive(Clone, Debug, Default)]
pub struct EntryInfo {
    /// Entry name.
    pub name: String,
    /// Full path.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// Modification time.
    pub modified: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Content hash (hex), present when content was compared.
    pub hash: Option<String>,
}

/// A single row in a directory comparison.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// Entry name (common to all sides).
    pub name: String,
    /// Classification of this entry across the compared sides.
    pub status: DirEntryStatus,
    /// Left-side metadata, `None` when absent on the left.
    pub left: Option<EntryInfo>,
    /// Right-side metadata, `None` when absent on the right.
    pub right: Option<EntryInfo>,
    /// Center-side metadata, present only in 3-way comparisons.
    pub center: Option<EntryInfo>,
    /// Sub-entries for directories (recursive comparison result).
    pub sub_entries: Option<Arc<DirComparison>>,
}

/// Result of comparing two (or three) directory trees.
#[derive(Clone, Debug, Default)]
pub struct DirComparison {
    /// Comparison entries.
    pub entries: Vec<DirEntry>,
    /// Total counts by status for the status panel.
    pub counts: HashMap<DirEntryStatus, usize>,
    /// Errors encountered during comparison.
    pub errors: Vec<crate::FsError>,
}

impl DirComparison {
    /// Return the total number of entries.
    pub fn total(&self) -> usize {
        self.entries.len()
    }

    /// Return the number of differing entries.
    pub fn different_count(&self) -> usize {
        self.counts
            .get(&DirEntryStatus::Different)
            .copied()
            .unwrap_or(0)
            + self
                .counts
                .get(&DirEntryStatus::Similar)
                .copied()
                .unwrap_or(0)
    }

    /// Return the number of same entries.
    pub fn same_count(&self) -> usize {
        self.counts.get(&DirEntryStatus::Same).copied().unwrap_or(0)
            + self
                .counts
                .get(&DirEntryStatus::SameBinary)
                .copied()
                .unwrap_or(0)
    }

    /// Return the number of orphan entries.
    pub fn orphan_count(&self) -> usize {
        self.counts
            .get(&DirEntryStatus::LeftOnly)
            .copied()
            .unwrap_or(0)
            + self
                .counts
                .get(&DirEntryStatus::RightOnly)
                .copied()
                .unwrap_or(0)
    }
}

/// Configuration for a directory comparison.
#[derive(Clone, Debug, Default)]
pub struct CompareConfig {
    /// Compare file contents, not just structure.
    pub compare_files: bool,
    /// Compare directory structure.
    pub compare_structure: bool,
    /// Follow symlinks during scan.
    pub follow_symlinks: bool,
    /// Maximum scan depth.
    pub max_depth: Option<usize>,
    /// Size tolerance ratio for "similar" classification (0.0–1.0).
    pub size_tolerance: f64,
}

impl CompareConfig {
    /// Full comparison: structure + content.
    pub fn full() -> Self {
        Self {
            compare_files: true,
            compare_structure: true,
            follow_symlinks: false,
            max_depth: None,
            size_tolerance: 0.1,
        }
    }

    /// Structure-only comparison (no content hashing).
    pub fn structure_only() -> Self {
        Self {
            compare_files: false,
            compare_structure: true,
            follow_symlinks: false,
            max_depth: None,
            size_tolerance: 0.1,
        }
    }
}

/// Compare two directory trees and produce a unified `DirComparison`.
///
/// Both paths are scanned with the given filesystem provider and config,
/// then merged. When `compare_files` is enabled, content hashes are used
/// to determine equality.
pub async fn compare_directories(
    fs: &Arc<dyn FileSystem>,
    left_path: &Path,
    right_path: &Path,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirComparison> {
    let scan_config = ScanConfig {
        follow_symlinks: config.follow_symlinks,
        max_depth: config.max_depth,
    };

    let left_result =
        crate::scan_directory(fs, left_path, &scan_config).await?;
    let right_result =
        crate::scan_directory(fs, right_path, &scan_config).await?;

    // Collect all directory pairs for sub-comparison.
    let dir_pairs = collect_dir_pairs(&left_result, &right_result);

    // Do the top-level merge.
    let mut comparison = merge_sync(
        fs,
        left_path,
        &left_result,
        right_path,
        &right_result,
        config,
        cache,
    )
    .await?;

    // Recursively compare subdirectories.
    for (left_sub, right_sub) in dir_pairs {
        let sub_left = left_path.join(&left_sub.path);
        let sub_right = right_path.join(&right_sub.path);

        let sub_scan_config = ScanConfig {
            follow_symlinks: config.follow_symlinks,
            max_depth: config.max_depth,
        };

        let left_entries =
            scan_dir_entries(fs, &sub_left, &sub_scan_config).await;
        let right_entries =
            scan_dir_entries(fs, &sub_right, &sub_scan_config).await;

        match (left_entries, right_entries) {
            (Ok(le), Ok(re)) => {
                let sub_comparison = merge_sync(
                    fs, &sub_left, &le, &sub_right, &re, config, cache,
                )
                .await;

                match sub_comparison {
                    Ok(sub_comp) => {
                        // Attach sub-comparison to the parent directory entry.
                        attach_sub_comparison(
                            &mut comparison,
                            &left_sub.name,
                            Arc::new(sub_comp),
                        );
                    }
                    Err(e) => {
                        comparison.errors.push(e);
                    }
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                comparison.errors.push(e);
            }
        }
    }

    Ok(comparison)
}

/// Scan a directory and return its entries as a ScanResult.
async fn scan_dir_entries(
    fs: &Arc<dyn FileSystem>,
    path: &Path,
    config: &ScanConfig,
) -> Result<ScanResult> {
    crate::scan_directory(fs, path, config).await
}

// ---------------------------------------------------------------------------
// Node-based comparison
// ---------------------------------------------------------------------------

/// Compare two directory trees using the node-based `NodeFileSystem` API.
///
/// Both paths are resolved to node identifiers, then scanned with
/// [`scan_directory_node`], and merged. When `compare_files` is enabled,
/// content hashes are computed via node-based reads.
pub async fn compare_directories_node<N>(
    fs: &N,
    left_path: &Path,
    right_path: &Path,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirComparison>
where
    N: NodeFileSystem<Nid = u64>,
{
    let scan_config = ScanConfig {
        follow_symlinks: config.follow_symlinks,
        max_depth: config.max_depth,
    };

    let left_result = scan_directory_node(fs, left_path, &scan_config).await?;
    let right_result =
        scan_directory_node(fs, right_path, &scan_config).await?;

    // Collect all directory pairs for sub-comparison.
    let dir_pairs = collect_dir_pairs(&left_result, &right_result);

    // Do the top-level merge.
    let mut comparison = merge_sync_node(
        fs, left_path, &left_result, right_path, &right_result, config, cache,
    )
    .await?;

    // Recursively compare subdirectories.
    for (left_sub, right_sub) in dir_pairs {
        let sub_left = left_path.join(&left_sub.path);
        let sub_right = right_path.join(&right_sub.path);

        let sub_scan_config = ScanConfig {
            follow_symlinks: config.follow_symlinks,
            max_depth: config.max_depth,
        };

        let left_entries =
            scan_directory_node(fs, &sub_left, &sub_scan_config).await;
        let right_entries =
            scan_directory_node(fs, &sub_right, &sub_scan_config).await;

        match (left_entries, right_entries) {
            (Ok(le), Ok(re)) => {
                let sub_comparison = merge_sync_node(
                    fs, &sub_left, &le, &sub_right, &re, config, cache,
                )
                .await;

                match sub_comparison {
                    Ok(sub_comp) => {
                        attach_sub_comparison(
                            &mut comparison,
                            &left_sub.name,
                            Arc::new(sub_comp),
                        );
                    }
                    Err(e) => {
                        comparison.errors.push(e);
                    }
                }
            }
            (Err(e), _) | (_, Err(e)) => {
                comparison.errors.push(e);
            }
        }
    }

    Ok(comparison)
}

/// Node-aware version of `merge_sync` that uses node-based file hashing.
async fn merge_sync_node<N>(
    fs: &N,
    left_root: &Path,
    left: &ScanResult,
    right_root: &Path,
    right: &ScanResult,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirComparison>
where
    N: NodeFileSystem<Nid = u64>,
{
    let mut comparison = DirComparison::default();

    let left_map: HashMap<&str, &ScanEntry> =
        left.entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let right_map: HashMap<&str, &ScanEntry> =
        right.entries.iter().map(|e| (e.name.as_str(), e)).collect();

    let mut names: Vec<&str> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in left_map.keys().chain(right_map.keys()) {
        if seen.insert(name) {
            names.push(name);
        }
    }
    names.sort();

    for &name in &names {
        let left_entry = left_map.get(name).copied();
        let right_entry = right_map.get(name).copied();

        let dir_entry = merge_entry_sync_node(
            fs, left_root, left_entry, right_root, right_entry, config, cache,
        )
        .await?;

        *comparison.counts.entry(dir_entry.status).or_insert(0) += 1;
        comparison.entries.push(dir_entry);
    }

    Ok(comparison)
}

/// Node-aware version of `merge_entry_sync`.
async fn merge_entry_sync_node<N>(
    fs: &N,
    left_root: &Path,
    left: Option<&ScanEntry>,
    right_root: &Path,
    right: Option<&ScanEntry>,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirEntry>
where
    N: NodeFileSystem<Nid = u64>,
{
    let name = left
        .map(|e| e.name.as_str())
        .or(right.map(|e| e.name.as_str()))
        .unwrap_or("")
        .to_string();

    let left_info = left.map(scan_entry_to_info);
    let right_info = right.map(scan_entry_to_info);

    let status = if let (Some(le), Some(re)) = (left, right) {
        if le.meta.is_dir != re.meta.is_dir {
            DirEntryStatus::IdenticalNameDifferentType
        } else if le.meta.is_dir {
            DirEntryStatus::Same // placeholder; refined by caller
        } else {
            compare_file_status_node(
                fs, left_root, le, right_root, re, config, cache,
            )
            .await?
        }
    } else if left.is_some() {
        DirEntryStatus::LeftOnly
    } else {
        DirEntryStatus::RightOnly
    };

    Ok(DirEntry {
        name,
        status,
        left: left_info,
        right: right_info,
        center: None,
        sub_entries: None,
    })
}

/// Node-aware file comparison. Uses node IDs for hashing instead of paths.
async fn compare_file_status_node<N>(
    fs: &N,
    left_root: &Path,
    left: &ScanEntry,
    right_root: &Path,
    right: &ScanEntry,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirEntryStatus>
where
    N: NodeFileSystem<Nid = u64>,
{
    if !config.compare_files {
        return if left.meta.size == right.meta.size {
            Ok(DirEntryStatus::Same)
        } else {
            Ok(DirEntryStatus::Different)
        };
    }

    // Empty files are always the same.
    if left.meta.size == right.meta.size && left.meta.size == 0 {
        return Ok(DirEntryStatus::Same);
    }

    // Different sizes — check tolerance for "similar".
    if left.meta.size != right.meta.size {
        let size_diff = left.meta.size.abs_diff(right.meta.size);
        let larger = std::cmp::max(left.meta.size, right.meta.size);
        if larger > 0
            && (size_diff as f64 / larger as f64) <= config.size_tolerance
        {
            return Ok(DirEntryStatus::Similar);
        }
        return Ok(DirEntryStatus::Different);
    }

    // Same size — resolve to node IDs and hash via the node API.
    let left_path = left_root.join(&left.path);
    let right_path = right_root.join(&right.path);
    let label = fs.label_node();

    let left_id = fs.resolve_path(&left_path).await.ok();
    let right_id = fs.resolve_path(&right_path).await.ok();

    // Check cache using paths (ContentCache is path-keyed).
    let left_cid = cache.get(label, &left_path);
    let right_cid = cache.get(label, &right_path);

    if matches!(
        (&left_cid, &right_cid),
        (Some(lc), Some(rc)) if lc.hash == rc.hash
    ) {
        return Ok(DirEntryStatus::Same);
    }

    // Hash both files via node IDs.
    let left_hash = match (left_id, fs.get_node(left_id.unwrap())) {
        (Some(id), Ok(node)) => {
            match hash_file_node(fs, FileId::new(*id.get()), &node).await {
                Ok(h) => h,
                Err(_) => return Ok(DirEntryStatus::Different),
            }
        }
        _ => return Ok(DirEntryStatus::Different),
    };

    let right_hash = match (right_id, fs.get_node(right_id.unwrap())) {
        (Some(id), Ok(node)) => {
            match hash_file_node(fs, FileId::new(*id.get()), &node).await {
                Ok(h) => h,
                Err(_) => return Ok(DirEntryStatus::Different),
            }
        }
        _ => return Ok(DirEntryStatus::Different),
    };

    let left_hex = left_hash.to_hex().to_string();
    let right_hex = right_hash.to_hex().to_string();

    // Cache the results.
    let left_cid_new =
        ContentId::from_blake3(&left.meta, &left_hash);
    let right_cid_new =
        ContentId::from_blake3(&right.meta, &right_hash);
    cache.insert(label, &left_path, left_cid_new);
    cache.insert(label, &right_path, right_cid_new);

    if left_hex == right_hex {
        return Ok(DirEntryStatus::Same);
    }

    Ok(DirEntryStatus::Different)
}

/// Collect pairs of entries that are directories on both sides.
fn collect_dir_pairs(
    left: &ScanResult,
    right: &ScanResult,
) -> Vec<(ScanEntry, ScanEntry)> {
    let left_map: HashMap<&str, &ScanEntry> =
        left.entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let right_map: HashMap<&str, &ScanEntry> =
        right.entries.iter().map(|e| (e.name.as_str(), e)).collect();

    let mut pairs = Vec::new();
    for (name, &le) in &left_map {
        if le.meta.is_dir
            && let Some(&re) = right_map.get(*name)
            && re.meta.is_dir
        {
            pairs.push((le.clone(), re.clone()));
        }
    }
    pairs
}

/// Attach a sub-comparison to the matching directory entry in the parent.
fn attach_sub_comparison(
    comparison: &mut DirComparison,
    dir_name: &str,
    sub: Arc<DirComparison>,
) {
    for entry in &mut comparison.entries {
        if entry.name == dir_name
            && entry.left.as_ref().is_some_and(|l| l.is_dir)
        {
            // Update the parent status based on sub-comparison.
            let has_diff = sub.different_count() > 0;
            entry.sub_entries = Some(sub);
            if has_diff {
                entry.status = DirEntryStatus::Different;
            }
            return;
        }
    }
}

/// Merge two scan results (file-level comparison only;
/// directories get a placeholder status that is refined later).
async fn merge_sync(
    fs: &Arc<dyn FileSystem>,
    left_root: &Path,
    left: &ScanResult,
    right_root: &Path,
    right: &ScanResult,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirComparison> {
    let mut comparison = DirComparison::default();

    let left_map: HashMap<&str, &ScanEntry> =
        left.entries.iter().map(|e| (e.name.as_str(), e)).collect();
    let right_map: HashMap<&str, &ScanEntry> =
        right.entries.iter().map(|e| (e.name.as_str(), e)).collect();

    let mut names: Vec<&str> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in left_map.keys().chain(right_map.keys()) {
        if seen.insert(name) {
            names.push(name);
        }
    }
    names.sort();

    let fs_ref: &dyn FileSystem = &**fs;

    for &name in &names {
        let left_entry = left_map.get(name).copied();
        let right_entry = right_map.get(name).copied();

        let dir_entry = merge_entry_sync(
            fs_ref,
            left_root,
            left_entry,
            right_root,
            right_entry,
            config,
            cache,
        )
        .await?;

        *comparison.counts.entry(dir_entry.status).or_insert(0) += 1;
        comparison.entries.push(dir_entry);
    }

    Ok(comparison)
}

/// Merge a single pair of entries into a `DirEntry`.
async fn merge_entry_sync(
    fs: &dyn FileSystem,
    left_root: &Path,
    left: Option<&ScanEntry>,
    right_root: &Path,
    right: Option<&ScanEntry>,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirEntry> {
    let name = left
        .map(|e| e.name.as_str())
        .or(right.map(|e| e.name.as_str()))
        .unwrap_or("")
        .to_string();

    let left_info = left.map(scan_entry_to_info);
    let right_info = right.map(scan_entry_to_info);

    let status = if let (Some(le), Some(re)) = (left, right) {
        if le.meta.is_dir != re.meta.is_dir {
            DirEntryStatus::IdenticalNameDifferentType
        } else if le.meta.is_dir {
            DirEntryStatus::Same // placeholder; refined by caller
        } else {
            compare_file_status(
                fs, left_root, le, right_root, re, config, cache,
            )
            .await?
        }
    } else if left.is_some() {
        DirEntryStatus::LeftOnly
    } else {
        DirEntryStatus::RightOnly
    };

    Ok(DirEntry {
        name,
        status,
        left: left_info,
        right: right_info,
        center: None,
        sub_entries: None,
    })
}

/// Compare two file entries and determine their status.
async fn compare_file_status(
    fs: &dyn FileSystem,
    left_root: &Path,
    left: &ScanEntry,
    right_root: &Path,
    right: &ScanEntry,
    config: &CompareConfig,
    cache: &ContentCache,
) -> Result<DirEntryStatus> {
    if !config.compare_files {
        return if left.meta.size == right.meta.size {
            Ok(DirEntryStatus::Same)
        } else {
            Ok(DirEntryStatus::Different)
        };
    }

    // Empty files are always the same.
    if left.meta.size == right.meta.size && left.meta.size == 0 {
        return Ok(DirEntryStatus::Same);
    }

    // Different sizes — check tolerance for "similar".
    if left.meta.size != right.meta.size {
        let size_diff = left.meta.size.abs_diff(right.meta.size);
        let larger = std::cmp::max(left.meta.size, right.meta.size);
        if larger > 0
            && (size_diff as f64 / larger as f64) <= config.size_tolerance
        {
            return Ok(DirEntryStatus::Similar);
        }
        return Ok(DirEntryStatus::Different);
    }

    // Same size — check cache first, then hash.
    let left_path = left_root.join(&left.path);
    let right_path = right_root.join(&right.path);
    let label = fs.label();

    let left_cid = cache.get(label, &left_path);
    let right_cid = cache.get(label, &right_path);

    if matches!((&left_cid, &right_cid), (Some(lc), Some(rc)) if lc.hash == rc.hash)
    {
        return Ok(DirEntryStatus::Same);
    }

    // Hash both files.
    let left_hash = match hash_file(fs, &left_path).await {
        Ok(h) => h,
        Err(_) => return Ok(DirEntryStatus::Different),
    };
    let right_hash = match hash_file(fs, &right_path).await {
        Ok(h) => h,
        Err(_) => return Ok(DirEntryStatus::Different),
    };

    let left_hex = left_hash.to_hex().to_string();
    let right_hex = right_hash.to_hex().to_string();

    // Cache the results.
    let left_cid_new = ContentId::from_blake3(&left.meta, &left_hash);
    let right_cid_new = ContentId::from_blake3(&right.meta, &right_hash);
    cache.insert(label, &left_path, left_cid_new);
    cache.insert(label, &right_path, right_cid_new);

    if left_hex == right_hex {
        return Ok(DirEntryStatus::Same);
    }

    Ok(DirEntryStatus::Different)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn scan_entry_to_info(entry: &ScanEntry) -> EntryInfo {
    EntryInfo {
        name: entry.name.clone(),
        path: entry.path.clone(),
        size: entry.meta.size,
        modified: entry.meta.modified.to_rfc3339(),
        is_dir: entry.meta.is_dir,
        hash: None,
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::*;
    use crate::local::LocalFs;

    fn make_test_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
        let base = env::temp_dir()
            .join(format!("cocomo_compare_{}", std::process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        // Common file with same content.
        fs_err::create_dir_all(left.join("common")).unwrap();
        fs_err::create_dir_all(right.join("common")).unwrap();
        fs_err::write(left.join("same.txt"), "hello").unwrap();
        fs_err::write(right.join("same.txt"), "hello").unwrap();

        // Different content.
        fs_err::write(left.join("diff.txt"), "version 1").unwrap();
        fs_err::write(right.join("diff.txt"), "version 2").unwrap();

        // Left-only.
        fs_err::write(left.join("left_only.txt"), "only left").unwrap();

        // Right-only.
        fs_err::write(right.join("right_only.txt"), "only right").unwrap();

        // Common directory with files.
        fs_err::write(left.join("common/inner.txt"), "inner").unwrap();
        fs_err::write(right.join("common/inner.txt"), "inner").unwrap();

        (left, right)
    }

    #[tokio::test]
    async fn compare_detects_same_and_different() {
        let (left, right) = make_test_dirs();
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));
        let cache = ContentCache::default_config();
        let config = CompareConfig::full();

        let result = compare_directories(&fs, &left, &right, &config, &cache)
            .await
            .unwrap();

        assert!(result.same_count() > 0);
        assert!(result.different_count() > 0);
        assert!(result.orphan_count() > 0);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn compare_structure_only() {
        let (left, right) = make_test_dirs();
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));
        let cache = ContentCache::default_config();
        let config = CompareConfig::structure_only();

        let result = compare_directories(&fs, &left, &right, &config, &cache)
            .await
            .unwrap();

        assert!(result.total() > 0);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[test]
    fn status_symbols() {
        assert_eq!(DirEntryStatus::Same.symbol(), "=");
        assert_eq!(DirEntryStatus::Different.symbol(), "!");
        assert_eq!(DirEntryStatus::LeftOnly.symbol(), "<");
        assert_eq!(DirEntryStatus::RightOnly.symbol(), ">");
        assert_eq!(DirEntryStatus::Conflict.symbol(), "X");
    }

    // -----------------------------------------------------------------------
    // Node-based compare tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn node_compare_detects_same_and_different() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");
        let cache = ContentCache::default_config();
        let config = CompareConfig::full();

        let result =
            compare_directories_node(&fs, &left, &right, &config, &cache)
                .await
                .unwrap();

        assert!(result.same_count() > 0);
        assert!(result.different_count() > 0);
        assert!(result.orphan_count() > 0);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn node_compare_structure_only() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");
        let cache = ContentCache::default_config();
        let config = CompareConfig::structure_only();

        let result =
            compare_directories_node(&fs, &left, &right, &config, &cache)
                .await
                .unwrap();

        assert!(result.total() > 0);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }
}
