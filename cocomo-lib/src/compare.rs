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

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use futures::future::BoxFuture;

use crate::{
    FileSystem, NodeFileSystem, Result,
    hash::{ContentCache, ContentId, hash_and_cache_node, hash_file},
    identity::{FileId, NodeId},
    scan::{ScanConfig, ScanEntry, scan_directory, scan_directory_node},
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
/// Both paths are scanned once with the given filesystem provider and
/// config, then the two scanned trees are merged in a single recursive
/// walk over the `children` of each `ScanEntry`; no subdirectory is ever
/// re-scanned. When `compare_files` is enabled, content hashes are used
/// to determine equality. The cache is optional and used as an optimization
/// to avoid re-hashing files that were already processed.
pub async fn compare_directories(
    fs: &Arc<dyn FileSystem>,
    left_path: &Path,
    right_path: &Path,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
) -> Result<DirComparison> {
    let scan_config = ScanConfig {
        follow_symlinks: config.follow_symlinks,
        max_depth: config.max_depth,
    };

    let left_result = scan_directory(fs, left_path, &scan_config).await?;
    let right_result = scan_directory(fs, right_path, &scan_config).await?;

    let mut comparison = compare_children(
        &**fs,
        &left_result.entries,
        &right_result.entries,
        left_path,
        right_path,
        config,
        cache,
    )
    .await;

    // Surface non-fatal scan errors instead of dropping them.
    comparison.errors.extend(left_result.errors);
    comparison.errors.extend(right_result.errors);

    Ok(comparison)
}

/// Merge two lists of scanned sibling entries into a `DirComparison`.
///
/// For a pair of files the status is determined by [`compare_file_status`].
/// For a pair of directories the children already attached to the scan
/// entries are merged recursively, so every nesting level is compared
/// exactly once. Directory entries whose subtree contains differences get
/// their placeholder `Same` status refined to `Different`.
fn compare_children<'a>(
    fs: &'a dyn FileSystem,
    left: &'a [ScanEntry],
    right: &'a [ScanEntry],
    left_root: &'a Path,
    right_root: &'a Path,
    config: &'a CompareConfig,
    cache: Option<&'a ContentCache>,
) -> BoxFuture<'a, DirComparison> {
    Box::pin(async move {
        let mut comparison = DirComparison::default();

        let left_map: HashMap<&str, &ScanEntry> =
            left.iter().map(|e| (e.name.as_str(), e)).collect();
        let right_map: HashMap<&str, &ScanEntry> =
            right.iter().map(|e| (e.name.as_str(), e)).collect();

        let mut names: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        for name in left_map.keys().chain(right_map.keys()) {
            if seen.insert(name) {
                names.push(name);
            }
        }
        names.sort();

        for &name in &names {
            let left_entry = left_map.get(name).copied();
            let right_entry = right_map.get(name).copied();

            let mut entry = match merge_entry_sync(
                fs,
                left_root,
                left_entry,
                right_root,
                right_entry,
                config,
                cache,
            )
            .await
            {
                Ok(e) => e,
                Err(e) => {
                    comparison.errors.push(e);
                    continue;
                }
            };

            // For a directory pair, recurse one level into the children
            // that the scan already attached to the entries.
            let both_dirs = left_entry.is_some_and(|e| e.is_dir())
                && right_entry.is_some_and(|e| e.is_dir());

            if both_dirs {
                let lc = left_entry.and_then(|e| e.children()).unwrap_or(&[]);
                let rc = right_entry.and_then(|e| e.children()).unwrap_or(&[]);

                let sub = compare_children(
                    fs, lc, rc, left_root, right_root, config, cache,
                )
                .await;

                let has_diff = sub.different_count() > 0;
                entry.sub_entries = Some(Arc::new(sub));
                if has_diff {
                    entry.status = DirEntryStatus::Different;
                }
            }

            *comparison.counts.entry(entry.status).or_insert(0) += 1;
            comparison.entries.push(entry);
        }

        comparison
    })
}

// ---------------------------------------------------------------------------
// Node-based comparison
// ---------------------------------------------------------------------------

/// Compare two directory trees using the node-based `NodeFileSystem` API.
///
/// Both paths are resolved to node identifiers and scanned once with
/// [`scan_directory_node`], then the two scanned trees are merged in a
/// single recursive walk over the `children` of each `ScanEntry`; no
/// subdirectory is ever re-scanned. When `compare_files` is enabled,
/// content hashes are computed via node-based reads and cached on the
/// provider node. The optional `cache` provides a secondary LRU layer
/// across provider instances.
pub async fn compare_directories_node<N>(
    fs: &N,
    left_path: &Path,
    right_path: &Path,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
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

    let mut comparison = compare_children_node(
        fs,
        &left_result.entries,
        &right_result.entries,
        left_path,
        right_path,
        config,
        cache,
    )
    .await;

    // Surface non-fatal scan errors instead of dropping them.
    comparison.errors.extend(left_result.errors);
    comparison.errors.extend(right_result.errors);

    Ok(comparison)
}

/// Node-aware version of [`compare_children`], mirroring the recursive
/// tree walk of the path-based pipeline.
fn compare_children_node<'a, N>(
    fs: &'a N,
    left: &'a [ScanEntry],
    right: &'a [ScanEntry],
    left_root: &'a Path,
    right_root: &'a Path,
    config: &'a CompareConfig,
    cache: Option<&'a ContentCache>,
) -> BoxFuture<'a, DirComparison>
where
    N: NodeFileSystem<Nid = u64>,
{
    Box::pin(async move {
        let mut comparison = DirComparison::default();

        let left_map: HashMap<&str, &ScanEntry> =
            left.iter().map(|e| (e.name.as_str(), e)).collect();
        let right_map: HashMap<&str, &ScanEntry> =
            right.iter().map(|e| (e.name.as_str(), e)).collect();

        let mut names: Vec<&str> = Vec::new();
        let mut seen = HashSet::new();
        for name in left_map.keys().chain(right_map.keys()) {
            if seen.insert(name) {
                names.push(name);
            }
        }
        names.sort();

        for &name in &names {
            let left_entry = left_map.get(name).copied();
            let right_entry = right_map.get(name).copied();

            let mut entry = match merge_entry_sync_node(
                fs,
                left_root,
                left_entry,
                right_root,
                right_entry,
                config,
                cache,
            )
            .await
            {
                Ok(e) => e,
                Err(e) => {
                    comparison.errors.push(e);
                    continue;
                }
            };

            // For a directory pair, recurse one level into the children
            // that the scan already attached to the entries.
            let both_dirs = left_entry.is_some_and(|e| e.is_dir())
                && right_entry.is_some_and(|e| e.is_dir());

            if both_dirs {
                let lc = left_entry.and_then(|e| e.children()).unwrap_or(&[]);
                let rc = right_entry.and_then(|e| e.children()).unwrap_or(&[]);

                let sub = compare_children_node(
                    fs, lc, rc, left_root, right_root, config, cache,
                )
                .await;

                let has_diff = sub.different_count() > 0;
                entry.sub_entries = Some(Arc::new(sub));
                if has_diff {
                    entry.status = DirEntryStatus::Different;
                }
            }

            *comparison.counts.entry(entry.status).or_insert(0) += 1;
            comparison.entries.push(entry);
        }

        comparison
    })
}

/// Node-aware version of `merge_entry_sync`.
async fn merge_entry_sync_node<N>(
    fs: &N,
    left_root: &Path,
    left: Option<&ScanEntry>,
    right_root: &Path,
    right: Option<&ScanEntry>,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
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
    cache: Option<&ContentCache>,
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

    // Same size — resolve to node IDs and compare hashes.
    let left_path = left_root.join(&left.path);
    let right_path = right_root.join(&right.path);
    let label = fs.label_node();

    let left_id = fs.resolve_path(&left_path).await.ok();
    let right_id = fs.resolve_path(&right_path).await.ok();

    // Check the optional global cache first.
    if let Some(c) = cache {
        let left_cid = c.get(label, &left_path);
        let right_cid = c.get(label, &right_path);
        if matches!(
            (&left_cid, &right_cid),
            (Some(lc), Some(rc)) if lc.hash == rc.hash
        ) {
            return Ok(DirEntryStatus::Same);
        }
    }

    // Helper: get the hash for a file, checking node cache first,
    // then computing and caching.
    async fn get_hash<N>(
        fs: &N,
        id: NodeId<u64>,
        node: &crate::node::Node,
    ) -> Option<blake3::Hash>
    where
        N: NodeFileSystem<Nid = u64>,
    {
        // Check if the node already has a cached hash.
        if let Some(cached) = node.cached_hash() {
            // Parse the hex string back to a blake3::Hash for comparison.
            return blake3::Hash::from_hex(cached).ok();
        }
        // Compute and cache on the provider.
        hash_and_cache_node(fs, id, FileId::new(*id.get()), node)
            .await
            .ok()
    }

    let left_hash = match (left_id, fs.get_node(left_id.unwrap())) {
        (Some(id), Ok(node)) => get_hash(fs, id, &node).await,
        _ => None,
    };

    let right_hash = match (right_id, fs.get_node(right_id.unwrap())) {
        (Some(id), Ok(node)) => get_hash(fs, id, &node).await,
        _ => None,
    };

    let (Some(lh), Some(rh)) = (left_hash, right_hash) else {
        return Ok(DirEntryStatus::Different);
    };

    // Store in the optional global cache.
    if let Some(c) = cache {
        let left_cid = ContentId::from_blake3(&left.meta, &lh);
        let right_cid = ContentId::from_blake3(&right.meta, &rh);
        c.insert(label, &left_path, left_cid);
        c.insert(label, &right_path, right_cid);
    }

    if lh == rh {
        return Ok(DirEntryStatus::Same);
    }

    Ok(DirEntryStatus::Different)
}

/// Merge a single pair of entries into a `DirEntry`.
async fn merge_entry_sync(
    fs: &dyn FileSystem,
    left_root: &Path,
    left: Option<&ScanEntry>,
    right_root: &Path,
    right: Option<&ScanEntry>,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
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
    cache: Option<&ContentCache>,
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

    // Check the optional global cache.
    if let Some(c) = cache {
        let left_cid = c.get(label, &left_path);
        let right_cid = c.get(label, &right_path);
        if matches!((&left_cid, &right_cid), (Some(lc), Some(rc)) if lc.hash == rc.hash)
        {
            return Ok(DirEntryStatus::Same);
        }
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

    // Store in the optional global cache.
    if let Some(c) = cache {
        let left_cid_new = ContentId::from_blake3(&left.meta, &left_hash);
        let right_cid_new = ContentId::from_blake3(&right.meta, &right_hash);
        c.insert(label, &left_path, left_cid_new);
        c.insert(label, &right_path, right_cid_new);
    }

    if left_hash == right_hash {
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

        let result =
            compare_directories(&fs, &left, &right, &config, Some(&cache))
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

        let result =
            compare_directories(&fs, &left, &right, &config, Some(&cache))
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

        let result = compare_directories_node(
            &fs,
            &left,
            &right,
            &config,
            Some(&cache),
        )
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

        let result = compare_directories_node(
            &fs,
            &left,
            &right,
            &config,
            Some(&cache),
        )
        .await
        .unwrap();

        assert!(result.total() > 0);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    // -----------------------------------------------------------------------
    // Nested-tree tests
    // -----------------------------------------------------------------------

    /// Create `x/y/deep.txt` on both sides with identical-size,
    /// different-content payloads.
    fn make_nested_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
        let base = env::temp_dir()
            .join(format!("cocomo_nested_{}", std::process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        fs_err::create_dir_all(left.join("x/y")).unwrap();
        fs_err::create_dir_all(right.join("x/y")).unwrap();
        fs_err::write(left.join("x/y/deep.txt"), "aaaaaaaaaa").unwrap();
        fs_err::write(right.join("x/y/deep.txt"), "bbbbbbbbbb").unwrap();

        (left, right)
    }

    /// Look up an entry by name in a comparison.
    fn find_entry<'a>(
        comp: &'a DirComparison,
        name: &str,
    ) -> Option<&'a DirEntry> {
        comp.entries.iter().find(|e| e.name == name)
    }

    /// Verify that `x`, `x/y`, and `x/y/deep.txt` are all flagged as
    /// `Different` and that sub-comparisons are attached at every nesting
    /// level.
    fn check_nested_differences(comp: &DirComparison) {
        let x = find_entry(comp, "x").expect("top-level dir `x` missing");
        assert_eq!(x.status, DirEntryStatus::Different);
        let sub_x = x
            .sub_entries
            .as_ref()
            .expect("sub comparison for `x` missing");
        let y = find_entry(sub_x, "y").expect("nested dir `x/y` missing");
        assert_eq!(y.status, DirEntryStatus::Different);
        let sub_y = y
            .sub_entries
            .as_ref()
            .expect("sub comparison for `x/y` missing");
        let deep = find_entry(sub_y, "deep.txt")
            .expect("file `x/y/deep.txt` missing");
        assert_eq!(deep.status, DirEntryStatus::Different);
    }

    #[tokio::test]
    async fn compare_detects_nested_differences() {
        let (left, right) = make_nested_dirs();
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));
        let cache = ContentCache::default_config();
        let config = CompareConfig::full();

        let result =
            compare_directories(&fs, &left, &right, &config, Some(&cache))
                .await
                .unwrap();

        check_nested_differences(&result);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn node_compare_detects_nested_differences() {
        let (left, right) = make_nested_dirs();
        let fs = LocalFs::new("test");
        let cache = ContentCache::default_config();
        let config = CompareConfig::full();

        let result = compare_directories_node(
            &fs,
            &left,
            &right,
            &config,
            Some(&cache),
        )
        .await
        .unwrap();

        check_nested_differences(&result);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }
}
