// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Synchronization engine that applies sync strategies over directory
//! comparison results.
//!
//! A [`SyncOperation`] defines the strategy (mirror, update newer, etc.).
//! The engine compares two directory trees, plans the necessary transfers,
//! and executes them using the node-based API. Both the comparison and
//! the executor address the two sides through an [`FsPair`]: one
//! provider instance shared by both sides, or two distinct instances.
//!
//! # Dry-run mode
//!
//! When `dry_run` is `true`, the engine plans all transfers but does not
//! execute them. The returned [`SyncResult`] contains the planned actions
//! for review.

use std::path::Path;

use crate::{
    CompareConfig, DirComparison, DirEntryStatus,
    compare::{compare_directories_node, compare_directories_pair_node},
    transfer::{
        FsPair, TransferAction, TransferItem, TransferResult,
        execute_transfers,
    },
};
// Used in tests via `use super::*`.
#[allow(unused_imports)]
use crate::{DirEntry, EntryInfo};

// ---------------------------------------------------------------------------
// Sync operations
// ---------------------------------------------------------------------------

/// The synchronization strategy to apply.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SyncOperation {
    /// Make right match left exactly. Copies left-only and different files to
    /// right, deletes right-only files.
    #[default]
    MirrorLeft,
    /// Make left match right exactly. Copies right-only and different files to
    /// left, deletes left-only files.
    MirrorRight,
    /// Copy newer files over older ones.
    UpdateNewer,
    /// Copy newer files in both directions.
    UpdateBoth,
    /// Copy left-only files to right. Does not delete anything.
    CopyLeft,
    /// Copy right-only files to left. Does not delete anything.
    CopyRight,
    /// Copy only newer files.
    CopyNewer,
    /// Delete files that exist on one side only.
    DeleteOrphans,
}

impl SyncOperation {
    /// Return a human-readable label for this operation.
    pub fn label(&self) -> &'static str {
        match self {
            SyncOperation::MirrorLeft => "mirror left → right",
            SyncOperation::MirrorRight => "mirror right → left",
            SyncOperation::UpdateNewer => "update newer",
            SyncOperation::UpdateBoth => "update both",
            SyncOperation::CopyLeft => "copy left → right",
            SyncOperation::CopyRight => "copy right → left",
            SyncOperation::CopyNewer => "copy newer",
            SyncOperation::DeleteOrphans => "delete orphans",
        }
    }
}

// ---------------------------------------------------------------------------
// Sync rules
// ---------------------------------------------------------------------------

/// Configuration for a synchronization operation.
#[derive(Clone, Debug, Default)]
pub struct SyncRules {
    /// The sync strategy to apply.
    pub operation: SyncOperation,
    /// Whether to execute the sync or just plan it.
    pub dry_run: bool,
    /// Maximum depth for recursive comparison. `None` means unlimited.
    pub max_depth: Option<usize>,
    /// Compare file contents. If `false`, uses size/mtime only.
    pub compare_files: bool,
}

// ---------------------------------------------------------------------------
// Sync result
// ---------------------------------------------------------------------------

/// Result of a synchronization operation.
#[derive(Clone, Debug, Default)]
pub struct SyncResult {
    /// The transfer result, if transfers were executed.
    pub transfer: Option<TransferResult>,
    /// Planned transfer items (present in dry-run mode or alongside
    /// execution).
    pub planned: Vec<TransferItem>,
    /// Errors encountered during comparison or planning.
    pub errors: Vec<crate::FsError>,
}

impl SyncResult {
    /// Return the total number of planned operations.
    pub fn planned_count(&self) -> usize {
        self.planned.len()
    }

    /// Return `true` if this is a dry-run result (no transfers executed).
    pub fn is_dry_run(&self) -> bool {
        self.transfer.is_none()
    }
}

// ---------------------------------------------------------------------------
// Sync pipeline
// ---------------------------------------------------------------------------

/// Plan all transfers for a pair of filesystem providers without executing
/// them.
///
/// Compares the two directory trees and returns a [`SyncResult`] with the
/// planned actions. No I/O is performed beyond the comparison scan. With a
/// [`FsPair::Separate`] pair each side is scanned on its own provider;
/// [`plan_sync`] is the thin single-provider wrapper around this function.
pub async fn plan_sync_pair<L, R>(
    pair: FsPair<'_, L, R>,
    left_path: &Path,
    right_path: &Path,
    rules: &SyncRules,
) -> crate::Result<SyncResult>
where
    L: crate::NodeFileSystem<Nid = u64>,
    R: crate::NodeFileSystem<Nid = u64>,
{
    let cache = crate::hash::ContentCache::default_config();
    let compare_config = CompareConfig {
        compare_files: rules.compare_files,
        compare_structure: true,
        follow_symlinks: false,
        max_depth: rules.max_depth,
        size_tolerance: 0.1,
    };

    let comparison = match pair {
        // One provider instance addresses both sides.
        FsPair::Shared(fs) => {
            compare_directories_node(
                fs,
                left_path,
                right_path,
                &compare_config,
                Some(&cache),
            )
            .await?
        }
        // Two distinct provider instances, one per side.
        FsPair::Separate(left_fs, right_fs) => {
            compare_directories_pair_node(
                left_fs,
                right_fs,
                left_path,
                right_path,
                &compare_config,
                Some(&cache),
            )
            .await?
        }
    };

    let planned = plan_sync_items(&comparison, rules.operation);

    Ok(SyncResult {
        transfer: None,
        planned,
        // Surface non-fatal scan errors instead of dropping them.
        errors: comparison.errors,
    })
}

/// Plan all transfers for a given sync operation without executing them.
///
/// Thin wrapper around [`plan_sync_pair`] that addresses both sides with
/// the same provider instance.
pub async fn plan_sync<N>(
    fs: &N,
    left_path: &Path,
    right_path: &Path,
    rules: &SyncRules,
) -> crate::Result<SyncResult>
where
    N: crate::NodeFileSystem<Nid = u64>,
{
    plan_sync_pair(FsPair::shared(fs), left_path, right_path, rules).await
}

/// Plan all transfers for a pair of filesystem providers and execute them.
///
/// This is the main entry point for synchronization. It compares the two
/// directory trees, plans the necessary transfers, and executes them if
/// `dry_run` is `false`. Cross-boundary copies and moves in a
/// [`FsPair::Separate`] pair stream the source content into a freshly
/// created destination entry (directories are mirrored recursively, moves
/// run as copy + delete).
pub async fn sync_directories_pair<L, R>(
    pair: FsPair<'_, L, R>,
    left_path: &Path,
    right_path: &Path,
    rules: &SyncRules,
) -> crate::Result<SyncResult>
where
    L: crate::fs::WritableFileSystem<Nid = u64>,
    R: crate::fs::WritableFileSystem<Nid = u64>,
{
    // Plan the sync.
    let mut result =
        plan_sync_pair(pair, left_path, right_path, rules).await?;

    // Refuse to execute when the planning comparison carried errors: a
    // missing entry could turn a mirror into a mass deletion. Dry runs keep
    // the errors in the result so the caller can surface them.
    if !rules.dry_run && !result.errors.is_empty() {
        return Err(crate::FsError::Incomplete {
            errors: std::mem::take(&mut result.errors),
        });
    }

    if rules.dry_run || result.planned.is_empty() {
        result.transfer = Some(TransferResult::default());
        return Ok(result);
    }

    // Execute the planned transfers.
    let transfer_result =
        execute_transfers(&result.planned, left_path, right_path, pair).await;

    result.transfer = Some(transfer_result);
    Ok(result)
}

/// Plan all transfers and execute them with a single filesystem shared by
/// both sides.
///
/// Thin wrapper around [`sync_directories_pair`] that addresses both sides
/// with the same provider instance.
pub async fn sync_directories<N>(
    fs: &N,
    left_path: &Path,
    right_path: &Path,
    rules: &SyncRules,
) -> crate::Result<SyncResult>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    sync_directories_pair(FsPair::shared(fs), left_path, right_path, rules)
        .await
}

// ---------------------------------------------------------------------------
// Sync planning logic
// ---------------------------------------------------------------------------

/// Generate transfer items for a given sync operation.
fn plan_sync_items(
    comparison: &DirComparison,
    operation: SyncOperation,
) -> Vec<TransferItem> {
    match operation {
        SyncOperation::MirrorLeft => {
            let mut items = Vec::new();
            collect_mirror_items(comparison, &mut items, MirrorDir::Right);
            items
        }
        SyncOperation::MirrorRight => {
            let mut items = Vec::new();
            collect_mirror_items(comparison, &mut items, MirrorDir::Left);
            items
        }
        SyncOperation::UpdateNewer | SyncOperation::UpdateBoth => {
            collect_update_newer_items(comparison)
        }
        SyncOperation::CopyLeft => {
            // Copy right-only and different to left.
            let mut items = Vec::new();
            collect_copy_items(comparison, &mut items, CopyDir::ToLeft);
            items
        }
        SyncOperation::CopyRight => {
            // Copy left-only and different to right.
            let mut items = Vec::new();
            collect_copy_items(comparison, &mut items, CopyDir::ToRight);
            items
        }
        SyncOperation::CopyNewer => collect_update_newer_items(comparison),
        SyncOperation::DeleteOrphans => {
            let mut items = Vec::new();
            collect_orphan_delete_items(comparison, &mut items);
            items
        }
    }
}

/// Which direction to mirror.
#[derive(Clone, Copy, PartialEq)]
enum MirrorDir {
    /// Make right match left.
    Right,
    /// Make left match right.
    Left,
}

/// Collect transfer items for a mirror operation.
fn collect_mirror_items(
    comparison: &DirComparison,
    items: &mut Vec<TransferItem>,
    direction: MirrorDir,
) {
    for entry in &comparison.entries {
        match entry.status {
            DirEntryStatus::LeftOnly => {
                if direction == MirrorDir::Right {
                    // Copy left → right.
                    items.push(TransferItem::new(
                        TransferAction::CopyRight,
                        entry.name.clone(),
                        entry.left.as_ref().is_some_and(|l| l.is_dir),
                        entry
                            .left
                            .as_ref()
                            .map(|l| l.path.clone())
                            .unwrap_or_default(),
                    ));
                } else {
                    // Delete from left.
                    items.push(TransferItem::new(
                        TransferAction::DeleteLeft,
                        entry.name.clone(),
                        entry.left.as_ref().is_some_and(|l| l.is_dir),
                        entry
                            .left
                            .as_ref()
                            .map(|l| l.path.clone())
                            .unwrap_or_default(),
                    ));
                }
            }
            DirEntryStatus::RightOnly => {
                if direction == MirrorDir::Left {
                    // Copy right → left.
                    items.push(TransferItem::new(
                        TransferAction::CopyLeft,
                        entry.name.clone(),
                        entry.right.as_ref().is_some_and(|r| r.is_dir),
                        entry
                            .right
                            .as_ref()
                            .map(|r| r.path.clone())
                            .unwrap_or_default(),
                    ));
                } else {
                    // Delete from right.
                    items.push(TransferItem::new(
                        TransferAction::DeleteRight,
                        entry.name.clone(),
                        entry.right.as_ref().is_some_and(|r| r.is_dir),
                        entry
                            .right
                            .as_ref()
                            .map(|r| r.path.clone())
                            .unwrap_or_default(),
                    ));
                }
            }
            DirEntryStatus::Different | DirEntryStatus::Similar => {
                if direction == MirrorDir::Right {
                    // Copy left → right (overwrite).
                    items.push(TransferItem::new(
                        TransferAction::CopyRight,
                        entry.name.clone(),
                        entry.left.as_ref().is_some_and(|l| l.is_dir),
                        entry
                            .left
                            .as_ref()
                            .map(|l| l.path.clone())
                            .unwrap_or_default(),
                    ));
                } else {
                    // Copy right → left (overwrite).
                    items.push(TransferItem::new(
                        TransferAction::CopyLeft,
                        entry.name.clone(),
                        entry.right.as_ref().is_some_and(|r| r.is_dir),
                        entry
                            .right
                            .as_ref()
                            .map(|r| r.path.clone())
                            .unwrap_or_default(),
                    ));
                }
            }
            _ => {}
        }

        // Recurse into sub-directories.
        if let Some(ref sub) = entry.sub_entries {
            collect_mirror_items(sub, items, direction);
        }
    }
}

/// Which direction to copy.
#[derive(Clone, Copy, PartialEq)]
enum CopyDir {
    ToLeft,
    ToRight,
}

/// Collect copy items (no deletion).
fn collect_copy_items(
    comparison: &DirComparison,
    items: &mut Vec<TransferItem>,
    direction: CopyDir,
) {
    for entry in &comparison.entries {
        match entry.status {
            DirEntryStatus::LeftOnly => {
                if direction == CopyDir::ToRight {
                    items.push(TransferItem::new(
                        TransferAction::CopyRight,
                        entry.name.clone(),
                        entry.left.as_ref().is_some_and(|l| l.is_dir),
                        entry
                            .left
                            .as_ref()
                            .map(|l| l.path.clone())
                            .unwrap_or_default(),
                    ));
                }
            }
            DirEntryStatus::RightOnly => {
                if direction == CopyDir::ToLeft {
                    items.push(TransferItem::new(
                        TransferAction::CopyLeft,
                        entry.name.clone(),
                        entry.right.as_ref().is_some_and(|r| r.is_dir),
                        entry
                            .right
                            .as_ref()
                            .map(|r| r.path.clone())
                            .unwrap_or_default(),
                    ));
                }
            }
            DirEntryStatus::Different | DirEntryStatus::Similar => {
                if direction == CopyDir::ToRight {
                    items.push(TransferItem::new(
                        TransferAction::CopyRight,
                        entry.name.clone(),
                        entry.left.as_ref().is_some_and(|l| l.is_dir),
                        entry
                            .left
                            .as_ref()
                            .map(|l| l.path.clone())
                            .unwrap_or_default(),
                    ));
                } else {
                    items.push(TransferItem::new(
                        TransferAction::CopyLeft,
                        entry.name.clone(),
                        entry.right.as_ref().is_some_and(|r| r.is_dir),
                        entry
                            .right
                            .as_ref()
                            .map(|r| r.path.clone())
                            .unwrap_or_default(),
                    ));
                }
            }
            _ => {}
        }

        if let Some(ref sub) = entry.sub_entries {
            collect_copy_items(sub, items, direction);
        }
    }
}

/// Collect update-newer items.
fn collect_update_newer_items(
    comparison: &DirComparison,
) -> Vec<TransferItem> {
    let mut items = Vec::new();

    for entry in &comparison.entries {
        if matches!(
            entry.status,
            DirEntryStatus::Different | DirEntryStatus::Similar
        ) && let (Some(left), Some(right)) = (&entry.left, &entry.right)
        {
            // Compare modification times.
            let left_time = parse_mtime(&left.modified);
            let right_time = parse_mtime(&right.modified);

            if left_time > right_time {
                // Left is newer.
                items.push(TransferItem::new(
                    TransferAction::CopyRight,
                    entry.name.clone(),
                    left.is_dir,
                    left.path.clone(),
                ));
            } else if right_time > left_time {
                // Right is newer.
                items.push(TransferItem::new(
                    TransferAction::CopyLeft,
                    entry.name.clone(),
                    right.is_dir,
                    right.path.clone(),
                ));
            }
            // If equal, skip.
        }

        if let Some(ref sub) = entry.sub_entries {
            items.extend(collect_update_newer_items(sub));
        }
    }

    items
}

/// Collect orphan deletion items.
fn collect_orphan_delete_items(
    comparison: &DirComparison,
    items: &mut Vec<TransferItem>,
) {
    for entry in &comparison.entries {
        match entry.status {
            DirEntryStatus::LeftOnly => {
                items.push(TransferItem::new(
                    TransferAction::DeleteLeft,
                    entry.name.clone(),
                    entry.left.as_ref().is_some_and(|l| l.is_dir),
                    entry
                        .left
                        .as_ref()
                        .map(|l| l.path.clone())
                        .unwrap_or_default(),
                ));
            }
            DirEntryStatus::RightOnly => {
                items.push(TransferItem::new(
                    TransferAction::DeleteRight,
                    entry.name.clone(),
                    entry.right.as_ref().is_some_and(|r| r.is_dir),
                    entry
                        .right
                        .as_ref()
                        .map(|r| r.path.clone())
                        .unwrap_or_default(),
                ));
            }
            _ => {}
        }

        if let Some(ref sub) = entry.sub_entries {
            collect_orphan_delete_items(sub, items);
        }
    }
}

/// Parse modification time from RFC3339 string into a comparable value.
/// Returns a string for lexicographic comparison (sufficient for ordering).
fn parse_mtime(mtime_str: &str) -> &str {
    mtime_str
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        env,
        path::{Path, PathBuf},
        process,
    };

    use tempfile::TempDir;

    use super::*;
    use crate::{FsError, FsOperation, LocalFs, MockFs, fs::FileSystem};

    /// Read a file's content from a `MockFs` tree.
    async fn mock_content(fs: &MockFs, path: &str) -> Vec<u8> {
        fs.read(Path::new(path), None).await.unwrap().to_vec()
    }

    fn make_sync_dirs() -> (PathBuf, PathBuf) {
        let base =
            env::temp_dir().join(format!("cocomo_sync_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        fs_err::create_dir_all(&left).unwrap();
        fs_err::create_dir_all(&right).unwrap();

        // Left-only.
        fs_err::write(left.join("left_only.txt"), "left").unwrap();
        // Right-only.
        fs_err::write(right.join("right_only.txt"), "right").unwrap();
        // Same.
        fs_err::write(left.join("same.txt"), "identical").unwrap();
        fs_err::write(right.join("same.txt"), "identical").unwrap();

        (left, right)
    }

    // Helper for time-based sync tests (UpdateNewer/UpdateBoth).
    #[allow(dead_code)]
    fn make_time_dirs() -> (PathBuf, PathBuf) {
        let base = env::temp_dir()
            .join(format!("cocomo_sync_time_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        fs_err::create_dir_all(&left).unwrap();
        fs_err::create_dir_all(&right).unwrap();

        // Different content — mtime will be set by filesystem.
        fs_err::write(left.join("file.txt"), "version 1").unwrap();
        // Small delay to ensure different mtime.
        std::thread::sleep(std::time::Duration::from_millis(10));
        fs_err::write(right.join("file.txt"), "version 2").unwrap();

        (left, right)
    }

    /// Build a `left`/`right` directory pair under `base`: one file on
    /// the left side only, one on the right side only, and one
    /// identical file on both sides.
    fn make_sync_dirs_at(base: &Path) -> (PathBuf, PathBuf) {
        let left = base.join("left");
        let right = base.join("right");

        fs_err::create_dir_all(&left).unwrap();
        fs_err::create_dir_all(&right).unwrap();

        fs_err::write(left.join("left_only.txt"), "left").unwrap();
        fs_err::write(right.join("right_only.txt"), "right").unwrap();
        fs_err::write(left.join("same.txt"), "identical").unwrap();
        fs_err::write(right.join("same.txt"), "identical").unwrap();

        (left, right)
    }

    /// Flatten a plan into `(action label, name)` pairs, so single-`fs`
    /// and pair plans can be compared for equality.
    fn flatten_plan(items: &[TransferItem]) -> Vec<(&'static str, String)> {
        items
            .iter()
            .map(|item| (item.action.label(), item.name.clone()))
            .collect()
    }

    #[test]
    fn sync_operation_labels() {
        assert_eq!(SyncOperation::MirrorLeft.label(), "mirror left → right");
        assert_eq!(SyncOperation::DeleteOrphans.label(), "delete orphans");
        assert_eq!(SyncOperation::UpdateBoth.label(), "update both");
    }

    #[test]
    fn sync_result_dry_run() {
        let result = SyncResult {
            planned: vec![TransferItem::new(
                TransferAction::CopyLeft,
                "test.txt".to_string(),
                false,
                "/test.txt".to_string(),
            )],
            ..Default::default()
        };
        assert!(result.is_dry_run());
        assert_eq!(result.planned_count(), 1);
    }

    #[test]
    fn plan_mirror_items_left_only() {
        let comparison = DirComparison {
            entries: vec![
                DirEntry {
                    name: "left_only.txt".to_string(),
                    status: DirEntryStatus::LeftOnly,
                    left: Some(EntryInfo {
                        name: "left_only.txt".to_string(),
                        path: "/left/left_only.txt".to_string(),
                        size: 10,
                        modified: "2026-01-01T00:00:00Z".to_string(),
                        is_dir: false,
                        hash: None,
                    }),
                    right: None,
                    center: None,
                    sub_entries: None,
                },
                DirEntry {
                    name: "same.txt".to_string(),
                    status: DirEntryStatus::Same,
                    left: Some(EntryInfo {
                        name: "same.txt".to_string(),
                        path: "/left/same.txt".to_string(),
                        size: 10,
                        modified: "2026-01-01T00:00:00Z".to_string(),
                        is_dir: false,
                        hash: None,
                    }),
                    right: Some(EntryInfo {
                        name: "same.txt".to_string(),
                        path: "/right/same.txt".to_string(),
                        size: 10,
                        modified: "2026-01-01T00:00:00Z".to_string(),
                        is_dir: false,
                        hash: None,
                    }),
                    center: None,
                    sub_entries: None,
                },
            ],
            ..Default::default()
        };

        // Mirror left → right: copy left_only to right.
        let items = plan_sync_items(&comparison, SyncOperation::MirrorLeft);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action, TransferAction::CopyRight);
        assert_eq!(items[0].name, "left_only.txt");

        // Mirror right → left: delete left_only.
        let items = plan_sync_items(&comparison, SyncOperation::MirrorRight);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].action, TransferAction::DeleteLeft);
    }

    #[test]
    fn plan_delete_orphans() {
        let comparison = DirComparison {
            entries: vec![
                DirEntry {
                    name: "left_only.txt".to_string(),
                    status: DirEntryStatus::LeftOnly,
                    left: Some(EntryInfo {
                        name: "left_only.txt".to_string(),
                        path: "/left/left_only.txt".to_string(),
                        size: 10,
                        modified: "2026-01-01T00:00:00Z".to_string(),
                        is_dir: false,
                        hash: None,
                    }),
                    right: None,
                    center: None,
                    sub_entries: None,
                },
                DirEntry {
                    name: "right_only.txt".to_string(),
                    status: DirEntryStatus::RightOnly,
                    left: None,
                    right: Some(EntryInfo {
                        name: "right_only.txt".to_string(),
                        path: "/right/right_only.txt".to_string(),
                        size: 10,
                        modified: "2026-01-01T00:00:00Z".to_string(),
                        is_dir: false,
                        hash: None,
                    }),
                    center: None,
                    sub_entries: None,
                },
            ],
            ..Default::default()
        };

        let items = plan_sync_items(&comparison, SyncOperation::DeleteOrphans);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].action, TransferAction::DeleteLeft);
        assert_eq!(items[1].action, TransferAction::DeleteRight);
    }

    #[tokio::test]
    async fn plan_sync_dry_run() {
        let (left, right) = make_sync_dirs();
        let fs = LocalFs::new("test");

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: true,
            ..Default::default()
        };

        let result = plan_sync(&fs, &left, &right, &rules).await.unwrap();
        assert!(result.is_dry_run());
        // Should plan: copy left_only → right, delete right_only.
        assert!(result.planned_count() >= 1);

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn sync_mirror_left() {
        let (left, right) = make_sync_dirs();
        let fs = LocalFs::new("test");

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: false,
            ..Default::default()
        };

        let result =
            sync_directories(&fs, &left, &right, &rules).await.unwrap();
        let transfer = result.transfer.unwrap();
        assert!(
            transfer.is_ok(),
            "sync should succeed: {:?}",
            transfer.errors
        );

        // After mirror left → right:
        // - left_only.txt should exist on right
        // - right_only.txt should be deleted from right
        assert!(right.join("left_only.txt").exists());
        assert!(!right.join("right_only.txt").exists());
        // same.txt should still exist
        assert!(right.join("same.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn sync_delete_orphans() {
        let (left, right) = make_sync_dirs();
        let fs = LocalFs::new("test");

        let rules = SyncRules {
            operation: SyncOperation::DeleteOrphans,
            dry_run: false,
            ..Default::default()
        };

        let result =
            sync_directories(&fs, &left, &right, &rules).await.unwrap();
        let transfer = result.transfer.unwrap();
        assert!(
            transfer.is_ok(),
            "sync should succeed: {:?}",
            transfer.errors
        );

        // Orphans should be deleted.
        assert!(!left.join("left_only.txt").exists());
        assert!(!right.join("right_only.txt").exists());
        // Same file should remain.
        assert!(left.join("same.txt").exists());
        assert!(right.join("same.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn sync_copy_right() {
        let (left, right) = make_sync_dirs();
        let fs = LocalFs::new("test");

        let rules = SyncRules {
            operation: SyncOperation::CopyRight,
            dry_run: false,
            ..Default::default()
        };

        let result =
            sync_directories(&fs, &left, &right, &rules).await.unwrap();
        let transfer = result.transfer.unwrap();
        assert!(
            transfer.is_ok(),
            "sync should succeed: {:?}",
            transfer.errors
        );

        // CopyRight copies left-only to right, but does NOT delete right-only.
        assert!(right.join("left_only.txt").exists());
        assert!(right.join("right_only.txt").exists()); // not deleted

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn sync_empty_comparison() {
        let base = env::temp_dir()
            .join(format!("cocomo_sync_empty_{}", process::id()));
        let left = base.join("left");
        let right = base.join("right");
        fs_err::create_dir_all(&left).unwrap();
        fs_err::create_dir_all(&right).unwrap();

        let fs = LocalFs::new("test");

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: false,
            ..Default::default()
        };

        let result =
            sync_directories(&fs, &left, &right, &rules).await.unwrap();
        assert_eq!(result.planned_count(), 0);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn sync_pair_mirror_left_parity() {
        let base1 = TempDir::new().unwrap();
        let base2 = TempDir::new().unwrap();
        let (left1, right1) = make_sync_dirs_at(base1.path());
        let (left2, right2) = make_sync_dirs_at(base2.path());

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: false,
            ..Default::default()
        };

        // Single-`fs` reference run.
        let fs1 = LocalFs::new("left-parity-1");
        let result1 = sync_directories(&fs1, &left1, &right1, &rules)
            .await
            .unwrap();

        // Pair run: one provider instance, addressed as `Shared`.
        let fs2 = LocalFs::new("left-parity-2");
        let result2 = sync_directories_pair(
            FsPair::shared(&fs2),
            &left2,
            &right2,
            &rules,
        )
        .await
        .unwrap();

        let transfer1 = result1.transfer.unwrap();
        let transfer2 = result2.transfer.unwrap();
        assert!(
            transfer1.is_ok() && transfer2.is_ok(),
            "syncs should succeed: {:?} / {:?}",
            transfer1.errors,
            transfer2.errors
        );
        // Parity: the pair pipeline plans and executes exactly what
        // the single-`fs` wrapper plans and executes...
        assert_eq!(
            flatten_plan(&result1.planned),
            flatten_plan(&result2.planned)
        );
        assert_eq!(transfer1.succeeded, transfer2.succeeded);
        // ...and the same file outcome on both trees.
        assert!(right2.join("left_only.txt").exists());
        assert!(!right2.join("right_only.txt").exists());
    }

    #[tokio::test]
    async fn sync_pair_mirror_right_parity() {
        let base1 = TempDir::new().unwrap();
        let base2 = TempDir::new().unwrap();
        let (left1, right1) = make_sync_dirs_at(base1.path());
        let (left2, right2) = make_sync_dirs_at(base2.path());

        let rules = SyncRules {
            operation: SyncOperation::MirrorRight,
            dry_run: false,
            ..Default::default()
        };

        // Single-`fs` reference run.
        let fs1 = LocalFs::new("right-parity-1");
        let result1 = sync_directories(&fs1, &left1, &right1, &rules)
            .await
            .unwrap();

        // Pair run: one provider instance, addressed as `Shared`.
        let fs2 = LocalFs::new("right-parity-2");
        let result2 = sync_directories_pair(
            FsPair::shared(&fs2),
            &left2,
            &right2,
            &rules,
        )
        .await
        .unwrap();

        let transfer1 = result1.transfer.unwrap();
        let transfer2 = result2.transfer.unwrap();
        assert!(
            transfer1.is_ok() && transfer2.is_ok(),
            "syncs should succeed: {:?} / {:?}",
            transfer1.errors,
            transfer2.errors
        );
        assert_eq!(
            flatten_plan(&result1.planned),
            flatten_plan(&result2.planned)
        );
        assert_eq!(transfer1.succeeded, transfer2.succeeded);
        // Mirror right → left: `right_only.txt` was copied to the
        // left side, `left_only.txt` was deleted from it.
        assert!(left2.join("right_only.txt").exists());
        assert!(!left2.join("left_only.txt").exists());
    }

    #[tokio::test]
    async fn sync_pair_separate_dry_run() {
        let base1 = TempDir::new().unwrap();
        let base2 = TempDir::new().unwrap();
        let (left1, right1) = make_sync_dirs_at(base1.path());
        let (left2, right2) = make_sync_dirs_at(base2.path());

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: true,
            ..Default::default()
        };

        let fs1 = LocalFs::new("dry-1");
        let result1 = plan_sync(&fs1, &left1, &right1, &rules).await.unwrap();

        // Two provider instances, one per side.
        let fs_left = LocalFs::new("dry-left");
        let fs_right = LocalFs::new("dry-right");
        let result2 = plan_sync_pair(
            FsPair::Separate(&fs_left, &fs_right),
            &left2,
            &right2,
            &rules,
        )
        .await
        .unwrap();

        // Dry-run planning must not execute anything...
        assert!(result1.is_dry_run() && result2.is_dry_run());
        // ...and the pair pipeline plans what the single-`fs` wrapper
        // plans.
        assert_eq!(
            flatten_plan(&result1.planned),
            flatten_plan(&result2.planned)
        );
        // Nothing was executed, so tree 2 keeps its original
        // content on both sides.
        assert!(left2.join("left_only.txt").exists());
        assert!(left2.join("same.txt").exists());
        assert!(right2.join("right_only.txt").exists());
        assert!(right2.join("same.txt").exists());
    }

    #[tokio::test]
    async fn sync_pair_separate_cross_boundary() {
        let base = TempDir::new().unwrap();
        let (left, right) = make_sync_dirs_at(base.path());

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: false,
            ..Default::default()
        };

        let fs_left = LocalFs::new("sep-left");
        let fs_right = LocalFs::new("sep-right");
        let result = sync_directories_pair(
            FsPair::Separate(&fs_left, &fs_right),
            &left,
            &right,
            &rules,
        )
        .await
        .unwrap();

        let transfer = result.transfer.unwrap();
        // The cross-boundary copy (left → right) streams the content
        // into a freshly created destination file, and the orphan delete
        // runs on the provider addressing its side.
        assert!(
            transfer.is_ok(),
            "sync should succeed: {:?}",
            transfer.errors
        );
        assert!(right.join("left_only.txt").exists());
        assert!(!right.join("right_only.txt").exists());
    }

    #[tokio::test]
    async fn sync_pair_separate_mockfs_cross_provider() {
        // Two independent `MockFs` trees: the left side has an extra
        // file and a directory, the right side has an orphan.
        let left_fs = MockFs::new("mock-left")
            .with_dir("/left")
            .with_file("/left/left_only.txt", "only left")
            .with_file("/left/same.txt", "identical")
            .with_dir("/left/shared")
            .with_file("/left/shared/nested.txt", "nested");
        let right_fs = MockFs::new("mock-right")
            .with_dir("/right")
            .with_file("/right/right_only.txt", "only right")
            .with_file("/right/same.txt", "identical");

        // `compare_files` must be enabled: the mock's mtimes are
        // non-deterministic, so a size/mtime-only comparison would
        // misclassify the identical file.
        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: false,
            compare_files: true,
            ..Default::default()
        };

        let result = sync_directories_pair(
            FsPair::Separate(&left_fs, &right_fs),
            Path::new("/left"),
            Path::new("/right"),
            &rules,
        )
        .await
        .unwrap();

        // The identical file was classified as Same, so the plan only
        // covers the left-only entries and the right-side orphan.
        assert!(
            !result.planned.iter().any(|i| i.name == "same.txt"),
            "plan should not touch same.txt: {:?}",
            result.planned
        );
        assert_eq!(result.planned_count(), 3);

        let transfer = result.transfer.unwrap();
        assert!(
            transfer.is_ok(),
            "sync should succeed: {:?}",
            transfer.errors
        );
        // Mirror left → right: the left-only file and directory were
        // mirrored across the boundary, the orphan was deleted.
        assert_eq!(
            mock_content(&right_fs, "/right/left_only.txt").await,
            b"only left"
        );
        assert_eq!(
            mock_content(&right_fs, "/right/shared/nested.txt").await,
            b"nested"
        );
        assert!(
            right_fs
                .read(Path::new("/right/right_only.txt"), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn sync_pair_refuses_execution_when_comparison_has_errors() {
        // The left side has a subdirectory that cannot be read, so the
        // planning comparison carries an error. Executing a mirror on top
        // of an incomplete plan could delete entries that were never seen,
        // so the sync must abort.
        let left_fs = MockFs::new("mock-left")
            .with_dir("/left")
            .with_file("/left/left_only.txt", "only left")
            .with_dir("/left/blocked")
            .with_error(
                "/left/blocked",
                FsError::PermissionDenied {
                    operation: FsOperation::ReadDir,
                    path: PathBuf::from("/left/blocked"),
                },
            );
        let right_fs = MockFs::new("mock-right").with_dir("/right");

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: false,
            compare_files: true,
            ..Default::default()
        };

        let result = sync_directories_pair(
            FsPair::Separate(&left_fs, &right_fs),
            Path::new("/left"),
            Path::new("/right"),
            &rules,
        )
        .await;

        match result {
            Err(FsError::Incomplete { errors }) => {
                assert_eq!(errors.len(), 1);
                assert!(matches!(errors[0], FsError::PermissionDenied { .. }));
            }
            other => panic!("expected Err(Incomplete), got {other:?}"),
        }

        // Nothing was transferred: the destination tree is untouched.
        assert!(
            right_fs
                .read(Path::new("/right/left_only.txt"), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn sync_pair_dry_run_keeps_comparison_errors() {
        // Dry runs never execute, so the errors stay in the result for the
        // caller to surface instead of aborting.
        let left_fs = MockFs::new("mock-left")
            .with_dir("/left")
            .with_file("/left/left_only.txt", "only left")
            .with_dir("/left/blocked")
            .with_error(
                "/left/blocked",
                FsError::PermissionDenied {
                    operation: FsOperation::ReadDir,
                    path: PathBuf::from("/left/blocked"),
                },
            );
        let right_fs = MockFs::new("mock-right").with_dir("/right");

        let rules = SyncRules {
            operation: SyncOperation::MirrorLeft,
            dry_run: true,
            compare_files: true,
            ..Default::default()
        };

        let result = sync_directories_pair(
            FsPair::Separate(&left_fs, &right_fs),
            Path::new("/left"),
            Path::new("/right"),
            &rules,
        )
        .await
        .unwrap();

        assert_eq!(result.errors.len(), 1);
        assert!(matches!(result.errors[0], FsError::PermissionDenied { .. }));
    }
}
