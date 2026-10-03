// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! SFTP filesystem provider.
//!
//! Provides access to remote SFTP (SSH file transfer) servers through the
//! [`FileSystem`], [`NodeFileSystem`], and [`WritableFileSystem`] traits.
//! Uses `russh` for the SSH transport and `russh-sftp` for the SFTP
//! protocol.
//!
//! # Connection management
//!
//! The SSH connection and SFTP session are established lazily on first use
//! and protected by a [`tokio::sync::Mutex`]. Opened file handles keep the
//! session alive through a shared reference, so they stay usable after the
//! mutex is released.
//!
//! # Host key verification
//!
//! The server host key is verified against the standard
//! `~/.ssh/known_hosts` file. Unknown or changed keys are refused (fail
//! closed), matching the default behaviour of `ssh`.
//!
//! # Authentication
//!
//! Key-based authentication is used when a key file is configured, with a
//! password fallback otherwise.

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    ffi::OsStr,
    fmt,
    future::Future,
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

use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;
use russh::{
    client::{self as ssh_client, Handler as SshHandler},
    keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use russh_sftp::{
    client::{SftpSession, error::Error as SftpError, fs::File as SftpHandle},
    protocol::{FileAttributes, OpenFlags},
};
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, ReadBuf,
};

use crate::{
    error::{FsError, FsOperation, Result},
    file::FsFile,
    fs::{
        DirEntryMeta, DirStream, FileSystem, NodeFileSystem, OpenMode,
        WritableFileSystem,
    },
    identity::{DirId, FileId, FileSystemId, NodeId},
    meta::Metadata,
    node::{Node, NodeKind, SymlinkTarget},
};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Concrete node ID type for the SFTP provider.
pub type SftpNodeId = NodeId<u64>;

/// Concrete directory ID type for the SFTP provider.
pub type SftpDirId = DirId<u64>;

/// Concrete file ID type for the SFTP provider.
pub type SftpFileId = FileId<u64>;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// SFTP provider configuration.
///
/// Holds the parameters needed to connect to an SFTP server. SSH encrypts
/// its own transport, so there is no separate TLS flag.
#[derive(Clone)]
pub struct SftpConfig {
    /// Server hostname or IP address.
    pub host: String,
    /// Server port (default 22).
    pub port: u16,
    /// Username for authentication.
    pub username: String,
    /// Password for authentication. Should be loaded from a secure store.
    pub password: Option<String>,
    /// Path to a private key file for key-based authentication. Takes
    /// precedence over the password when set.
    pub key_file: Option<PathBuf>,
    /// Optional root directory that all paths are relative to.
    pub root_path: Option<PathBuf>,
}

impl fmt::Debug for SftpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SftpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("key_file", &self.key_file)
            .field("root_path", &self.root_path)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Error conversion
// ---------------------------------------------------------------------------

/// Convert an SFTP client error into an `FsError` with operation context.
fn sftp_error_to_fs(
    err: SftpError,
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
// SSH client handler
// ---------------------------------------------------------------------------

/// Error type for the SSH client handler.
#[derive(Debug)]
struct SshHandlerError(String);

impl fmt::Display for SshHandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SshHandlerError {}

impl From<russh::Error> for SshHandlerError {
    fn from(err: russh::Error) -> Self {
        Self(err.to_string())
    }
}

/// SSH client handler that verifies the server host key against the
/// standard `~/.ssh/known_hosts` file.
struct HostKeyHandler {
    /// The host the connection is addressed to.
    host: String,
    /// The port the connection is addressed to.
    port: u16,
}

impl HostKeyHandler {
    /// Verify `key` against the known hosts file.
    ///
    /// Fails closed: a host that is not recorded, or whose recorded key no
    /// longer matches, is refused rather than accepted.
    fn verify_key(
        &self,
        key: &PublicKeyOrCertificate,
    ) -> std::result::Result<bool, SshHandlerError> {
        let pub_key = key.public_key();
        match russh::keys::check_known_hosts(&self.host, self.port, &pub_key) {
            Ok(true) => Ok(true),
            Ok(false) => Err(SshHandlerError(format!(
                "host {}:{} is not in known_hosts; connect once with `ssh` \
                 to record its key",
                self.host, self.port
            ))),
            Err(e) => Err(SshHandlerError(format!(
                "host key verification failed for {}:{}: {e}",
                self.host, self.port
            ))),
        }
    }
}

impl SshHandler for HostKeyHandler {
    type Error = SshHandlerError;

    fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> impl Future<Output = std::result::Result<bool, Self::Error>> + Send
    {
        let result = self.verify_key(key);
        async move { result }
    }
}

// ---------------------------------------------------------------------------
// SftpFile — handle for an opened SFTP file
// ---------------------------------------------------------------------------

/// A file handle backed by an SFTP session.
///
/// Reads and writes are streamed directly through the SFTP protocol; the
/// handle keeps the underlying session alive through a shared reference.
struct SftpFile {
    /// Absolute path of the file (our internal representation).
    path: PathBuf,
    /// Cached metadata captured at open time.
    meta: Metadata,
    /// Whether the handle was opened for writing. Read-only handles discard
    /// writes instead of failing them, mirroring the FTP provider.
    writable: bool,
    /// The opened remote file. `None` once the handle has been consumed by
    /// a shutdown.
    file: Option<SftpHandle>,
}

#[async_trait]
impl FsFile for SftpFile {
    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    async fn flush(&mut self) -> Result<()> {
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.flush().await.map_err(|e| {
            io_error_to_fs(e, FsOperation::Flush, self.path.clone())
        })
    }
}

impl AsyncRead for SftpFile {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(file) = self.file.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        Pin::new(file).poll_read(cx, buf)
    }
}

impl AsyncWrite for SftpFile {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let writable = self.writable;
        let Some(file) = self.file.as_mut() else {
            return Poll::Ready(Ok(buf.len()));
        };
        if !writable {
            // In read mode, writing discards the data.
            return Poll::Ready(Ok(buf.len()));
        }
        Pin::new(file).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(file) = self.file.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        Pin::new(file).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(file) = self.file.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        Pin::new(file).poll_shutdown(cx)
    }
}

// ---------------------------------------------------------------------------
// SftpFs — main provider
// ---------------------------------------------------------------------------

/// SFTP filesystem provider.
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
/// The [`SftpSession`] is wrapped in a [`tokio::sync::Mutex`] because the
/// session is shared between concurrent file handles. The connection is
/// established lazily on first use.
pub struct SftpFs {
    /// Human-readable label for this provider instance.
    label: String,
    /// Filesystem instance identifier (host hash).
    fs_id: FileSystemId<u64>,
    /// SFTP configuration.
    config: SftpConfig,
    /// SFTP session, protected by a mutex. `None` means not connected.
    pub(crate) connection: tokio::sync::Mutex<Option<SftpSession>>,
    /// Node cache: node ID \u{2192} node.
    nodes: parking_lot::RwLock<HashMap<u64, Arc<Node>>>,
    /// Reverse lookup: absolute path \u{2192} node ID.
    path_to_id: parking_lot::RwLock<HashMap<PathBuf, u64>>,
    /// Monotonically increasing counter for node ID generation.
    next_id: AtomicU64,
}

impl SftpFs {
    /// Create a new SFTP provider instance.
    pub fn new(label: impl Into<String>, config: SftpConfig) -> Self {
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

    /// Return the SFTP configuration.
    pub fn config(&self) -> &SftpConfig {
        &self.config
    }

    /// Generate a new unique node ID.
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    // ── Connection management ──

    /// Establish or return the existing SFTP session.
    ///
    /// Connects to the server, authenticates, and opens the `sftp`
    /// subsystem. Returns a locked guard over the session.
    async fn connect(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<SftpSession>>> {
        let mut conn = self.connection.lock().await;
        if conn.is_none() {
            *conn = Some(self.establish_session().await?);
        }
        Ok(conn)
    }

    /// Open a new SSH connection and SFTP session.
    async fn establish_session(&self) -> Result<SftpSession> {
        let cfg = &self.config;
        let ssh_config = Arc::new(ssh_client::Config::default());
        let handler = HostKeyHandler {
            host: cfg.host.clone(),
            port: cfg.port,
        };
        let mut handle = ssh_client::connect(
            ssh_config,
            (cfg.host.as_str(), cfg.port),
            handler,
        )
        .await
        .map_err(|e| self.conn_error(e.to_string()))?;

        // Authenticate: key-based first, password as the fallback.
        let auth = match (&cfg.key_file, &cfg.password) {
            (Some(key_file), _) => {
                let pem = fs_err::read_to_string(key_file)
                    .map_err(|e| self.conn_error(e.to_string()))?;
                let key = russh::keys::decode_secret_key(&pem, None)
                    .map_err(|e| self.conn_error(e.to_string()))?;
                handle
                    .authenticate_publickey(
                        &cfg.username,
                        PrivateKeyWithHashAlg::new(Arc::new(key), None),
                    )
                    .await
                    .map_err(|e| self.conn_error(e.to_string()))?
            }
            (None, Some(password)) => handle
                .authenticate_password(&cfg.username, password)
                .await
                .map_err(|e| self.conn_error(e.to_string()))?,
            (None, None) => {
                return Err(self.conn_error(
                    "no credentials configured for SFTP connection".into(),
                ));
            }
        };
        if !auth.success() {
            return Err(self.conn_error(format!(
                "authentication failed for user `{}`",
                cfg.username
            )));
        }

        // Open the sftp subsystem on a session channel. The channel stream
        // keeps the SSH connection alive; the handle itself can be dropped.
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| self.conn_error(e.to_string()))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| self.conn_error(e.to_string()))?;
        let stream = channel.into_stream();

        SftpSession::new(stream)
            .await
            .map_err(|e| self.conn_error(e.to_string()))
    }

    /// Build a connection-phase error addressed to the server host.
    fn conn_error(&self, message: String) -> FsError {
        FsError::Io {
            operation: FsOperation::Open,
            path: PathBuf::from(self.config.host.clone()),
            message,
        }
    }

    /// Helper to get a reference to the SftpSession from a MutexGuard.
    fn session<'a>(
        guard: &'a tokio::sync::MutexGuard<'_, Option<SftpSession>>,
    ) -> Result<&'a SftpSession> {
        guard.as_ref().ok_or(FsError::Io {
            operation: FsOperation::Open,
            path: PathBuf::from("(sftp session)"),
            message: "SFTP connection not established".into(),
        })
    }

    // ── Path translation ──

    /// Convert our internal absolute path to an SFTP path.
    ///
    /// SFTP paths are absolute POSIX paths on the server. If `root_path` is
    /// `Some("/data")`, then `/data/sub/file.txt` becomes `"/sub/file.txt"`
    /// and the root itself becomes `"/"`. Without a `root_path`, the path is
    /// used as-is.
    fn to_sftp_path(&self, path: &Path) -> String {
        let path_str = path.to_string_lossy();
        if let Some(ref root) = self.config.root_path {
            let root_str = root.to_string_lossy();
            if let Some(stripped) = path_str.strip_prefix(root_str.as_ref()) {
                // Only strip a whole path component, so `/datafoo` is not
                // mistaken for a child of `/data`.
                if stripped.is_empty() || stripped.starts_with('/') {
                    if stripped.is_empty() {
                        return "/".to_owned();
                    }
                    return stripped.to_string();
                }
            }
        }
        // No root (or a path outside the root): use the absolute path.
        if path_str.starts_with('/') {
            path_str.to_string()
        } else {
            format!("/{path_str}")
        }
    }

    // ── Node helpers ──

    /// Build a [`Node`] from SFTP metadata.
    ///
    /// SFTP does not expose inodes or device IDs, so those are set to
    /// `None`. Symlinks have their target read eagerly so that
    /// [`NodeFileSystem::resolve_symlink`] can use it later.
    async fn build_node(
        &self,
        path: &Path,
        meta: &Metadata,
        parent_id: Option<u64>,
    ) -> Node {
        let name = path
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_default();

        if meta.is_symlink {
            let target = self.read_link(path).await.unwrap_or_default();
            Node::symlink(
                name,
                path.to_path_buf(),
                meta.clone(),
                SymlinkTarget::new(target),
            )
            .with_parent(parent_id)
        } else if meta.is_dir {
            Node::directory(name, path.to_path_buf(), meta.clone())
                .with_parent(parent_id)
        } else {
            Node::file(name, path.to_path_buf(), meta.clone())
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

    // ── Server operations ──

    /// Stat a path on the SFTP server without following symlinks and return
    /// metadata.
    async fn lstat_path(&self, path: &Path) -> Result<Metadata> {
        let sftp_path = self.to_sftp_path(path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        let attrs =
            session.symlink_metadata(&sftp_path).await.map_err(|e| {
                sftp_error_to_fs(e, FsOperation::Open, path.to_path_buf())
            })?;
        Ok(attrs_to_metadata(&attrs))
    }

    /// List entries in a directory. Returns `(name, metadata)` for each
    /// entry; symlink attributes are not followed.
    async fn list_entries(
        &self,
        dir_path: &Path,
    ) -> Result<Vec<(String, Metadata)>> {
        let sftp_dir = self.to_sftp_path(dir_path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        let read_dir = session.read_dir(&sftp_dir).await.map_err(|e| {
            sftp_error_to_fs(e, FsOperation::ReadDir, dir_path.to_path_buf())
        })?;

        let mut entries = Vec::new();
        for entry in read_dir {
            let name = entry.file_name();
            if name == "." || name == ".." {
                continue;
            }
            entries.push((name, attrs_to_metadata(&entry.metadata())));
        }
        Ok(entries)
    }
}

/// Convert SFTP file attributes into our [`Metadata`].
fn attrs_to_metadata(attrs: &FileAttributes) -> Metadata {
    let modified = attrs
        .mtime
        .and_then(|t| DateTime::from_timestamp(t as i64, 0))
        .unwrap_or_else(Utc::now);
    let size = attrs.size.unwrap_or(0);

    if attrs.is_dir() {
        Metadata::dir(modified)
    } else if attrs.is_symlink() {
        let mut meta = Metadata::file(size, modified);
        meta.is_symlink = true;
        meta
    } else {
        Metadata::file(size, modified)
    }
}

// ---------------------------------------------------------------------------
// FileSystem trait
// ---------------------------------------------------------------------------

#[async_trait]
impl FileSystem for SftpFs {
    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        self.lstat_path(path).await
    }

    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>> {
        let entries = self.list_entries(path).await?;

        let metas: Vec<DirEntryMeta> = entries
            .into_iter()
            .map(|(name, meta)| DirEntryMeta { name, meta })
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

        let sftp_path = self.to_sftp_path(path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;

        let (flags, writable) = match mode {
            OpenMode::Read => (OpenFlags::READ, false),
            OpenMode::Write => (
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
                true,
            ),
            OpenMode::Append => (
                OpenFlags::CREATE | OpenFlags::APPEND | OpenFlags::WRITE,
                true,
            ),
        };

        let mut file = session
            .open_with_flags(&sftp_path, flags)
            .await
            .map_err(|e| {
                sftp_error_to_fs(e, FsOperation::Open, path.to_path_buf())
            })?;

        if matches!(mode, OpenMode::Append) {
            // The SFTP client writes at explicit offsets starting at zero,
            // so move the position to the end for append semantics.
            file.seek(std::io::SeekFrom::End(0)).await.map_err(|e| {
                io_error_to_fs(e, FsOperation::Open, path.to_path_buf())
            })?;
        }

        Ok(Box::new(SftpFile {
            path: path.to_path_buf(),
            meta,
            writable,
            file: Some(file),
        }))
    }

    async fn read(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        // Validate the range before allocating or reading.
        if let Some(r) = &range
            && r.start >= r.end
        {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::Read,
                path: path.to_path_buf(),
                message: format!(
                    "invalid range: start ({}) must be less than end ({})",
                    r.start, r.end
                ),
            });
        }

        let sftp_path = self.to_sftp_path(path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        let mut file = session.open(&sftp_path).await.map_err(|e| {
            sftp_error_to_fs(e, FsOperation::Read, path.to_path_buf())
        })?;

        let mut buf = Vec::new();
        match range {
            Some(r) => {
                file.seek(std::io::SeekFrom::Start(r.start)).await.map_err(
                    |e| {
                        io_error_to_fs(
                            e,
                            FsOperation::Read,
                            path.to_path_buf(),
                        )
                    },
                )?;
                // `take` stops at the range end or EOF, so a short read at
                // the end of file yields the available bytes.
                file.take(r.end - r.start)
                    .read_to_end(&mut buf)
                    .await
                    .map_err(|e| {
                        io_error_to_fs(
                            e,
                            FsOperation::Read,
                            path.to_path_buf(),
                        )
                    })?;
            }
            None => {
                file.read_to_end(&mut buf).await.map_err(|e| {
                    io_error_to_fs(e, FsOperation::Read, path.to_path_buf())
                })?;
            }
        }
        Ok(Bytes::from(buf))
    }

    async fn read_stream(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        // Read the file, optionally with range, then chunk it into a
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
        let sftp_path = self.to_sftp_path(path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        let mut file = session.create(&sftp_path).await.map_err(|e| {
            sftp_error_to_fs(e, FsOperation::Write, path.to_path_buf())
        })?;
        file.write_all(data.as_ref()).await.map_err(|e| {
            io_error_to_fs(e, FsOperation::Write, path.to_path_buf())
        })?;
        file.close().await.map_err(|e| {
            io_error_to_fs(e, FsOperation::Write, path.to_path_buf())
        })?;
        Ok(())
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        let sftp_path = self.to_sftp_path(path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        session.create_dir(&sftp_path).await.map_err(|e| {
            sftp_error_to_fs(e, FsOperation::CreateDir, path.to_path_buf())
        })?;
        Ok(())
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        let sftp_path = self.to_sftp_path(path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;

        // Try removing as a file first, then as a directory.
        match session.remove_file(&sftp_path).await {
            Ok(()) => Ok(()),
            Err(_) => session.remove_dir(&sftp_path).await.map_err(|e| {
                sftp_error_to_fs(e, FsOperation::Remove, path.to_path_buf())
            }),
        }
    }

    async fn remove_all(&self, path: &Path) -> Result<()> {
        // Recursive removal via lstat + readdir + remove.
        self.remove_all_impl(path).await
    }

    async fn rename(&self, src: &Path, dst: &Path) -> Result<()> {
        let src_sftp = self.to_sftp_path(src);
        let dst_sftp = self.to_sftp_path(dst);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        session.rename(&src_sftp, &dst_sftp).await.map_err(|e| {
            sftp_error_to_fs(e, FsOperation::Rename, src.to_path_buf())
        })?;
        Ok(())
    }

    async fn copy(&self, src: &Path, dst: &Path) -> Result<()> {
        // SFTP has no native copy; read source then write to destination.
        let data = self.read(src, None).await?;
        self.write(dst, data).await
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        let sftp_path = self.to_sftp_path(path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        let target = session.read_link(&sftp_path).await.map_err(|e| {
            sftp_error_to_fs(e, FsOperation::ReadLink, path.to_path_buf())
        })?;
        Ok(PathBuf::from(target))
    }

    async fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
        let link_sftp = self.to_sftp_path(link);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        session
            .symlink(&link_sftp, target.to_string_lossy().as_ref())
            .await
            .map_err(|e| {
                sftp_error_to_fs(e, FsOperation::Symlink, link.to_path_buf())
            })?;
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }
}

impl SftpFs {
    /// Recursively remove a path from the SFTP server.
    async fn remove_all_impl(&self, path: &Path) -> Result<()> {
        // Use boxed future to handle recursive async call.
        Box::pin(async {
            // lstat so that symlinks are removed as links, not followed.
            let meta = self.lstat_path(path).await?;

            if meta.is_dir {
                let entries = self.list_entries(path).await?;
                for (name, _) in &entries {
                    self.remove_all_impl(&path.join(name)).await?;
                }

                let sftp_path = self.to_sftp_path(path);
                let conn = self.connect().await?;
                let session = Self::session(&conn)?;
                session.remove_dir(&sftp_path).await.map_err(|e| {
                    sftp_error_to_fs(
                        e,
                        FsOperation::Remove,
                        path.to_path_buf(),
                    )
                })?;
            } else {
                self.remove(path).await?;
            }

            Ok::<_, FsError>(())
        })
        .await
    }

    /// Recursively copy a directory on the SFTP server.
    async fn copy_dir_all(&self, src: &Path, dst: &Path) -> Result<()> {
        // Use boxed future to handle recursive async call.
        Box::pin(async {
            self.create_dir(dst).await?;

            let entries = self.list_entries(src).await?;

            for (name, meta) in &entries {
                let src_entry = src.join(name);
                let dst_entry = dst.join(name);

                if meta.is_dir {
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
impl NodeFileSystem for SftpFs {
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

        // lstat the path to determine node kind without following symlinks.
        let meta = self.lstat_path(path).await?;
        let parent_id = self.resolve_parent_id(path);

        let node = self.build_node(path, &meta, parent_id).await;
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
        for (name, meta) in entries {
            child_names.push(name.clone());

            let entry_path = dir_path.join(&name);
            let node = self
                .build_node(&entry_path, &meta, Some(*dir_id.get()))
                .await;
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
impl WritableFileSystem for SftpFs {
    async fn create_file(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<FileId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let file_path = parent_node.path().join(name);

        // Create the empty file on the server.
        let sftp_path = self.to_sftp_path(&file_path);
        let conn = self.connect().await?;
        let session = Self::session(&conn)?;
        session.create(&sftp_path).await.map_err(|e| {
            sftp_error_to_fs(e, FsOperation::CreateFile, file_path.clone())
        })?;

        // Stat and cache the new file.
        let meta = self.lstat_path(&file_path).await?;
        let node = self
            .build_node(&file_path, &meta, Some(*parent.get()))
            .await;
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

        // Create the directory on the SFTP server.
        self.create_dir(&dir_path).await?;

        // Stat and cache.
        let meta = self.lstat_path(&dir_path).await?;
        let node =
            self.build_node(&dir_path, &meta, Some(*parent.get())).await;
        let id = self.cache_node(node);

        Ok(DirId::new(id))
    }

    async fn create_symlink(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
        target: &Path,
    ) -> Result<NodeId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let link_path = parent_node.path().join(name);

        // Create the symlink on the SFTP server.
        self.symlink(target, &link_path).await?;

        // Stat and cache the new symlink.
        let meta = self.lstat_path(&link_path).await?;
        let node = self
            .build_node(&link_path, &meta, Some(*parent.get()))
            .await;
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

    async fn flush_node(&self, _id: FileId<Self::Nid>) -> Result<()> {
        // SFTP writes are streamed; no flush needed.
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

        // Remove from the SFTP server.
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

        // Remove recursively from the SFTP server.
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

        // Rename on the SFTP server.
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

        // Rename on the SFTP server.
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

impl Clone for SftpFs {
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

    fn make_config() -> SftpConfig {
        SftpConfig {
            host: "sftp.example.com".into(),
            port: 22,
            username: "user".into(),
            password: Some("pass".into()),
            key_file: None,
            root_path: None,
        }
    }

    fn make_config_with_root(root: &str) -> SftpConfig {
        SftpConfig {
            host: "sftp.example.com".into(),
            port: 22,
            username: "user".into(),
            password: Some("pass".into()),
            key_file: None,
            root_path: Some(PathBuf::from(root)),
        }
    }

    #[test]
    fn sftp_config_smoke() {
        let config = make_config();
        let fs = SftpFs::new("test-sftp", config);
        assert_eq!(fs.label(), "test-sftp");
    }

    #[test]
    fn sftpfs_id_is_deterministic() {
        let config = make_config();
        let fs1 = SftpFs::new("x", config.clone());
        let fs2 = SftpFs::new("y", config);
        assert_eq!(fs1.id(), fs2.id());
    }

    #[test]
    fn path_translation_no_root() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        // Without a root, absolute paths are used as-is (SFTP paths are
        // absolute on the server).
        assert_eq!(
            fs.to_sftp_path(Path::new("/sub/file.txt")),
            "/sub/file.txt",
        );
        assert_eq!(fs.to_sftp_path(Path::new("/file.txt")), "/file.txt");
        assert_eq!(fs.to_sftp_path(Path::new("/")), "/");
    }

    #[test]
    fn path_translation_with_root() {
        let config = make_config_with_root("/data");
        let fs = SftpFs::new("test", config);

        // Path under root: strip the root prefix, keep it absolute.
        assert_eq!(
            fs.to_sftp_path(Path::new("/data/sub/file.txt")),
            "/sub/file.txt",
        );
        assert_eq!(fs.to_sftp_path(Path::new("/data/file.txt")), "/file.txt");
        assert_eq!(fs.to_sftp_path(Path::new("/data")), "/");

        // A sibling of the root is not a child: no stripping.
        assert_eq!(
            fs.to_sftp_path(Path::new("/datafoo/file.txt")),
            "/datafoo/file.txt",
        );
    }

    #[test]
    fn sftpfs_connection_error_returns_fs_error() {
        // Use a local address with an unlikely port to fail fast.
        let config = SftpConfig {
            host: "127.0.0.1".into(),
            port: 1, // Port 1 is typically refused immediately.
            username: "user".into(),
            password: Some("pass".into()),
            key_file: None,
            root_path: None,
        };
        let fs = SftpFs::new("test", config);

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
    fn sftpfs_metadata_returns_error_when_no_server() {
        let config = SftpConfig {
            host: "127.0.0.1".into(),
            port: 1,
            username: "user".into(),
            password: Some("pass".into()),
            key_file: None,
            root_path: None,
        };
        let fs = SftpFs::new("test", config);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(fs.metadata(Path::new("/some/path")));
        assert!(result.is_err());
    }

    #[test]
    fn sftpfs_alloc_id_is_monotonic() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        let id1 = fs.alloc_id();
        let id2 = fs.alloc_id();
        let id3 = fs.alloc_id();

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn sftpfs_cache_node_returns_existing_id() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        let path = PathBuf::from("/test/file.txt");
        let node1 = Node::file(
            "file.txt".into(),
            path.clone(),
            Metadata::file(100, Utc::now()),
        );
        let id1 = fs.cache_node(node1);

        // Caching the same path should return the same ID.
        let node2 = Node::file(
            "file.txt".into(),
            path.clone(),
            Metadata::file(200, Utc::now()),
        );
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
    fn sftpfs_lookup_path_returns_none_for_missing() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        assert!(fs.lookup_path(Path::new("/nonexistent")).is_none());
    }

    #[test]
    fn sftpfs_build_node_directory() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        let path = PathBuf::from("/data/mydir");
        let meta = Metadata::dir(Utc::now());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let node = rt.block_on(fs.build_node(&path, &meta, None));

        assert!(node.kind().is_directory());
        assert!(!node.kind().is_file());
        assert!(node.metadata().is_dir);
        assert_eq!(node.metadata().inode, None);
        assert_eq!(node.metadata().device_id, None);
    }

    #[test]
    fn sftpfs_build_node_file() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        let path = PathBuf::from("/data/file.txt");
        let meta = Metadata::file(42, Utc::now());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let node = rt.block_on(fs.build_node(&path, &meta, None));

        assert!(node.kind().is_file());
        assert!(!node.kind().is_directory());
        assert_eq!(node.metadata().size, 42);
        assert_eq!(node.metadata().inode, None);
    }

    #[test]
    fn sftpfs_get_node_returns_stale_for_deleted() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        let path = PathBuf::from("/test/file.txt");
        let node = Node::file(
            "file.txt".into(),
            path.clone(),
            Metadata::file(100, Utc::now()),
        );
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
    fn sftpfs_set_node_hash_updates_cache() {
        let config = make_config();
        let fs = SftpFs::new("test", config);

        let path = PathBuf::from("/test/file.txt");
        let node = Node::file(
            "file.txt".into(),
            path.clone(),
            Metadata::file(100, Utc::now()),
        );
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
    fn sftp_config_debug_redacts_password() {
        let config = SftpConfig {
            host: "sftp.example.com".into(),
            port: 22,
            username: "user".into(),
            password: Some("xS3cR3tX".into()),
            key_file: None,
            root_path: None,
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("sftp.example.com"));
        assert!(debug.contains("user"));
        assert!(debug.contains("REDACTED"));
        assert!(
            !debug.contains("xS3cR3tX"),
            "password should not appear in debug output"
        );
    }

    #[test]
    fn attrs_to_metadata_maps_kinds() {
        let mut dir = FileAttributes::default();
        dir.set_dir(true);
        dir.mtime = Some(0);
        let meta = attrs_to_metadata(&dir);
        assert!(meta.is_dir);
        assert!(!meta.is_symlink);

        let mut link = FileAttributes::default();
        link.set_symlink(true);
        link.size = Some(12);
        let meta = attrs_to_metadata(&link);
        assert!(meta.is_symlink);
        assert_eq!(meta.size, 12);

        let mut file = FileAttributes::default();
        file.set_regular(true);
        file.size = Some(7);
        let meta = attrs_to_metadata(&file);
        assert!(meta.is_file());
        assert_eq!(meta.size, 7);
    }
}
