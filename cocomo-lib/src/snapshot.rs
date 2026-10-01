// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Point-in-time snapshots of directory trees.
//!
//! A snapshot captures the state of a directory tree at a specific moment:
//! paths, sizes, modification times, and content hashes. Snapshots are
//! compared against live filesystems or other snapshots, avoiding
//! re-reading files when the metadata has not changed.
//!
//! # Provider identification
//!
//! Each snapshot stores a [`ProviderId`] that references the provider it was
//! captured from. The provider ID is a serializable reference (scheme +
//! optional profile name) that can be resolved back to a live
//! [`NodeFileSystem`](crate::fs::NodeFileSystem) via a provider registry.
//!
//! # Path-based entries
//!
//! [`SnapshotEntry`] stores paths because they are serializable and portable.
//! When comparing a snapshot against a live filesystem, the paths are resolved
//! to [`NodeId`] values first via
//! [`NodeFileSystem::resolve_path`](crate::fs::NodeFileSystem::resolve_path).

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Result,
    fs::{FileSystem, NodeFileSystem},
    node::Node,
    scan::{
        ScanConfig, ScanEntry, ScanResult, scan_directory, scan_directory_node,
    },
};

// ---------------------------------------------------------------------------
// Provider identifier
// ---------------------------------------------------------------------------

/// A serializable identifier that references a provider.
///
/// Resolved lazily back to a live
/// [`NodeFileSystem`](crate::fs::NodeFileSystem) when a snapshot is loaded for
/// comparison. Uses `(scheme, profile)` because provider instances are
/// typically identified by their type and a named profile (e.g., `"s3" +
/// "prod-bucket"`).
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ProviderId {
    /// Provider scheme (e.g., `"file"`, `"s3"`, `"ftp"`).
    pub scheme: String,
    /// Optional named profile. `None` means the default provider of this
    /// scheme.
    #[serde(default)]
    pub profile: Option<String>,
}

impl ProviderId {
    /// Create a new provider identifier.
    pub fn new(scheme: impl Into<String>, profile: Option<String>) -> Self {
        Self {
            scheme: scheme.into(),
            profile,
        }
    }

    /// Create a local filesystem provider ID.
    pub fn local() -> Self {
        Self {
            scheme: "file".to_string(),
            profile: None,
        }
    }

    /// Return a human-readable label for this provider ID.
    pub fn label(&self) -> String {
        match &self.profile {
            Some(p) => format!("{}/{}", self.scheme, p),
            None => self.scheme.clone(),
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ---------------------------------------------------------------------------
// Snapshot entry
// ---------------------------------------------------------------------------

/// A single entry in a snapshot.
///
/// Stores the path and metadata of a file or directory at the time the
/// snapshot was captured. The `hash` field is the hex-encoded blake3
/// content hash for files; it is empty for directories.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SnapshotEntry {
    /// Relative path within the snapshot root.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// Last modification time.
    #[serde(with = "chrono::serde::ts_nanoseconds")]
    pub modified: DateTime<Utc>,
    /// Blake3 content hash (hex-encoded). Empty for directories.
    #[serde(default)]
    pub hash: String,
    /// `true` if this entry is a directory.
    pub is_dir: bool,
}

impl SnapshotEntry {
    /// Create a new file snapshot entry.
    pub fn file(
        path: PathBuf,
        size: u64,
        modified: DateTime<Utc>,
        hash: String,
    ) -> Self {
        Self {
            path,
            size,
            modified,
            hash,
            is_dir: false,
        }
    }

    /// Create a new directory snapshot entry.
    pub fn dir(path: PathBuf, modified: DateTime<Utc>) -> Self {
        Self {
            path,
            size: 0,
            modified,
            hash: String::new(),
            is_dir: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// A point-in-time capture of a directory tree.
///
/// Contains the provider that the snapshot belongs to, the root path,
/// all entries (files and directories), and metadata about the capture.
/// Snapshots are serializable and can be stored on disk for later
/// comparison against live filesystems or other snapshots.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Snapshot {
    /// Unique snapshot identifier.
    pub id: Uuid,
    /// Provider this snapshot was captured from.
    pub provider_id: ProviderId,
    /// Root path of the captured directory.
    pub path: PathBuf,
    /// All entries in the snapshot (files and directories).
    pub entries: Vec<SnapshotEntry>,
    /// When the snapshot was created.
    #[serde(with = "chrono::serde::ts_nanoseconds")]
    pub created_at: DateTime<Utc>,
    /// Human-readable labels/tags for this snapshot.
    #[serde(default)]
    pub labels: Vec<String>,
}

impl Snapshot {
    /// Create a new empty snapshot.
    pub fn new(provider_id: ProviderId, path: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4(),
            provider_id,
            path,
            entries: Vec::new(),
            created_at: Utc::now(),
            labels: Vec::new(),
        }
    }

    /// Create a new empty snapshot with a specific ID.
    pub fn with_id(id: Uuid, provider_id: ProviderId, path: PathBuf) -> Self {
        Self {
            id,
            provider_id,
            path,
            entries: Vec::new(),
            created_at: Utc::now(),
            labels: Vec::new(),
        }
    }

    /// Add an entry to the snapshot.
    pub fn add_entry(&mut self, entry: SnapshotEntry) {
        self.entries.push(entry);
    }

    /// Return all file entries (non-directory).
    pub fn file_entries(&self) -> impl Iterator<Item = &SnapshotEntry> {
        self.entries.iter().filter(|e| !e.is_dir)
    }

    /// Return all directory entries.
    pub fn dir_entries(&self) -> impl Iterator<Item = &SnapshotEntry> {
        self.entries.iter().filter(|e| e.is_dir)
    }

    /// Find an entry by relative path.
    pub fn get_entry(&self, path: &Path) -> Option<&SnapshotEntry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// Return the number of entries in this snapshot.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Serialize this snapshot to a TOML string.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| crate::error::FsError::Io {
            operation: crate::error::FsOperation::Write,
            path: PathBuf::new(),
            message: format!("failed to serialize snapshot: {e}"),
        })
    }

    /// Deserialize a snapshot from a TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self> {
        toml::from_str(toml_str).map_err(|e| crate::error::FsError::Io {
            operation: crate::error::FsOperation::Read,
            path: PathBuf::new(),
            message: format!("failed to parse snapshot: {e}"),
        })
    }

    /// Save this snapshot to a file.
    pub async fn save_to_file(&self, path: &Path) -> Result<()> {
        let content = self.to_toml()?;
        fs_err::tokio::write(path, content).await.map_err(|e| {
            crate::error::FsError::Io {
                operation: crate::error::FsOperation::Write,
                path: path.to_path_buf(),
                message: format!("failed to write snapshot file: {e}"),
            }
        })
    }

    /// Load a snapshot from a file.
    pub async fn load_from_file(path: &Path) -> Result<Self> {
        let content =
            fs_err::tokio::read_to_string(path).await.map_err(|e| {
                crate::error::FsError::Io {
                    operation: crate::error::FsOperation::Read,
                    path: path.to_path_buf(),
                    message: format!("failed to read snapshot file: {e}"),
                }
            })?;
        Self::from_toml(&content)
    }
}

// ---------------------------------------------------------------------------
// Snapshot capture
// ---------------------------------------------------------------------------

/// Capture a snapshot of a directory tree from a live filesystem.
///
/// Scans the directory tree using the path-based [`FileSystem`] API. Hash
/// computation is deferred: the returned snapshot entries have empty hashes.
/// Callers that need hashes should compute them on demand when comparing
/// against a live filesystem.
#[deprecated(note = "use `capture_snapshot_node` instead")]
pub async fn capture_snapshot(
    fs: &Arc<dyn FileSystem>,
    provider_id: ProviderId,
    root_path: &Path,
) -> Result<Snapshot> {
    let mut snapshot = Snapshot::new(provider_id, root_path.to_path_buf());

    // Scan the directory tree.
    let scan_result =
        scan_directory(fs, root_path, &ScanConfig::default()).await?;

    // Populate snapshot entries from the scan result.
    collect_entries_from_scan(root_path, &scan_result, &mut snapshot);

    Ok(snapshot)
}

/// Capture a snapshot of a directory tree from a live node-based
/// filesystem.
///
/// Scans the directory tree using the node-based [`NodeFileSystem`]
/// API, so it works with any provider implementing that trait, including
/// [`Provider`](crate::provider::Provider), without going through a `dyn
/// FileSystem` trait object. Hash computation is deferred like in
/// [`capture_snapshot`]: the returned snapshot entries have empty hashes.
pub async fn capture_snapshot_node<N>(
    fs: &N,
    provider_id: ProviderId,
    root_path: &Path,
) -> Result<Snapshot>
where
    N: NodeFileSystem<Nid = u64>,
{
    let mut snapshot = Snapshot::new(provider_id, root_path.to_path_buf());

    // Scan the directory tree. The node-based scan yields root-relative
    // entry paths, which `collect_entries_from_scan` passes through
    // because `strip_prefix` fails on an already-relative path.
    let scan_result =
        scan_directory_node(fs, root_path, &ScanConfig::default()).await?;

    // Populate snapshot entries from the scan result.
    collect_entries_from_scan(root_path, &scan_result, &mut snapshot);

    Ok(snapshot)
}

/// Recursively collect entries from a scan tree into a snapshot.
fn collect_entries_from_scan(
    root_path: &Path,
    result: &ScanResult,
    snapshot: &mut Snapshot,
) {
    for entry in &result.entries {
        collect_scan_entry(root_path, entry, snapshot);
    }
}

/// Convert a single scan entry and its children to snapshot entries.
fn collect_scan_entry(
    root_path: &Path,
    entry: &ScanEntry,
    snapshot: &mut Snapshot,
) {
    // Build the relative path from the entry's full path.
    let entry_path = Path::new(&entry.path);
    let rel_path = entry_path
        .strip_prefix(root_path)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| entry_path.to_path_buf());

    if entry.is_dir() {
        snapshot.add_entry(SnapshotEntry::dir(
            rel_path.clone(),
            entry.meta.modified,
        ));

        // Recurse into children.
        if let Some(children) = entry.children() {
            for child in children {
                collect_scan_entry(root_path, child, snapshot);
            }
        }
    } else {
        snapshot.add_entry(SnapshotEntry::file(
            rel_path,
            entry.meta.size,
            entry.meta.modified,
            String::new(), // Hash is empty; computed on demand.
        ));
    }
}

// ---------------------------------------------------------------------------
// Snapshot comparison helpers
// ---------------------------------------------------------------------------

/// Compare a snapshot entry against a live node.
///
/// Returns `true` if the content is unchanged (same size, same hash).
/// Used to quickly determine which files need re-hashing during a
/// comparison.
pub fn entry_matches_node(entry: &SnapshotEntry, node: &Node) -> bool {
    if entry.is_dir != node.kind().is_directory() {
        return false;
    }

    if entry.size != node.metadata().size {
        return false;
    }

    // For files, compare hash if available.
    if !entry.is_dir && !entry.hash.is_empty() {
        if let Some(node_hash) = node.cached_hash() {
            return entry.hash == node_hash;
        }
        return false;
    }

    entry.modified == node.metadata().modified
}

/// Result of comparing a snapshot entry against a live filesystem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotEntryStatus {
    /// Entry is unchanged since the snapshot.
    Unchanged,
    /// Entry was modified (size, hash, or mtime differs).
    Modified,
    /// Entry is new (exists in live FS but not in snapshot).
    Added,
    /// Entry was deleted (exists in snapshot but not in live FS).
    Deleted,
}

// ---------------------------------------------------------------------------
// Snapshot manager
// ---------------------------------------------------------------------------

/// Manages a collection of snapshots.
pub struct SnapshotManager {
    /// Directory where snapshot files are stored.
    snapshot_dir: PathBuf,
    /// Currently loaded snapshots.
    snapshots: Vec<Snapshot>,
}

impl SnapshotManager {
    /// Create a new snapshot manager.
    pub fn new(snapshot_dir: PathBuf) -> Self {
        Self {
            snapshot_dir,
            snapshots: Vec::new(),
        }
    }

    /// Add a snapshot to the manager.
    pub fn add_snapshot(&mut self, snapshot: Snapshot) {
        self.snapshots.push(snapshot);
    }

    /// Return all managed snapshots.
    pub fn snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }

    /// Find a snapshot by ID.
    pub fn get_snapshot(&self, id: &Uuid) -> Option<&Snapshot> {
        self.snapshots.iter().find(|s| s.id == *id)
    }

    /// Remove a snapshot by ID. Returns `true` if it existed.
    pub fn remove_snapshot(&mut self, id: &Uuid) -> bool {
        let before = self.snapshots.len();
        self.snapshots.retain(|s| s.id != *id);
        self.snapshots.len() < before
    }

    /// Save a snapshot to the snapshot directory.
    pub async fn save_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let path = self.snapshot_dir.join(format!("{}.snap", snapshot.id));
        snapshot.save_to_file(&path).await
    }

    /// List all snapshot files.
    pub async fn list_snapshot_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        if !self.snapshot_dir.exists() {
            return Ok(files);
        }

        let mut entries = fs_err::tokio::read_dir(&self.snapshot_dir)
            .await
            .map_err(|e| crate::error::FsError::Io {
                operation: crate::error::FsOperation::ReadDir,
                path: self.snapshot_dir.clone(),
                message: format!("failed to read snapshot directory: {e}"),
            })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            crate::error::FsError::Io {
                operation: crate::error::FsOperation::ReadDir,
                path: self.snapshot_dir.clone(),
                message: format!("failed to read directory entry: {e}"),
            }
        })? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("snap") {
                files.push(path);
            }
        }

        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{local::LocalFs, mockfs::MockFs};

    #[test]
    fn provider_id_new() {
        let id = ProviderId::new("s3", Some("prod".to_string()));
        assert_eq!(id.scheme, "s3");
        assert_eq!(id.profile, Some("prod".to_string()));
    }

    #[test]
    fn provider_id_local() {
        let id = ProviderId::local();
        assert_eq!(id.scheme, "file");
        assert!(id.profile.is_none());
    }

    #[test]
    fn provider_id_label_with_profile() {
        let id = ProviderId::new("s3", Some("prod".to_string()));
        assert_eq!(id.label(), "s3/prod");
    }

    #[test]
    fn provider_id_label_without_profile() {
        let id = ProviderId::local();
        assert_eq!(id.label(), "file");
    }

    #[test]
    fn provider_id_display() {
        let id = ProviderId::new("ftp", Some("backup".to_string()));
        assert_eq!(format!("{id}"), "ftp/backup");
    }

    #[test]
    fn snapshot_entry_file() {
        let entry = SnapshotEntry::file(
            PathBuf::from("src/main.rs"),
            1024,
            Utc::now(),
            "abc123".to_string(),
        );
        assert!(!entry.is_dir);
        assert_eq!(entry.size, 1024);
        assert_eq!(entry.hash, "abc123");
    }

    #[test]
    fn snapshot_entry_dir() {
        let entry = SnapshotEntry::dir(PathBuf::from("src"), Utc::now());
        assert!(entry.is_dir);
        assert_eq!(entry.size, 0);
        assert!(entry.hash.is_empty());
    }

    #[test]
    fn snapshot_create_and_add_entries() {
        let mut snap =
            Snapshot::new(ProviderId::local(), PathBuf::from("/home/project"));
        snap.add_entry(SnapshotEntry::file(
            PathBuf::from("main.rs"),
            100,
            Utc::now(),
            "hash1".to_string(),
        ));
        snap.add_entry(SnapshotEntry::dir(PathBuf::from("src"), Utc::now()));

        assert_eq!(snap.entry_count(), 2);
        assert_eq!(snap.file_entries().count(), 1);
        assert_eq!(snap.dir_entries().count(), 1);
    }

    #[test]
    fn snapshot_get_entry() {
        let mut snap =
            Snapshot::new(ProviderId::local(), PathBuf::from("/root"));
        let entry = SnapshotEntry::file(
            PathBuf::from("file.txt"),
            50,
            Utc::now(),
            "h".to_string(),
        );
        snap.add_entry(entry);

        assert!(snap.get_entry(Path::new("file.txt")).is_some());
        assert!(snap.get_entry(Path::new("other.txt")).is_none());
    }

    #[test]
    fn snapshot_roundtrip_toml() {
        let now = Utc::now();
        let snap = Snapshot {
            id: Uuid::new_v4(),
            provider_id: ProviderId::new("s3", Some("bucket".to_string())),
            path: PathBuf::from("/data"),
            entries: vec![
                SnapshotEntry::dir(PathBuf::from("sub"), now),
                SnapshotEntry::file(
                    PathBuf::from("sub/file.txt"),
                    100,
                    now,
                    "abcdef".to_string(),
                ),
            ],
            created_at: now,
            labels: vec!["baseline".to_string()],
        };

        let toml_str = snap.to_toml().unwrap();
        let loaded = Snapshot::from_toml(&toml_str).unwrap();
        assert_eq!(snap, loaded);
    }

    #[test]
    fn snapshot_entry_matches_node_same() {
        use std::ffi::OsString;

        use crate::{meta::Metadata, node::Node};

        let meta = Metadata::file(100, Utc::now());
        let mut node = Node::file(
            OsString::from("test.txt"),
            PathBuf::from("/test.txt"),
            meta,
        );
        node.set_cached_hash("abc123".to_string());

        let entry = SnapshotEntry::file(
            PathBuf::from("test.txt"),
            100,
            node.metadata().modified,
            "abc123".to_string(),
        );

        assert!(entry_matches_node(&entry, &node));
    }

    #[test]
    fn snapshot_entry_matches_node_different_size() {
        use std::ffi::OsString;

        use crate::{meta::Metadata, node::Node};

        let meta = Metadata::file(200, Utc::now());
        let node = Node::file(
            OsString::from("test.txt"),
            PathBuf::from("/test.txt"),
            meta,
        );

        let entry = SnapshotEntry::file(
            PathBuf::from("test.txt"),
            100,
            node.metadata().modified,
            "abc123".to_string(),
        );

        assert!(!entry_matches_node(&entry, &node));
    }

    #[test]
    fn snapshot_entry_matches_node_different_type() {
        use std::ffi::OsString;

        use crate::{meta::Metadata, node::Node};

        let meta = Metadata::dir(Utc::now());
        let node = Node::directory(
            OsString::from("dir"),
            PathBuf::from("/dir"),
            meta,
        );

        let entry = SnapshotEntry::file(
            PathBuf::from("dir"),
            0,
            node.metadata().modified,
            String::new(),
        );

        assert!(!entry_matches_node(&entry, &node));
    }

    #[test]
    fn snapshot_manager_add_and_get() {
        let mut mgr = SnapshotManager::new(PathBuf::from("/tmp/snapshots"));
        let snap = Snapshot::new(ProviderId::local(), PathBuf::from("/data"));
        let id = snap.id;

        mgr.add_snapshot(snap);
        assert_eq!(mgr.snapshots().len(), 1);
        assert!(mgr.get_snapshot(&id).is_some());
    }

    #[test]
    fn snapshot_manager_remove() {
        let mut mgr = SnapshotManager::new(PathBuf::from("/tmp/snapshots"));
        let snap = Snapshot::new(ProviderId::local(), PathBuf::from("/data"));
        let id = snap.id;

        mgr.add_snapshot(snap);
        assert!(mgr.remove_snapshot(&id));
        assert!(!mgr.remove_snapshot(&id));
        assert_eq!(mgr.snapshots().len(), 0);
    }

    #[allow(deprecated)]
    #[tokio::test]
    async fn capture_snapshot_node_matches_path_based_capture() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("tree");
        fs_err::create_dir_all(root.join("sub")).unwrap();
        fs_err::write(root.join("a.txt"), b"alpha").unwrap();
        fs_err::write(root.join("sub").join("b.txt"), b"bravo").unwrap();

        let fs: Arc<dyn FileSystem> = Arc::new(LocalFs::new("test"));
        let by_path = capture_snapshot(&fs, ProviderId::local(), &root)
            .await
            .unwrap();
        let fs_node = LocalFs::new("test");
        let by_node =
            capture_snapshot_node(&fs_node, ProviderId::local(), &root)
                .await
                .unwrap();

        assert_eq!(by_path.entry_count(), by_node.entry_count());
        for entry in &by_node.entries {
            let other = by_path.get_entry(&entry.path).unwrap_or_else(|| {
                panic!("path-based snapshot lacks {:?}", entry.path)
            });
            assert_eq!(other.is_dir, entry.is_dir);
            assert_eq!(other.size, entry.size);
        }
    }

    #[tokio::test]
    async fn capture_snapshot_node_on_mock_fs() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_file("/root/a.txt", "alpha")
            .with_dir("/root/sub")
            .with_file("/root/sub/b.txt", "bravo");

        let snapshot = capture_snapshot_node(
            &fs,
            ProviderId::new("ftp", None),
            Path::new("/root"),
        )
        .await
        .unwrap();

        assert_eq!(snapshot.provider_id, ProviderId::new("ftp", None));
        assert_eq!(snapshot.entry_count(), 3);
        for rel in ["a.txt", "sub", "sub/b.txt"] {
            let entry = snapshot
                .get_entry(Path::new(rel))
                .unwrap_or_else(|| panic!("snapshot lacks {rel:?}"));
            assert_eq!(entry.is_dir, rel == "sub");
        }
    }
}
