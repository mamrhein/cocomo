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
//! # Configuration
//!
//! FTP providers are configured via a profile that specifies the host, port,
//! and credentials. The profile is resolved at construction time; see
//! [`FtpConfig`].

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    ffi::OsStr,
    hash::{Hash, Hasher},
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::{
    error::{FsError, Result},
    file::FsFile,
    fs::{
        DirStream, FileSystem, NodeFileSystem, OpenMode, WritableFileSystem,
    },
    identity::{DirId, FileId, FileSystemId, NodeId},
    meta::Metadata,
    node::Node,
};

/// Concrete node ID type for the FTP provider.
pub type FtpNodeId = NodeId<u64>;

/// Concrete directory ID type for the FTP provider.
pub type FtpDirId = DirId<u64>;

/// Concrete file ID type for the FTP provider.
pub type FtpFileId = FileId<u64>;

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
    /// Use FTPS (FTP over TLS).
    pub tls: bool,
    /// Optional initial working directory.
    pub root_path: Option<PathBuf>,
}

/// FTP filesystem provider.
///
/// # Node cache
///
/// Every resolved remote path is cached as a [`Node`] keyed by a
/// monotonically increasing [`u64`] identifier. A reverse lookup map
/// maintains path \u{2192} ID mappings. Deleted nodes are tombstoned rather
/// than removed from the cache.
///
/// # Implementation status
///
/// This is a scaffolding stub. All I/O methods are unimplemented. The
/// structural pattern mirrors [`LocalFs`](crate::local::LocalFs).
#[allow(dead_code)]
pub struct FtpFs {
    /// Human-readable label for this provider instance.
    label: String,
    /// Filesystem instance identifier (host hash).
    fs_id: FileSystemId<u64>,
    /// FTP configuration.
    config: FtpConfig,
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
    #[allow(dead_code)]
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[async_trait]
impl FileSystem for FtpFs {
    async fn metadata(&self, _path: &Path) -> Result<Metadata> {
        unimplemented!("FtpFs::metadata — implement with async-ftp STAT")
    }

    async fn read_dir(&self, _path: &Path) -> Result<DirStream<'_>> {
        unimplemented!("FtpFs::read_dir — implement with async-ftp LIST/NLST")
    }

    async fn open(
        &self,
        _path: &Path,
        _mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        unimplemented!("FtpFs::open — implement with async-ftp RETR(STOR)")
    }

    async fn read(
        &self,
        _path: &Path,
        _range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        unimplemented!(
            "FtpFs::read — implement with async-ftp RETR (REST offset)"
        )
    }

    async fn read_stream(
        &self,
        _path: &Path,
        _range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        unimplemented!(
            "FtpFs::read_stream — implement with async-ftp streaming RETR"
        )
    }

    async fn write(&self, _path: &Path, _data: Bytes) -> Result<()> {
        unimplemented!("FtpFs::write — implement with async-ftp STOR")
    }

    async fn create_dir(&self, _path: &Path) -> Result<()> {
        unimplemented!("FtpFs::create_dir — implement with async-ftp MKD")
    }

    async fn remove(&self, _path: &Path) -> Result<()> {
        unimplemented!("FtpFs::remove — implement with async-ftp DELE/RMD")
    }

    async fn remove_all(&self, _path: &Path) -> Result<()> {
        unimplemented!(
            "FtpFs::remove_all — implement with recursive LIST + DELE/RMD"
        )
    }

    async fn rename(&self, _src: &Path, _dst: &Path) -> Result<()> {
        unimplemented!("FtpFs::rename — implement with async-ftp RNFR/RNTO")
    }

    async fn copy(&self, _src: &Path, _dst: &Path) -> Result<()> {
        unimplemented!(
            "FtpFs::copy — implement via read + write (FTP has no native \
             copy)"
        )
    }

    async fn read_link(&self, _path: &Path) -> Result<PathBuf> {
        Err(FsError::Io {
            operation: crate::error::FsOperation::ReadLink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
    }

    async fn symlink(&self, _target: &Path, _link: &Path) -> Result<()> {
        Err(FsError::Io {
            operation: crate::error::FsOperation::Symlink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
    }

    fn label(&self) -> &str {
        &self.label
    }
}

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

    async fn resolve_path(&self, _path: &Path) -> Result<NodeId<Self::Nid>> {
        unimplemented!(
            "FtpFs::resolve_path — implement with async-ftp STAT and node \
             cache"
        )
    }

    async fn resolve_symlink(
        &self,
        _id: NodeId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        Err(FsError::Io {
            operation: crate::error::FsOperation::ReadLink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
    }

    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>> {
        self.nodes
            .read()
            .get(id.get())
            .cloned()
            .ok_or(FsError::StaleNode)
    }

    fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata> {
        let node = self.get_node(id)?;
        Ok(node.metadata().clone())
    }

    fn set_node_hash(
        &self,
        _id: NodeId<Self::Nid>,
        _hash: String,
    ) -> Result<()> {
        unimplemented!(
            "FtpFs::set_node_hash — implement with Arc::make_mut on cached \
             node"
        )
    }

    async fn read_dir_node(&self, _id: DirId<Self::Nid>) -> Result<()> {
        unimplemented!(
            "FtpFs::read_dir_node — implement with async-ftp LIST and \
             populate node cache"
        )
    }

    async fn open_node(
        &self,
        _id: FileId<Self::Nid>,
        _mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        unimplemented!("FtpFs::open_node — implement with async-ftp RETR/STOR")
    }

    async fn read_node(
        &self,
        _id: FileId<Self::Nid>,
        _range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        unimplemented!("FtpFs::read_node — implement with async-ftp RETR")
    }

    async fn read_stream_node(
        &self,
        _id: FileId<Self::Nid>,
        _range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        unimplemented!(
            "FtpFs::read_stream_node — implement with async-ftp streaming \
             RETR"
        )
    }
}

#[async_trait]
impl WritableFileSystem for FtpFs {
    async fn create_file(
        &self,
        _parent: DirId<Self::Nid>,
        _name: &OsStr,
    ) -> Result<FileId<Self::Nid>> {
        unimplemented!(
            "FtpFs::create_file — implement with async-ftp STOR (zero-byte)"
        )
    }

    async fn create_dir_node(
        &self,
        _parent: DirId<Self::Nid>,
        _name: &OsStr,
    ) -> Result<DirId<Self::Nid>> {
        unimplemented!("FtpFs::create_dir_node — implement with async-ftp MKD")
    }

    async fn create_symlink(
        &self,
        _parent: DirId<Self::Nid>,
        _name: &OsStr,
        _target: &Path,
    ) -> Result<NodeId<Self::Nid>> {
        Err(FsError::Io {
            operation: crate::error::FsOperation::CreateSymlink,
            path: PathBuf::new(),
            message: "FTP does not support symbolic links".into(),
        })
    }

    async fn write_node(
        &self,
        _id: FileId<Self::Nid>,
        _data: Bytes,
    ) -> Result<()> {
        unimplemented!("FtpFs::write_node — implement with async-ftp STOR")
    }

    async fn flush_node(&self, _id: FileId<Self::Nid>) -> Result<()> {
        Ok(()) // FTP writes are streamed; no flush needed.
    }

    async fn remove_node(&self, _id: NodeId<Self::Nid>) -> Result<()> {
        unimplemented!(
            "FtpFs::remove_node — implement with async-ftp DELE/RMD"
        )
    }

    async fn remove_all_node(&self, _id: NodeId<Self::Nid>) -> Result<()> {
        unimplemented!(
            "FtpFs::remove_all_node — implement with recursive LIST + \
             DELE/RMD"
        )
    }

    async fn rename_node(
        &self,
        _id: NodeId<Self::Nid>,
        _new_name: &OsStr,
    ) -> Result<()> {
        unimplemented!(
            "FtpFs::rename_node — implement with async-ftp RNFR/RNTO"
        )
    }

    async fn copy_node(
        &self,
        _src: NodeId<Self::Nid>,
        _dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        unimplemented!("FtpFs::copy_node — implement via read + write")
    }

    async fn move_node(
        &self,
        _src: NodeId<Self::Nid>,
        _dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        unimplemented!("FtpFs::move_node — implement with async-ftp RNFR/RNTO")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ftp_config_smoke() {
        let config = FtpConfig {
            host: "ftp.example.com".into(),
            port: 21,
            username: "user".into(),
            password: "pass".into(),
            tls: false,
            root_path: None,
        };
        let fs = FtpFs::new("test-ftp", config);
        assert_eq!(fs.label(), "test-ftp");
    }

    #[test]
    fn ftpfs_id_is_deterministic() {
        let config = FtpConfig {
            host: "ftp.example.com".into(),
            port: 21,
            username: "a".into(),
            password: "b".into(),
            tls: false,
            root_path: None,
        };
        let fs1 = FtpFs::new("x", config.clone());
        let fs2 = FtpFs::new("y", config);
        assert_eq!(fs1.id(), fs2.id());
    }

    #[test]
    fn ftpfs_symlink_returns_error() {
        let config = FtpConfig {
            host: "ftp.example.com".into(),
            port: 21,
            username: "user".into(),
            password: "pass".into(),
            tls: false,
            root_path: None,
        };
        let fs = FtpFs::new("test-ftp", config);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result =
            rt.block_on(fs.symlink(Path::new("/target"), Path::new("/link")));
        assert!(result.is_err());
    }
}
