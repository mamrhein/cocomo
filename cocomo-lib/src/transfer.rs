// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Transfer operations between filesystem providers.
//!
//! A [`TransferAction`] describes a single copy, move, or delete operation.
//! Actions are collected into a batch and executed via
//! [`execute_transfers`], which uses the node-based [`WritableFileSystem`]
//! API to perform all I/O through opaque node identifiers.
//!
//! # Design
//!
//! Transfers operate on a per-entry basis. Each [`TransferItem`] carries the
//! source node (if any), destination node (if any), and the action to perform.
//! The executor walks the batch sequentially, reporting progress and errors.
//! Non-fatal errors are collected rather than aborting the entire batch.

use std::path::Path;
#[allow(unused_imports)]
use std::sync::Arc;

use crate::{DirComparison, DirEntryStatus, FsError, FsOperation};

// ---------------------------------------------------------------------------
// Transfer actions
// ---------------------------------------------------------------------------

/// The kind of transfer operation to perform on a single entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferAction {
    /// Copy from left to right side.
    CopyLeft,
    /// Copy from right to left side.
    CopyRight,
    /// Copy from center to both left and right (3-way merge).
    CopyCenter,
    /// Delete from the left side.
    DeleteLeft,
    /// Delete from the right side.
    DeleteRight,
    /// Move from left to right side.
    MoveLeft,
    /// Move from right to left side.
    MoveRight,
}

impl TransferAction {
    /// Return a human-readable label for this action.
    pub fn label(&self) -> &'static str {
        match self {
            TransferAction::CopyLeft => "copy left → right",
            TransferAction::CopyRight => "copy right → left",
            TransferAction::CopyCenter => "copy center → both",
            TransferAction::DeleteLeft => "delete left",
            TransferAction::DeleteRight => "delete right",
            TransferAction::MoveLeft => "move left → right",
            TransferAction::MoveRight => "move right → left",
        }
    }

    /// Return the status variants that this action applies to.
    pub fn applicable_statuses(&self) -> &[DirEntryStatus] {
        match self {
            TransferAction::CopyLeft => &[DirEntryStatus::RightOnly],
            TransferAction::CopyRight => &[DirEntryStatus::LeftOnly],
            TransferAction::CopyCenter => &[DirEntryStatus::CenterOnly],
            TransferAction::DeleteLeft => &[DirEntryStatus::LeftOnly],
            TransferAction::DeleteRight => &[DirEntryStatus::RightOnly],
            TransferAction::MoveLeft => &[DirEntryStatus::RightOnly],
            TransferAction::MoveRight => &[DirEntryStatus::LeftOnly],
        }
    }
}

// ---------------------------------------------------------------------------
// Transfer items
// ---------------------------------------------------------------------------

/// A single transfer operation to be executed.
#[derive(Clone, Debug)]
pub struct TransferItem {
    /// The action to perform.
    pub action: TransferAction,
    /// Entry name.
    pub name: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Relative path within the sync root.
    pub rel_path: String,
}

impl TransferItem {
    /// Create a new transfer item.
    pub fn new(
        action: TransferAction,
        name: String,
        is_dir: bool,
        rel_path: String,
    ) -> Self {
        Self {
            action,
            name,
            is_dir,
            rel_path,
        }
    }
}

// ---------------------------------------------------------------------------
// Transfer result
// ---------------------------------------------------------------------------

/// Result of executing a batch of transfers.
#[derive(Clone, Debug, Default)]
pub struct TransferResult {
    /// Number of successfully completed operations.
    pub succeeded: usize,
    /// Number of operations that failed.
    pub failed: usize,
    /// Errors encountered during the transfer.
    pub errors: Vec<FsError>,
}

impl TransferResult {
    /// Return `true` if all operations succeeded.
    pub fn is_ok(&self) -> bool {
        self.failed == 0
    }

    /// Merge another result into this one.
    pub fn merge(&mut self, other: TransferResult) {
        self.succeeded += other.succeeded;
        self.failed += other.failed;
        self.errors.extend(other.errors);
    }
}

// ---------------------------------------------------------------------------
// Transfer planning
// ---------------------------------------------------------------------------

/// Generate a list of transfer items from a [`DirComparison`] result.
///
/// Walks the comparison tree and collects all entries that match the given
/// action. For example, [`TransferAction::CopyLeft`] will collect all
/// `RightOnly` entries, because they need to be copied to the left side.
pub fn plan_transfers(
    comparison: &DirComparison,
    action: TransferAction,
) -> Vec<TransferItem> {
    let mut items = Vec::new();
    collect_transfer_items(comparison, &action, &mut items);
    items
}

fn collect_transfer_items(
    comparison: &DirComparison,
    action: &TransferAction,
    items: &mut Vec<TransferItem>,
) {
    let applicable = action.applicable_statuses();

    for entry in &comparison.entries {
        if applicable.contains(&entry.status) {
            let is_dir = entry
                .left
                .as_ref()
                .or(entry.right.as_ref())
                .is_some_and(|info| info.is_dir);

            // If the entry is a directory with resolved sub-entries, recurse
            // into them instead of adding the directory itself. This avoids
            // duplicate operations when the executor would process the
            // directory contents individually.
            if is_dir && entry.sub_entries.is_some() {
                if let Some(ref sub) = entry.sub_entries {
                    collect_transfer_items(sub, action, items);
                }
            } else {
                items.push(TransferItem::new(
                    *action,
                    entry.name.clone(),
                    is_dir,
                    entry
                        .left
                        .as_ref()
                        .or(entry.right.as_ref())
                        .map(|info| info.path.clone())
                        .unwrap_or_default(),
                ));
            }
        } else if let Some(ref sub) = entry.sub_entries {
            // Recurse into sub-directories even when the entry itself doesn't
            // match the action, to handle nested differences.
            collect_transfer_items(sub, action, items);
        }
    }
}

// ---------------------------------------------------------------------------
// Transfer execution
// ---------------------------------------------------------------------------

/// Execute a batch of transfer items using the node-based API.
///
/// This function resolves the source and destination paths to node IDs and
/// performs the actual I/O through [`WritableFileSystem`]. Non-fatal errors
/// are collected rather than aborting the batch.
///
/// # Arguments
///
/// * `items` - The transfer items to execute.
/// * `left_path` - Root path of the left side.
/// * `right_path` - Root path of the right side.
/// * `src_fs` - The source filesystem (where files are read from).
/// * `dst_fs` - The destination filesystem (where files are written).
///
/// # Note
///
/// For same-provider operations (e.g., left and right are the same
/// `LocalFs` instance), `src_fs` and `dst_fs` should be the same reference.
pub async fn execute_transfers<N>(
    items: &[TransferItem],
    left_path: &Path,
    right_path: &Path,
    src_fs: &N,
    dst_fs: &N,
) -> TransferResult
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let mut result = TransferResult::default();

    for item in items {
        match execute_single_transfer(
            item, left_path, right_path, src_fs, dst_fs,
        )
        .await
        {
            Ok(()) => result.succeeded += 1,
            Err(e) => {
                result.failed += 1;
                result.errors.push(e);
            }
        }
    }

    result
}

async fn execute_single_transfer<N>(
    item: &TransferItem,
    left_path: &Path,
    right_path: &Path,
    src_fs: &N,
    dst_fs: &N,
) -> Result<(), FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    match item.action {
        TransferAction::CopyLeft => {
            // Copy from right to left.
            copy_entry(
                src_fs,
                right_path,
                &item.name,
                item.is_dir,
                dst_fs,
                left_path,
                &item.name,
            )
            .await
        }
        TransferAction::CopyRight => {
            // Copy from left to right.
            copy_entry(
                src_fs,
                left_path,
                &item.name,
                item.is_dir,
                dst_fs,
                right_path,
                &item.name,
            )
            .await
        }
        TransferAction::CopyCenter => {
            // Not supported in 2-way mode — requires center path.
            Err(FsError::InvalidArgument {
                operation: FsOperation::Copy,
                path: item.rel_path.clone().into(),
                message: "center copy requires 3-way comparison".into(),
            })
        }
        TransferAction::DeleteLeft => {
            delete_entry(dst_fs, left_path, &item.name, item.is_dir).await
        }
        TransferAction::DeleteRight => {
            delete_entry(dst_fs, right_path, &item.name, item.is_dir).await
        }
        TransferAction::MoveLeft => {
            // Move from right to left.
            move_entry(
                src_fs, right_path, &item.name, dst_fs, left_path, &item.name,
            )
            .await
        }
        TransferAction::MoveRight => {
            // Move from left to right.
            move_entry(
                src_fs, left_path, &item.name, dst_fs, right_path, &item.name,
            )
            .await
        }
    }
}

/// Copy a file or directory from source to destination.
async fn copy_entry<N>(
    src_fs: &N,
    src_root: &Path,
    name: &str,
    is_dir: bool,
    dst_fs: &N,
    dst_root: &Path,
    _dst_name: &str,
) -> Result<(), FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let src_path = src_root.join(name);
    let dst_node_id = resolve_dir_for_copy(dst_fs, dst_root, name).await?;

    // Resolve the source node.
    let src_node_id = src_fs.resolve_path(&src_path).await?;
    let src_node = src_fs.get_node(src_node_id)?;

    if is_dir {
        // Copy directory.
        src_fs
            .copy_node(src_node_id, crate::DirId::new(*dst_node_id.get()))
            .await?;
    } else {
        // Copy file: create destination file, then stream content.
        let dst_name = src_node.name();
        let new_file_id = dst_fs
            .create_file(crate::DirId::new(*dst_node_id.get()), dst_name)
            .await?;

        // Stream content from source to destination.
        let mut stream = src_fs
            .read_stream_node(crate::FileId::new(*src_node_id.get()), None)
            .await?;
        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| FsError::Io {
                operation: FsOperation::Read,
                path: src_node.path().to_path_buf(),
                message: format!("stream error: {e}"),
            })?;
            dst_fs.write_node(new_file_id, chunk).await?;
        }
    }

    Ok(())
}

/// Delete a file or directory.
async fn delete_entry<N>(
    fs: &N,
    root: &Path,
    name: &str,
    _is_dir: bool,
) -> Result<(), FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let path = root.join(name);
    let node_id = fs.resolve_path(&path).await?;
    fs.remove_all_node(node_id).await
}

/// Move a file or directory to a new location.
async fn move_entry<N>(
    src_fs: &N,
    src_root: &Path,
    name: &str,
    dst_fs: &N,
    dst_root: &Path,
    _dst_name: &str,
) -> Result<(), FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let src_path = src_root.join(name);
    let src_node_id = src_fs.resolve_path(&src_path).await?;

    let dst_node_id = resolve_dir_for_copy(dst_fs, dst_root, name).await?;

    src_fs
        .move_node(src_node_id, crate::DirId::new(*dst_node_id.get()))
        .await?;

    Ok(())
}

/// Resolve the destination directory for a copy/move operation. Returns the
/// node ID of the root directory, which the caller uses to construct a
/// [`crate::DirId`].
async fn resolve_dir_for_copy<N>(
    fs: &N,
    root: &Path,
    _name: &str,
) -> Result<crate::NodeId<u64>, FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let node_id = fs.resolve_path(root).await?;
    let node = fs.get_node(node_id)?;

    // Verify it's a directory.
    if !node.kind().is_directory() {
        return Err(FsError::WrongKind {
            expected: "directory",
            actual: "file",
        });
    }

    Ok(node_id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf, process};

    use super::*;
    use crate::{
        CompareConfig, DirEntry, DirEntryStatus, EntryInfo, LocalFs,
        compare::compare_directories_node, hash::ContentCache,
    };

    fn make_test_dirs() -> (PathBuf, PathBuf) {
        let base =
            env::temp_dir().join(format!("cocomo_transfer_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        // Left-only file.
        fs_err::create_dir_all(&left).unwrap();
        fs_err::create_dir_all(&right).unwrap();
        fs_err::write(left.join("left_only.txt"), "only on left").unwrap();

        // Right-only file.
        fs_err::write(right.join("right_only.txt"), "only on right").unwrap();

        // Same file.
        fs_err::write(left.join("same.txt"), "identical").unwrap();
        fs_err::write(right.join("same.txt"), "identical").unwrap();

        (left, right)
    }

    fn make_nested_dirs() -> (PathBuf, PathBuf) {
        let base = env::temp_dir()
            .join(format!("cocomo_transfer_nested_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);

        let left = base.join("left");
        let right = base.join("right");

        fs_err::create_dir_all(left.join("subdir")).unwrap();
        fs_err::create_dir_all(right.join("subdir")).unwrap();
        fs_err::write(left.join("file.txt"), "content").unwrap();
        fs_err::write(left.join("subdir/nested.txt"), "nested").unwrap();

        (left, right)
    }

    #[test]
    fn transfer_action_labels() {
        assert_eq!(TransferAction::CopyLeft.label(), "copy left → right");
        assert_eq!(TransferAction::DeleteRight.label(), "delete right");
        assert_eq!(TransferAction::MoveLeft.label(), "move left → right");
    }

    #[test]
    fn transfer_action_applicable_statuses() {
        assert!(
            TransferAction::CopyLeft
                .applicable_statuses()
                .contains(&DirEntryStatus::RightOnly)
        );
        assert!(
            TransferAction::DeleteLeft
                .applicable_statuses()
                .contains(&DirEntryStatus::LeftOnly)
        );
        assert!(
            TransferAction::CopyCenter
                .applicable_statuses()
                .contains(&DirEntryStatus::CenterOnly)
        );
    }

    #[test]
    fn transfer_result_default() {
        let result = TransferResult::default();
        assert!(result.is_ok());
        assert_eq!(result.succeeded, 0);
        assert_eq!(result.failed, 0);
    }

    #[test]
    fn transfer_result_merge() {
        let mut result = TransferResult {
            succeeded: 2,
            failed: 1,
            errors: vec![FsError::StaleNode],
        };
        let other = TransferResult {
            succeeded: 3,
            failed: 0,
            errors: vec![],
        };
        result.merge(other);
        assert_eq!(result.succeeded, 5);
        assert_eq!(result.failed, 1);
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn plan_transfers_copy_left() {
        // Build a comparison with right-only entries.
        let comparison = DirComparison {
            entries: vec![
                DirEntry {
                    name: "right_file.txt".to_string(),
                    status: DirEntryStatus::RightOnly,
                    left: None,
                    right: Some(EntryInfo {
                        name: "right_file.txt".to_string(),
                        path: "/right/right_file.txt".to_string(),
                        size: 10,
                        modified: "2026-01-01T00:00:00Z".to_string(),
                        is_dir: false,
                        hash: None,
                    }),
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

        let items = plan_transfers(&comparison, TransferAction::CopyLeft);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "right_file.txt");
        assert!(!items[0].is_dir);
    }

    #[test]
    fn plan_transfers_delete_left() {
        let comparison = DirComparison {
            entries: vec![DirEntry {
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
            }],
            ..Default::default()
        };

        let items = plan_transfers(&comparison, TransferAction::DeleteLeft);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "left_only.txt");
    }

    #[test]
    fn plan_transfers_recursive() {
        let sub_comparison = Arc::new(DirComparison {
            entries: vec![DirEntry {
                name: "nested.txt".to_string(),
                status: DirEntryStatus::LeftOnly,
                left: Some(EntryInfo {
                    name: "nested.txt".to_string(),
                    path: "/left/sub/nested.txt".to_string(),
                    size: 5,
                    modified: "2026-01-01T00:00:00Z".to_string(),
                    is_dir: false,
                    hash: None,
                }),
                right: None,
                center: None,
                sub_entries: None,
            }],
            ..Default::default()
        });

        let comparison = DirComparison {
            entries: vec![
                DirEntry {
                    name: "left.txt".to_string(),
                    status: DirEntryStatus::LeftOnly,
                    left: Some(EntryInfo {
                        name: "left.txt".to_string(),
                        path: "/left/left.txt".to_string(),
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
                    name: "sub".to_string(),
                    status: DirEntryStatus::LeftOnly,
                    left: Some(EntryInfo {
                        name: "sub".to_string(),
                        path: "/left/sub".to_string(),
                        size: 0,
                        modified: "2026-01-01T00:00:00Z".to_string(),
                        is_dir: true,
                        hash: None,
                    }),
                    right: None,
                    center: None,
                    sub_entries: Some(sub_comparison),
                },
            ],
            ..Default::default()
        };

        let items = plan_transfers(&comparison, TransferAction::DeleteLeft);
        // Should find the top-level entry and the nested entry.
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "left.txt");
        assert_eq!(items[1].name, "nested.txt");
    }

    #[tokio::test]
    async fn execute_copy_right() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");

        // left_only.txt exists only on left; CopyRight should copy it to
        // right.
        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "left_only.txt".to_string(),
            false,
            left.join("left_only.txt").to_string_lossy().into_owned(),
        )];

        let result = execute_transfers(&items, &left, &right, &fs, &fs).await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // Verify the file was copied.
        assert!(right.join("left_only.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn execute_delete_left() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");

        let items = vec![TransferItem::new(
            TransferAction::DeleteLeft,
            "left_only.txt".to_string(),
            false,
            left.join("left_only.txt").to_string_lossy().into_owned(),
        )];

        let result = execute_transfers(&items, &left, &right, &fs, &fs).await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // Verify the file was deleted.
        assert!(!left.join("left_only.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn execute_copy_directory() {
        let (left, right) = make_nested_dirs();
        let fs = LocalFs::new("test");

        // Copy subdir from left to right.
        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "subdir".to_string(),
            true,
            left.join("subdir").to_string_lossy().into_owned(),
        )];

        let result = execute_transfers(&items, &left, &right, &fs, &fs).await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // Verify the directory was copied.
        assert!(right.join("subdir").is_dir());
        assert!(right.join("subdir/nested.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn execute_transfer_nonexistent_fails() {
        let base = env::temp_dir()
            .join(format!("cocomo_transfer_fail_{}", process::id()));
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");

        let items = vec![TransferItem::new(
            TransferAction::DeleteLeft,
            "nonexistent.txt".to_string(),
            false,
            base.join("nonexistent.txt").to_string_lossy().into_owned(),
        )];

        let result = execute_transfers(&items, &base, &base, &fs, &fs).await;

        assert!(!result.is_ok());
        assert_eq!(result.failed, 1);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn plan_and_execute_from_comparison() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");
        let cache = ContentCache::default_config();
        let config = CompareConfig::full();

        // Compare the directories.
        let comparison = compare_directories_node(
            &fs,
            &left,
            &right,
            &config,
            Some(&cache),
        )
        .await
        .unwrap();

        // Plan: copy all right-only files to left.
        let items = plan_transfers(&comparison, TransferAction::CopyLeft);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "right_only.txt");

        // Execute.
        let result = execute_transfers(&items, &left, &right, &fs, &fs).await;
        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );

        // Verify: right_only.txt now exists on left.
        assert!(left.join("right_only.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }
}
