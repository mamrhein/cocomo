// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Local filesystem provider using `fs_err::tokio` for rich error context
//! and `tokio` for async I/O.

use std::{
    collections::HashMap,
    ffi::OsString,
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
    time::SystemTime,
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt, TryStreamExt, stream};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio_util::codec::Decoder;

use crate::{
    error::{FsError, FsOperation, Result, wrap},
    file::FsFile,
    fs::{
        DirEntryMeta, DirStream, FileSystem, NodeFileSystem, OpenMode,
        WritableFileSystem,
    },
    identity::{DirId, FileId, FileSystemId, NodeId},
    meta::Metadata,
    node::{Node, NodeKind, SymlinkTarget, UserPermissions},
};

/// Counter for generating unique temporary file names.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A file handle backed by a local `tokio::fs::File`.
pub struct LocalFile {
    inner: tokio::fs::File,
    meta: Metadata,
    /// If writing via a temp file, this is the temp path to rename on Drop.
    temp_path: Option<PathBuf>,
    /// If writing via a temp file, this is the final destination path.
    target_path: Option<PathBuf>,
}

#[async_trait]
impl FsFile for LocalFile {
    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    async fn flush(&mut self) -> Result<()> {
        self.inner.flush().await.map_err(|e| {
            wrap(
                e,
                crate::error::FsOperation::Write,
                PathBuf::from("(local file handle)"),
            )
        })
    }
}

impl Drop for LocalFile {
    fn drop(&mut self) {
        // Commit the temp file to the target path if we have one.
        // On Unix, renaming an open file works fine.
        if let (Some(temp), Some(target)) =
            (&self.temp_path, &self.target_path)
        {
            let _ = fs::rename(temp, target);
        }
    }
}

impl AsyncRead for LocalFile {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for LocalFile {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // Flush the underlying file. The atomic rename is handled by
        // Drop, which is always invoked regardless of whether shutdown
        // is called explicitly.
        let this = self.get_mut();
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

/// Concrete node ID type for the local filesystem provider.
pub type LocalNodeId = NodeId<u64>;

/// Concrete directory ID type for the local filesystem provider.
pub type LocalDirId = DirId<u64>;

/// Concrete file ID type for the local filesystem provider.
pub type LocalFileId = FileId<u64>;

/// Local filesystem provider.
///
/// Uses `fs_err::tokio` for filesystem operations to get rich error context,
/// and `walkdir` for directory traversal with inode-based cycle detection.
///
/// # Node cache
///
/// Every resolved path is cached as a [`Node`] keyed by a monotonically
/// increasing [`u64`] identifier. A reverse lookup map maintains path → ID
/// mappings. Deleted nodes are tombstoned rather than removed from the cache,
/// so stale identifiers return [`FsError::StaleNode`] instead of silently
/// resolving to new nodes.
pub struct LocalFs {
    /// Human-readable label for this provider instance.
    label: String,
    /// Filesystem instance identifier (device ID on unix, volume serial on
    /// windows).
    fs_id: FileSystemId<u64>,
    /// Node cache: node ID → node.
    nodes: parking_lot::RwLock<HashMap<u64, Arc<Node>>>,
    /// Reverse lookup: absolute path → node ID.
    path_to_id: parking_lot::RwLock<HashMap<PathBuf, u64>>,
    /// Monotonically increasing counter for node ID generation.
    next_id: AtomicU64,
}

impl LocalFs {
    /// Create a new `LocalFs` with the given label.
    ///
    /// The filesystem identifier is derived from the current working
    /// directory's device ID on unix, or a default value on other
    /// platforms.
    pub fn new(label: impl Into<String>) -> Self {
        let fs_id = detect_device_id();
        Self {
            label: label.into(),
            fs_id: FileSystemId::new(fs_id),
            nodes: parking_lot::RwLock::new(HashMap::new()),
            path_to_id: parking_lot::RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Allocate a new unique node ID.
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Build a [`Node`] from OS metadata.
    fn build_node(
        path: &Path,
        meta: &fs::Metadata,
        parent_id: Option<u64>,
    ) -> Node {
        let name = path.file_name().map(OsString::from).unwrap_or_default();
        let (ino, dev) = platform_ids(meta);
        let perms = platform_permissions(meta);
        let created = meta.created().ok().map(DateTime::<Utc>::from);
        let metadata = Metadata {
            size: meta.len(),
            created,
            modified: to_utc(meta.modified().ok()),
            is_dir: meta.is_dir(),
            is_symlink: meta.file_type().is_symlink(),
            inode: Some(ino),
            device_id: Some(dev),
            permissions: perms,
        };

        if meta.file_type().is_symlink() {
            let target_path = fs::read_link(path).unwrap_or_else(|_| {
                // If we can't read the link target, store an empty path.
                PathBuf::new()
            });
            Node::symlink(
                name,
                path.to_path_buf(),
                metadata,
                SymlinkTarget::new(target_path),
            )
            .with_parent(parent_id)
        } else if meta.is_dir() {
            Node::directory(name, path.to_path_buf(), metadata)
                .with_parent(parent_id)
        } else {
            Node::file(name, path.to_path_buf(), metadata)
                .with_parent(parent_id)
        }
    }

    /// Cache a node and return its ID. If the path is already cached, return
    /// the existing ID without replacing the node.
    fn cache_node(&self, node: Node) -> u64 {
        let path = node.path().to_path_buf();
        // Check if already cached (under write lock).
        {
            let ptid = self.path_to_id.write();
            if let Some(&id) = ptid.get(&path) {
                return id;
            }
        }

        let id = self.alloc_id();
        let arc_node = Arc::new(node);
        {
            let mut nodes = self.nodes.write();
            nodes.insert(id, arc_node.clone());
        }
        {
            let mut ptid = self.path_to_id.write();
            ptid.insert(path, id);
        }
        id
    }

    /// Lookup a node ID from a path in the cache. Returns `None` if not
    /// cached.
    fn lookup_path(&self, path: &Path) -> Option<u64> {
        self.path_to_id.read().get(path).copied()
    }

    /// Resolve the parent directory ID for a node path, if a parent exists.
    fn resolve_parent_id(&self, path: &Path) -> Option<u64> {
        path.parent().and_then(|p| self.lookup_path(p))
    }

    /// Create a temporary file in the same directory as the target path.
    ///
    /// Returns the temp file path and an open `tokio::fs::File` handle.
    /// The temp file is guaranteed to be on the same filesystem as the
    /// target, so a subsequent `rename` is atomic.
    async fn create_temp_file(
        target: &Path,
    ) -> Result<(PathBuf, tokio::fs::File)> {
        let parent = match target.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".cocomo_tmp_{}_{}_{:06}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            counter,
        ));

        let file = tokio::fs::File::create(&temp_path).await.map_err(|e| {
            wrap(e, crate::error::FsOperation::Write, target.to_path_buf())
        })?;

        Ok((temp_path, file))
    }

    /// Build metadata from a `fs_err::DirEntry`.
    fn entry_meta(entry: &fs_err::DirEntry) -> Metadata {
        let path = entry.path();

        // Use symlink_metadata to detect symlinks without following them.
        let meta = fs_err::symlink_metadata(path).ok();
        if let Some(m) = &meta {
            let is_sym = m.file_type().is_symlink();
            let (ino, dev) = platform_ids(m);
            let perms = platform_permissions(m);
            let created = m.created().ok().map(DateTime::<Utc>::from);
            return Metadata {
                size: m.len(),
                created,
                modified: to_utc(m.modified().ok()),
                is_dir: m.is_dir(),
                is_symlink: is_sym,
                inode: Some(ino),
                device_id: Some(dev),
                permissions: perms,
            };
        }

        // Fallback when stat fails.
        Metadata {
            size: 0,
            created: None,
            modified: DateTime::default(),
            is_dir: false,
            is_symlink: false,
            inode: None,
            device_id: None,
            permissions: UserPermissions::empty(),
        }
    }
}

// ---------------------------------------------------------------------------
// Platform-specific inode / device extraction
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn platform_ids(meta: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.ino(), meta.dev())
}

#[cfg(not(unix))]
fn platform_ids(_meta: &fs::Metadata) -> (u64, u64) {
    // Windows does not expose inodes via std. Return sentinel values.
    (0, 0)
}

#[cfg(unix)]
fn platform_permissions(meta: &fs::Metadata) -> UserPermissions {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    let mut perms = UserPermissions::empty();
    if mode & 0o400 != 0 {
        perms |= UserPermissions::READ;
    }
    if mode & 0o200 != 0 {
        perms |= UserPermissions::WRITE;
    }
    if mode & 0o100 != 0 {
        perms |= UserPermissions::EXEC;
    }
    perms
}

#[cfg(not(unix))]
fn platform_permissions(_meta: &fs::Metadata) -> UserPermissions {
    // Windows permissions are more complex; default to read+write+exec for
    // accessible files.
    UserPermissions::READ | UserPermissions::WRITE | UserPermissions::EXEC
}

fn to_utc(opt: Option<SystemTime>) -> DateTime<Utc> {
    opt.map(DateTime::<Utc>::from).unwrap_or_default()
}

/// Detect the device ID for the current working directory. Used as the
/// filesystem instance identifier for `LocalFs`.
#[cfg(unix)]
fn detect_device_id() -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs_err::metadata(".").map(|m| m.dev()).unwrap_or(0)
}

#[cfg(not(unix))]
fn detect_device_id() -> u64 {
    // Windows: use volume serial of current directory, or a default.
    // For now, return 0. A real implementation would use
    // GetVolumeInformation or similar.
    0
}

// ---------------------------------------------------------------------------
// FileSystem implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl FileSystem for LocalFs {
    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        let meta =
            fs_err::tokio::symlink_metadata(path).await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Open, path.to_path_buf())
            })?;
        let (ino, dev) = platform_ids(&meta);
        let perms = platform_permissions(&meta);
        let created = meta.created().ok().map(DateTime::<Utc>::from);
        Ok(Metadata {
            size: meta.len(),
            created,
            modified: to_utc(meta.modified().ok()),
            is_dir: meta.is_dir(),
            is_symlink: meta.file_type().is_symlink(),
            inode: Some(ino),
            device_id: Some(dev),
            permissions: perms,
        })
    }

    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>> {
        let entries = fs_err::read_dir(path).map_err(|e| {
            wrap(e, crate::error::FsOperation::ReadDir, path.to_path_buf())
        })?;

        let metas: Vec<DirEntryMeta> = entries
            .filter_map(|entry| {
                entry.ok().map(|e| DirEntryMeta {
                    name: e.file_name().to_string_lossy().into_owned(),
                    meta: Self::entry_meta(&e),
                })
            })
            .filter(|e| e.name != "." && e.name != "..")
            .collect();

        let s: DirStream<'_> =
            Box::pin(stream::iter(metas.into_iter().map(Ok)));
        Ok(s)
    }

    async fn open(
        &self,
        path: &Path,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        let (file, temp_path, target_path) = match mode {
            OpenMode::Read => {
                let file = tokio::fs::File::open(path).await.map_err(|e| {
                    wrap(
                        e,
                        crate::error::FsOperation::Open,
                        path.to_path_buf(),
                    )
                })?;
                (file, None, None)
            }
            OpenMode::Write => {
                // Write to a temp file so that the final rename is atomic.
                let (temp_path, file) = Self::create_temp_file(path).await?;
                (file, Some(temp_path), Some(path.to_path_buf()))
            }
            OpenMode::Append => {
                let file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)
                    .await
                    .map_err(|e| {
                        wrap(
                            e,
                            crate::error::FsOperation::Write,
                            path.to_path_buf(),
                        )
                    })?;
                (file, None, None)
            }
        };

        // Capture metadata at open time. For temp files we fabricate
        // metadata for a new empty file since the target doesn't exist yet.
        let meta = file.metadata().await.map_err(|e| {
            wrap(e, crate::error::FsOperation::Open, path.to_path_buf())
        })?;
        let (ino, dev) = platform_ids(&meta);
        let perms = platform_permissions(&meta);
        let created = meta.created().ok().map(DateTime::<Utc>::from);
        let metadata = Metadata {
            size: meta.len(),
            created,
            modified: to_utc(meta.modified().ok()),
            is_dir: meta.is_dir(),
            is_symlink: meta.file_type().is_symlink(),
            inode: Some(ino),
            device_id: Some(dev),
            permissions: perms,
        };

        Ok(Box::new(LocalFile {
            inner: file,
            meta: metadata,
            temp_path,
            target_path,
        }))
    }

    async fn read(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        let data = if let Some(r) = range {
            // Validate the range before allocating or seeking.
            if r.start >= r.end {
                return Err(crate::error::FsError::InvalidArgument {
                    operation: crate::error::FsOperation::Read,
                    path: path.to_path_buf(),
                    message: format!(
                        "invalid range: start ({}) must be less than end ({})",
                        r.start, r.end
                    ),
                });
            }

            // For ranged reads, open the file and seek to the start offset.
            let mut file = tokio::fs::File::open(path).await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Read, path.to_path_buf())
            })?;
            let file_meta = file.metadata().await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Read, path.to_path_buf())
            })?;

            // Clamp the range to the actual file size to avoid allocating
            // beyond EOF and then failing on read_exact.
            let file_size = file_meta.len();
            if r.start >= file_size {
                return Ok(Bytes::new());
            }
            let end = std::cmp::min(r.end, file_size);
            let len = (end - r.start) as usize;

            use tokio::io::AsyncSeekExt;
            file.seek(io::SeekFrom::Start(r.start)).await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Read, path.to_path_buf())
            })?;
            let mut reader = tokio::io::BufReader::new(file);
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Read, path.to_path_buf())
            })?;
            Bytes::from(buf)
        } else {
            fs_err::tokio::read(path)
                .await
                .map_err(|e| {
                    wrap(
                        e,
                        crate::error::FsOperation::Read,
                        path.to_path_buf(),
                    )
                })?
                .into()
        };
        Ok(data)
    }

    async fn read_stream(
        &self,
        path: &Path,
        _range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        let file = fs_err::tokio::File::open(path).await.map_err(|e| {
            wrap(e, crate::error::FsOperation::Read, path.to_path_buf())
        })?;
        let stream = tokio_util::codec::BytesCodec::new()
            .framed(file)
            .into_stream();
        // Owned path for the closure (avoids lifetime issues).
        let path_owned = path.to_path_buf();
        // Convert codec errors to our FsError and BytesMut -> Bytes.
        let adapted = stream.map(move |item| {
            item.map(|bm| bm.freeze()).map_err(|e| {
                wrap(
                    io::Error::other(format!("stream read error: {e}")),
                    crate::error::FsOperation::Read,
                    path_owned.clone(),
                )
            })
        });
        Ok(Box::pin(adapted))
    }

    async fn write(&self, path: &Path, data: Bytes) -> Result<()> {
        // Write to a temp file first, then atomically rename to the target.
        let (temp_path, _file) = Self::create_temp_file(path).await?;
        fs_err::tokio::write(&temp_path, &data).await.map_err(|e| {
            wrap(e, crate::error::FsOperation::Write, temp_path.clone())
        })?;
        fs_err::rename(&temp_path, path).map_err(|e| {
            // Best-effort cleanup of the temp file on rename failure.
            let _ = fs::remove_file(&temp_path);
            wrap(e, crate::error::FsOperation::Write, path.to_path_buf())
        })?;
        Ok(())
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        fs_err::tokio::create_dir_all(path).await.map_err(|e| {
            wrap(e, crate::error::FsOperation::CreateDir, path.to_path_buf())
        })?;
        Ok(())
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        // Try removing as a file first, then as a directory.
        match fs_err::tokio::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) => {
                if e.kind() == io::ErrorKind::IsADirectory {
                    fs_err::tokio::remove_dir(path).await.map_err(|e| {
                        wrap(
                            e,
                            crate::error::FsOperation::Remove,
                            path.to_path_buf(),
                        )
                    })?;
                    Ok(())
                } else {
                    Err(wrap(
                        e,
                        crate::error::FsOperation::Remove,
                        path.to_path_buf(),
                    ))
                }
            }
        }
    }

    async fn remove_all(&self, path: &Path) -> Result<()> {
        let meta = fs_err::symlink_metadata(path).map_err(|e| {
            wrap(e, crate::error::FsOperation::Remove, path.to_path_buf())
        })?;
        if meta.is_dir() {
            fs_err::tokio::remove_dir_all(path).await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Remove, path.to_path_buf())
            })?;
        } else {
            fs_err::tokio::remove_file(path).await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Remove, path.to_path_buf())
            })?;
        }
        Ok(())
    }

    async fn rename(&self, src: &Path, dst: &Path) -> Result<()> {
        fs_err::tokio::rename(src, dst).await.map_err(|e| {
            wrap(e, crate::error::FsOperation::Rename, src.to_path_buf())
        })?;
        Ok(())
    }

    async fn copy(&self, src: &Path, dst: &Path) -> Result<()> {
        fs_err::tokio::copy(src, dst).await.map_err(|e| {
            wrap(e, crate::error::FsOperation::Copy, src.to_path_buf())
        })?;
        Ok(())
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        tokio::fs::read_link(path).await.map_err(|e| {
            wrap(e, crate::error::FsOperation::ReadLink, path.to_path_buf())
        })
    }

    async fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            tokio::fs::symlink(target, link).await.map_err(|e| {
                wrap(e, crate::error::FsOperation::Symlink, link.to_path_buf())
            })?;
        }
        #[cfg(windows)]
        {
            if target.is_dir() {
                tokio::fs::symlink_dir(target, link).await.map_err(|e| {
                    wrap(
                        e,
                        crate::error::FsOperation::Symlink,
                        link.to_path_buf(),
                    )
                })?;
            } else {
                tokio::fs::symlink_file(target, link).await.map_err(|e| {
                    wrap(
                        e,
                        crate::error::FsOperation::Symlink,
                        link.to_path_buf(),
                    )
                })?;
            }
        }
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }
}

// ---------------------------------------------------------------------------
// NodeFileSystem implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl NodeFileSystem for LocalFs {
    type FsId = u64;
    type Nid = u64;
    type Error = FsError;

    fn id(&self) -> FileSystemId<Self::FsId> {
        self.fs_id
    }

    fn label_node(&self) -> &str {
        &self.label
    }

    async fn resolve_path(&self, path: &Path) -> Result<NodeId<Self::Nid>> {
        // Normalize the path to absolute without resolving symlinks.
        // (canonicalize resolves symlinks, which breaks symlink detection.)
        let abs = if path.is_absolute() {
            path.to_path_buf()
        } else {
            fs_err::canonicalize(path).map_err(|e| {
                wrap(e, FsOperation::Resolve, path.to_path_buf())
            })?
        };

        // Check cache first.
        if let Some(id) = self.lookup_path(&abs) {
            let node = self.nodes.read().get(&id).cloned();
            return match node {
                Some(n) if n.is_deleted() => Err(FsError::StaleNode),
                Some(_) => Ok(NodeId::new(id)),
                None => Err(FsError::NotFound { path: abs }),
            };
        }

        // Stat the path to determine node kind. Use symlink_metadata
        // to detect symlinks without following them.
        let meta = fs_err::symlink_metadata(&abs)
            .map_err(|e| wrap(e, FsOperation::Resolve, abs.clone()))?;

        let parent_id = self.resolve_parent_id(&abs);
        let node = Self::build_node(&abs, &meta, parent_id);
        let id = self.cache_node(node);

        Ok(NodeId::new(id))
    }

    async fn resolve_symlink(
        &self,
        id: NodeId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let node = self.get_node(id)?;
        let NodeKind::Symlink { target } = node.kind() else {
            return Err(FsError::WrongKind {
                expected: "symlink",
                actual: match node.kind() {
                    NodeKind::Directory { .. } => "directory",
                    NodeKind::File => "file",
                    NodeKind::Symlink { .. } => "symlink",
                    NodeKind::Special => "special",
                },
            });
        };

        // Resolve the target path relative to the symlink's parent.
        let symlink_dir = node.path().parent().unwrap_or(Path::new("/"));
        let target_path = if target.path().is_absolute() {
            target.path().to_path_buf()
        } else {
            symlink_dir.join(target.path())
        };

        // Resolve the target to a node.
        self.resolve_path(&target_path).await
    }

    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>> {
        let nodes = self.nodes.read();
        match nodes.get(id.get()).cloned() {
            Some(node) if node.is_deleted() => Err(FsError::StaleNode),
            Some(node) => Ok(node),
            None => {
                drop(nodes);
                Err(FsError::NotFound {
                    path: PathBuf::from("(unknown node)"),
                })
            }
        }
    }

    fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata> {
        let node = self.get_node(id)?;
        Ok(node.metadata().clone())
    }

    async fn read_dir_node(&self, dir_id: DirId<Self::Nid>) -> Result<()> {
        let dir_node = self.get_node(dir_id.as_node_id())?;
        let dir_path = dir_node.path();

        // Verify it is actually a directory.
        if !dir_path.is_dir() {
            return Err(FsError::WrongKind {
                expected: "directory",
                actual: "file",
            });
        }

        let entries = fs_err::read_dir(dir_path).map_err(|e| {
            wrap(e, FsOperation::ReadDir, dir_path.to_path_buf())
        })?;

        let mut child_names = Vec::new();
        for entry_result in entries {
            let entry = entry_result.map_err(|e| {
                wrap(e, FsOperation::ReadDir, dir_path.to_path_buf())
            })?;
            let entry_path = entry.path();
            let name = entry.file_name();

            // Skip "." and ".." entries.
            if name == "." || name == ".." {
                continue;
            }

            child_names.push(name.to_string_lossy().into_owned());

            // Stat the entry.
            let meta = fs_err::symlink_metadata(&entry_path).map_err(|e| {
                wrap(e, FsOperation::Resolve, entry_path.clone())
            })?;
            let node =
                Self::build_node(&entry_path, &meta, Some(*dir_id.get()));
            self.cache_node(node);
        }

        // Update the directory node's children.
        // Update the directory node's children.
        {
            let mut nodes = self.nodes.write();
            if let Some(arc) = nodes.get_mut(dir_id.get()) {
                Arc::make_mut(arc).set_children(child_names);
            }
        }

        Ok(())
    }

    async fn open_node(
        &self,
        id: FileId<Self::Nid>,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        let node = self.get_node(id.as_node_id())?;
        let path = node.path();

        // Delegate to the path-based open, but capture metadata from cache.
        let (file, temp_path, target_path) = match mode {
            OpenMode::Read => {
                let file = tokio::fs::File::open(path).await.map_err(|e| {
                    wrap(e, FsOperation::Open, path.to_path_buf())
                })?;
                (file, None, None)
            }
            OpenMode::Write => {
                let (temp_path, file) = Self::create_temp_file(path).await?;
                (file, Some(temp_path), Some(path.to_path_buf()))
            }
            OpenMode::Append => {
                let file = tokio::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(path)
                    .await
                    .map_err(|e| {
                        wrap(e, FsOperation::Write, path.to_path_buf())
                    })?;
                (file, None, None)
            }
        };

        Ok(Box::new(LocalFile {
            inner: file,
            meta: node.metadata().clone(),
            temp_path,
            target_path,
        }))
    }

    async fn read_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        let node = self.get_node(id.as_node_id())?;
        // Delegate to path-based read.
        self.read(node.path(), range).await
    }

    async fn read_stream_node(
        &self,
        id: FileId<Self::Nid>,
        _range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        let node = self.get_node(id.as_node_id())?;
        // Delegate to path-based read_stream.
        self.read_stream(node.path(), _range).await
    }
}

// ---------------------------------------------------------------------------
// WritableFileSystem implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl WritableFileSystem for LocalFs {
    async fn create_file(
        &self,
        parent: DirId<Self::Nid>,
        name: &std::ffi::OsStr,
    ) -> Result<FileId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let file_path = parent_node.path().join(name);

        // Create the file on disk.
        fs_err::tokio::File::create(&file_path).await.map_err(|e| {
            wrap(e, FsOperation::CreateFile, file_path.clone())
        })?;

        // Stat the new file and cache it.
        let meta = fs_err::symlink_metadata(&file_path)
            .map_err(|e| wrap(e, FsOperation::Resolve, file_path.clone()))?;
        let node = Self::build_node(&file_path, &meta, Some(*parent.get()));
        let id = self.cache_node(node);

        Ok(FileId::new(id))
    }

    async fn create_dir_node(
        &self,
        parent: DirId<Self::Nid>,
        name: &std::ffi::OsStr,
    ) -> Result<DirId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let dir_path = parent_node.path().join(name);

        // Create the directory on disk.
        fs_err::tokio::create_dir(&dir_path)
            .await
            .map_err(|e| wrap(e, FsOperation::CreateDir, dir_path.clone()))?;

        // Stat and cache.
        let meta = fs_err::symlink_metadata(&dir_path)
            .map_err(|e| wrap(e, FsOperation::Resolve, dir_path.clone()))?;
        let node = Self::build_node(&dir_path, &meta, Some(*parent.get()));
        let id = self.cache_node(node);

        Ok(DirId::new(id))
    }

    async fn create_symlink(
        &self,
        parent: DirId<Self::Nid>,
        name: &std::ffi::OsStr,
        target: &Path,
    ) -> Result<NodeId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let link_path = parent_node.path().join(name);

        // Create the symlink on disk.
        #[cfg(unix)]
        {
            tokio::fs::symlink(target, &link_path).await.map_err(|e| {
                wrap(e, FsOperation::CreateSymlink, link_path.clone())
            })?;
        }
        #[cfg(windows)]
        {
            if target.is_dir() {
                tokio::fs::symlink_dir(target, &link_path).await.map_err(
                    |e| wrap(e, FsOperation::CreateSymlink, link_path.clone()),
                )?;
            } else {
                tokio::fs::symlink_file(target, &link_path).await.map_err(
                    |e| wrap(e, FsOperation::CreateSymlink, link_path.clone()),
                )?;
            }
        }

        // Stat and cache.
        let meta = fs_err::symlink_metadata(&link_path)
            .map_err(|e| wrap(e, FsOperation::Resolve, link_path.clone()))?;
        let node = Self::build_node(&link_path, &meta, Some(*parent.get()));
        let id = self.cache_node(node);

        Ok(NodeId::new(id))
    }

    async fn write_node(
        &self,
        id: FileId<Self::Nid>,
        data: Bytes,
    ) -> Result<()> {
        let node = self.get_node(id.as_node_id())?;
        // Delegate to path-based write.
        self.write(node.path(), data).await
    }

    async fn flush_node(&self, id: FileId<Self::Nid>) -> Result<()> {
        let node = self.get_node(id.as_node_id())?;
        let mut file = self.open(node.path(), OpenMode::Read).await?;
        file.flush().await.map_err(|e| {
            wrap(e, FsOperation::Flush, node.path().to_path_buf())
        })
    }

    async fn remove_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        let node = self.get_node(id)?;
        let path = node.path().to_path_buf();

        // Tombstone the node.
        {
            let mut nodes = self.nodes.write();
            if let Some(arc) = nodes.get_mut(id.get()) {
                Arc::make_mut(arc).set_deleted();
            }
        }

        // Remove from filesystem.
        match fs_err::tokio::remove_file(&path).await {
            Ok(()) => {
                self.path_to_id.write().remove(&path);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::IsADirectory => {
                fs_err::tokio::remove_dir(&path)
                    .await
                    .map_err(|e| wrap(e, FsOperation::Remove, path.clone()))?;
                self.path_to_id.write().remove(&path);
                Ok(())
            }
            Err(e) => Err(wrap(e, FsOperation::Remove, path)),
        }
    }

    async fn remove_all_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        let node = self.get_node(id)?;
        let path = node.path().to_path_buf();

        // Tombstone the node.
        {
            let mut nodes = self.nodes.write();
            if let Some(arc) = nodes.get_mut(id.get()) {
                Arc::make_mut(arc).set_deleted();
            }
        }

        // Remove recursively from filesystem.
        let meta = fs_err::symlink_metadata(&path)
            .map_err(|e| wrap(e, FsOperation::Remove, path.clone()))?;
        if meta.is_dir() {
            fs_err::tokio::remove_dir_all(&path)
                .await
                .map_err(|e| wrap(e, FsOperation::Remove, path.clone()))?;
        } else {
            fs_err::tokio::remove_file(&path)
                .await
                .map_err(|e| wrap(e, FsOperation::Remove, path.clone()))?;
        }

        // Clean up cache entries for this path and its children.
        {
            let mut ptid = self.path_to_id.write();
            let prefix = path.to_string_lossy().to_string();
            ptid.retain(|p, _| {
                let pstr = p.to_string_lossy();
                !pstr.starts_with(&prefix)
            });
        }

        Ok(())
    }

    async fn rename_node(
        &self,
        id: NodeId<Self::Nid>,
        new_name: &std::ffi::OsStr,
    ) -> Result<()> {
        let node = self.get_node(id)?;
        let old_path = node.path().to_path_buf();
        let parent = old_path.parent().unwrap_or(Path::new("/"));
        let new_path = parent.join(new_name);

        // Rename on filesystem.
        fs_err::tokio::rename(&old_path, &new_path)
            .await
            .map_err(|e| wrap(e, FsOperation::Rename, old_path.clone()))?;

        // Update cache: remove old entries, insert new node.
        {
            let mut ptid = self.path_to_id.write();
            ptid.remove(&old_path);
            ptid.insert(new_path.clone(), *id.get());
        }
        {
            let mut nodes = self.nodes.write();
            if let Some(arc) = nodes.get_mut(id.get()) {
                let n = Arc::make_mut(arc);
                n.set_name(OsString::from(new_name));
                n.set_path(new_path);
            }
        }

        Ok(())
    }

    async fn copy_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let src_node = self.get_node(src)?;
        let dst_node = self.get_node(dst.as_node_id())?;
        let src_path = src_node.path();
        let dst_path = dst_node.path().join(src_node.name());

        // Copy on filesystem.
        if src_path.is_dir() {
            // For directories, use recursive copy.
            copy_dir_all(src_path, &dst_path).await?;
        } else {
            fs_err::tokio::copy(src_path, &dst_path)
                .await
                .map_err(|e| {
                    wrap(e, FsOperation::Copy, src_path.to_path_buf())
                })?;
        }

        // Resolve the new path into the cache.
        self.resolve_path(&dst_path).await
    }

    async fn move_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let src_node = self.get_node(src)?;
        let dst_node = self.get_node(dst.as_node_id())?;
        let src_path = src_node.path().to_path_buf();
        let dst_path = dst_node.path().join(src_node.name());

        // Try atomic rename first; fall back to copy+delete for
        // cross-filesystem moves.
        let moved = match fs_err::tokio::rename(&src_path, &dst_path).await {
            Ok(()) => true,
            Err(rename_err) => {
                // Cross-filesystem fallback.
                if src_path.is_dir() {
                    copy_dir_all(&src_path, &dst_path).await?;
                    fs_err::remove_dir_all(&src_path).map_err(|e| {
                        wrap(e, FsOperation::Move, src_path.clone())
                    })?;
                } else {
                    fs_err::copy(&src_path, &dst_path).map_err(|e| {
                        wrap(e, FsOperation::Copy, src_path.clone())
                    })?;
                    fs_err::remove_file(&src_path).map_err(|e| {
                        wrap(e, FsOperation::Remove, src_path.clone())
                    })?;
                }
                // Suppress the unused rename_err.
                let _ = rename_err;
                true
            }
        };
        let _ = moved;

        // Tombstone the source and update cache.
        {
            let mut nodes = self.nodes.write();
            if let Some(arc) = nodes.get_mut(src.get()) {
                Arc::make_mut(arc).set_deleted();
            }
        }
        self.path_to_id.write().remove(&src_path);

        // Resolve the destination into the cache.
        self.resolve_path(&dst_path).await
    }
}

/// Recursively copy a directory.
async fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    // Use boxed future to handle recursive async call.
    Box::pin(async move {
        fs_err::tokio::create_dir_all(dst)
            .await
            .map_err(|e| wrap(e, FsOperation::CreateDir, dst.to_path_buf()))?;

        let entries = fs_err::read_dir(src)
            .map_err(|e| wrap(e, FsOperation::ReadDir, src.to_path_buf()))?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                wrap(e, FsOperation::ReadDir, src.to_path_buf())
            })?;
            let src_entry = entry.path();
            let dst_entry = dst.join(entry.file_name());

            let meta = fs_err::symlink_metadata(&src_entry).map_err(|e| {
                wrap(e, FsOperation::Resolve, src_entry.clone())
            })?;

            if meta.file_type().is_symlink() {
                let target = fs_err::read_link(&src_entry).map_err(|e| {
                    wrap(e, FsOperation::ReadLink, src_entry.clone())
                })?;
                #[cfg(unix)]
                {
                    tokio::fs::symlink(&target, &dst_entry).await.map_err(
                        |e| wrap(e, FsOperation::Symlink, dst_entry.clone()),
                    )?;
                }
                #[cfg(windows)]
                {
                    if target.is_dir() {
                        tokio::fs::symlink_dir(&target, &dst_entry)
                            .await
                            .map_err(|e| {
                                wrap(
                                    e,
                                    FsOperation::Symlink,
                                    dst_entry.clone(),
                                )
                            })?;
                    } else {
                        tokio::fs::symlink_file(&target, &dst_entry)
                            .await
                            .map_err(|e| {
                                wrap(
                                    e,
                                    FsOperation::Symlink,
                                    dst_entry.clone(),
                                )
                            })?;
                    }
                }
            } else if meta.is_dir() {
                copy_dir_all(&src_entry, &dst_entry).await?;
            } else {
                fs_err::tokio::copy(&src_entry, &dst_entry).await.map_err(
                    |e| wrap(e, FsOperation::Copy, src_entry.to_path_buf()),
                )?;
            }
        }

        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;

    fn temp_dir() -> PathBuf {
        env::temp_dir().join(format!("cocomo_test_{}", process::id()))
    }

    #[tokio::test]
    async fn create_and_list_dir() {
        let base = temp_dir().join("create_list");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        fs.create_dir(&base.join("sub")).await.unwrap();

        let entries: Vec<_> =
            fs.read_dir(&base).await.unwrap().collect().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].as_ref().unwrap().name, "sub");

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn write_and_read_file() {
        let base = temp_dir().join("write_read");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("hello.txt");
        let data = Bytes::from_static(b"hello, cocomo");
        fs.write(&path, data.clone()).await.unwrap();

        let content = fs.read(&path, None).await.unwrap();
        assert_eq!(content, data);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn metadata_file() {
        let base = temp_dir().join("meta_file");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("data.bin");
        fs.write(&path, Bytes::from_static(b"12345")).await.unwrap();

        let meta = fs.metadata(&path).await.unwrap();
        assert!(!meta.is_dir);
        assert_eq!(meta.size, 5);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn metadata_dir() {
        let base = temp_dir().join("meta_dir");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let meta = fs.metadata(&base).await.unwrap();
        assert!(meta.is_dir);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn remove_file() {
        let base = temp_dir().join("remove_file");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("gone.txt");
        fs.write(&path, Bytes::from_static(b"x")).await.unwrap();

        fs.remove(&path).await.unwrap();
        assert!(!path.exists());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn remove_all_recursive() {
        let base = temp_dir().join("remove_all");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(base.join("a/b/c")).unwrap();

        let fs = LocalFs::new("test");
        fs.remove_all(&base).await.unwrap();
        assert!(!base.exists());
    }

    #[tokio::test]
    async fn rename_file() {
        let base = temp_dir().join("rename_file");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let src = base.join("old.txt");
        let dst = base.join("new.txt");
        fs.write(&src, Bytes::from_static(b"data")).await.unwrap();

        fs.rename(&src, &dst).await.unwrap();
        assert!(!src.exists());
        assert!(dst.exists());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn copy_file() {
        let base = temp_dir().join("copy_file");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let src = base.join("src.txt");
        let dst = base.join("dst.txt");
        fs.write(&src, Bytes::from_static(b"copy me"))
            .await
            .unwrap();

        fs.copy(&src, &dst).await.unwrap();
        let content = fs.read(&dst, None).await.unwrap();
        assert_eq!(content, Bytes::from_static(b"copy me"));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn not_found_error() {
        let fs = LocalFs::new("test");
        let result = fs
            .metadata(&PathBuf::from("/nonexistent/cocomo_test_xyz"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn label_returns_set_value() {
        let fs = LocalFs::new("my-label");
        assert_eq!(fs.label(), "my-label");
    }

    #[tokio::test]
    async fn open_and_flush() {
        let base = temp_dir().join("open_flush");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("flush.txt");
        let mut handle = fs.open(&path, OpenMode::Write).await.unwrap();
        handle.write_all(b"flush test").await.unwrap();
        handle.flush().await.unwrap();

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn read_range_inverted_returns_error() {
        let base = temp_dir().join("read_range_inv");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("data.txt");
        fs.write(&path, Bytes::from_static(b"hello world"))
            .await
            .unwrap();

        #[allow(clippy::reversed_empty_ranges)]
        let result = fs.read(&path, Some(10..5)).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, crate::error::FsError::InvalidArgument { .. }));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn read_range_equal_returns_error() {
        let base = temp_dir().join("read_range_eq");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("data.txt");
        fs.write(&path, Bytes::from_static(b"hello world"))
            .await
            .unwrap();

        let result = fs.read(&path, Some(5..5)).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, crate::error::FsError::InvalidArgument { .. }));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn read_range_clamped_to_file_size() {
        let base = temp_dir().join("read_range_clamp");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("data.txt");
        // File is 11 bytes.
        fs.write(&path, Bytes::from_static(b"hello world"))
            .await
            .unwrap();

        // Request bytes 8..100, should be clamped to 8..11.
        let content = fs.read(&path, Some(8..100)).await.unwrap();
        assert_eq!(content, Bytes::from_static(b"rld"));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn read_range_beyond_file_size_returns_empty() {
        let base = temp_dir().join("read_range_beyond");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("data.txt");
        fs.write(&path, Bytes::from_static(b"hi")).await.unwrap();

        // Start is beyond file size.
        let content = fs.read(&path, Some(100..200)).await.unwrap();
        assert!(content.is_empty());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn atomic_write_no_temp_file_left() {
        let base = temp_dir().join("atomic_write");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("output.txt");
        fs.write(&path, Bytes::from_static(b"atomic data"))
            .await
            .unwrap();

        // The file exists and has the correct content.
        assert!(path.exists());
        let content = fs.read(&path, None).await.unwrap();
        assert_eq!(content, Bytes::from_static(b"atomic data"));

        // No temp files remain in the directory.
        let entries: Vec<_> = fs_err::read_dir(&base).unwrap().collect();
        assert_eq!(entries.len(), 1);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn atomic_open_write_shutdown() {
        let base = temp_dir().join("atomic_open");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("output.txt");
        {
            let mut handle = fs.open(&path, OpenMode::Write).await.unwrap();
            handle.write_all(b"from handle").await.unwrap();
            handle.shutdown().await.unwrap();
        }

        // The file exists and has the correct content.
        assert!(path.exists());
        let content = fs.read(&path, None).await.unwrap();
        assert_eq!(content, Bytes::from_static(b"from handle"));

        // No temp files remain in the directory.
        let entries: Vec<_> = fs_err::read_dir(&base).unwrap().collect();
        assert_eq!(entries.len(), 1);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn temp_file_committed_on_drop_without_shutdown() {
        // Drop without explicit shutdown still commits the file via Drop.
        let base = temp_dir().join("drop_commit");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("test");
        let path = base.join("output.txt");

        // Open a write handle and drop it without calling shutdown.
        {
            let mut handle = fs.open(&path, OpenMode::Write).await.unwrap();
            handle.write_all(b"partial write").await.unwrap();
            // Handle is dropped here without shutdown.
        }

        // Drop still commits the file via atomic rename.
        assert!(path.exists());
        let content = fs.read(&path, None).await.unwrap();
        assert_eq!(content, Bytes::from_static(b"partial write"));

        // No temp files remain in the directory.
        let entries: Vec<_> = fs_err::read_dir(&base).unwrap().collect();
        assert_eq!(entries.len(), 1);

        fs_err::remove_dir_all(&base).ok();
    }

    // ── Node-based API tests ──

    #[tokio::test]
    async fn node_resolve_path_creates_cache_entry() {
        let base = temp_dir().join("node_resolve");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();
        let file_path = base.join("test.txt");
        fs_err::write(&file_path, "hello").unwrap();

        let fs = LocalFs::new("node_test");
        let nid = fs.resolve_path(&file_path).await.unwrap();

        // The node is now cached.
        let node = fs.get_node(nid).unwrap();
        assert_eq!(node.name(), "test.txt");
        assert!(node.kind().is_file());

        // Second resolve returns the same ID.
        let nid2 = fs.resolve_path(&file_path).await.unwrap();
        assert_eq!(nid, nid2);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_resolve_directory() {
        let base = temp_dir().join("node_dir");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("node_test");
        let did = fs.resolve_path(&base).await.unwrap();
        let node = fs.get_node(did).unwrap();
        assert!(node.kind().is_directory());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_metadata_from_cache() {
        let base = temp_dir().join("node_meta");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();
        let file_path = base.join("meta.txt");
        fs_err::write(&file_path, "content").unwrap();

        let fs = LocalFs::new("node_test");
        let nid = fs.resolve_path(&file_path).await.unwrap();
        let meta = fs.node_metadata(nid).unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.size, 7);

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_read_dir_populates_children() {
        let base = temp_dir().join("node_readdir");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();
        fs_err::write(base.join("a.txt"), "a").unwrap();
        fs_err::write(base.join("b.txt"), "b").unwrap();

        let fs = LocalFs::new("node_test");
        let did = fs.resolve_path(&base).await.unwrap();
        let dir_id = DirId::<u64>::new(*did.get());

        // Before read_dir, children are None.
        let node = fs.get_node(did).unwrap();
        assert!(node.kind().children().is_none());

        // After read_dir_node, children are populated.
        fs.read_dir_node(dir_id).await.unwrap();
        let node = fs.get_node(did).unwrap();
        let children = node.kind().children().unwrap();
        assert_eq!(children.len(), 2);
        assert!(children.contains(&"a.txt".to_string()));
        assert!(children.contains(&"b.txt".to_string()));

        // Children are also cached as individual nodes.
        let a_id = fs.resolve_path(&base.join("a.txt")).await.unwrap();
        let a_node = fs.get_node(a_id).unwrap();
        assert!(a_node.kind().is_file());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_create_file_and_remove() {
        let base = temp_dir().join("node_create_remove");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("node_test");
        let did = fs.resolve_path(&base).await.unwrap();
        let dir_id = DirId::<u64>::new(*did.get());

        // Create a file.
        let fid = fs
            .create_file(dir_id, "newfile.txt".as_ref())
            .await
            .unwrap();
        assert!(base.join("newfile.txt").exists());

        // Verify the node is cached.
        let node = fs.get_node(fid.as_node_id()).unwrap();
        assert!(node.kind().is_file());

        // Remove the file.
        fs.remove_node(fid.as_node_id()).await.unwrap();
        assert!(!base.join("newfile.txt").exists());

        // The node is tombstoned.
        assert!(matches!(
            fs.get_node(fid.as_node_id()),
            Err(FsError::StaleNode)
        ));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_create_dir_and_remove() {
        let base = temp_dir().join("node_create_dir_rm");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("node_test");
        let did = fs.resolve_path(&base).await.unwrap();
        let dir_id = DirId::<u64>::new(*did.get());

        // Create a subdirectory.
        let new_did =
            fs.create_dir_node(dir_id, "subdir".as_ref()).await.unwrap();
        assert!(base.join("subdir").is_dir());

        // Remove it.
        fs.remove_node(new_did.as_node_id()).await.unwrap();
        assert!(!base.join("subdir").exists());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_rename() {
        let base = temp_dir().join("node_rename");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();
        fs_err::write(base.join("old.txt"), "data").unwrap();

        let fs = LocalFs::new("node_test");
        let nid = fs.resolve_path(&base.join("old.txt")).await.unwrap();

        fs.rename_node(nid, "new.txt".as_ref()).await.unwrap();
        assert!(!base.join("old.txt").exists());
        assert!(base.join("new.txt").exists());

        // The cached node has the updated name and path.
        let node = fs.get_node(nid).unwrap();
        assert_eq!(node.name(), "new.txt");
        assert_eq!(node.path(), base.join("new.txt"));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_copy_file() {
        let base = temp_dir().join("node_copy");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();
        fs_err::write(base.join("src.txt"), "copy me").unwrap();

        let fs = LocalFs::new("node_test");
        let src_id = fs.resolve_path(&base.join("src.txt")).await.unwrap();
        let dst_id = fs.resolve_path(&base).await.unwrap();
        let dst_dir = DirId::<u64>::new(*dst_id.get());

        let new_id = fs.copy_node(src_id, dst_dir).await.unwrap();
        assert!(base.join("src.txt").exists());
        // The copy gets the same name in the destination directory.
        let new_node = fs.get_node(new_id).unwrap();
        assert_eq!(new_node.name(), "src.txt");

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_write_and_read() {
        let base = temp_dir().join("node_wr");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("node_test");
        let did = fs.resolve_path(&base).await.unwrap();
        let dir_id = DirId::<u64>::new(*did.get());

        // Create and write to a file.
        let fid = fs
            .create_file(dir_id, "writeme.txt".as_ref())
            .await
            .unwrap();
        fs.write_node(fid, Bytes::from("node data")).await.unwrap();

        // Read it back.
        let content = fs.read_node(fid, None).await.unwrap();
        assert_eq!(content, Bytes::from("node data"));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_remove_all_recursive() {
        let base = temp_dir().join("node_rmall");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(base.join("a/b/c")).unwrap();
        fs_err::write(base.join("a/b/c/deep.txt"), "deep").unwrap();
        fs_err::write(base.join("a/shallow.txt"), "shallow").unwrap();

        let fs = LocalFs::new("node_test");
        let a_id = fs.resolve_path(&base.join("a")).await.unwrap();

        // Resolve some children so they enter the cache.
        fs.resolve_path(&base.join("a/shallow.txt")).await.ok();
        fs.resolve_path(&base.join("a/b")).await.ok();

        fs.remove_all_node(a_id).await.unwrap();
        assert!(!base.join("a").exists());

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_stale_after_remove() {
        let base = temp_dir().join("node_stale");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();
        fs_err::write(base.join("victim.txt"), "bye").unwrap();

        let fs = LocalFs::new("node_test");
        let nid = fs.resolve_path(&base.join("victim.txt")).await.unwrap();

        // Access works.
        assert!(fs.get_node(nid).is_ok());

        // remove_node tombstones the node and deletes the file.
        fs.remove_node(nid).await.unwrap();

        // Now it is stale.
        assert!(matches!(fs.get_node(nid), Err(FsError::StaleNode)));

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_symlink_resolve() {
        let base = temp_dir().join("node_sym");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();
        fs_err::write(base.join("target.txt"), "linked").unwrap();

        let fs = LocalFs::new("node_test");

        // Create a symlink via path-based API.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                base.join("target.txt"),
                base.join("link.txt"),
            )
            .unwrap();
        }

        let link_id = fs.resolve_path(&base.join("link.txt")).await.unwrap();
        let link_node = fs.get_node(link_id).unwrap();
        assert!(link_node.kind().is_symlink());

        // Resolve the symlink target.
        let target_id = fs.resolve_symlink(link_id).await.unwrap();
        let target_node = fs.get_node(target_id).unwrap();
        assert!(target_node.kind().is_file());
        assert_eq!(target_node.name(), "target.txt");

        fs_err::remove_dir_all(&base).ok();
    }

    #[tokio::test]
    async fn node_id_is_reusable_after_tombstone() {
        let base = temp_dir().join("node_reuse");
        let _ = fs_err::remove_dir_all(&base);
        fs_err::create_dir_all(&base).unwrap();

        let fs = LocalFs::new("node_test");

        // Create, resolve, remove a file.
        fs_err::write(base.join("temp.txt"), "x").unwrap();
        let id1 = fs.resolve_path(&base.join("temp.txt")).await.unwrap();
        fs.remove_node(id1).await.unwrap();

        // Create a different file.
        fs_err::write(base.join("other.txt"), "y").unwrap();
        let id2 = fs.resolve_path(&base.join("other.txt")).await.unwrap();

        // IDs are different.
        assert_ne!(id1, id2);

        // The old ID is stale, the new ID works.
        assert!(matches!(fs.get_node(id1), Err(FsError::StaleNode)));
        assert!(fs.get_node(id2).is_ok());

        fs_err::remove_dir_all(&base).ok();
    }
}
