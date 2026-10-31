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
use tokio_util::sync::CancellationToken;

use crate::{
    NodeFileSystem, Result,
    error::FsError,
    hash::{ContentCache, ContentId, hash_and_cache_node},
    identity::{FileId, NodeId},
    scan::{ScanConfig, ScanEntry, scan_directory_node},
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
    /// Set on both orphan rows of a name clash where one side has a file
    /// and the other a directory under the same name. Transfer operations
    /// must not act on such entries.
    pub name_conflict: bool,
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
    /// Compare file contents, not just meta data.
    pub compare_content: bool,
    /// Compare node meta data (name, size, modification date).
    pub compare_meta_data: bool,
    /// Follow symlinks during scan.
    pub follow_symlinks: bool,
    /// Size tolerance ratio for "similar" classification (0.0–1.0).
    pub size_tolerance: f64,
}

impl CompareConfig {
    /// Full comparison: meta data + content.
    pub fn full() -> Self {
        Self {
            compare_content: true,
            compare_meta_data: true,
            follow_symlinks: false,
            size_tolerance: 0.1,
        }
    }

    /// Meta-data-only comparison (no content hashing).
    pub fn meta_data_only() -> Self {
        Self {
            compare_content: false,
            compare_meta_data: true,
            follow_symlinks: false,
            size_tolerance: 0.1,
        }
    }
}

// ---------------------------------------------------------------------------
// Directory comparison
// ---------------------------------------------------------------------------

/// Compare two directory trees that live on the same filesystem provider.
///
/// Thin wrapper around [`compare_directories_pair_node`] that addresses
/// both sides with the same provider instance.
pub async fn compare_directories_node<N>(
    fs: &N,
    left_path: &Path,
    right_path: &Path,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
    cancel: Option<&CancellationToken>,
) -> Result<DirComparison>
where
    N: NodeFileSystem<Nid = u64>,
{
    compare_directories_pair_node(
        fs, fs, left_path, right_path, config, cache, cancel,
    )
    .await
}

/// Compare two directory trees that may live on two different filesystem
/// providers.
///
/// Each side is scanned once on its own provider with
/// [`scan_directory_node`], then the two scanned trees are merged in a
/// single recursive walk over the `children` of each `ScanEntry`; no
/// subdirectory is ever re-scanned. The two providers are *positional*
/// (left and right), not source and destination: a file pair is hashed
/// with the left entry on `left_fs` and the right entry on `right_fs`
/// within one call, so neither parameter can be pinned as "the source".
/// When both parameters address the same provider instance the pipeline
/// behaves exactly like [`compare_directories_node`].
///
/// When `compare_files` is enabled, content hashes are computed via
/// node-based reads and cached on the node of the side being hashed.
/// The optional `cache` is shared across the sides; its `(label, path)`
/// keys identify the provider instance through the label, which stays
/// correct while the labels of the two sides are unique per provider
/// instance (equal endpoint identities share one provider, different
/// ones resolve to different lookup keys, hence different labels).
///
/// When `cancel` is provided and triggered, the scan or the merge walk
/// aborts with [`FsError::Cancelled`]; partial results are never
/// returned as `Ok`.
#[allow(clippy::too_many_arguments)]
pub async fn compare_directories_pair_node<L, R>(
    left_fs: &L,
    right_fs: &R,
    left_path: &Path,
    right_path: &Path,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
    cancel: Option<&CancellationToken>,
) -> Result<DirComparison>
where
    L: NodeFileSystem<Nid = u64>,
    R: NodeFileSystem<Nid = u64>,
{
    let scan_config = ScanConfig {
        follow_symlinks: config.follow_symlinks,
    };

    let left_result =
        scan_directory_node(left_fs, left_path, &scan_config, cancel).await?;
    let right_result =
        scan_directory_node(right_fs, right_path, &scan_config, cancel)
            .await?;

    let mut comparison = compare_children_node(
        left_fs,
        right_fs,
        &left_result.entries,
        &right_result.entries,
        left_path,
        right_path,
        config,
        cache,
        cancel,
    )
    .await?;

    // Surface non-fatal scan errors instead of dropping them.
    comparison.errors.extend(left_result.errors);
    comparison.errors.extend(right_result.errors);

    Ok(comparison)
}

/// Refine a metadata-only comparison with content hashes.
///
/// Second pass for the "metadata first, content in the background"
/// flow: takes an existing [`DirComparison`] (typically produced with
/// [`CompareConfig::meta_data_only`]) and re-checks every top-level
/// file pair that metadata alone classified as equal (`Same` or
/// `SameBinary` with equal sizes) by comparing blake3 content hashes.
/// Pairs whose hashes differ are downgraded to `Different` and the
/// status counts are updated in place.
///
/// Hashes are read through the same node API and optional
/// [`ContentCache`] as the main comparison, so hashes computed during
/// an earlier pass are reused instead of re-read. A side whose content
/// cannot be resolved or read is treated as unknown, which classifies
/// the pair as `Different` — unknown content is never assumed equal.
///
/// `on_progress` is invoked with `(done, total)` after each checked
/// file pair so callers can report progress. When `cancel` is
/// triggered the pass stops with [`FsError::Cancelled`]; the partially
/// refined comparison is dropped.
///
/// Only top-level entries are refined; subdirectory subtrees keep the
/// statuses from the metadata pass. Callers that display a subtree
/// re-compare it when the user navigates in.
#[allow(clippy::too_many_arguments)]
pub async fn compare_content_pair_node<L, R>(
    left_fs: &L,
    right_fs: &R,
    left_path: &Path,
    right_path: &Path,
    mut comparison: DirComparison,
    cache: Option<&ContentCache>,
    cancel: Option<&CancellationToken>,
    on_progress: &(dyn Fn(usize, usize) + Send + Sync),
) -> Result<DirComparison>
where
    L: NodeFileSystem<Nid = u64>,
    R: NodeFileSystem<Nid = u64>,
{
    let candidates: Vec<usize> = comparison
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            matches!(
                e.status,
                DirEntryStatus::Same | DirEntryStatus::SameBinary
            ) && e.left.as_ref().is_some_and(|l| {
                !l.is_dir
                    && l.size == e.right.as_ref().map_or(u64::MAX, |r| r.size)
            }) && e.right.as_ref().is_some_and(|r| !r.is_dir)
        })
        .map(|(i, _)| i)
        .collect();
    let total = candidates.len();
    on_progress(0, total);

    for (done, &index) in candidates.iter().enumerate() {
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(FsError::Cancelled);
        }
        // The side `path`s are relative to the comparison roots.
        let (left_file, right_file) = {
            let entry = &comparison.entries[index];
            let left_info =
                entry.left.as_ref().expect("candidate has a left side");
            let right_info =
                entry.right.as_ref().expect("candidate has a right side");
            (
                left_path.join(&left_info.path),
                right_path.join(&right_info.path),
            )
        };
        let left_hash = side_content_hash(left_fs, &left_file, cache).await;
        let right_hash = side_content_hash(right_fs, &right_file, cache).await;
        let differs = match (&left_hash, &right_hash) {
            (Ok(lh), Ok(rh)) => lh != rh,
            _ => true,
        };
        if differs {
            let old = comparison.entries[index].status;
            comparison.entries[index].status = DirEntryStatus::Different;
            if let Some(count) = comparison.counts.get_mut(&old) {
                *count -= 1;
            }
            *comparison
                .counts
                .entry(DirEntryStatus::Different)
                .or_insert(0) += 1;
        }
        on_progress(done + 1, total);
    }

    Ok(comparison)
}

/// Compute the blake3 hash of one file, consulting the optional
/// [`ContentCache`] and the node's cached hash before reading content.
async fn side_content_hash<N>(
    fs: &N,
    path: &Path,
    cache: Option<&ContentCache>,
) -> Result<blake3::Hash>
where
    N: NodeFileSystem<Nid = u64>,
{
    let label = fs.label();
    if let Some(c) = cache
        && let Some(cid) = c.get(label, path)
        && let Ok(hash) = blake3::Hash::from_hex(&cid.hash)
    {
        return Ok(hash);
    }
    let id = fs.resolve_path(path).await?;
    let node = fs.get_node(id)?;
    let hash = match node
        .cached_hash()
        .and_then(|h| blake3::Hash::from_hex(h).ok())
    {
        Some(h) => h,
        None => {
            hash_and_cache_node(fs, id, FileId::new(*id.get()), &node).await?
        }
    };
    if let Some(c) = cache {
        c.insert(label, path, ContentId::from_blake3(node.metadata(), &hash));
    }
    Ok(hash)
}

/// Merge two lists of scanned sibling entries into a `DirComparison`.
///
/// For a pair of files the status is determined by
/// [`compare_file_status_node`]. For a pair of directories the children
/// already attached to the scan entries are merged recursively, so every
/// nesting level is compared exactly once. A directory that exists on only
/// one side is descended into as well, merging its children against an
/// empty list so that the entire orphan subtree appears in the comparison.
/// Directory entries whose subtree contains differences or orphan entries
/// get their placeholder `Same` status refined to `Different`.
///
/// A file is never compared against a directory: when one side has a file
/// and the other a directory under the same name, both halves are
/// reported as orphans of their own, and the directory half is walked like
/// any other orphan directory.
///
/// `left_fs` and `right_fs` address the left and right side of the
/// comparison; a file pair is hashed on the provider that owns it.
// The side parameters are positional by design (left and right, not
// source and destination), so threading them costs an argument.
#[allow(clippy::too_many_arguments)]
fn compare_children_node<'a, L, R>(
    left_fs: &'a L,
    right_fs: &'a R,
    left: &'a [ScanEntry],
    right: &'a [ScanEntry],
    left_root: &'a Path,
    right_root: &'a Path,
    config: &'a CompareConfig,
    cache: Option<&'a ContentCache>,
    cancel: Option<&'a CancellationToken>,
) -> BoxFuture<'a, Result<DirComparison>>
where
    L: NodeFileSystem<Nid = u64>,
    R: NodeFileSystem<Nid = u64>,
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
            // Abort the walk between entries when the caller cancelled
            // the comparison; a partial tree is never a valid result.
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(FsError::Cancelled);
            }
            let left_entry = left_map.get(name).copied();
            let right_entry = right_map.get(name).copied();

            let merged = match merge_entry_sync_node(
                left_fs,
                right_fs,
                left_root,
                left_entry,
                right_root,
                right_entry,
                config,
                cache,
            )
            .await
            {
                Ok(entries) => entries,
                Err(e) => {
                    comparison.errors.push(e);
                    continue;
                }
            };

            // A name clash between a file and a directory yields two
            // orphan entries; every other merge yields exactly one.
            for mut entry in merged {
                // Recurse one level into the children that the scan already
                // attached to the entries. This covers a directory pair and
                // an orphan directory alike: for an orphan directory the
                // absent side contributes an empty child list, so the whole
                // subtree is compared and its entries stay left/right-only.
                // The decision is made from the merged entry's own sides so
                // that the directory half of a file-vs-directory clash is
                // descended into while the colliding file is ignored.
                let left_is_dir =
                    entry.left.as_ref().is_some_and(|i| i.is_dir);
                let right_is_dir =
                    entry.right.as_ref().is_some_and(|i| i.is_dir);
                let descend = (left_is_dir || right_is_dir)
                    && entry.left.as_ref().is_none_or(|i| i.is_dir)
                    && entry.right.as_ref().is_none_or(|i| i.is_dir);

                if descend {
                    let lc = if left_is_dir {
                        left_entry.and_then(|e| e.children()).unwrap_or(&[])
                    } else {
                        &[]
                    };
                    let rc = if right_is_dir {
                        right_entry.and_then(|e| e.children()).unwrap_or(&[])
                    } else {
                        &[]
                    };

                    let sub = compare_children_node(
                        left_fs, right_fs, lc, rc, left_root, right_root,
                        config, cache, cancel,
                    )
                    .await?;

                    // The subtree counts as different when it contains
                    // content differences or orphan entries.
                    let has_changes =
                        sub.different_count() > 0 || sub.orphan_count() > 0;
                    entry.sub_entries = Some(Arc::new(sub));
                    // Only refine the `Same` placeholder of a shared
                    // directory; an orphan directory keeps its
                    // left/right-only status regardless of its contents.
                    if has_changes && entry.status == DirEntryStatus::Same {
                        entry.status = DirEntryStatus::Different;
                    }
                }

                *comparison.counts.entry(entry.status).or_insert(0) += 1;
                comparison.entries.push(entry);
            }
        }

        Ok(comparison)
    })
}

/// Merge a single pair of entries into one or more `DirEntry` rows. The
/// side parameters decide which provider a file pair is hashed on.
///
/// A file is never compared against a directory: when the sides share a
/// name but differ in type, the result is two orphan rows (`LeftOnly` for
/// the left half, `RightOnly` for the right half) instead of a paired
/// row. Every other merge produces exactly one row.
#[allow(clippy::too_many_arguments)]
async fn merge_entry_sync_node<L, R>(
    left_fs: &L,
    right_fs: &R,
    left_root: &Path,
    left: Option<&ScanEntry>,
    right_root: &Path,
    right: Option<&ScanEntry>,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
) -> Result<Vec<DirEntry>>
where
    L: NodeFileSystem<Nid = u64>,
    R: NodeFileSystem<Nid = u64>,
{
    let name = left
        .map(|e| e.name.as_str())
        .or(right.map(|e| e.name.as_str()))
        .unwrap_or("")
        .to_string();

    let left_info = left.map(scan_entry_to_info);
    let right_info = right.map(scan_entry_to_info);

    if let (Some(le), Some(re)) = (left, right)
        && le.meta.is_dir != re.meta.is_dir
    {
        // A file colliding with a directory is not a comparison
        // candidate: each side becomes an orphan entry of its own.
        return Ok(vec![
            DirEntry {
                name: name.clone(),
                status: DirEntryStatus::LeftOnly,
                left: left_info,
                right: None,
                center: None,
                sub_entries: None,
                name_conflict: true,
            },
            DirEntry {
                name,
                status: DirEntryStatus::RightOnly,
                left: None,
                right: right_info,
                center: None,
                sub_entries: None,
                name_conflict: true,
            },
        ]);
    }

    let status = if let (Some(le), Some(re)) = (left, right) {
        // The type clash was handled above, so a pair is now either two
        // directories or two files.
        if le.meta.is_dir {
            DirEntryStatus::Same // placeholder; refined by caller
        } else {
            compare_file_status_node(
                left_fs, right_fs, left_root, le, right_root, re, config,
                cache,
            )
            .await?
        }
    } else if left.is_some() {
        DirEntryStatus::LeftOnly
    } else {
        DirEntryStatus::RightOnly
    };

    Ok(vec![DirEntry {
        name,
        status,
        left: left_info,
        right: right_info,
        center: None,
        sub_entries: None,
        name_conflict: false,
    }])
}

/// Node-aware file comparison. Uses node IDs for hashing instead of
/// paths, querying each side's own provider: the left hash is computed
/// on `left_fs`, the right hash on `right_fs`.
#[allow(clippy::too_many_arguments)]
async fn compare_file_status_node<L, R>(
    left_fs: &L,
    right_fs: &R,
    left_root: &Path,
    left: &ScanEntry,
    right_root: &Path,
    right: &ScanEntry,
    config: &CompareConfig,
    cache: Option<&ContentCache>,
) -> Result<DirEntryStatus>
where
    L: NodeFileSystem<Nid = u64>,
    R: NodeFileSystem<Nid = u64>,
{
    if !config.compare_content {
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

    // Same size — resolve to node IDs and compare hashes, each side on
    // its own provider.
    let left_path = left_root.join(&left.path);
    let right_path = right_root.join(&right.path);
    let left_label = left_fs.label();
    let right_label = right_fs.label();

    let left_id = left_fs.resolve_path(&left_path).await.ok();
    let right_id = right_fs.resolve_path(&right_path).await.ok();

    // Check the optional global cache first. Labels are taken per side,
    // but the cache itself is shared across the sides.
    if let Some(c) = cache {
        let left_cid = c.get(left_label, &left_path);
        let right_cid = c.get(right_label, &right_path);
        if matches!(
            (&left_cid, &right_cid),
            (Some(lc), Some(rc)) if lc.hash == rc.hash
        ) {
            return Ok(DirEntryStatus::Same);
        }
    }

    // Helper: get the hash for a file on its side's provider, checking
    // the node cache first, then computing and caching.
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

    // A side whose node cannot be resolved or read stays without a
    // hash, which classifies the pair as `Different` — unknown content
    // is never assumed equal.
    let left_hash = match left_id {
        Some(id) => match left_fs.get_node(id) {
            Ok(node) => get_hash(left_fs, id, &node).await,
            Err(_) => None,
        },
        None => None,
    };

    let right_hash = match right_id {
        Some(id) => match right_fs.get_node(id) {
            Ok(node) => get_hash(right_fs, id, &node).await,
            Err(_) => None,
        },
        None => None,
    };

    let (Some(lh), Some(rh)) = (left_hash, right_hash) else {
        return Ok(DirEntryStatus::Different);
    };

    // Store in the optional global cache.
    if let Some(c) = cache {
        let left_cid = ContentId::from_blake3(&left.meta, &lh);
        let right_cid = ContentId::from_blake3(&right.meta, &rh);
        c.insert(left_label, &left_path, left_cid);
        c.insert(right_label, &right_path, right_cid);
    }

    if lh == rh {
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
    use std::{env, path::Path};

    use super::*;
    use crate::{FsError, FsOperation, local::LocalFs, mockfs::MockFs};

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
            None,
        )
        .await
        .unwrap();

        assert!(result.same_count() > 0);
        assert!(result.different_count() > 0);
        assert!(result.orphan_count() > 0);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn node_compare_meta_data_only() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");
        let cache = ContentCache::default_config();
        let config = CompareConfig::meta_data_only();

        let result = compare_directories_node(
            &fs,
            &left,
            &right,
            &config,
            Some(&cache),
            None,
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
            None,
        )
        .await
        .unwrap();

        check_nested_differences(&result);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    // -----------------------------------------------------------------------
    // Orphan subtree tests
    // -----------------------------------------------------------------------

    /// Create a fixture where orphan directories nest further entries:
    /// a shared dir containing a left-only subdir, a multi-level left-only
    /// dir, a right-only dir, and a dir-vs-file name clash.
    fn make_orphan_dirs() -> (std::path::PathBuf, std::path::PathBuf) {
        let base = env::temp_dir()
            .join(format!("cocomo_orphan_{}", std::process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        // Shared dir containing a left-only sub-dir with a nested file.
        fs_err::create_dir_all(left.join("shared/lo-sub/deeper")).unwrap();
        fs_err::create_dir_all(right.join("shared")).unwrap();
        fs_err::write(left.join("shared/lo-sub/deeper/f.txt"), "f").unwrap();

        // Multi-level orphan dir at the root.
        fs_err::create_dir_all(left.join("lo-dir/inner")).unwrap();
        fs_err::write(left.join("lo-dir/inner/g.txt"), "g").unwrap();

        // Right-only dir with a nested file.
        fs_err::create_dir_all(right.join("ro-dir")).unwrap();
        fs_err::write(right.join("ro-dir/h.txt"), "h").unwrap();

        (left, right)
    }

    /// Verify that the whole subtree of an orphan directory is compared and
    /// classified as orphan on the owning side.
    fn check_orphan_subtrees(comp: &DirComparison) {
        let mut flat = Vec::new();
        flat_statuses(comp, "", &mut flat);

        for path in [
            "lo-dir",
            "lo-dir/inner",
            "lo-dir/inner/g.txt",
            "shared/lo-sub",
            "shared/lo-sub/deeper",
            "shared/lo-sub/deeper/f.txt",
        ] {
            assert_eq!(
                status_of(comp, path),
                Some(DirEntryStatus::LeftOnly),
                "`{path}` should be left-only"
            );
        }
        for path in ["ro-dir", "ro-dir/h.txt"] {
            assert_eq!(
                status_of(comp, path),
                Some(DirEntryStatus::RightOnly),
                "`{path}` should be right-only"
            );
        }

        // The orphan directory itself keeps its orphan status even though
        // its subtree is now compared.
        let lo = find_entry(comp, "lo-dir").expect("`lo-dir` missing");
        assert_eq!(lo.status, DirEntryStatus::LeftOnly);
        assert!(
            lo.sub_entries.is_some(),
            "orphan subtree should be attached"
        );
    }

    #[tokio::test]
    async fn node_compare_walks_into_orphan_dirs() {
        let (left, right) = make_orphan_dirs();
        let fs = LocalFs::new("test");
        let cache = ContentCache::default_config();
        let config = CompareConfig::full();

        let result = compare_directories_node(
            &fs,
            &left,
            &right,
            &config,
            Some(&cache),
            None,
        )
        .await
        .unwrap();

        check_orphan_subtrees(&result);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    /// A paired directory whose subtree contains only orphan changes is
    /// flagged `Different`, the refinement propagates through every parent
    /// level, and a fully identical directory pair stays `Same`.
    #[tokio::test]
    async fn node_compare_flags_dirs_containing_orphans() {
        let base = env::temp_dir()
            .join(format!("cocomo_orphan_flag_{}", std::process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        // Paired dir whose only difference is a new left-only file.
        fs_err::create_dir_all(left.join("shared")).unwrap();
        fs_err::create_dir_all(right.join("shared")).unwrap();
        fs_err::write(left.join("shared/same.txt"), "same").unwrap();
        fs_err::write(right.join("shared/same.txt"), "same").unwrap();
        fs_err::write(left.join("shared/new.txt"), "new").unwrap();

        // Orphan three levels down: every parent must be flagged.
        fs_err::create_dir_all(left.join("nest/a/b")).unwrap();
        fs_err::create_dir_all(right.join("nest/a/b")).unwrap();
        fs_err::write(left.join("nest/a/b/deep-new.txt"), "d").unwrap();

        // Fully identical paired dir stays `Same`.
        fs_err::create_dir_all(left.join("equal")).unwrap();
        fs_err::create_dir_all(right.join("equal")).unwrap();
        fs_err::write(left.join("equal/e.txt"), "e").unwrap();
        fs_err::write(right.join("equal/e.txt"), "e").unwrap();

        let fs = LocalFs::new("test");
        let config = CompareConfig::full();

        let result =
            compare_directories_node(&fs, &left, &right, &config, None, None)
                .await
                .unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        assert_eq!(
            status_of(&result, "shared"),
            Some(DirEntryStatus::Different),
            "dir with an orphan-only change should be flagged"
        );
        assert_eq!(
            status_of(&result, "shared/same.txt"),
            Some(DirEntryStatus::Same)
        );
        assert_eq!(
            status_of(&result, "shared/new.txt"),
            Some(DirEntryStatus::LeftOnly)
        );
        for path in ["nest", "nest/a", "nest/a/b"] {
            assert_eq!(
                status_of(&result, path),
                Some(DirEntryStatus::Different),
                "`{path}` should carry the orphan refinement"
            );
        }
        assert_eq!(
            status_of(&result, "equal"),
            Some(DirEntryStatus::Same),
            "untouched dir must stay same"
        );
        assert_eq!(
            status_of(&result, "equal/e.txt"),
            Some(DirEntryStatus::Same)
        );

        fs_err::remove_dir_all(&base).ok();
    }

    // -----------------------------------------------------------------------
    // File-vs-directory clash tests
    // -----------------------------------------------------------------------

    /// Look up all top-level entries with the given name.
    fn entries_named<'a>(
        comp: &'a DirComparison,
        name: &str,
    ) -> Vec<&'a DirEntry> {
        comp.entries.iter().filter(|e| e.name == name).collect()
    }

    /// A file and a directory that share a name are never compared
    /// against each other: both halves appear as orphan rows, the file
    /// half is not descended into, and the directory half is walked like
    /// any other orphan directory.
    #[tokio::test]
    async fn node_compare_reports_file_dir_clash_as_orphans() {
        let base = env::temp_dir()
            .join(format!("cocomo_clash_{}", std::process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        // Left: file `clash`, directory `dir-clash` with a nested file.
        fs_err::create_dir_all(left.join("dir-clash/inner")).unwrap();
        fs_err::write(left.join("clash"), "i am a file").unwrap();
        fs_err::write(left.join("dir-clash/inner/n.txt"), "n").unwrap();

        // Right: directory `clash` with a nested file, file `dir-clash`.
        fs_err::create_dir_all(right.join("clash")).unwrap();
        fs_err::write(right.join("clash/m.txt"), "m").unwrap();
        fs_err::write(right.join("dir-clash"), "i am a file").unwrap();

        let fs = LocalFs::new("test");
        let config = CompareConfig::meta_data_only();

        let result =
            compare_directories_node(&fs, &left, &right, &config, None, None)
                .await
                .unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);

        // `clash`: a left-only file row and a right-only directory row.
        let clash = entries_named(&result, "clash");
        assert_eq!(clash.len(), 2, "both clash halves must be reported");
        assert_eq!(clash[0].status, DirEntryStatus::LeftOnly);
        assert!(
            clash[0].left.as_ref().is_some_and(|i| !i.is_dir),
            "left half should be a file"
        );
        assert!(clash[0].right.is_none(), "left half must not pair up");
        assert!(
            clash[0].sub_entries.is_none(),
            "the colliding file must not be descended into"
        );
        assert_eq!(clash[1].status, DirEntryStatus::RightOnly);
        assert!(
            clash[1].right.as_ref().is_some_and(|i| i.is_dir),
            "right half should be a directory"
        );
        assert!(clash[1].left.is_none(), "right half must not pair up");

        // The directory half is walked like any other orphan directory.
        let sub_clash = clash[1]
            .sub_entries
            .as_ref()
            .expect("subtree of the orphan directory half missing");
        assert_eq!(
            status_of(sub_clash, "m.txt"),
            Some(DirEntryStatus::RightOnly)
        );

        // The mirrored clash (`dir-clash`) behaves the same on the other
        // side: left-only directory with subtree, right-only file.
        let dir_clash = entries_named(&result, "dir-clash");
        assert_eq!(dir_clash.len(), 2);
        assert_eq!(dir_clash[0].status, DirEntryStatus::LeftOnly);
        let sub_dir = dir_clash[0]
            .sub_entries
            .as_ref()
            .expect("subtree of the orphan directory half missing");
        assert_eq!(
            status_of(sub_dir, "inner/n.txt"),
            Some(DirEntryStatus::LeftOnly)
        );
        assert_eq!(dir_clash[1].status, DirEntryStatus::RightOnly);
        assert!(dir_clash[1].sub_entries.is_none());

        // All four clash rows count as orphans; none as differences.
        assert_eq!(
            result.orphan_count(),
            4,
            "both clash halves must count as orphans"
        );
        assert_eq!(result.different_count(), 0);

        fs_err::remove_dir_all(&base).ok();
    }

    // -----------------------------------------------------------------------
    // Pair (two-provider) compare tests
    // -----------------------------------------------------------------------

    /// Recursively collect `(path, status)` pairs, joining entry names
    /// with `/`.
    fn flat_statuses(
        comp: &DirComparison,
        prefix: &str,
        out: &mut Vec<(String, DirEntryStatus)>,
    ) {
        for entry in &comp.entries {
            let path = if prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{prefix}/{}", entry.name)
            };
            out.push((path.clone(), entry.status));
            if let Some(sub) = &entry.sub_entries {
                flat_statuses(sub, &path, out);
            }
        }
    }

    /// Look up the status of an entry by its `/`-separated relative path.
    fn status_of(comp: &DirComparison, path: &str) -> Option<DirEntryStatus> {
        let parts: Vec<&str> = path.split('/').collect();
        let mut comp = comp;
        for (i, part) in parts.iter().enumerate() {
            let entry = comp.entries.iter().find(|e| e.name == *part)?;
            if i + 1 == parts.len() {
                return Some(entry.status);
            }
            comp = &**entry.sub_entries.as_ref()?;
        }
        None
    }

    /// Build a fixture pair `(left, right)` under a fresh temp dir: an
    /// equal file, a same-size different-content file, a within-tolerance
    /// size pair, one orphan per side, and a nested directory pair whose
    /// child differs.
    fn make_pair_fixture() -> (std::path::PathBuf, std::path::PathBuf) {
        let base = env::temp_dir()
            .join(format!("cocomo_pair_parity_{}", std::process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        fs_err::create_dir_all(left.join("common")).unwrap();
        fs_err::create_dir_all(right.join("common")).unwrap();
        fs_err::write(left.join("same.txt"), "hello").unwrap();
        fs_err::write(right.join("same.txt"), "hello").unwrap();
        fs_err::write(left.join("diff.txt"), "version 1").unwrap();
        fs_err::write(right.join("diff.txt"), "version 2").unwrap();
        fs_err::write(left.join("tol.txt"), [b'x'; 10]).unwrap();
        fs_err::write(right.join("tol.txt"), [b'x'; 11]).unwrap();
        fs_err::write(left.join("l-only.txt"), "only left").unwrap();
        fs_err::write(right.join("r-only.txt"), "only right").unwrap();
        fs_err::write(left.join("common/inner.txt"), "aaaaaaaaaa").unwrap();
        fs_err::write(right.join("common/inner.txt"), "bbbbbbbbbb").unwrap();

        (left, right)
    }

    /// A compare across two provider instances must yield exactly the
    /// same (path, status) pairs as the same compare on one shared
    /// provider, including entries nested in subdirectories.
    #[tokio::test]
    async fn pair_compare_matches_single_fs_compare() {
        let (left, right) = make_pair_fixture();
        let config = CompareConfig::full();

        let single_fs = LocalFs::new("single");
        let single_cache = ContentCache::default_config();
        let single = compare_directories_node(
            &single_fs,
            &left,
            &right,
            &config,
            Some(&single_cache),
            None,
        )
        .await
        .unwrap();

        let left_fs = LocalFs::new("left");
        let right_fs = LocalFs::new("right");
        let pair_cache = ContentCache::default_config();
        let pair = compare_directories_pair_node(
            &left_fs,
            &right_fs,
            &left,
            &right,
            &config,
            Some(&pair_cache),
            None,
        )
        .await
        .unwrap();

        let mut single_flat = Vec::new();
        flat_statuses(&single, "", &mut single_flat);
        let mut pair_flat = Vec::new();
        flat_statuses(&pair, "", &mut pair_flat);
        assert_eq!(single_flat, pair_flat);

        assert!(pair.errors.is_empty(), "errors: {:?}", pair.errors);
        assert!(pair.same_count() > 0);
        assert!(pair.different_count() > 0);
        assert!(pair.orphan_count() > 0);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    /// A pair compare with the right side on a `MockFs` tree must
    /// classify every pair on its own provider, and an injected stream
    /// error on the right side must degrade to `Different` instead of
    /// panicking.
    #[tokio::test]
    async fn pair_compare_cross_provider_mockfs_vs_localfs() {
        let base = env::temp_dir()
            .join(format!("cocomo_pair_cross_{}", std::process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("local");
        fs_err::create_dir_all(left.join("d")).unwrap();
        fs_err::write(left.join("same.txt"), "hello").unwrap();
        fs_err::write(left.join("diff.txt"), "version 1").unwrap();
        fs_err::write(left.join("tol.txt"), [b'x'; 10]).unwrap();
        fs_err::write(left.join("l-only.txt"), "only left").unwrap();
        fs_err::write(left.join("d/inner.txt"), "aaaaaaaaaa").unwrap();

        let mirror = MockFs::new("mirror")
            .with_dir("/mirror")
            .with_file("/mirror/same.txt", "hello")
            .with_file("/mirror/diff.txt", "version 2")
            .with_file("/mirror/tol.txt", [b'x'; 11])
            .with_file("/mirror/r-only.txt", "only right")
            .with_dir("/mirror/d")
            .with_file("/mirror/d/inner.txt", "bbbbbbbbbb");

        let left_fs = LocalFs::new("local");
        let config = CompareConfig::full();

        let result = compare_directories_pair_node(
            &left_fs,
            &mirror,
            &left,
            Path::new("/mirror"),
            &config,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(result.errors.is_empty(), "errors: {:?}", result.errors);
        assert_eq!(status_of(&result, "same.txt"), Some(DirEntryStatus::Same));
        assert_eq!(
            status_of(&result, "diff.txt"),
            Some(DirEntryStatus::Different)
        );
        assert_eq!(
            status_of(&result, "tol.txt"),
            Some(DirEntryStatus::Similar)
        );
        assert_eq!(
            status_of(&result, "l-only.txt"),
            Some(DirEntryStatus::LeftOnly)
        );
        assert_eq!(
            status_of(&result, "r-only.txt"),
            Some(DirEntryStatus::RightOnly)
        );
        assert_eq!(
            status_of(&result, "d/inner.txt"),
            Some(DirEntryStatus::Different)
        );

        // The same tree, but with a stream error injected on the equal
        // file: its hash cannot be computed on the right provider, so the
        // pair must degrade to `Different` instead of panicking.
        let mirror_err = MockFs::new("mirror-err")
            .with_dir("/mirror")
            .with_file("/mirror/same.txt", "hello")
            .with_file("/mirror/diff.txt", "version 2")
            .with_file("/mirror/tol.txt", [b'x'; 11])
            .with_file("/mirror/r-only.txt", "only right")
            .with_dir("/mirror/d")
            .with_file("/mirror/d/inner.txt", "bbbbbbbbbb")
            .with_stream_error(
                "/mirror/same.txt",
                FsError::Io {
                    operation: FsOperation::Read,
                    path: "/mirror/same.txt".into(),
                    message: "injected read failure".to_owned(),
                },
            );

        let result2 = compare_directories_pair_node(
            &left_fs,
            &mirror_err,
            &left,
            Path::new("/mirror"),
            &config,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(result2.errors.is_empty(), "errors: {:?}", result2.errors);
        assert_eq!(
            status_of(&result2, "same.txt"),
            Some(DirEntryStatus::Different)
        );

        fs_err::remove_dir_all(&base).ok();
    }

    // -----------------------------------------------------------------------
    // Cancellation tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn cancelled_token_aborts_comparison() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");
        let token = CancellationToken::new();
        token.cancel();

        let result = compare_directories_node(
            &fs,
            &left,
            &right,
            &CompareConfig::meta_data_only(),
            None,
            Some(&token),
        )
        .await;

        assert!(matches!(result, Err(FsError::Cancelled)));
        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    // -----------------------------------------------------------------------
    // Background content pass tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn content_pass_refines_equal_size_pairs() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");

        // Metadata alone cannot tell `diff.txt` apart: same size, so it
        // is marked `Same` by the first pass.
        let comparison = compare_directories_node(
            &fs,
            &left,
            &right,
            &CompareConfig::meta_data_only(),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            status_of(&comparison, "diff.txt"),
            Some(DirEntryStatus::Same)
        );

        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = events.clone();
        let refined = compare_content_pair_node(
            &fs,
            &fs,
            &left,
            &right,
            comparison,
            None,
            None,
            &move |done, total| sink.lock().unwrap().push((done, total)),
        )
        .await
        .unwrap();

        // The same-size pair with different content is downgraded; the
        // genuinely equal file keeps its status.
        assert_eq!(
            status_of(&refined, "diff.txt"),
            Some(DirEntryStatus::Different)
        );
        assert_eq!(
            status_of(&refined, "same.txt"),
            Some(DirEntryStatus::Same)
        );
        // The status counts follow the refinement (`common` is a shared
        // directory that stays `Same`).
        assert_eq!(refined.counts.get(&DirEntryStatus::Same), Some(&2));
        assert_eq!(refined.counts.get(&DirEntryStatus::Different), Some(&1));

        // Progress starts at zero and ends at the candidate total (the
        // two shared files; the orphan pair is not a candidate).
        let events = events.lock().unwrap();
        assert_eq!(events.first(), Some(&(0, 2)));
        assert_eq!(events.last(), Some(&(2, 2)));

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn content_pass_honours_cancellation() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");
        let comparison = compare_directories_node(
            &fs,
            &left,
            &right,
            &CompareConfig::meta_data_only(),
            None,
            None,
        )
        .await
        .unwrap();

        let token = CancellationToken::new();
        token.cancel();
        let result = compare_content_pair_node(
            &fs,
            &fs,
            &left,
            &right,
            comparison,
            None,
            Some(&token),
            &|_done, _total| {},
        )
        .await;

        assert!(matches!(result, Err(FsError::Cancelled)));
        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }
}
