// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Directory scanning that produces a tree of `ScanEntry` nodes.
//!
//! Uses the node-based `NodeFileSystem` API to traverse directories, so it
//! works with any provider. Cycle detection relies on the provider's node
//! cache: if a directory resolves to an already-visited node ID, the cycle
//! is skipped.

use std::{collections::HashMap, mem, path::Path};

use tokio_util::sync::CancellationToken;

use crate::{
    error::{FsError, Result},
    fs::NodeFileSystem,
    identity::NodeId,
    meta::Metadata,
    node::Node,
};

/// A single entry in the scanned directory tree.
#[derive(Clone, Debug)]
pub struct ScanEntry {
    /// Name of the file or directory (not the full path).
    pub name: String,
    /// Full path relative to the scan root.
    pub path: String,
    /// Metadata captured during the scan.
    pub meta: Metadata,
    /// Children, present only for directories.
    pub children: Option<Vec<ScanEntry>>,
}

impl ScanEntry {
    /// Return `true` when this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.meta.is_dir
    }

    /// Return the children of this directory entry, if any.
    pub fn children(&self) -> Option<&[ScanEntry]> {
        self.children.as_deref()
    }
}

/// Configuration for a directory scan.
#[derive(Clone, Debug, Default)]
pub struct ScanConfig {
    /// Follow symbolic links during traversal. Disabled by default to avoid
    /// unintended cycles and cross-filesystem traversal.
    pub follow_symlinks: bool,
    /// Maximum recursion depth. `None` means unlimited.
    pub max_depth: Option<usize>,
}

/// Result of a directory scan.
#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    /// Root entries (direct children of the scanned path).
    pub entries: Vec<ScanEntry>,
    /// Errors encountered during the scan (e.g., permission denied on
    /// subdirectories). These are non-fatal and reported individually.
    pub errors: Vec<crate::error::FsError>,
}

// ---------------------------------------------------------------------------
// Directory scanning
// ---------------------------------------------------------------------------

/// Recursively scan a directory using the node-based `NodeFileSystem` API.
///
/// Operates on opaque [`NodeId`] identifiers instead of paths, eliminating
/// TOCTOU races. The provider's node cache is populated during the scan.
///
/// Symlink cycle detection relies on the provider's cache: if a symlink
/// resolves to an already-visited node ID, the cycle is detected.
pub async fn scan_directory_node<N>(
    fs: &N,
    path: &Path,
    config: &ScanConfig,
    cancel: Option<&CancellationToken>,
) -> Result<ScanResult>
where
    N: NodeFileSystem<Nid = u64>,
{
    let mut result = ScanResult::default();

    // Resolve the root path to a node.
    let root_id = fs.resolve_path(path).await?;
    let root_node = fs.get_node(root_id)?;

    // Use the absolute path of the resolved root for computing relative
    // entry paths, regardless of whether the caller passed a relative or
    // absolute scan root.
    let scan_root = root_node.path();

    if !root_node.kind().is_directory() {
        // The root is not a directory; return the single entry.
        result
            .entries
            .push(scan_node_to_entry(&root_node, &root_id, scan_root));
        return Ok(result);
    }

    // Track visited directory node IDs to detect cycles.
    let mut visited: std::collections::HashSet<u64> =
        std::collections::HashSet::new();
    visited.insert(*root_id.get());

    // BFS work queue: (node_id, relative_path, depth)
    type QueueItem = (NodeId<u64>, String, usize);
    let mut queue: Vec<QueueItem> = Vec::new();
    queue.push((root_id, String::new(), 0));

    while let Some((dir_id, rel_prefix, depth)) = queue.pop() {
        // Abort promptly when the caller's cancellation token fires, so a
        // long walk over a big tree does not run to completion just to be
        // discarded.
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(FsError::Cancelled);
        }
        // Check max depth.
        if let Some(max) = config.max_depth
            && depth >= max
        {
            continue;
        }

        // Populate children via the node API.
        let dir_id_typed = crate::identity::DirId::<u64>::new(*dir_id.get());
        if let Err(e) = fs.read_dir_node(dir_id_typed).await {
            // A root that cannot be listed means the endpoint is unusable;
            // abort instead of returning an empty result.
            if depth == 0 {
                return Err(e);
            }
            result.errors.push(e);
            continue;
        }

        // Get the directory node to access children.
        let dir_node = match fs.get_node(dir_id) {
            Ok(n) => n,
            Err(e) => {
                result.errors.push(e);
                continue;
            }
        };

        let children = match dir_node.kind().children() {
            Some(c) => c.to_vec(),
            None => continue,
        };

        // For each child name, resolve via the provider's path-based lookup.
        // The parent node stores the absolute path.
        let parent_path = dir_node.path();
        let mut child_nodes = Vec::new();
        for name in &children {
            let child_path = parent_path.join(name);
            match fs.resolve_path(&child_path).await {
                Ok(child_id) => {
                    if let Ok(child_node) = fs.get_node(child_id) {
                        child_nodes.push((child_id, child_node));
                    }
                }
                Err(e) => result.errors.push(e),
            }
        }

        for (child_id, child_node) in child_nodes {
            let rel_path = if rel_prefix.is_empty() {
                child_node.name().to_string_lossy().into_owned()
            } else {
                format!(
                    "{}/{}",
                    rel_prefix,
                    child_node.name().to_string_lossy()
                )
            };

            // Cycle detection for directories.
            if child_node.kind().is_directory() {
                let child_id_val = *child_id.get();
                if visited.contains(&child_id_val) {
                    continue;
                }
                visited.insert(child_id_val);
            }

            // Skip symlink targets unless configured to follow them.
            if child_node.kind().is_symlink() && !config.follow_symlinks {
                result.entries.push(scan_node_to_entry(
                    &child_node,
                    &child_id,
                    scan_root,
                ));
                continue;
            }

            if child_node.kind().is_directory() {
                // Queue for BFS traversal.
                queue.push((child_id, rel_path.clone(), depth + 1));
                // Add placeholder.
                result.entries.push(scan_node_to_entry(
                    &child_node,
                    &child_id,
                    scan_root,
                ));
            } else {
                result.entries.push(scan_node_to_entry(
                    &child_node,
                    &child_id,
                    scan_root,
                ));
            }
        }
    }

    // Build the tree from the flat BFS list.
    if !result.entries.is_empty() {
        let flat = mem::take(&mut result.entries);
        result.entries = build_tree(flat);
    }

    Ok(result)
}

/// Convert a `Node` into a `ScanEntry`.
fn scan_node_to_entry(
    node: &Node,
    _node_id: &NodeId<u64>,
    scan_root: &Path,
) -> ScanEntry {
    let name = node.name().to_string_lossy().into_owned();
    let meta = node.metadata().clone();

    // Compute relative path from scan root.
    let node_path = node.path();
    let rel = node_path
        .strip_prefix(scan_root)
        .unwrap_or(node_path)
        .to_string_lossy()
        .into_owned();

    ScanEntry {
        name,
        path: rel,
        meta,
        children: None,
    }
}

/// Build a tree of `ScanEntry` nodes from a flat BFS-ordered list.
fn build_tree(flat: Vec<ScanEntry>) -> Vec<ScanEntry> {
    // Index entries by their path for O(1) lookup.
    let mut by_parent: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, entry) in flat.iter().enumerate() {
        if let Some((parent, _name)) = entry.path.rsplit_once('/') {
            by_parent.entry(parent.to_string()).or_default().push(i);
        }
    }

    // Build the tree by collecting children into temporary buffers.
    let mut entries: Vec<Option<ScanEntry>> =
        flat.into_iter().map(Some).collect();

    // Build a path-to-index map for efficient parent lookup.
    let mut path_idx: HashMap<String, usize> = HashMap::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(e) = entry {
            path_idx.insert(e.path.clone(), i);
        }
    }

    // Process parents deepest-first so that child directories are assembled
    // before their own parents claim them.
    let mut parents: Vec<_> = by_parent.into_iter().collect();
    parents.sort_by(|(a, _), (b, _)| {
        b.rsplit('/').count().cmp(&a.rsplit('/').count())
    });

    for (parent_path, child_indices) in parents {
        // Collect children first to avoid double mutable borrow.
        let children: Vec<ScanEntry> = child_indices
            .into_iter()
            .filter_map(|idx| entries[idx].take())
            .collect();

        if let Some(parent_idx) = path_idx.get(&parent_path).copied() {
            let parent =
                entries[parent_idx].as_mut().expect("parent should exist");
            parent.children = Some(children);
        }
    }

    entries.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        ops::Range,
        path::{Path, PathBuf},
        pin::Pin,
        process,
        sync::Arc,
    };

    use async_trait::async_trait;
    use bytes::Bytes;
    use futures::Stream;

    use super::*;
    use crate::{
        DirId, FileId, FileSystemId, FsError, FsFile, FsOperation, Metadata,
        MockFs, OpenMode, local::LocalFs,
    };

    async fn setup_test_dir() -> PathBuf {
        let base =
            env::temp_dir().join(format!("cocomo_scan_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(base.join("sub/nested")).unwrap();
        fs_err::write(base.join("a.txt"), "alpha").unwrap();
        fs_err::write(base.join("sub/b.txt"), "bravo").unwrap();
        fs_err::write(base.join("sub/nested/c.txt"), "charlie").unwrap();
        base
    }

    // -----------------------------------------------------------------------
    // Node-based scan tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn node_scan_flat_directory() {
        let base = setup_test_dir().await;
        let fs = LocalFs::new("test");

        let result =
            scan_directory_node(&fs, &base, &ScanConfig::default(), None)
                .await
                .unwrap();

        let names: Vec<&str> =
            result.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a.txt"));
        assert!(names.contains(&"sub"));
        assert_eq!(result.errors.len(), 0);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_scan_recursive() {
        let base = setup_test_dir().await;
        let fs = LocalFs::new("test");

        let result =
            scan_directory_node(&fs, &base, &ScanConfig::default(), None)
                .await
                .unwrap();

        let sub = result
            .entries
            .iter()
            .find(|e| e.name == "sub")
            .expect("sub directory should exist");
        assert!(sub.is_dir());
        let children = sub.children().expect("sub should have children");
        let child_names: Vec<&str> =
            children.iter().map(|e| e.name.as_str()).collect();
        assert!(child_names.contains(&"b.txt"));
        assert!(child_names.contains(&"nested"));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_scan_max_depth() {
        let base = setup_test_dir().await;
        let fs = LocalFs::new("test");

        let config = ScanConfig {
            max_depth: Some(1),
            ..Default::default()
        };
        let result = scan_directory_node(&fs, &base, &config, None)
            .await
            .unwrap();

        let sub = result
            .entries
            .iter()
            .find(|e| e.name == "sub")
            .expect("sub directory should exist");
        assert!(
            sub.children().is_none() || sub.children().unwrap().is_empty()
        );

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_scan_nonexistent_path() {
        let fs = LocalFs::new("test");
        let path = Path::new("/nonexistent/cocomo_scan_xyz");
        let result =
            scan_directory_node(&fs, path, &ScanConfig::default(), None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn node_scan_single_file() {
        let base = setup_test_dir().await;
        let fs = LocalFs::new("test");

        let file_path = base.join("a.txt");
        let result =
            scan_directory_node(&fs, &file_path, &ScanConfig::default(), None)
                .await
                .unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].name, "a.txt");
        assert!(!result.entries[0].is_dir());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_scan_empty_directory() {
        let base = env::temp_dir()
            .join(format!("cocomo_scan_node_empty_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let result =
            scan_directory_node(&fs, &base, &ScanConfig::default(), None)
                .await
                .unwrap();

        assert!(result.entries.is_empty());

        fs_err::remove_dir_all(&base).ok();
    }

    // -----------------------------------------------------------------
    // Root-level error propagation
    // -----------------------------------------------------------------

    /// A [`NodeFileSystem`] that fails every `read_dir_node` but delegates
    /// all other operations to the wrapped [`MockFs`]. Simulates a provider
    /// whose connection drops after the root resolves.
    struct FailingReadDirNode(MockFs);

    #[async_trait]
    impl NodeFileSystem for FailingReadDirNode {
        type FsId = u64;
        type Nid = u64;
        type Error = FsError;

        fn id(&self) -> FileSystemId<Self::FsId> {
            self.0.id()
        }

        fn label(&self) -> &str {
            self.0.label()
        }

        async fn resolve_path(
            &self,
            path: &Path,
        ) -> Result<NodeId<Self::Nid>> {
            self.0.resolve_path(path).await
        }

        async fn resolve_symlink(
            &self,
            id: NodeId<Self::Nid>,
        ) -> Result<NodeId<Self::Nid>> {
            self.0.resolve_symlink(id).await
        }

        fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>> {
            self.0.get_node(id)
        }

        fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata> {
            self.0.node_metadata(id)
        }

        fn set_node_hash(
            &self,
            id: NodeId<Self::Nid>,
            hash: String,
        ) -> Result<()> {
            self.0.set_node_hash(id, hash)
        }

        async fn read_dir_node(&self, _id: DirId<Self::Nid>) -> Result<()> {
            Err(FsError::Io {
                operation: FsOperation::ReadDir,
                path: PathBuf::from("/data"),
                message: "connection lost".to_string(),
            })
        }

        async fn open_node(
            &self,
            id: FileId<Self::Nid>,
            mode: OpenMode,
        ) -> Result<Box<dyn FsFile>> {
            self.0.open_node(id, mode).await
        }

        async fn read_node(
            &self,
            id: FileId<Self::Nid>,
            range: Option<Range<u64>>,
        ) -> Result<Bytes> {
            self.0.read_node(id, range).await
        }

        async fn read_stream_node(
            &self,
            id: FileId<Self::Nid>,
            range: Option<Range<u64>>,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>>
        {
            self.0.read_stream_node(id, range).await
        }
    }

    #[tokio::test]
    async fn node_scan_root_read_dir_error_aborts() {
        let fs = FailingReadDirNode(
            MockFs::new("mock")
                .with_dir("/data")
                .with_file("/data/a.txt", "alpha"),
        );

        let result = scan_directory_node(
            &fs,
            Path::new("/data"),
            &ScanConfig::default(),
            None,
        )
        .await;

        assert!(matches!(result, Err(FsError::Io { .. })));
    }

    #[tokio::test]
    async fn node_scan_subdir_error_is_collected() {
        let fs = MockFs::new("mock")
            .with_dir("/data")
            .with_file("/data/a.txt", "alpha")
            .with_dir("/data/sub")
            .with_error(
                "/data/sub",
                FsError::PermissionDenied {
                    operation: FsOperation::ReadDir,
                    path: PathBuf::from("/data/sub"),
                },
            );

        let result = scan_directory_node(
            &fs,
            Path::new("/data"),
            &ScanConfig::default(),
            None,
        )
        .await
        .unwrap();

        assert_eq!(result.errors.len(), 1);
        assert!(matches!(result.errors[0], FsError::PermissionDenied { .. }));
    }
}
