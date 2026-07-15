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
//! Uses the `FileSystem` trait to traverse directories, so it works with any
//! provider. For the local filesystem, inode and device ID tracking is used
//! to detect hard-link and symlink cycles and to scope inode uniqueness per
//! mount point.

use std::{
    collections::HashMap,
    mem,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    error::Result,
    fs::{FileSystem, NodeFileSystem},
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

/// Recursively scan a directory using the given `FileSystem` provider.
///
/// Tracks visited `(inode, device_id)` pairs to detect and skip hard-link and
/// symlink cycles. Per-entry errors (e.g., permission denied) are collected
/// rather than aborting the entire scan.
///
/// Uses an iterative BFS traversal to avoid async recursion issues.
pub async fn scan_directory(
    fs: &Arc<dyn FileSystem>,
    path: &Path,
    config: &ScanConfig,
) -> Result<ScanResult> {
    let mut result = ScanResult::default();
    // Track visited inodes per device to detect cycles.
    let mut visited: HashMap<(u64, u64), ()> = HashMap::new();

    let root_meta = fs.metadata(path).await.map_err(|e| match e {
        crate::error::FsError::NotFound { .. } => e,
        _ => e,
    })?;

    if !root_meta.is_dir {
        // The root is not a directory; return the single entry.
        result.entries.push(ScanEntry {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.to_string_lossy().into_owned(),
            meta: root_meta,
            children: None,
        });
        return Ok(result);
    }

    // Track the root directory's inode to prevent re-entering it.
    if let (Some(ino), Some(dev)) = (root_meta.inode, root_meta.device_id) {
        visited.insert((ino, dev), ());
    }

    // BFS work queue: (absolute_path, relative_path, depth)
    let mut queue: Vec<(PathBuf, String, usize)> = Vec::new();
    queue.push((path.to_path_buf(), String::new(), 0));

    while let Some((dir_path, rel_prefix, depth)) = queue.pop() {
        // Check max depth.
        if let Some(max) = config.max_depth
            && depth >= max
        {
            continue;
        }

        // Read directory entries.
        let mut dir_stream = match fs.read_dir(&dir_path).await {
            Ok(s) => s,
            Err(e) => {
                result.errors.push(e);
                continue;
            }
        };

        use futures::StreamExt;

        let mut entries: Vec<_> = Vec::new();
        while let Some(entry_result) = dir_stream.next().await {
            match entry_result {
                Ok(em) => entries.push(em),
                Err(e) => result.errors.push(e),
            }
        }

        let mut root_children: Vec<ScanEntry> = Vec::new();

        for entry_meta in entries {
            let entry_path = dir_path.join(&entry_meta.name);
            let rel_path = if rel_prefix.is_empty() {
                entry_meta.name.clone()
            } else {
                format!("{rel_prefix}/{}", entry_meta.name)
            };

            // Cycle detection: skip already-visited inodes.
            if let (Some(ino), Some(dev)) =
                (entry_meta.meta.inode, entry_meta.meta.device_id)
            {
                if entry_meta.meta.is_dir && visited.contains_key(&(ino, dev))
                {
                    continue;
                }
                if entry_meta.meta.is_dir {
                    visited.insert((ino, dev), ());
                }
            }

            // Skip symlink targets unless configured to follow them.
            if entry_meta.meta.is_symlink && !config.follow_symlinks {
                root_children.push(ScanEntry {
                    name: entry_meta.name,
                    path: rel_path,
                    meta: entry_meta.meta,
                    children: None,
                });
                continue;
            }

            if entry_meta.meta.is_dir {
                // Queue for BFS traversal.
                queue.push((entry_path, rel_path.clone(), depth + 1));
                // Add a placeholder entry that will be populated after
                // scanning.
                root_children.push(ScanEntry {
                    name: entry_meta.name,
                    path: rel_path,
                    meta: entry_meta.meta,
                    children: None,
                });
            } else {
                root_children.push(ScanEntry {
                    name: entry_meta.name,
                    path: rel_path,
                    meta: entry_meta.meta,
                    children: None,
                });
            }
        }

        result.entries.extend(root_children);
    }

    // Second pass: link children to their parent directories.
    // Because we used BFS, children of a directory appear after the directory
    // in the flat list. We rebuild the tree by matching paths.
    if !result.entries.is_empty() {
        let flat = mem::take(&mut result.entries);
        let tree = build_tree(flat);
        result.entries = tree;
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// Node-based scanning
// ---------------------------------------------------------------------------

/// Recursively scan a directory using the node-based `NodeFileSystem` API.
///
/// Unlike [`scan_directory`], this operates on opaque [`NodeId`] identifiers
/// instead of paths, eliminating TOCTOU races. The provider's node cache is
/// populated during the scan.
///
/// Symlink cycle detection relies on the provider's cache: if a symlink
/// resolves to an already-visited node ID, the cycle is detected.
pub async fn scan_directory_node<N>(
    fs: &N,
    path: &Path,
    config: &ScanConfig,
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
        // Check max depth.
        if let Some(max) = config.max_depth
            && depth >= max
        {
            continue;
        }

        // Populate children via the node API.
        let dir_id_typed = crate::identity::DirId::<u64>::new(*dir_id.get());
        if let Err(e) = fs.read_dir_node(dir_id_typed).await {
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
    use std::{env, process};

    use super::*;
    use crate::local::LocalFs;

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

    #[tokio::test]
    async fn scan_flat_directory() {
        let base = setup_test_dir().await;
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));

        let result = scan_directory(&fs, &base, &ScanConfig::default())
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
    async fn scan_recursive() {
        let base = setup_test_dir().await;
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));

        let result = scan_directory(&fs, &base, &ScanConfig::default())
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
    async fn scan_max_depth() {
        let base = setup_test_dir().await;
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));

        let config = ScanConfig {
            max_depth: Some(1),
            ..Default::default()
        };
        let result = scan_directory(&fs, &base, &config).await.unwrap();

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
    async fn scan_nonexistent_path() {
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));
        let path = Path::new("/nonexistent/cocomo_scan_xyz");
        let result = scan_directory(&fs, path, &ScanConfig::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn scan_single_file() {
        let base = setup_test_dir().await;
        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));

        let file_path = base.join("a.txt");
        let result = scan_directory(&fs, &file_path, &ScanConfig::default())
            .await
            .unwrap();

        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].name, "a.txt");
        assert!(!result.entries[0].is_dir());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn scan_empty_directory() {
        let base = env::temp_dir()
            .join(format!("cocomo_scan_empty_{}", process::id()));
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));
        let result = scan_directory(&fs, &base, &ScanConfig::default())
            .await
            .unwrap();

        assert!(result.entries.is_empty());

        fs_err::remove_dir_all(&base).ok();
    }

    // -----------------------------------------------------------------------
    // Node-based scan tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn node_scan_flat_directory() {
        let base = setup_test_dir().await;
        let fs = LocalFs::new("test");

        let result = scan_directory_node(&fs, &base, &ScanConfig::default())
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

        let result = scan_directory_node(&fs, &base, &ScanConfig::default())
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
        let result = scan_directory_node(&fs, &base, &config).await.unwrap();

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
            scan_directory_node(&fs, path, &ScanConfig::default()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn node_scan_single_file() {
        let base = setup_test_dir().await;
        let fs = LocalFs::new("test");

        let file_path = base.join("a.txt");
        let result =
            scan_directory_node(&fs, &file_path, &ScanConfig::default())
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
        let result = scan_directory_node(&fs, &base, &ScanConfig::default())
            .await
            .unwrap();

        assert!(result.entries.is_empty());

        fs_err::remove_dir_all(&base).ok();
    }
}
