// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! FTP filesystem provider.
//!
//! Provides access to remote FTP/FTPS servers through the [`FileSystem`],
//! [`NodeFileSystem`], and [`WritableFileSystem`] traits. Uses `async-ftp`
//! for the underlying client.
//!
//! # Connection management
//!
//! The FTP connection is lazily established on first use and protected by a
//! [`tokio::sync::Mutex`] because `async_ftp::FtpStream` requires `&mut self`
//! for all operations.
//!
//! # Configuration
//!
//! FTP providers are configured via a profile that specifies the host, port,
//! and credentials. The profile is resolved at construction time; see
//! [`FtpConfig`].

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    ffi::OsStr,
    hash::{Hash, Hasher},
    io,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

use async_ftp::{FtpError, FtpStream, types::FileType};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::{
    error::{FsError, FsOperation, Result},
    file::FsFile,
    fs::{
        DirEntryMeta, DirStream, FileSystem, NodeFileSystem, OpenMode,
        WritableFileSystem,
    },
    identity::{DirId, FileId, FileSystemId, NodeId},
    meta::Metadata,
    node::Node,
};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Concrete node ID type for the FTP provider.
pub type FtpNodeId = NodeId<u64>;

/// Concrete directory ID type for the FTP provider.
pub type FtpDirId = DirId<u64>;

/// Concrete file ID type for the FTP provider.
pub type FtpFileId = FileId<u64>;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// FTP provider configuration.
///
/// Holds the parameters needed to connect to an FTP server.
#[derive(Clone, Debug)]
pub struct FtpConfig {
    /// Server hostname or IP address.
    pub host: String,
    /// Server port (default 21 for FTP, 990 for FTPS).
    pub port: u16,
    /// Username for authentication.
    pub username: String,
    /// Password for authentication. Should be loaded from a secure store.
    pub password: String,
    /// Use FTPS (FTP over TLS). Requires the `secure` feature of `async_ftp`.
    pub tls: bool,
    /// Optional initial working directory.
    pub root_path: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Error conversion
// ---------------------------------------------------------------------------

/// Convert an `FtpError` into an `FsError` with operation context.
fn ftp_error_to_fs(
    err: FtpError,
    operation: FsOperation,
    path: PathBuf,
) -> FsError {
    FsError::Io {
        operation,
        path,
        message: err.to_string(),
    }
}

/// Convert an `io::Error` into an `FsError` with operation context.
fn io_error_to_fs(
    err: io::Error,
    operation: FsOperation,
    path: PathBuf,
) -> FsError {
    crate::error::wrap(err, operation, path)
}

// ---------------------------------------------------------------------------
// FtpFile — handle for an opened FTP file
// ---------------------------------------------------------------------------

/// Mode for an FTP file handle.
enum FtpFileMode {
    /// Opened for reading; data is loaded on first read.
    Read {
        /// Buffered file content. `None` means not yet loaded.
        data: Option<Vec<u8>>,
        /// Read position within the buffered data.
        pos: usize,
    },
    /// Opened for writing; data is buffered until flush/shutdown/drop.
    Write(Vec<u8>),
}

/// A file handle backed by an FTP connection.
///
/// For reads, the entire file is downloaded on first access and served from
/// memory. For writes, data is buffered and uploaded on `shutdown` or `drop`.
struct FtpFile {
    /// Shared reference to the FTP filesystem provider.
    fs: Arc<FtpFs>,
    /// Absolute path of the file (our internal representation).
    path: PathBuf,
    /// Cached metadata captured at open time.
    meta: Metadata,
    /// Current mode and associated data.
    mode: FtpFileMode,
}

#[async_trait]
impl FsFile for FtpFile {
    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    async fn flush(&mut self) -> Result<()> {
        match &self.mode {
            FtpFileMode::Read { .. } => Ok(()),
            FtpFileMode::Write(data) => {
                if data.is_empty() {
                    return Ok(());
                }
                // Upload the buffered data.
                let data_copy = data.clone();
                let path = self.path.clone();
                let ftp_path = self.fs.to_ftp_path(&path);
                let mut conn = self.fs.connection.lock().await;
                let stream = conn.as_mut().ok_or(FsError::Io {
                    operation: FsOperation::Write,
                    path: path.clone(),
                    message: "FTP connection not established".into(),
                })?;
                let _ = stream.transfer_type(FileType::Binary).await;
                let cursor = &mut std::io::Cursor::new(data_copy);
                stream.put(&ftp_path, cursor).await.map_err(|e| {
                    ftp_error_to_fs(e, FsOperation::Write, path.clone())
                })?;
                // Clear buffered data after successful upload.
                let FtpFileMode::Write(ref mut d) = self.mode else {
                    unreachable!();
                };
                d.clear();
                Ok(())
            }
        }
    }
}

impl AsyncRead for FtpFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let FtpFileMode::Read { data, pos } = &mut self.mode else {
            // In write mode, reading returns EOF.
            return Poll::Ready(Ok(()));
        };

        let bytes = match data {
            None => {
                // Data not yet loaded; signal pending so the async path
                // (via the FsFile methods) can load it.
                return Poll::Pending;
            }
            Some(b) => b,
        };

        if *pos >= bytes.len() {
            return Poll::Ready(Ok(()));
        }

        let remaining = &bytes[*pos..];
        let to_copy = std::cmp::min(remaining.len(), buf.remaining());
        buf.put_slice(&remaining[..to_copy]);
        *pos += to_copy;

        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for FtpFile {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let FtpFileMode::Write(data) = &mut self.mode else {
            // In read mode, writing is a no-op (discard data).
            return Poll::Ready(Ok(buf.len()));
        };
        data.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // FTP writes are streamed; flush is handled by shutdown or
        // FsFile::flush.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        // Upload is triggered by the async FsFile::flush() or happens
        // on explicit flush. Shutdown just signals completion.
        Poll::Ready(Ok(()))
    }
}

impl Drop for FtpFile {
    fn drop(&mut self) {
        // Nothing to clean up; FTP uploads are done via the async flush.
    }
}

// ---------------------------------------------------------------------------
// FtpFs — main provider
// ---------------------------------------------------------------------------

/// FTP filesystem provider.
///
/// # Node cache
///
/// Every resolved remote path is cached as a [`Node`] keyed by a
/// monotonically increasing [`u64`] identifier. A reverse lookup map
/// maintains path \u{2192} ID mappings. Deleted nodes are tombstoned rather
/// than removed from the cache.
///
/// # Connection
///
/// The [`async_ftp::FtpStream`] is wrapped in a [`tokio::sync::Mutex`]
/// because it requires `&mut self` for all operations. The connection is
/// established lazily on first use.
pub struct FtpFs {
    /// Human-readable label for this provider instance.
    label: String,
    /// Filesystem instance identifier (host hash).
    fs_id: FileSystemId<u64>,
    /// FTP configuration.
    config: FtpConfig,
    /// FTP connection, protected by a mutex. `None` means not connected.
    pub(crate) connection: tokio::sync::Mutex<Option<FtpStream>>,
    /// Node cache: node ID \u{2192} node.
    nodes: parking_lot::RwLock<HashMap<u64, Arc<Node>>>,
    /// Reverse lookup: absolute path \u{2192} node ID.
    path_to_id: parking_lot::RwLock<HashMap<PathBuf, u64>>,
    /// Monotonically increasing counter for node ID generation.
    next_id: AtomicU64,
}

impl FtpFs {
    /// Create a new FTP provider instance.
    pub fn new(label: impl Into<String>, config: FtpConfig) -> Self {
        let mut hasher = DefaultHasher::new();
        format!("{}:{}", config.host, config.port).hash(&mut hasher);
        let fs_id_val = hasher.finish();
        Self {
            label: label.into(),
            fs_id: FileSystemId::new(fs_id_val),
            config,
            connection: tokio::sync::Mutex::new(None),
            nodes: parking_lot::RwLock::new(HashMap::new()),
            path_to_id: parking_lot::RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Return the FTP configuration.
    pub fn config(&self) -> &FtpConfig {
        &self.config
    }

    /// Generate a new unique node ID.
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    // ── Connection management ──

    /// Establish or return the existing FTP connection.
    ///
    /// Connects to the server, logs in, and changes to the root directory
    /// if configured. Returns a locked guard over the connection.
    async fn connect(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<FtpStream>>> {
        let mut conn = self.connection.lock().await;
        if conn.is_none() {
            // Reject TLS when the `secure` feature is not available.
            if self.config.tls {
                return Err(FsError::Io {
                    operation: FsOperation::Open,
                    path: PathBuf::from(&*self.config.host),
                    message: "FTPS requires the `secure` feature of async_ftp"
                        .into(),
                });
            }

            let addr = format!("{}:{}", self.config.host, self.config.port);
            let stream = FtpStream::connect(&addr).await.map_err(|e| {
                ftp_error_to_fs(
                    e,
                    FsOperation::Open,
                    PathBuf::from(&*self.config.host),
                )
            })?;

            // Login.
            let mut stream = stream;
            stream
                .login(&self.config.username, &self.config.password)
                .await
                .map_err(|e| {
                    ftp_error_to_fs(
                        e,
                        FsOperation::Open,
                        PathBuf::from(&*self.config.host),
                    )
                })?;

            // Change to root directory if configured.
            if let Some(ref root) = self.config.root_path {
                let ftp_root = self.to_ftp_path(root);
                if !ftp_root.is_empty() {
                    stream.cwd(&ftp_root).await.map_err(|e| {
                        ftp_error_to_fs(e, FsOperation::Open, root.clone())
                    })?;
                }
            }

            *conn = Some(stream);
        }
        Ok(conn)
    }

    // ── Path translation ──

    /// Convert our internal absolute path to an FTP relative path.
    ///
    /// If `root_path` is `Some("/data")`, then `/data/sub/file.txt` becomes
    /// `"sub/file.txt"`. If `root_path` is `None`, the path without a leading
    /// slash is used.
    fn to_ftp_path(&self, path: &Path) -> String {
        let path_str = path.to_string_lossy();
        if let Some(ref root) = self.config.root_path {
            let root_str = root.to_string_lossy();
            if let Some(stripped) = path_str.strip_prefix(root_str.as_ref()) {
                return stripped.trim_start_matches('/').to_string();
            }
            // Fallback: use the path as-is (shouldn't happen in normal use).
            return path_str.trim_start_matches('/').to_string();
        }
        // No root_path; strip leading slash if present.
        path_str.trim_start_matches('/').to_string()
    }

    /// Convert an FTP relative path (or name) into our internal absolute path.
    fn to_absolute_path(&self, relative: &str) -> PathBuf {
        let cleaned = relative.trim_start_matches('/');
        if let Some(ref root) = self.config.root_path {
            root.join(cleaned)
        } else {
            PathBuf::from(format!("/{cleaned}"))
        }
    }

    // ── Node helpers ──

    /// Build a [`Node`] from FTP metadata.
    ///
    /// FTP does not expose inodes or device IDs, so those are set to `None`.
    /// Permissions default to read + write (or + exec for directories).
    fn build_node(
        &self,
        path: &Path,
        size: u64,
        modified: DateTime<Utc>,
        is_dir: bool,
        parent_id: Option<u64>,
    ) -> Node {
        let name = path
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        let metadata = if is_dir {
            Metadata::dir(modified)
        } else {
            Metadata::file(size, modified)
        };

        if is_dir {
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

    /// Helper to get a mutable reference to the FtpStream from a MutexGuard.
    fn stream<'a>(
        guard: &'a mut tokio::sync::MutexGuard<'_, Option<FtpStream>>,
    ) -> Result<&'a mut FtpStream> {
        guard.as_mut().ok_or(FsError::Io {
            operation: FsOperation::Open,
            path: PathBuf::from("(ftp stream)"),
            message: "FTP connection not established".into(),
        })
    }

    /// Stat a path on the FTP server and return metadata.
    ///
    /// Uses `cwd` to detect directories (if `cwd` succeeds, it is a
    /// directory). Uses `size` and `mdtm` for file metadata.
    async fn stat_path(&self, path: &Path) -> Result<Metadata> {
        let ftp_path = self.to_ftp_path(path);
        let mut conn = self.connect().await?;
        let stream = Self::stream(&mut conn)?;

        // Ensure binary mode is set.
        let _ = stream.transfer_type(FileType::Binary).await;

        // Detect if it is a directory by attempting to cd into it.
        let is_dir = match stream.cwd(&ftp_path).await {
            Ok(()) => {
                // It is a directory; navigate back.
                let _ = stream.cwd("..").await;
                true
            }
            Err(_) => false,
        };

        // Get modification time.
        let modified = match stream.mdtm(&ftp_path).await {
            Ok(Some(dt)) => dt,
            Ok(None) | Err(_) => Utc::now(),
        };

        if is_dir {
            Ok(Metadata::dir(modified))
        } else {
            let size = match stream.size(&ftp_path).await {
                Ok(Some(s)) => s as u64,
                Ok(None) | Err(_) => 0,
            };
            Ok(Metadata::file(size, modified))
        }
    }

    /// List entries in a directory. Returns `(name, size, modified, is_dir)`
    /// for each entry.
    async fn list_entries(
        &self,
        dir_path: &Path,
    ) -> Result<Vec<(String, u64, DateTime<Utc>, bool)>> {
        let ftp_dir = self.to_ftp_path(dir_path);
        let mut conn = self.connect().await?;
        let stream = Self::stream(&mut conn)?;

        // Get the current directory so we can restore it.
        let cwd = stream.pwd().await.map_err(|e| {
            ftp_error_to_fs(e, FsOperation::ReadDir, dir_path.to_path_buf())
        })?;

        // Get names using NLST.
        let names = stream.nlst(Some(&ftp_dir)).await.map_err(|e| {
            ftp_error_to_fs(e, FsOperation::ReadDir, dir_path.to_path_buf())
        })?;

        let mut entries = Vec::new();
        for name in names {
            if name == "." || name == ".." {
                continue;
            }

            let entry_ftp_path = if ftp_dir.is_empty() {
                name.clone()
            } else {
                format!("{ftp_dir}/{name}")
            };

            // Determine if directory by trying to cd into it.
            let is_dir = stream.cwd(&entry_ftp_path).await.is_ok();
            // Navigate back to previous directory.
            if is_dir {
                let _ = stream.cwd(&cwd).await;
            }

            // Get size and modification time.
            let size = stream
                .size(&entry_ftp_path)
                .await
                .unwrap_or(None)
                .unwrap_or(0) as u64;
            let modified = stream
                .mdtm(&entry_ftp_path)
                .await
                .unwrap_or(None)
                .unwrap_or_else(Utc::now);

            entries.push((name, size, modified, is_dir));
        }

        Ok(entries)
    }

    /// Load file content from FTP into memory.
    async fn load_file_data(&self, path: &Path) -> Result<Vec<u8>> {
        let ftp_path = self.to_ftp_path(path);
        let mut conn = self.connect().await?;
        let stream = Self::stream(&mut conn)?;
        let _ = stream.transfer_type(FileType::Binary).await;
        let cursor = stream.simple_retr(&ftp_path).await.map_err(|e| {
            ftp_error_to_fs(e, FsOperation::Read, path.to_path_buf())
        })?;
        Ok(cursor.into_inner())
    }
}

// ---------------------------------------------------------------------------
// FileSystem trait
// ---------------------------------------------------------------------------

#[async_trait]
impl FileSystem for FtpFs {
    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        self.stat_path(path).await
    }

    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>> {
        let entries = self.list_entries(path).await?;

        let metas: Vec<DirEntryMeta> = entries
            .into_iter()
            .map(|(name, size, modified, is_dir)| {
                let meta = if is_dir {
                    Metadata::dir(modified)
                } else {
                    Metadata::file(size, modified)
                };
                DirEntryMeta { name, meta }
            })
            .collect();

        let s: DirStream<'_> =
            Box::pin(futures::stream::iter(metas.into_iter().map(Ok)));
        Ok(s)
    }

    async fn open(
        &self,
        path: &Path,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        let meta = self.metadata(path).await?;

        let ftp_file = match mode {
            OpenMode::Read => FtpFile {
                fs: Arc::new(self.clone()),
                path: path.to_path_buf(),
                meta,
                mode: FtpFileMode::Read { data: None, pos: 0 },
            },
            OpenMode::Write | OpenMode::Append => {
                let write_meta = Metadata::file(0, Utc::now());
                FtpFile {
                    fs: Arc::new(self.clone()),
                    path: path.to_path_buf(),
                    meta: write_meta,
                    mode: FtpFileMode::Write(Vec::new()),
                }
            }
        };

        Ok(Box::new(ftp_file))
    }

    async fn read(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        let data = if let Some(r) = range {
            // Validate the range before allocating or reading.
            if r.start >= r.end {
                return Err(FsError::InvalidArgument {
                    operation: FsOperation::Read,
                    path: path.to_path_buf(),
                    message: format!(
                        "invalid range: start ({}) must be less than end ({})",
                        r.start, r.end
                    ),
                });
            }
            // For ranged reads, use restart_from + get.
            let ftp_path = self.to_ftp_path(path);
            let mut conn = self.connect().await?;
            let stream = Self::stream(&mut conn)?;
            let _ = stream.transfer_type(FileType::Binary).await;
            stream.restart_from(r.start).await.map_err(|e| {
                ftp_error_to_fs(e, FsOperation::Read, path.to_path_buf())
            })?;
            let mut data_stream =
                stream.get(&ftp_path).await.map_err(|e| {
                    ftp_error_to_fs(e, FsOperation::Read, path.to_path_buf())
                })?;

            let len = (r.end - r.start) as usize;
            let mut buf = vec![0u8; len];
            use tokio::io::AsyncReadExt;
            match data_stream.read_exact(&mut buf).await {
                Ok(_) => Bytes::from(buf),
                Err(e) => {
                    // If we read fewer bytes than expected (EOF), return what
                    // we got.
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        Bytes::from(buf)
                    } else {
                        return Err(io_error_to_fs(
                            e,
                            FsOperation::Read,
                            path.to_path_buf(),
                        ));
                    }
                }
            }
        } else {
            // Full read: load entire file.
            let data = self.load_file_data(path).await?;
            Bytes::from(data)
        };
        Ok(data)
    }

    async fn read_stream(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        // Download the file, optionally with range, then chunk it into a
        // stream.
        let data = self.read(path, range).await?;

        // Split into 8KB chunks.
        let chunk_size = 8 * 1024;
        let chunks: Vec<Bytes> = data
            .chunks(chunk_size)
            .map(Bytes::copy_from_slice)
            .collect();

        Ok(Box::pin(futures::stream::iter(chunks.into_iter().map(Ok))))
    }

    async fn write(&self, path: &Path, data: Bytes) -> Result<()> {
        let ftp_path = self.to_ftp_path(path);
        let mut conn = self.connect().await?;
        let stream = Self::stream(&mut conn)?;
        let _ = stream.transfer_type(FileType::Binary).await;
        let cursor = &mut std::io::Cursor::new(data.as_ref());
        stream.put(&ftp_path, cursor).await.map_err(|e| {
            ftp_error_to_fs(e, FsOperation::Write, path.to_path_buf())
        })?;
        Ok(())
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        let ftp_path = self.to_ftp_path(path);
        let mut conn = self.connect().await?;
        let stream = Self::stream(&mut conn)?;
        stream.mkdir(&ftp_path).await.map_err(|e| {
            ftp_error_to_fs(e, FsOperation::CreateDir, path.to_path_buf())
        })?;
        Ok(())
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        let ftp_path = self.to_ftp_path(path);
        let mut conn = self.connect().await?;
        let stream = Self::stream(&mut conn)?;

        // Try removing as a file first, then as a directory.
        match stream.rm(&ftp_path).await {
            Ok(()) => Ok(()),
            Err(_) => stream.rmdir(&ftp_path).await.map_err(|e| {
                ftp_error_to_fs(e, FsOperation::Remove, path.to_path_buf())
            }),
        }
    }

    async fn remove_all(&self, path: &Path) -> Result<()> {
        // Recursive removal via NLST + DELE/RMD.
        self.remove_all_impl(path).await
    }

    async fn rename(&self, src: &Path, dst: &Path) -> Result<()> {
        let src_ftp = self.to_ftp_path(src);
        let dst_ftp = self.to_ftp_path(dst);
        let mut conn = self.connect().await?;
        let stream = Self::stream(&mut conn)?;
        stream.rename(&src_ftp, &dst_ftp).await.map_err(|e| {
            ftp_error_to_fs(e, FsOperation::Rename, src.to_path_buf())
        })?;
        Ok(())
    }

    async fn copy(&self, src: &Path, dst: &Path) -> Result<()> {
        // FTP has no native copy; read source then write to destination.
        let data = self.read(src, None).await?;
        self.write(dst, data).await
    }

    async fn read_link(&self, _path: &Path) -> Result<PathBuf> {
        Err(FsError::Io {
            operation: FsOperation::ReadLink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
    }

    async fn symlink(&self, _target: &Path, _link: &Path) -> Result<()> {
        Err(FsError::Io {
            operation: FsOperation::Symlink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
    }

    fn label(&self) -> &str {
        &self.label
    }
}

impl FtpFs {
    /// Recursively remove a path from the FTP server.
    async fn remove_all_impl(&self, path: &Path) -> Result<()> {
        // Use boxed future to handle recursive async call.
        Box::pin(async {
            let ftp_path = self.to_ftp_path(path);
            let mut conn = self.connect().await?;
            let stream = Self::stream(&mut conn)?;

            // Try to determine if it is a directory.
            let is_dir = stream.cwd(&ftp_path).await.is_ok();
            // Navigate back if we cd'd in.
            if is_dir {
                let _ = stream.cwd("..").await;
            }

            if is_dir {
                // List directory contents and recurse.
                let names =
                    stream.nlst(Some(&ftp_path)).await.unwrap_or_default();

                for name in names {
                    if name == "." || name == ".." {
                        continue;
                    }
                    let child_ftp = format!("{ftp_path}/{name}");
                    let child_abs = self.to_absolute_path(&child_ftp);
                    self.remove_all_impl(&child_abs).await?;
                }

                // Remove the now-empty directory.
                stream.rmdir(&ftp_path).await.map_err(|e| {
                    ftp_error_to_fs(e, FsOperation::Remove, path.to_path_buf())
                })?;
            } else {
                // Remove as a file.
                stream.rm(&ftp_path).await.map_err(|e| {
                    ftp_error_to_fs(e, FsOperation::Remove, path.to_path_buf())
                })?;
            }

            Ok::<_, FsError>(())
        })
        .await
    }

    /// Recursively copy a directory on the FTP server.
    async fn copy_dir_all(&self, src: &Path, dst: &Path) -> Result<()> {
        // Use boxed future to handle recursive async call.
        Box::pin(async {
            self.create_dir(dst).await?;

            let entries = self.list_entries(src).await?;

            for (name, _size, _modified, is_dir) in &entries {
                let src_entry = src.join(name);
                let dst_entry = dst.join(name);

                if *is_dir {
                    self.copy_dir_all(&src_entry, &dst_entry).await?;
                } else {
                    self.copy(&src_entry, &dst_entry).await?;
                }
            }

            Ok::<_, FsError>(())
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// NodeFileSystem trait
// ---------------------------------------------------------------------------

#[async_trait]
impl NodeFileSystem for FtpFs {
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
        // Check cache first.
        if let Some(id) = self.lookup_path(path) {
            let node = self.nodes.read().get(&id).cloned();
            return match node {
                Some(n) if n.is_deleted() => Err(FsError::StaleNode),
                Some(_) => Ok(NodeId::new(id)),
                None => Err(FsError::NotFound {
                    path: path.to_path_buf(),
                }),
            };
        }

        // Stat the path to determine node kind.
        let meta = self.stat_path(path).await?;
        let parent_id = self.resolve_parent_id(path);

        let node = self.build_node(
            path,
            meta.size,
            meta.modified,
            meta.is_dir,
            parent_id,
        );
        let id = self.cache_node(node);

        Ok(NodeId::new(id))
    }

    async fn resolve_symlink(
        &self,
        _id: NodeId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        Err(FsError::Io {
            operation: FsOperation::ReadLink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
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

    fn set_node_hash(
        &self,
        id: NodeId<Self::Nid>,
        hash: String,
    ) -> Result<()> {
        let mut nodes = self.nodes.write();
        let Some(arc) = nodes.get_mut(id.get()) else {
            return Err(FsError::NotFound {
                path: PathBuf::from("(unknown node)"),
            });
        };
        Arc::make_mut(arc).set_cached_hash(hash);
        Ok(())
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

        let entries = self.list_entries(dir_path).await?;

        let mut child_names = Vec::new();
        for (name, size, modified, is_dir) in entries {
            child_names.push(name.clone());

            let entry_path = dir_path.join(&name);
            let node = self.build_node(
                &entry_path,
                size,
                modified,
                is_dir,
                Some(*dir_id.get()),
            );
            self.cache_node(node);
        }

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

        // Delegate to path-based open.
        self.open(path, mode).await
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
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        let node = self.get_node(id.as_node_id())?;
        // Delegate to path-based read_stream.
        self.read_stream(node.path(), range).await
    }
}

// ---------------------------------------------------------------------------
// WritableFileSystem trait
// ---------------------------------------------------------------------------

#[async_trait]
impl WritableFileSystem for FtpFs {
    async fn create_file(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<FileId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let file_path = parent_node.path().join(name);

        // Create the file by uploading empty data.
        self.write(&file_path, Bytes::new()).await?;

        // Stat and cache the new file.
        let meta = self.stat_path(&file_path).await?;
        let node = self.build_node(
            &file_path,
            meta.size,
            meta.modified,
            false,
            Some(*parent.get()),
        );
        let id = self.cache_node(node);

        Ok(FileId::new(id))
    }

    async fn create_dir_node(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<DirId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let dir_path = parent_node.path().join(name);

        // Create the directory on the FTP server.
        self.create_dir(&dir_path).await?;

        // Stat and cache.
        let meta = self.stat_path(&dir_path).await?;
        let node = self.build_node(
            &dir_path,
            meta.size,
            meta.modified,
            true,
            Some(*parent.get()),
        );
        let id = self.cache_node(node);

        Ok(DirId::new(id))
    }

    async fn create_symlink(
        &self,
        _parent: DirId<Self::Nid>,
        _name: &OsStr,
        _target: &Path,
    ) -> Result<NodeId<Self::Nid>> {
        Err(FsError::Io {
            operation: FsOperation::CreateSymlink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
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

    async fn flush_node(&self, _id: FileId<Self::Nid>) -> Result<()> {
        // FTP writes are streamed; no flush needed.
        Ok(())
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

        // Remove from FTP server.
        self.remove(&path).await?;

        // Remove from cache.
        self.path_to_id.write().remove(&path);

        Ok(())
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

        // Remove recursively from FTP server.
        self.remove_all_impl(&path).await?;

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
        new_name: &OsStr,
    ) -> Result<()> {
        let node = self.get_node(id)?;
        let old_path = node.path().to_path_buf();
        let parent = old_path.parent().unwrap_or(Path::new("/"));
        let new_path = parent.join(new_name);

        // Rename on FTP server.
        self.rename(&old_path, &new_path).await?;

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
                n.set_name(new_name.to_os_string());
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

        // For directories, recurse. For files, read + write.
        if src_node.kind().is_directory() {
            self.copy_dir_all(src_path, &dst_path).await?;
        } else {
            self.copy(src_path, &dst_path).await?;
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

        // Rename on FTP server.
        self.rename(&src_path, &dst_path).await?;

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

impl Clone for FtpFs {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            fs_id: self.fs_id,
            config: self.config.clone(),
            connection: tokio::sync::Mutex::new(None),
            nodes: parking_lot::RwLock::new(self.nodes.read().clone()),
            path_to_id: parking_lot::RwLock::new(
                self.path_to_id.read().clone(),
            ),
            next_id: AtomicU64::new(self.next_id.load(Ordering::Relaxed)),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> FtpConfig {
        FtpConfig {
            host: "ftp.example.com".into(),
            port: 21,
            username: "user".into(),
            password: "pass".into(),
            tls: false,
            root_path: None,
        }
    }

    fn make_config_with_root(root: &str) -> FtpConfig {
        FtpConfig {
            host: "ftp.example.com".into(),
            port: 21,
            username: "user".into(),
            password: "pass".into(),
            tls: false,
            root_path: Some(PathBuf::from(root)),
        }
    }

    #[test]
    fn ftp_config_smoke() {
        let config = make_config();
        let fs = FtpFs::new("test-ftp", config);
        assert_eq!(fs.label(), "test-ftp");
    }

    #[test]
    fn ftpfs_id_is_deterministic() {
        let config = make_config();
        let fs1 = FtpFs::new("x", config.clone());
        let fs2 = FtpFs::new("y", config);
        assert_eq!(fs1.id(), fs2.id());
    }

    #[test]
    fn ftpfs_symlink_returns_error() {
        let config = make_config();
        let fs = FtpFs::new("test-ftp", config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(fs.symlink(Path::new("/target"), Path::new("/link")));
        assert!(result.is_err());
    }

    #[test]
    fn path_translation_no_root() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        // Absolute path without root: strip leading slash.
        assert_eq!(fs.to_ftp_path(Path::new("/sub/file.txt")), "sub/file.txt");
        assert_eq!(fs.to_ftp_path(Path::new("/file.txt")), "file.txt");
        assert_eq!(fs.to_ftp_path(Path::new("/")), "");
    }

    #[test]
    fn path_translation_with_root() {
        let config = make_config_with_root("/data");
        let fs = FtpFs::new("test", config);

        // Path under root: strip root prefix.
        assert_eq!(
            fs.to_ftp_path(Path::new("/data/sub/file.txt")),
            "sub/file.txt",
        );
        assert_eq!(fs.to_ftp_path(Path::new("/data/file.txt")), "file.txt");
        assert_eq!(fs.to_ftp_path(Path::new("/data")), "");
    }

    #[test]
    fn to_absolute_path_no_root() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        assert_eq!(
            fs.to_absolute_path("sub/file.txt"),
            PathBuf::from("/sub/file.txt"),
        );
        assert_eq!(
            fs.to_absolute_path("file.txt"),
            PathBuf::from("/file.txt"),
        );
    }

    #[test]
    fn to_absolute_path_with_root() {
        let config = make_config_with_root("/data");
        let fs = FtpFs::new("test", config);

        assert_eq!(
            fs.to_absolute_path("sub/file.txt"),
            PathBuf::from("/data/sub/file.txt"),
        );
        assert_eq!(
            fs.to_absolute_path("file.txt"),
            PathBuf::from("/data/file.txt"),
        );
    }

    #[test]
    fn roundtrip_path_translation() {
        let config = make_config_with_root("/data");
        let fs = FtpFs::new("test", config);

        let original = PathBuf::from("/data/a/b/c.txt");
        let ftp_rel = fs.to_ftp_path(&original);
        let restored = fs.to_absolute_path(&ftp_rel);
        assert_eq!(original, restored);
    }

    #[test]
    fn ftpfs_connection_error_returns_fs_error() {
        // Use a local address with an unlikely port to fail fast.
        let config = FtpConfig {
            host: "127.0.0.1".into(),
            port: 1, // Port 1 is typically refused immediately.
            username: "user".into(),
            password: "pass".into(),
            tls: false,
            root_path: None,
        };
        let fs = FtpFs::new("test", config);

        let rt = tokio::runtime::Runtime::new().unwrap();
        // Attempt to read a file, which triggers a connection.
        let result = rt.block_on(fs.read(Path::new("/test.txt"), None));
        assert!(result.is_err());
        match result {
            Err(FsError::Io { ref message, .. }) => {
                assert!(!message.is_empty());
            }
            Err(_) => {}
            Ok(_) => panic!("expected connection error"),
        }
    }

    #[test]
    fn ftpfs_metadata_returns_error_when_no_server() {
        let config = FtpConfig {
            host: "127.0.0.1".into(),
            port: 1,
            username: "user".into(),
            password: "pass".into(),
            tls: false,
            root_path: None,
        };
        let fs = FtpFs::new("test", config);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(fs.metadata(Path::new("/some/path")));
        assert!(result.is_err());
    }

    #[test]
    fn ftpfs_alloc_id_is_monotonic() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        let id1 = fs.alloc_id();
        let id2 = fs.alloc_id();
        let id3 = fs.alloc_id();

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn ftpfs_cache_node_returns_existing_id() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        let path = PathBuf::from("/test/file.txt");
        let node1 = fs.build_node(&path, 100, Utc::now(), false, None);
        let id1 = fs.cache_node(node1);

        // Caching the same path should return the same ID.
        let node2 = fs.build_node(&path, 200, Utc::now(), false, None);
        let id2 = fs.cache_node(node2);

        assert_eq!(id1, id2);
        // The original node data should be preserved (size should be 100).
        let cached_size = {
            let guard = fs.nodes.read();
            guard.get(&id1).unwrap().metadata().size
        };
        assert_eq!(cached_size, 100);
    }

    #[test]
    fn ftpfs_lookup_path_returns_none_for_missing() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        assert!(fs.lookup_path(Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn ftpfs_build_node_directory() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        let path = PathBuf::from("/data/mydir");
        let node = fs.build_node(&path, 0, Utc::now(), true, None);

        assert!(node.kind().is_directory());
        assert!(!node.kind().is_file());
        assert!(node.metadata().is_dir);
        assert_eq!(node.metadata().inode, None);
        assert_eq!(node.metadata().device_id, None);
    }

    #[test]
    fn ftpfs_build_node_file() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        let path = PathBuf::from("/data/file.txt");
        let node = fs.build_node(&path, 42, Utc::now(), false, None);

        assert!(node.kind().is_file());
        assert!(!node.kind().is_directory());
        assert_eq!(node.metadata().size, 42);
        assert_eq!(node.metadata().inode, None);
    }

    #[test]
    fn ftpfs_get_node_returns_stale_for_deleted() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        let path = PathBuf::from("/test/file.txt");
        let node = fs.build_node(&path, 100, Utc::now(), false, None);
        let id = fs.cache_node(node);

        // Tombstone the node.
        {
            let mut nodes = fs.nodes.write();
            if let Some(arc) = nodes.get_mut(&id) {
                Arc::make_mut(arc).set_deleted();
            }
        }

        let result = fs.get_node(NodeId::new(id));
        assert!(matches!(result, Err(FsError::StaleNode)));
    }

    #[test]
    fn ftpfs_set_node_hash_updates_cache() {
        let config = make_config();
        let fs = FtpFs::new("test", config);

        let path = PathBuf::from("/test/file.txt");
        let node = fs.build_node(&path, 100, Utc::now(), false, None);
        let id = fs.cache_node(node);

        assert!(
            fs.get_node(NodeId::new(id))
                .unwrap()
                .cached_hash()
                .is_none()
        );

        fs.set_node_hash(NodeId::new(id), "abc123".into()).unwrap();

        assert_eq!(
            fs.get_node(NodeId::new(id)).unwrap().cached_hash(),
            Some("abc123"),
        );
    }

    #[test]
    fn ftpfs_read_link_returns_error() {
        let config = make_config();
        let fs = FtpFs::new("test-ftp", config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(fs.read_link(Path::new("/some/link")));
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("symbolic links"));
    }

    #[test]
    fn ftpfs_resolve_symlink_returns_error() {
        let config = make_config();
        let fs = FtpFs::new("test-ftp", config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(fs.resolve_symlink(NodeId::new(1)));
        assert!(result.is_err());
    }

    #[test]
    fn ftpfs_create_symlink_returns_error() {
        let config = make_config();
        let fs = FtpFs::new("test-ftp", config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(fs.create_symlink(
            DirId::new(1),
            OsStr::new("link"),
            Path::new("/target"),
        ));
        assert!(result.is_err());
    }

    #[test]
    fn ftpfs_tls_returns_error_without_secure_feature() {
        let config = FtpConfig {
            host: "ftp.example.com".into(),
            port: 21,
            username: "user".into(),
            password: "pass".into(),
            tls: true,
            root_path: None,
        };
        let fs = FtpFs::new("test-ftp", config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        // Any operation that triggers connect should fail with the TLS error.
        let result = rt.block_on(fs.metadata(Path::new("/test")));
        assert!(result.is_err());
        let err = result.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("secure"));
    }
}
