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
//! and executes them using the node-based API.
//!
//! # Dry-run mode
//!
//! When `dry_run` is `true`, the engine plans all transfers but does not
//! execute them. The returned [`SyncResult`] contains the planned actions
//! for review.

use std::path::Path;

use crate::{
    CompareConfig, DirComparison, DirEntryStatus,
    compare::compare_directories_node,
    transfer::{
        TransferAction, TransferItem, TransferResult, execute_transfers,
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
// Sync planning
// ---------------------------------------------------------------------------

/// Plan all transfers for a given sync operation without executing them.
///
/// Compares the two directory trees and returns a [`SyncResult`] with the
/// planned actions. No I/O is performed beyond the comparison scan.
pub async fn plan_sync<N>(
    fs: &N,
    left_path: &Path,
    right_path: &Path,
    rules: &SyncRules,
) -> crate::Result<SyncResult>
where
    N: crate::NodeFileSystem<Nid = u64>,
{
    let cache = crate::hash::ContentCache::default_config();
    let compare_config = CompareConfig {
        compare_files: rules.compare_files,
        compare_structure: true,
        follow_symlinks: false,
        max_depth: rules.max_depth,
        size_tolerance: 0.1,
    };

    let comparison = compare_directories_node(
        fs,
        left_path,
        right_path,
        &compare_config,
        Some(&cache),
    )
    .await?;

    let planned = plan_sync_items(&comparison, rules.operation);

    Ok(SyncResult {
        transfer: None,
        planned,
        errors: Vec::new(),
    })
}

/// Plan all transfers and execute them.
///
/// This is the main entry point for synchronization. It compares the two
/// directory trees, plans the necessary transfers, and executes them if
/// `dry_run` is `false`.
pub async fn sync_directories<N>(
    fs: &N,
    left_path: &Path,
    right_path: &Path,
    rules: &SyncRules,
) -> crate::Result<SyncResult>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    // Plan the sync.
    let mut result = plan_sync(fs, left_path, right_path, rules).await?;

    if rules.dry_run || result.planned.is_empty() {
        result.transfer = Some(TransferResult::default());
        return Ok(result);
    }

    // Execute the planned transfers.
    let transfer_result =
        execute_transfers(&result.planned, left_path, right_path, fs, fs)
            .await;

    result.transfer = Some(transfer_result);
    Ok(result)
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
    use std::{env, path::PathBuf, process};

    use super::*;
    use crate::LocalFs;

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
}
