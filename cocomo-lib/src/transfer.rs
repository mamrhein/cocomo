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
//! API to perform all I/O through opaque node identifiers. The
//! provider(s) addressing the two sides arrive as an [`FsPair`]: one
//! provider instance shared by both sides, or two distinct instances.
//!
//! # Design
//!
//! Transfers operate on a per-entry basis. Each [`TransferItem`] carries the
//! entry's path relative to the compared side roots, and the action to
//! perform. The executor walks the batch sequentially, reporting progress
//! and errors. Non-fatal errors are collected rather than aborting the
//! entire batch. A nested entry transfers to the same relative path on the
//! target side; missing target-side directories are created on demand.

use std::path::Path;
#[allow(unused_imports)]
use std::sync::Arc;

use bytes::BytesMut;
use futures::future::BoxFuture;

use crate::{DirComparison, DirEntryStatus, FsError, FsOperation};

// ---------------------------------------------------------------------------
// Transfer actions
// ---------------------------------------------------------------------------

/// The kind of transfer operation to perform on a single entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferAction {
    /// Copy to the left side (from the right).
    CopyLeft,
    /// Copy to the right side (from the left).
    CopyRight,
    /// Copy from center to both left and right (3-way merge).
    CopyCenter,
    /// Delete from the left side.
    DeleteLeft,
    /// Delete from the right side.
    DeleteRight,
    /// Move to the left side (from the right).
    MoveLeft,
    /// Move to the right side (from the left).
    MoveRight,
}

impl TransferAction {
    /// Return a human-readable label for this action.
    pub fn label(&self) -> &'static str {
        match self {
            // The variant names the destination side, so the label shows
            // the data flow in the opposite direction.
            TransferAction::CopyLeft => "copy right → left",
            TransferAction::CopyRight => "copy left → right",
            TransferAction::CopyCenter => "copy center → both",
            TransferAction::DeleteLeft => "delete left",
            TransferAction::DeleteRight => "delete right",
            TransferAction::MoveLeft => "move right → left",
            TransferAction::MoveRight => "move left → right",
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
#[derive(Clone, Debug, PartialEq)]
pub struct TransferItem {
    /// The action to perform.
    pub action: TransferAction,
    /// Entry name.
    pub name: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Entry path relative to the compared side roots (`/`-separated).
    ///
    /// The executor addresses both sides by this path, so a nested entry
    /// lands at the same relative path on the target side. When empty, the
    /// executor falls back to [`Self::name`].
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
///
/// Entries flagged with [`crate::DirEntry::name_conflict`] are never
/// collected: a file cannot be transferred onto a directory of the same
/// name. Use [`crate::plan_sync_pair`] to have those clashes reported as
/// [`crate::NameConflict`]s.
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
        // A file colliding with a directory of the same name is never a
        // transfer candidate; the sync planner reports it instead.
        if entry.name_conflict {
            continue;
        }
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
// Filesystem pairs
// ---------------------------------------------------------------------------

/// The filesystem provider(s) addressed by one compare or sync run.
///
/// Node ids are provider-instance local, so the pipeline must know
/// statically whether the two sides share one provider instance or
/// address two distinct ones: a destination `DirId` resolved on a
/// provider other than the one executing the write would silently
/// address the wrong tree. Provider identity (`NodeFileSystem::id`)
/// cannot express this distinction — two `LocalFs` instances on the
/// same volume share a device-id `FileSystemId` while their node
/// caches stay per-instance — so the caller states it explicitly.
///
/// `Clone`/`Copy` are implemented manually (instead of derived) so they
/// do not constrain `L` and `R`: a pair is a pair of *references*, and
/// copying one must stay cheap regardless of the addressed providers.
pub enum FsPair<'a, L, R> {
    /// One provider instance addressing both sides (same-fs fast path).
    Shared(&'a L),
    /// Two distinct provider instances (left, right).
    Separate(&'a L, &'a R),
}

// Manual impls without `L: Clone`/`R: Clone` bounds (a derived `Copy`
// would add implicit bounds on *all* type parameters, which breaks
// reusing one pair across the compare and execute phases).
impl<L, R> Clone for FsPair<'_, L, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L, R> Copy for FsPair<'_, L, R> {}

impl<'a, N> FsPair<'a, N, N> {
    /// Address both sides with one shared provider instance.
    pub fn shared(fs: &'a N) -> Self {
        Self::Shared(fs)
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
/// * `pair` - The provider(s) addressing the two sides. With a
///   [`FsPair::Separate`] pair, cross-boundary copies stream the source
///   content into a freshly created destination entry (directories are
///   mirrored recursively) and cross-boundary moves run as copy + delete;
///   single-side operations run against the provider addressing their side.
pub async fn execute_transfers<L, R>(
    items: &[TransferItem],
    left_path: &Path,
    right_path: &Path,
    pair: FsPair<'_, L, R>,
) -> TransferResult
where
    L: crate::fs::WritableFileSystem<Nid = u64>,
    R: crate::fs::WritableFileSystem<Nid = u64>,
{
    let mut result = TransferResult::default();

    for item in items {
        match execute_single_transfer(item, left_path, right_path, pair).await
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

/// Execute a single transfer item against the provider pair.
///
/// The entry is addressed by its path relative to the side roots (see
/// [`item_rel`]), so a nested selection like `def/ghi/b.txt` transfers from
/// `<left_root>/def/ghi/b.txt` to `<right_root>/def/ghi/b.txt`, creating the
/// target-side nesting if needed.
async fn execute_single_transfer<L, R>(
    item: &TransferItem,
    left_path: &Path,
    right_path: &Path,
    pair: FsPair<'_, L, R>,
) -> Result<(), FsError>
where
    L: crate::fs::WritableFileSystem<Nid = u64>,
    R: crate::fs::WritableFileSystem<Nid = u64>,
{
    let rel = item_rel(item);
    match pair {
        // One provider instance addresses both sides, so node IDs
        // resolved on one side are valid on the other and the
        // same-filesystem helpers apply unchanged.
        FsPair::Shared(fs) => match item.action {
            TransferAction::CopyLeft => {
                // Copy from right to left.
                copy_entry(fs, right_path, rel, item.is_dir, fs, left_path)
                    .await
            }
            TransferAction::CopyRight => {
                // Copy from left to right.
                copy_entry(fs, left_path, rel, item.is_dir, fs, right_path)
                    .await
            }
            TransferAction::CopyCenter => Err(center_copy_error(item)),
            TransferAction::DeleteLeft => {
                delete_entry(fs, left_path, rel, item.is_dir).await
            }
            TransferAction::DeleteRight => {
                delete_entry(fs, right_path, rel, item.is_dir).await
            }
            TransferAction::MoveLeft => {
                // Move from right to left.
                move_entry(fs, right_path, rel, fs, left_path).await
            }
            TransferAction::MoveRight => {
                // Move from left to right.
                move_entry(fs, left_path, rel, fs, right_path).await
            }
        },
        // Two distinct provider instances. `copy_node` and `move_node`
        // execute *within* one provider, so cross-boundary copies stream
        // the source content into a freshly created destination entry and
        // cross-boundary moves run as copy + delete. Deletes only ever
        // touch one side and run against the provider addressing that
        // side.
        FsPair::Separate(left_fs, right_fs) => match item.action {
            TransferAction::CopyLeft => {
                // Copy from right to left.
                copy_entry_cross(
                    right_fs,
                    right_path,
                    rel,
                    item.is_dir,
                    left_fs,
                    left_path,
                )
                .await
            }
            TransferAction::CopyRight => {
                // Copy from left to right.
                copy_entry_cross(
                    left_fs,
                    left_path,
                    rel,
                    item.is_dir,
                    right_fs,
                    right_path,
                )
                .await
            }
            TransferAction::CopyCenter => Err(center_copy_error(item)),
            TransferAction::DeleteLeft => {
                delete_entry(left_fs, left_path, rel, item.is_dir).await
            }
            TransferAction::DeleteRight => {
                delete_entry(right_fs, right_path, rel, item.is_dir).await
            }
            TransferAction::MoveLeft => {
                // Move from right to left.
                move_entry_cross(
                    right_fs,
                    right_path,
                    rel,
                    item.is_dir,
                    left_fs,
                    left_path,
                )
                .await
            }
            TransferAction::MoveRight => {
                // Move from left to right.
                move_entry_cross(
                    left_fs,
                    left_path,
                    rel,
                    item.is_dir,
                    right_fs,
                    right_path,
                )
                .await
            }
        },
    }
}

/// The path a transfer item is addressed by, relative to the side roots.
///
/// Both sides of a comparison share the relative layout, so one path
/// addresses the entry on either side. Falls back to the bare entry name for
/// items built without a relative path.
fn item_rel(item: &TransferItem) -> &str {
    if item.rel_path.is_empty() {
        &item.name
    } else {
        &item.rel_path
    }
}

/// Split a relative entry path into its non-empty components.
///
/// Relative paths are `/`-separated (as produced by the scan); the platform
/// separator is accepted as well.
fn rel_components(rel: &str) -> Vec<&str> {
    rel.split(std::path::is_separator)
        .filter(|c| !c.is_empty())
        .collect()
}

/// The error for center copies, which need a 3-way comparison.
fn center_copy_error(item: &TransferItem) -> FsError {
    FsError::InvalidArgument {
        operation: FsOperation::Copy,
        path: item.rel_path.clone().into(),
        message: "center copy requires 3-way comparison".into(),
    }
}

/// Stream a source file's content into an already created destination
/// file, across (or within) provider instances.
async fn stream_file_content<S, D>(
    src_fs: &S,
    src_file_id: crate::FileId<u64>,
    src_path: &Path,
    dst_fs: &D,
    dst_file_id: crate::FileId<u64>,
) -> Result<(), FsError>
where
    S: crate::fs::WritableFileSystem<Nid = u64>,
    D: crate::fs::WritableFileSystem<Nid = u64>,
{
    // `write_node` replaces the destination file's whole content on every
    // provider, so the chunks must be collected before a single write:
    // writing each chunk as it arrives would leave only the last chunk on
    // the destination.
    let mut content = BytesMut::new();
    let mut stream = src_fs.read_stream_node(src_file_id, None).await?;
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| FsError::Io {
            operation: FsOperation::Read,
            path: src_path.to_path_buf(),
            message: format!("stream error: {e}"),
        })?;
        content.extend_from_slice(&chunk);
    }

    dst_fs.write_node(dst_file_id, content.freeze()).await
}

/// Copy a file or directory from source to destination.
///
/// `rel` is the entry path relative to both side roots; the destination
/// nesting is created on demand (see [`ensure_parent_dir`]).
///
/// Only reached for [`FsPair::Shared`] pairs: `copy_node` and the file
/// streaming path address one provider instance. Cross-boundary copies go
/// through [`copy_entry_cross`] instead.
async fn copy_entry<N>(
    src_fs: &N,
    src_root: &Path,
    rel: &str,
    is_dir: bool,
    dst_fs: &N,
    dst_root: &Path,
) -> Result<(), FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let src_path = src_root.join(rel);
    let dst_dir_id = ensure_parent_dir(dst_fs, dst_root, rel).await?;

    // Resolve the source node.
    let src_node_id = src_fs.resolve_path(&src_path).await?;
    let src_node = src_fs.get_node(src_node_id)?;

    if is_dir {
        // Copy directory.
        src_fs.copy_node(src_node_id, dst_dir_id).await?;
    } else {
        // Copy file: create destination file, then stream content.
        let dst_name = src_node.name();
        let new_file_id = dst_fs.create_file(dst_dir_id, dst_name).await?;

        // Stream content from source to destination.
        stream_file_content(
            src_fs,
            crate::FileId::new(*src_node_id.get()),
            src_node.path(),
            dst_fs,
            new_file_id,
        )
        .await?;
    }

    Ok(())
}

/// Delete a file or directory addressed by `rel` under `root`.
async fn delete_entry<N>(
    fs: &N,
    root: &Path,
    rel: &str,
    _is_dir: bool,
) -> Result<(), FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let path = root.join(rel);
    let node_id = fs.resolve_path(&path).await?;
    fs.remove_all_node(node_id).await
}

/// Move a file or directory to a new location.
///
/// `rel` addresses the entry relative to both side roots; the destination
/// nesting is created on demand (see [`ensure_parent_dir`]).
async fn move_entry<N>(
    src_fs: &N,
    src_root: &Path,
    rel: &str,
    dst_fs: &N,
    dst_root: &Path,
) -> Result<(), FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let src_path = src_root.join(rel);
    let src_node_id = src_fs.resolve_path(&src_path).await?;

    let dst_dir_id = ensure_parent_dir(dst_fs, dst_root, rel).await?;

    src_fs.move_node(src_node_id, dst_dir_id).await?;

    Ok(())
}

/// Copy a file or directory across two distinct provider instances.
///
/// `copy_node` executes *within* the provider it is called on, so the
/// destination entry is created on the destination side and the source
/// content is streamed (files) or mirrored recursively (directories)
/// across the boundary. Symlinks and other special entries are skipped:
/// their targets are provider-local and would dangle on the destination
/// side.
async fn copy_entry_cross<S, D>(
    src_fs: &S,
    src_root: &Path,
    rel: &str,
    is_dir: bool,
    dst_fs: &D,
    dst_root: &Path,
) -> Result<(), FsError>
where
    S: crate::fs::WritableFileSystem<Nid = u64>,
    D: crate::fs::WritableFileSystem<Nid = u64>,
{
    let dst_dir_id = ensure_parent_dir(dst_fs, dst_root, rel).await?;

    // Resolve the source node.
    let src_path = src_root.join(rel);
    let src_node_id = src_fs.resolve_path(&src_path).await?;
    let src_node = src_fs.get_node(src_node_id)?;

    if is_dir {
        // Create the top-level directory on the destination side, then
        // mirror its contents into it.
        let new_dir_id =
            dst_fs.create_dir_node(dst_dir_id, src_node.name()).await?;
        mirror_directory_tree(src_fs, src_node_id, dst_fs, new_dir_id).await?;
    } else {
        let new_file_id =
            dst_fs.create_file(dst_dir_id, src_node.name()).await?;
        stream_file_content(
            src_fs,
            crate::FileId::new(*src_node_id.get()),
            src_node.path(),
            dst_fs,
            new_file_id,
        )
        .await?;
    }

    Ok(())
}

/// Mirror the contents of a source directory into an already created
/// destination directory on another provider instance.
fn mirror_directory_tree<'a, S, D>(
    src_fs: &'a S,
    src_dir_id: crate::NodeId<u64>,
    dst_fs: &'a D,
    dst_dir_id: crate::DirId<u64>,
) -> BoxFuture<'a, Result<(), FsError>>
where
    S: crate::fs::WritableFileSystem<Nid = u64>,
    D: crate::fs::WritableFileSystem<Nid = u64>,
{
    Box::pin(async move {
        // Populate the source directory's children, then walk them.
        src_fs
            .read_dir_node(crate::DirId::new(*src_dir_id.get()))
            .await?;
        let src_dir = src_fs.get_node(src_dir_id)?;

        for child_name in src_dir.kind().children().into_iter().flatten() {
            let child_id = src_fs
                .resolve_path(&src_dir.path().join(child_name))
                .await?;
            let child = src_fs.get_node(child_id)?;

            if child.kind().is_directory() {
                let new_dir_id =
                    dst_fs.create_dir_node(dst_dir_id, child.name()).await?;
                mirror_directory_tree(src_fs, child_id, dst_fs, new_dir_id)
                    .await?;
            } else if child.kind().is_file() {
                let new_file_id =
                    dst_fs.create_file(dst_dir_id, child.name()).await?;
                stream_file_content(
                    src_fs,
                    crate::FileId::new(*child_id.get()),
                    child.path(),
                    dst_fs,
                    new_file_id,
                )
                .await?;
            }
            // Symlinks and other special entries are skipped: their
            // targets are provider-local and would dangle on the
            // destination side.
        }

        Ok(())
    })
}

/// Move a file or directory across two distinct provider instances.
///
/// Implemented as copy + delete: there is no atomic cross-provider move,
/// so if the copy succeeds but the delete fails, the entry exists on both
/// sides (no data loss, but a duplicate to clean up).
async fn move_entry_cross<S, D>(
    src_fs: &S,
    src_root: &Path,
    rel: &str,
    is_dir: bool,
    dst_fs: &D,
    dst_root: &Path,
) -> Result<(), FsError>
where
    S: crate::fs::WritableFileSystem<Nid = u64>,
    D: crate::fs::WritableFileSystem<Nid = u64>,
{
    copy_entry_cross(src_fs, src_root, rel, is_dir, dst_fs, dst_root).await?;
    delete_entry(src_fs, src_root, rel, is_dir).await
}

/// Resolve the destination directory that will hold the entry addressed by
/// `rel` under `root`, creating missing intermediate directories.
///
/// The last component of `rel` names the entry itself and is *not* created
/// here; the caller copies or moves the node into the returned directory,
/// keeping its name. A target side that does not know the nesting yet gets
/// the whole ancestor chain created (e.g. `def/ghi` for `def/ghi/b.txt`).
async fn ensure_parent_dir<N>(
    fs: &N,
    root: &Path,
    rel: &str,
) -> Result<crate::DirId<u64>, FsError>
where
    N: crate::fs::WritableFileSystem<Nid = u64>,
{
    let root_id = fs.resolve_path(root).await?;
    let root_node = fs.get_node(root_id)?;
    if !root_node.kind().is_directory() {
        return Err(FsError::WrongKind {
            expected: "directory",
            actual: "file",
        });
    }

    let mut path = root.to_path_buf();
    let mut parent = crate::DirId::new(*root_id.get());
    let comps = rel_components(rel);
    for name in &comps[..comps.len().saturating_sub(1)] {
        path.push(name);
        parent = match fs.resolve_path(&path).await {
            Ok(id) => {
                let node = fs.get_node(id)?;
                if !node.kind().is_directory() {
                    return Err(FsError::WrongKind {
                        expected: "directory",
                        actual: "file",
                    });
                }
                crate::DirId::new(*id.get())
            }
            // The target side does not know this nesting level yet.
            Err(FsError::NotFound { .. }) => {
                fs.create_dir_node(parent, std::ffi::OsStr::new(name))
                    .await?
            }
            Err(e) => return Err(e),
        };
    }

    Ok(parent)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{env, path::PathBuf, process};

    use super::*;
    use crate::{
        CompareConfig, DirEntry, DirEntryStatus, EntryInfo, LocalFs, MockFs,
        compare::compare_directories_node, fs::NodeFileSystem,
        hash::ContentCache,
    };

    /// Read a file's content from a `MockFs` tree.
    async fn mock_content(fs: &MockFs, path: &str) -> Vec<u8> {
        fs.read_path(Path::new(path), None).await.unwrap().to_vec()
    }

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
        // Labels name the data flow; the variant names the destination.
        assert_eq!(TransferAction::CopyLeft.label(), "copy right → left");
        assert_eq!(TransferAction::DeleteRight.label(), "delete right");
        assert_eq!(TransferAction::MoveLeft.label(), "move right → left");
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
                    name_conflict: false,
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
                    name_conflict: false,
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
                name_conflict: false,
            }],
            ..Default::default()
        };

        let items = plan_transfers(&comparison, TransferAction::DeleteLeft);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "left_only.txt");
    }

    #[test]
    fn plan_transfers_skips_name_conflicts() {
        // Both halves of a file-vs-directory clash carry the flag, so no
        // action may collect them.
        let clash = |name: &str, is_left: bool, is_dir: bool| DirEntry {
            name: name.to_string(),
            status: if is_left {
                DirEntryStatus::LeftOnly
            } else {
                DirEntryStatus::RightOnly
            },
            left: is_left.then(|| EntryInfo {
                name: name.to_string(),
                path: format!("/left/{name}"),
                size: 10,
                modified: "2026-01-01T00:00:00Z".to_string(),
                is_dir,
                hash: None,
            }),
            right: (!is_left).then(|| EntryInfo {
                name: name.to_string(),
                path: format!("/right/{name}"),
                size: 10,
                modified: "2026-01-01T00:00:00Z".to_string(),
                is_dir,
                hash: None,
            }),
            center: None,
            sub_entries: None,
            name_conflict: true,
        };
        let comparison = DirComparison {
            entries: vec![
                clash("notes", true, false),
                clash("notes", false, true),
            ],
            ..Default::default()
        };

        for action in [
            TransferAction::CopyLeft,
            TransferAction::CopyRight,
            TransferAction::DeleteLeft,
            TransferAction::DeleteRight,
        ] {
            assert!(
                plan_transfers(&comparison, action).is_empty(),
                "{action:?} must not transfer a name conflict"
            );
        }
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
                name_conflict: false,
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
                    name_conflict: false,
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
                    name_conflict: false,
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
            "left_only.txt".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;

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
    async fn execute_copy_right_nested_creates_target_dirs() {
        // The reported failure: a deeply nested selection whose target-side
        // directories do not exist yet.
        let base = env::temp_dir()
            .join(format!("cocomo_transfer_deep_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        let left = base.join("abc");
        let right = base.join("xyz");
        fs_err::create_dir_all(left.join("def/ghi")).unwrap();
        fs_err::create_dir_all(&right).unwrap();
        fs_err::write(left.join("def/ghi/b.txt"), "nested payload").unwrap();

        let fs = LocalFs::new("test");
        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "b.txt".to_string(),
            false,
            "def/ghi/b.txt".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;
        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // The full relative path was mirrored onto the target side.
        assert_eq!(
            fs_err::read_to_string(right.join("def/ghi/b.txt")).unwrap(),
            "nested payload"
        );
        // The source is untouched by a copy.
        assert!(left.join("def/ghi/b.txt").exists());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn execute_move_right_nested_creates_target_dirs() {
        let base = env::temp_dir()
            .join(format!("cocomo_transfer_deep_mv_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        let left = base.join("abc");
        let right = base.join("xyz");
        fs_err::create_dir_all(left.join("def/ghi")).unwrap();
        fs_err::create_dir_all(&right).unwrap();
        fs_err::write(left.join("def/ghi/b.txt"), "payload").unwrap();

        let fs = LocalFs::new("test");
        let items = vec![TransferItem::new(
            TransferAction::MoveRight,
            "b.txt".to_string(),
            false,
            "def/ghi/b.txt".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;
        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        assert_eq!(
            fs_err::read_to_string(right.join("def/ghi/b.txt")).unwrap(),
            "payload"
        );
        assert!(!left.join("def/ghi/b.txt").exists());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn execute_copy_right_nested_directory() {
        // A nested *directory* transfer creates the ancestors of the entry
        // itself but keeps the entry's own name as the leaf.
        let base = env::temp_dir()
            .join(format!("cocomo_transfer_deepdir_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        let left = base.join("abc");
        let right = base.join("xyz");
        fs_err::create_dir_all(left.join("def/ghi")).unwrap();
        fs_err::write(left.join("def/ghi/c.txt"), "charlie").unwrap();
        fs_err::create_dir_all(&right).unwrap();

        let fs = LocalFs::new("test");
        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "ghi".to_string(),
            true,
            "def/ghi".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;
        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        assert_eq!(
            fs_err::read_to_string(right.join("def/ghi/c.txt")).unwrap(),
            "charlie"
        );

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn execute_copy_right_beyond_one_chunk() {
        let (left, right) = make_test_dirs();
        let fs = LocalFs::new("test");

        // Content larger than the stream chunk size, so the copy streams
        // several chunks into the destination.
        let content = vec![0xA5u8; 100 * 1024];
        fs_err::write(left.join("big.bin"), &content).unwrap();

        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "big.bin".to_string(),
            false,
            "big.bin".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // The destination must hold the full source content, not just the
        // last chunk.
        assert_eq!(fs_err::read(right.join("big.bin")).unwrap(), content);

        // The cached node metadata the comparison displays must show the
        // copied size, not the zero bytes of the freshly created file.
        let right_id = fs.resolve_path(&right.join("big.bin")).await.unwrap();
        assert_eq!(
            fs.node_metadata(right_id).unwrap().size,
            content.len() as u64
        );

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
            "left_only.txt".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;

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
    async fn execute_delete_left_nested() {
        let base = env::temp_dir()
            .join(format!("cocomo_transfer_deepdel_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        let left = base.join("left");
        fs_err::create_dir_all(left.join("def/ghi")).unwrap();
        fs_err::write(left.join("def/ghi/b.txt"), "payload").unwrap();

        let fs = LocalFs::new("test");
        let items = vec![TransferItem::new(
            TransferAction::DeleteLeft,
            "b.txt".to_string(),
            false,
            "def/ghi/b.txt".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &left, FsPair::shared(&fs)).await;
        assert!(result.is_ok(), "delete should succeed: {:?}", result.errors);
        assert!(!left.join("def/ghi/b.txt").exists());

        fs_err::remove_dir_all(&base).ok();
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
            "subdir".to_string(),
        )];

        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;

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
            "nonexistent.txt".to_string(),
        )];

        let result =
            execute_transfers(&items, &base, &base, FsPair::shared(&fs)).await;

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
        let result =
            execute_transfers(&items, &left, &right, FsPair::shared(&fs))
                .await;
        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );

        // Verify: right_only.txt now exists on left.
        assert!(left.join("right_only.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn execute_separate_pair_copy_succeeds() {
        let (left, right) = make_test_dirs();
        let left_fs = LocalFs::new("left");
        let right_fs = LocalFs::new("right");

        // CopyRight reads from the left and writes to the right side;
        // across two distinct providers the content is streamed into a
        // freshly created destination file.
        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "left_only.txt".to_string(),
            false,
            "left_only.txt".to_string(),
        )];

        let result = execute_transfers(
            &items,
            &left,
            &right,
            FsPair::Separate(&left_fs, &right_fs),
        )
        .await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // The file was copied across the boundary with its content.
        assert!(right.join("left_only.txt").exists());
        assert_eq!(
            fs_err::read_to_string(right.join("left_only.txt")).unwrap(),
            "only on left"
        );

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn execute_separate_pair_delete_succeeds() {
        let (left, right) = make_test_dirs();
        let left_fs = LocalFs::new("left");
        let right_fs = LocalFs::new("right");

        // DeleteLeft only ever touches the left side, so it runs
        // against the left provider even for a separate pair.
        let items = vec![TransferItem::new(
            TransferAction::DeleteLeft,
            "left_only.txt".to_string(),
            false,
            "left_only.txt".to_string(),
        )];

        let result = execute_transfers(
            &items,
            &left,
            &right,
            FsPair::Separate(&left_fs, &right_fs),
        )
        .await;

        assert!(result.is_ok(), "delete should succeed: {:?}", result.errors);
        assert!(!left.join("left_only.txt").exists());

        fs_err::remove_dir_all(left.parent().unwrap()).ok();
    }

    #[tokio::test]
    async fn execute_separate_pair_copy_directory_cross() {
        let src = MockFs::new("src")
            .with_dir("/src")
            .with_dir("/src/tree")
            .with_file("/src/tree/a.txt", "alpha")
            .with_dir("/src/tree/sub")
            .with_file("/src/tree/sub/b.txt", "bravo");
        let dst = MockFs::new("dst").with_dir("/dst");

        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "tree".to_string(),
            true,
            "tree".to_string(),
        )];

        let result = execute_transfers(
            &items,
            Path::new("/src"),
            Path::new("/dst"),
            FsPair::Separate(&src, &dst),
        )
        .await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // The directory tree was mirrored across the boundary.
        assert_eq!(mock_content(&dst, "/dst/tree/a.txt").await, b"alpha");
        assert_eq!(mock_content(&dst, "/dst/tree/sub/b.txt").await, b"bravo");
    }

    #[tokio::test]
    async fn execute_separate_pair_copy_nested_creates_target_dirs() {
        // Cross-boundary variant: the destination side lacks the whole
        // nesting of the transferred entry.
        let src = MockFs::new("src")
            .with_dir("/src")
            .with_file("/src/def/ghi/b.txt", "bravo");
        let dst = MockFs::new("dst").with_dir("/dst");

        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "b.txt".to_string(),
            false,
            "def/ghi/b.txt".to_string(),
        )];

        let result = execute_transfers(
            &items,
            Path::new("/src"),
            Path::new("/dst"),
            FsPair::Separate(&src, &dst),
        )
        .await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        assert_eq!(mock_content(&dst, "/dst/def/ghi/b.txt").await, b"bravo");
    }

    #[tokio::test]
    async fn execute_separate_pair_move_cross() {
        let src = MockFs::new("src")
            .with_dir("/src")
            .with_file("/src/moved.txt", "payload");
        let dst = MockFs::new("dst").with_dir("/dst");

        let items = vec![TransferItem::new(
            TransferAction::MoveRight,
            "moved.txt".to_string(),
            false,
            "moved.txt".to_string(),
        )];

        let result = execute_transfers(
            &items,
            Path::new("/src"),
            Path::new("/dst"),
            FsPair::Separate(&src, &dst),
        )
        .await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        // The entry was copied across and removed from the source side.
        assert_eq!(mock_content(&dst, "/dst/moved.txt").await, b"payload");
        assert!(
            src.read_path(Path::new("/src/moved.txt"), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn execute_separate_pair_copy_stream_error() {
        let src = MockFs::new("src")
            .with_dir("/src")
            .with_file("/src/broken.txt", "payload")
            .with_stream_error(
                "/src/broken.txt",
                FsError::Io {
                    operation: FsOperation::Read,
                    path: "/src/broken.txt".into(),
                    message: "injected read failure".to_owned(),
                },
            );
        let dst = MockFs::new("dst").with_dir("/dst");

        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "broken.txt".to_string(),
            false,
            "broken.txt".to_string(),
        )];

        let result = execute_transfers(
            &items,
            Path::new("/src"),
            Path::new("/dst"),
            FsPair::Separate(&src, &dst),
        )
        .await;

        assert_eq!(result.failed, 1);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.to_string().contains("stream error")),
            "expected a stream error: {:?}",
            result.errors
        );
    }

    #[tokio::test]
    async fn execute_separate_pair_copy_dir_walk_error() {
        let src = MockFs::new("src")
            .with_dir("/src")
            .with_dir("/src/tree")
            .with_file("/src/tree/blocked.txt", "bravo")
            .with_error(
                "/src/tree/blocked.txt",
                FsError::PermissionDenied {
                    operation: FsOperation::Read,
                    path: "/src/tree/blocked.txt".into(),
                },
            )
            .with_file("/src/tree/ok.txt", "alpha");
        let dst = MockFs::new("dst").with_dir("/dst");

        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "tree".to_string(),
            true,
            "tree".to_string(),
        )];

        let result = execute_transfers(
            &items,
            Path::new("/src"),
            Path::new("/dst"),
            FsPair::Separate(&src, &dst),
        )
        .await;

        // The walk fails on the injected child, so the whole directory
        // copy is collected as one error.
        assert_eq!(result.failed, 1);
    }

    #[tokio::test]
    async fn execute_separate_pair_move_copy_failure_keeps_source() {
        let src = MockFs::new("src")
            .with_dir("/src")
            .with_file("/src/moved.txt", "payload");
        // The destination refuses the creation, so the copy fails before
        // the delete can run.
        let dst = MockFs::new("dst").with_dir("/dst").with_error(
            "/dst/moved.txt",
            FsError::PermissionDenied {
                operation: FsOperation::CreateFile,
                path: "/dst/moved.txt".into(),
            },
        );

        let items = vec![TransferItem::new(
            TransferAction::MoveRight,
            "moved.txt".to_string(),
            false,
            "moved.txt".to_string(),
        )];

        let result = execute_transfers(
            &items,
            Path::new("/src"),
            Path::new("/dst"),
            FsPair::Separate(&src, &dst),
        )
        .await;

        assert_eq!(result.failed, 1);
        // The source entry must survive a failed cross-boundary move.
        assert_eq!(mock_content(&src, "/src/moved.txt").await, b"payload");
    }

    #[tokio::test]
    async fn execute_separate_pair_mockfs_to_localfs() {
        let base = env::temp_dir()
            .join(format!("cocomo_transfer_hetero_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        let dst_root = base.join("dst");
        fs_err::create_dir_all(&dst_root).unwrap();

        // Heterogeneous pair: the source side is a `MockFs` tree, the
        // destination side a real `LocalFs`.
        let src = MockFs::new("src")
            .with_dir("/src")
            .with_file("/src/hetero.txt", "cross backend");
        let dst_fs = LocalFs::new("dst");

        let items = vec![TransferItem::new(
            TransferAction::CopyRight,
            "hetero.txt".to_string(),
            false,
            "hetero.txt".to_string(),
        )];

        let result = execute_transfers(
            &items,
            Path::new("/src"),
            &dst_root,
            FsPair::Separate(&src, &dst_fs),
        )
        .await;

        assert!(
            result.is_ok(),
            "transfer should succeed: {:?}",
            result.errors
        );
        assert_eq!(
            fs_err::read_to_string(dst_root.join("hetero.txt")).unwrap(),
            "cross backend"
        );

        fs_err::remove_dir_all(&base).ok();
    }
}
