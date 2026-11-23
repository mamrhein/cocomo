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

use std::{
    collections::HashMap,
    mem,
    path::{Path, PathBuf},
};

use tokio_util::sync::CancellationToken;

use crate::{
    error::{FsError, Result},
    fs::NodeFileSystem,
    identity::NodeId,
    meta::Metadata,
    node::{Node, NodeKind},
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
    /// Target recorded in a symbolic link, `None` for every other entry.
    pub link_target: Option<String>,
    /// `true` when the link had to be resolved but its chain never settled
    /// on a final target (broken link, cycle, or exhausted hop budget).
    pub link_unresolved: bool,
    /// Absolute path of the final target this link was resolved to, present
    /// only when [`ScanConfig::resolve_links`] resolved it. Content is read
    /// from this path rather than from the link itself.
    pub resolved_path: Option<PathBuf>,
}

impl ScanEntry {
    /// Return `true` when this entry is a directory.
    pub fn is_dir(&self) -> bool {
        self.meta.is_dir
    }

    /// Return the path whose content represents this entry: the final
    /// target of a resolved link, else the entry's own path below `root`.
    pub fn content_path(&self, root: &Path) -> PathBuf {
        self.resolved_path
            .clone()
            .unwrap_or_else(|| root.join(&self.path))
    }

    /// Return the children of this directory entry, if any.
    pub fn children(&self) -> Option<&[ScanEntry]> {
        self.children.as_deref()
    }
}

/// Configuration for a directory scan.
///
/// The scan always walks the full tree; there is no depth limit.
#[derive(Clone, Debug, Default)]
pub struct ScanConfig {
    /// Resolve symbolic links to their final target during traversal. The
    /// scanned entry keeps reporting a link, but carries the metadata of
    /// the target and a resolved directory target is walked. Disabled by
    /// default to avoid unintended cycles and cross-filesystem traversal.
    pub resolve_links: bool,
}

/// Max number of symlink hops followed when resolving a link chain.
pub const MAX_SYMLINK_HOPS: u32 = 8;

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
/// With [`ScanConfig::resolve_links`] a link is scanned with the metadata
/// of its final target (see [`resolve_final_target`]) while the entry
/// itself keeps reporting a link; a resolved directory target is walked
/// under the link's path. Unresolvable links are recorded as such on the
/// entry.
///
/// Cycle detection relies on the provider's node cache: if a directory
/// (or a resolved link target) resolves to an already-visited node ID,
/// the cycle is skipped.
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

    if !root_node.kind().is_directory() {
        // The root is not a directory; return the single entry, addressed
        // by the empty relative path.
        result.entries.push(scan_node_to_entry(&root_node, ""));
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

            // A link is either scanned as the link node itself or, when
            // links are resolved, with the metadata of its final target.
            if child_node.kind().is_symlink() {
                let mut entry = scan_node_to_entry(&child_node, &rel_path);
                if config.resolve_links {
                    match resolve_final_target(fs, child_id).await {
                        Some(target_id) => match fs.get_node(target_id) {
                            Ok(target_node) => {
                                // The entry stays a link (that is what
                                // the user sees and what a transfer
                                // preserves); only the content-side
                                // attributes come from the target.
                                entry.meta = target_node.metadata().clone();
                                entry.meta.is_symlink = true;
                                entry.resolved_path =
                                    Some(target_node.path().to_path_buf());
                                if entry.is_dir()
                                    && visited.insert(*target_id.get())
                                {
                                    // Walk the target directory under the
                                    // link's path, skipping a target that
                                    // was already visited (a cycle).
                                    queue.push((
                                        target_id,
                                        rel_path.clone(),
                                        depth + 1,
                                    ));
                                }
                            }
                            Err(e) => {
                                result.errors.push(e);
                                entry.link_unresolved = true;
                            }
                        },
                        None => {
                            // A broken link, a cycle, or a chain longer
                            // than the hop budget; the entry keeps the
                            // link's own metadata.
                            entry.link_unresolved = true;
                        }
                    }
                }
                result.entries.push(entry);
                continue;
            }

            // Cycle detection for directories.
            if child_node.kind().is_directory() {
                let child_id_val = *child_id.get();
                if visited.contains(&child_id_val) {
                    continue;
                }
                visited.insert(child_id_val);
            }

            if child_node.kind().is_directory() {
                // Queue for BFS traversal.
                queue.push((child_id, rel_path.clone(), depth + 1));
                // Add placeholder.
                result
                    .entries
                    .push(scan_node_to_entry(&child_node, &rel_path));
            } else {
                result
                    .entries
                    .push(scan_node_to_entry(&child_node, &rel_path));
            }
        }
    }

    // Build the tree from the flat BFS list.
    if !result.entries.is_empty() {
        let flat = mem::take(&mut result.entries);
        result.entries = build_tree(flat);
        // Provider metadata carries no meaningful size for directories,
        // so derive each directory's size from its contents.
        derive_dir_sizes(&mut result.entries);
    }

    Ok(result)
}

/// Convert a `Node` into a `ScanEntry` addressed by `rel_path`.
///
/// The relative path is passed in rather than derived from the node's own
/// path, because a walked link target lives outside of the scan root: its
/// children are addressed through the link's path, not their own.
fn scan_node_to_entry(node: &Node, rel_path: &str) -> ScanEntry {
    let name = node.name().to_string_lossy().into_owned();
    let meta = node.metadata().clone();
    let link_target = match node.kind() {
        NodeKind::Symlink { target } => {
            Some(target.path().to_string_lossy().into_owned())
        }
        _ => None,
    };

    ScanEntry {
        name,
        path: rel_path.to_string(),
        meta,
        children: None,
        link_target,
        link_unresolved: false,
        resolved_path: None,
    }
}

/// Follow a symlink chain to its final target.
///
/// Returns the identifier of the first non-link node reached from `start`,
/// `None` when a hop is broken, the chain cycles, or it does not settle on
/// a final target within [`MAX_SYMLINK_HOPS`] hops. Callers must only
/// invoke this for a node whose kind is [`NodeKind::Symlink`]; for any
/// other node the first hop fails and the answer is `None`.
pub async fn resolve_final_target<N>(
    fs: &N,
    start: NodeId<N::Nid>,
) -> Option<NodeId<N::Nid>>
where
    N: NodeFileSystem,
{
    let mut id = start;
    for _ in 0..MAX_SYMLINK_HOPS {
        id = fs.resolve_symlink(id).await.ok()?;
        let node = fs.get_node(id).ok()?;
        if !node.kind().is_symlink() {
            return Some(id);
        }
    }
    // The chain did not settle on a final target within the hop budget.
    None
}

/// Derive the size of every directory entry as the sum of the sizes of
/// its content items, recursively.
///
/// Runs bottom-up so a subdirectory's size is final before its parent
/// sums it. Directories whose subtree could not be scanned (errors,
/// cycle skips) contribute nothing; an unknown size is treated as zero
/// rather than as a bogus provider-reported value.
///
/// Returns the total size of `entries`.
fn derive_dir_sizes(entries: &mut [ScanEntry]) -> u64 {
    let mut total = 0;
    for entry in entries {
        let size = if entry.is_dir() {
            match entry.children.as_mut() {
                Some(children) => derive_dir_sizes(children),
                // An empty directory holds no content items.
                None => 0,
            }
        } else {
            entry.meta.size
        };
        entry.meta.size = size;
        total += size;
    }
    total
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
    async fn node_scan_derives_dir_sizes() {
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
        // The dir size is the sum of its contents: b.txt (5 bytes) plus
        // `nested` (7 bytes, itself the size of c.txt).
        assert_eq!(sub.meta.size, 12);
        let nested = sub
            .children()
            .expect("sub should have children")
            .iter()
            .find(|e| e.name == "nested")
            .expect("nested directory should exist");
        assert_eq!(nested.meta.size, 7);

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
                FsError::permission_denied(
                    FsOperation::ReadDir,
                    PathBuf::from("/data/sub"),
                ),
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

    // -----------------------------------------------------------------------
    // Link resolution tests
    //
    // They need real symlinks, so they only run on unix.
    // -----------------------------------------------------------------------

    /// Look up a scanned entry by its path relative to the scan root.
    #[cfg(unix)]
    fn find_entry<'a>(
        entries: &'a [ScanEntry],
        path: &str,
    ) -> Option<&'a ScanEntry> {
        for entry in entries {
            if entry.path == path {
                return Some(entry);
            }
            if let Some(children) = entry.children()
                && let Some(found) = find_entry(children, path)
            {
                return Some(found);
            }
        }
        None
    }

    #[cfg(unix)]
    fn symlink(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_links_adopts_the_metadata_of_a_file_target() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path();
        fs_err::write(root.join("target.txt"), "alpha").unwrap();
        symlink(Path::new("target.txt"), &root.join("link.txt"));

        let fs = LocalFs::new("test");
        let result = scan_directory_node(
            &fs,
            root,
            &ScanConfig {
                resolve_links: true,
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.errors.len(), 0);

        let link =
            find_entry(&result.entries, "link.txt").expect("link entry");
        // The entry keeps reporting a link, but carries the target's
        // metadata.
        assert!(link.meta.is_symlink);
        assert!(!link.meta.is_dir);
        assert_eq!(link.meta.size, 5);
        assert_eq!(link.link_target.as_deref(), Some("target.txt"));
        assert!(!link.link_unresolved);
        assert_eq!(
            link.resolved_path.as_deref(),
            Some(root.join("target.txt").as_path())
        );
        // Content is read from the target, not from the link.
        assert_eq!(link.content_path(root), root.join("target.txt"));

        let target =
            find_entry(&result.entries, "target.txt").expect("target entry");
        assert!(!target.meta.is_symlink);
        assert_eq!(target.content_path(root), root.join("target.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolve_links_walks_a_directory_target() {
        let base = tempfile::TempDir::new().unwrap();
        // The target lives outside of the scan root, so walking it cannot
        // collide with an entry the scan reaches by path.
        let root = base.path().join("root");
        let outside = base.path().join("outside");
        fs_err::create_dir_all(&root).unwrap();
        fs_err::create_dir_all(&outside).unwrap();
        fs_err::write(outside.join("c.txt"), "charlie").unwrap();
        symlink(&outside, &root.join("dlink"));

        let fs = LocalFs::new("test");
        let result = scan_directory_node(
            &fs,
            &root,
            &ScanConfig {
                resolve_links: true,
            },
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.errors.len(), 0);

        let dlink = find_entry(&result.entries, "dlink").expect("link entry");
        assert!(dlink.meta.is_symlink);
        assert!(dlink.is_dir());
        assert!(!dlink.link_unresolved);
        assert_eq!(dlink.resolved_path.as_deref(), Some(outside.as_path()));
        let children = dlink.children().expect("walked target directory");
        let child = children
            .iter()
            .find(|e| e.name == "c.txt")
            .expect("child of the target directory");
        assert_eq!(child.path, "dlink/c.txt");
        // The derived size of the walked link is the size of its content.
        assert_eq!(dlink.meta.size, 7);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_directory_target_is_not_walked_twice() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path();
        fs_err::write(root.join("a.txt"), "alpha").unwrap();
        // A link to its own directory; the target is the scan root, which
        // the walk has already visited.
        symlink(Path::new("."), &root.join("self"));

        let fs = LocalFs::new("test");
        let result = scan_directory_node(
            &fs,
            root,
            &ScanConfig {
                resolve_links: true,
            },
            None,
        )
        .await
        .unwrap();

        let link = find_entry(&result.entries, "self").expect("link entry");
        assert!(link.meta.is_symlink);
        assert!(link.is_dir());
        assert!(!link.link_unresolved);
        assert_eq!(link.resolved_path.as_deref(), Some(root));
        // The cycle stop keeps the link without a subtree.
        assert!(link.children().is_none_or(|c| c.is_empty()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_broken_link_is_recorded_as_unresolvable() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path();
        symlink(Path::new("missing.txt"), &root.join("dangling.txt"));

        let fs = LocalFs::new("test");
        let result = scan_directory_node(
            &fs,
            root,
            &ScanConfig {
                resolve_links: true,
            },
            None,
        )
        .await
        .unwrap();

        let link = find_entry(&result.entries, "dangling.txt")
            .expect("broken link entry");
        assert!(link.meta.is_symlink);
        assert!(link.link_unresolved);
        assert_eq!(link.link_target.as_deref(), Some("missing.txt"));
        assert!(link.resolved_path.is_none());
        // Content stays addressed by the link's own path.
        assert_eq!(link.content_path(root), root.join("dangling.txt"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_chain_without_final_target_is_unresolvable() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path();
        // A chain that never settles on a non-link node, so resolution
        // runs until the hop budget is exhausted.
        symlink(Path::new("b"), &root.join("a"));
        symlink(Path::new("a"), &root.join("b"));

        let fs = LocalFs::new("test");
        let result = scan_directory_node(
            &fs,
            root,
            &ScanConfig {
                resolve_links: true,
            },
            None,
        )
        .await
        .unwrap();

        for name in ["a", "b"] {
            let link = find_entry(&result.entries, name).expect("link entry");
            assert!(link.meta.is_symlink);
            assert!(link.link_unresolved, "{name} must stay unresolved");
            assert!(link.resolved_path.is_none());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_chain_needs_a_final_target_within_the_budget() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path();
        fs_err::write(root.join("target.txt"), "alpha").unwrap();
        // A chain of `MAX_SYMLINK_HOPS` links still settles on the
        // target, one link more does not.
        let chain_len = MAX_SYMLINK_HOPS as usize;
        let link_name = |i: usize| format!("l{i}");
        let mut target = PathBuf::from("target.txt");
        for i in (1..=chain_len).rev() {
            symlink(&target, &root.join(link_name(i)));
            target = PathBuf::from(link_name(i));
        }

        let fs = LocalFs::new("test");
        let result = scan_directory_node(
            &fs,
            root,
            &ScanConfig {
                resolve_links: true,
            },
            None,
        )
        .await
        .unwrap();
        let link = find_entry(&result.entries, "l1").expect("link entry");
        assert!(!link.link_unresolved);
        assert_eq!(link.meta.size, 5);

        // One more link pushes the final target out of the budget.
        symlink(Path::new("l1"), &root.join("l0"));
        let fs2 = LocalFs::new("test2");
        let result = scan_directory_node(
            &fs2,
            root,
            &ScanConfig {
                resolve_links: true,
            },
            None,
        )
        .await
        .unwrap();
        let link = find_entry(&result.entries, "l0").expect("link entry");
        assert!(link.link_unresolved);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn without_resolve_links_a_link_is_scanned_as_a_link() {
        let base = tempfile::TempDir::new().unwrap();
        let root = base.path();
        fs_err::create_dir(root.join("sub")).unwrap();
        fs_err::write(root.join("sub/b.txt"), "bravo").unwrap();
        symlink(Path::new("sub"), &root.join("dlink"));
        symlink(Path::new("nowhere"), &root.join("dangling"));

        let fs = LocalFs::new("test");
        let result =
            scan_directory_node(&fs, root, &ScanConfig::default(), None)
                .await
                .unwrap();

        let dlink = find_entry(&result.entries, "dlink").expect("link entry");
        // The link is scanned as the link node itself: neither resolved
        // nor walked.
        assert!(dlink.meta.is_symlink);
        assert!(!dlink.is_dir());
        assert!(!dlink.link_unresolved);
        assert_eq!(dlink.link_target.as_deref(), Some("sub"));
        assert!(dlink.resolved_path.is_none());
        assert!(dlink.children().is_none_or(|c| c.is_empty()));

        // A broken link stays a plain entry; nothing was resolved, so
        // nothing failed either.
        let dangling =
            find_entry(&result.entries, "dangling").expect("link entry");
        assert!(dangling.meta.is_symlink);
        assert!(!dangling.link_unresolved);
    }
}
