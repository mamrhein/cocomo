// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! In-memory `FileSystem` double for tests.
//!
//! [`MockFs`] is a read-only, path-based `FileSystem` implementation backed by
//! a `BTreeMap` of predefined entries plus two error-injection maps. It lets
//! provider-agnostic logic (scanning, comparison, hashing) be tested against
//! per-operation failures such as `PermissionDenied`, which cannot be
//! reproduced reliably on a real filesystem.
//!
//! `MockFs` implements only the read half of the `FileSystem` trait: every
//! mutating operation returns `FsError::Io` with a "not supported" message, so
//! tests never mask bugs in code that assumes a mutable backend.

use std::{
    collections::{BTreeMap, HashMap},
    io,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use futures::Stream;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::{
    error::{FsError, FsOperation, Result},
    file::FsFile,
    fs::{DirEntryMeta, DirStream, FileSystem, OpenMode},
    meta::Metadata,
};

/// Number of bytes `MockFs::read_stream` emits per chunk.
const CHUNK_SIZE: usize = 8192;

// ---------------------------------------------------------------------------
// MockTree
// ---------------------------------------------------------------------------

/// A node of the [`MockTree`] map.
#[derive(Clone, Debug)]
enum MockNode {
    /// A directory (without content).
    Dir,
    /// A regular file with the given content.
    File(Bytes),
}

/// The predefined contents and injected errors behind a [`MockFs`].
#[derive(Clone, Debug)]
struct MockTree {
    /// Known absolute paths and their kind or content.
    nodes: BTreeMap<PathBuf, MockNode>,
    /// Errors returned instead of performing *any* operation on the key.
    errors: HashMap<PathBuf, FsError>,
    /// Errors emitted as the first item of `read_stream` for the key.
    stream_errors: HashMap<PathBuf, FsError>,
}

impl MockTree {
    fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            errors: HashMap::new(),
            stream_errors: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// MockFs
// ---------------------------------------------------------------------------

/// A read-only, in-memory `FileSystem` for tests.
///
/// Build the tree with the `with_*` builder methods, then pass the instance
/// to anything that takes a `&Arc<dyn FileSystem>`:
///
/// ```text
/// let fs: Arc<dyn FileSystem> = Arc::new(
///     MockFs::new("mock")
///         .with_dir("/root")
///         .with_file("/root/a.txt", "alpha")
///         .with_error("/root/locked", FsError::PermissionDenied { .. }),
/// );
/// ```
#[derive(Clone, Debug)]
pub struct MockFs {
    label: String,
    tree: MockTree,
}

impl MockFs {
    /// Create a new, empty mock filesystem with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            tree: MockTree::new(),
        }
    }

    /// Add a directory node at `path`.
    #[must_use]
    pub fn with_dir(self, path: impl AsRef<Path>) -> Self {
        self.with_node(path, MockNode::Dir)
    }

    /// Add a file node with the given content at `path`.
    #[must_use]
    pub fn with_file(
        self,
        path: impl AsRef<Path>,
        content: impl Into<Bytes>,
    ) -> Self {
        self.with_node(path, MockNode::File(content.into()))
    }

    /// Make *every* operation on `path` return `error` instead of succeeding.
    #[must_use]
    pub fn with_error(
        mut self,
        path: impl AsRef<Path>,
        error: FsError,
    ) -> Self {
        self.tree.errors.insert(path.as_ref().to_path_buf(), error);
        self
    }

    /// Make `read_stream` on `path` yield `error` as its first stream item.
    #[must_use]
    pub fn with_stream_error(
        mut self,
        path: impl AsRef<Path>,
        error: FsError,
    ) -> Self {
        self.tree
            .stream_errors
            .insert(path.as_ref().to_path_buf(), error);
        self
    }

    fn with_node(mut self, path: impl AsRef<Path>, node: MockNode) -> Self {
        self.tree.nodes.insert(path.as_ref().to_path_buf(), node);
        self
    }

    /// The injected error for `path`, if one was registered.
    fn error_at(&self, path: &Path) -> Option<FsError> {
        self.tree.errors.get(path).cloned()
    }

    /// The content of the file at `path`, if one was registered.
    fn content(&self, path: &Path) -> Option<Bytes> {
        match self.tree.nodes.get(path) {
            Some(MockNode::File(data)) => Some(data.clone()),
            _ => None,
        }
    }

    /// Metadata of the direct children of `dir`, in path order.
    fn children_of(&self, dir: &Path) -> Vec<DirEntryMeta> {
        self.tree
            .nodes
            .iter()
            .filter(|(path, _)| path.parent() == Some(dir))
            .map(|(path, node)| {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let meta = match node {
                    MockNode::Dir => Metadata::dir(Utc::now()),
                    MockNode::File(data) => {
                        Metadata::file(data.len() as u64, Utc::now())
                    }
                };
                DirEntryMeta { name, meta }
            })
            .collect()
    }
}

/// Fail an unsupported operation with `FsError::Io` and "not supported".
fn unsupported(operation: FsOperation, path: &Path) -> FsError {
    FsError::Io {
        operation,
        path: path.to_path_buf(),
        message: "operation not supported on MockFs".to_string(),
    }
}

#[async_trait]
impl FileSystem for MockFs {
    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        match self.tree.nodes.get(path) {
            Some(MockNode::Dir) => Ok(Metadata::dir(Utc::now())),
            Some(MockNode::File(data)) => {
                Ok(Metadata::file(data.len() as u64, Utc::now()))
            }
            None => Err(FsError::NotFound {
                path: path.to_path_buf(),
            }),
        }
    }

    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        match self.tree.nodes.get(path) {
            Some(MockNode::Dir) => {
                let entries: Vec<_> =
                    self.children_of(path).into_iter().map(Ok).collect();
                let stream: DirStream<'_> =
                    Box::pin(futures::stream::iter(entries));
                Ok(stream)
            }
            Some(MockNode::File(_)) => Err(FsError::WrongKind {
                expected: "directory",
                actual: "file",
            }),
            None => Err(FsError::NotFound {
                path: path.to_path_buf(),
            }),
        }
    }

    async fn open(
        &self,
        path: &Path,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        if mode != OpenMode::Read {
            return Err(unsupported(FsOperation::Open, path));
        }
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        match self.tree.nodes.get(path) {
            Some(MockNode::File(data)) => {
                Ok(Box::new(MockFile::new(data.clone())))
            }
            _ => Err(FsError::NotFound {
                path: path.to_path_buf(),
            }),
        }
    }

    async fn read(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        let Some(data) = self.content(path) else {
            return Err(FsError::NotFound {
                path: path.to_path_buf(),
            });
        };
        let Some(r) = range else {
            return Ok(data);
        };
        // Mirror `LocalFs::read`: bounds are clamped to the content length
        // and inverted or empty ranges are rejected before slicing.
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
        let len = data.len() as u64;
        if r.start >= len {
            return Ok(Bytes::new());
        }
        let end = usize::try_from(std::cmp::min(r.end, len)).unwrap_or(0);
        Ok(data.slice(r.start as usize..end))
    }

    async fn read_stream(
        &self,
        path: &Path,
        _range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        if let Some(err) = self.tree.stream_errors.get(path) {
            let stream: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>> =
                Box::pin(futures::stream::iter([Err(err.clone())]));
            return Ok(stream);
        }
        let Some(mut data) = self.content(path) else {
            return Err(FsError::NotFound {
                path: path.to_path_buf(),
            });
        };
        // Chunk the content like a real provider so that streaming consumers
        // observe more than one item for larger files.
        let mut chunks = Vec::new();
        while !data.is_empty() {
            let n = usize::min(CHUNK_SIZE, data.len());
            chunks.push(Ok(data.split_to(n)));
        }
        let stream: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>> =
            Box::pin(futures::stream::iter(chunks));
        Ok(stream)
    }

    async fn write(&self, path: &Path, _data: Bytes) -> Result<()> {
        Err(unsupported(FsOperation::Write, path))
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        Err(unsupported(FsOperation::CreateDir, path))
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        Err(unsupported(FsOperation::Remove, path))
    }

    async fn remove_all(&self, path: &Path) -> Result<()> {
        Err(unsupported(FsOperation::Remove, path))
    }

    async fn rename(&self, src: &Path, _dst: &Path) -> Result<()> {
        Err(unsupported(FsOperation::Rename, src))
    }

    async fn copy(&self, src: &Path, _dst: &Path) -> Result<()> {
        Err(unsupported(FsOperation::Copy, src))
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        Err(unsupported(FsOperation::ReadLink, path))
    }

    async fn symlink(&self, _target: &Path, link: &Path) -> Result<()> {
        Err(unsupported(FsOperation::Symlink, link))
    }

    fn label(&self) -> &str {
        &self.label
    }
}

// ---------------------------------------------------------------------------
// MockFile
// ---------------------------------------------------------------------------

/// A read-only `FsFile` handle over an in-memory `Bytes` buffer.
#[derive(Clone, Debug)]
pub struct MockFile {
    meta: Metadata,
    data: Bytes,
    /// Read cursor. Kept separate from `data` on purpose: wrapping a
    /// `std::io::Cursor` would make `read` return `WriteZero` when polled
    /// with a full buffer, which hangs `read_to_end`.
    pos: usize,
}

impl MockFile {
    /// Create a new handle on the given file content.
    pub fn new(data: Bytes) -> Self {
        Self {
            meta: Metadata::file(data.len() as u64, Utc::now()),
            data,
            pos: 0,
        }
    }
}

#[async_trait]
impl FsFile for MockFile {
    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    async fn flush(&mut self) -> Result<()> {
        // Nothing is ever buffered, so flushing is trivially successful.
        Ok(())
    }
}

impl AsyncRead for MockFile {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let rest = &me.data[me.pos..];
        let n = usize::min(rest.len(), buf.remaining());
        buf.put_slice(&rest[..n]);
        me.pos += n;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for MockFile {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("MockFile is read-only")))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::hash::{hash_bytes, hash_file};

    #[tokio::test]
    async fn metadata_reflects_tree() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_file("/root/a.txt", "alpha");
        assert!(fs.metadata(Path::new("/root")).await.unwrap().is_dir);
        let file = fs.metadata(Path::new("/root/a.txt")).await.unwrap();
        assert!(file.is_file());
        assert_eq!(file.size, 5);
        assert!(matches!(
            fs.metadata(Path::new("/root/missing.txt")).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn injected_error_surfaces_on_all_operations() {
        let err = FsError::PermissionDenied {
            operation: FsOperation::Read,
            path: "/secret".into(),
        };
        let fs = MockFs::new("mock")
            .with_file("/secret", "x")
            .with_error("/secret", err);
        assert!(matches!(
            fs.metadata(Path::new("/secret")).await,
            Err(FsError::PermissionDenied { .. })
        ));
        assert!(fs.read_dir(Path::new("/secret")).await.is_err());
        assert!(fs.read(Path::new("/secret"), None).await.is_err());
        assert!(fs.read_stream(Path::new("/secret"), None).await.is_err());
    }

    #[tokio::test]
    async fn read_dir_lists_direct_children() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/sub")
            .with_file("/root/sub/b.txt", "bravo")
            .with_file("/root/a.txt", "alpha");
        let mut stream = fs.read_dir(Path::new("/root")).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = stream.next().await {
            names.push(entry.unwrap().name);
        }
        assert_eq!(names, ["a.txt", "sub"]);
    }

    #[tokio::test]
    async fn read_clamps_and_rejects_ranges() {
        let fs = MockFs::new("mock").with_file("/data.txt", "hello world");
        // Clamped to the content length.
        assert_eq!(
            fs.read(Path::new("/data.txt"), Some(8..100)).await.unwrap(),
            Bytes::from_static(b"rld")
        );
        // Entirely past EOF.
        assert!(
            fs.read(Path::new("/data.txt"), Some(100..200))
                .await
                .unwrap()
                .is_empty()
        );
        // Inverted range is rejected, mirroring `LocalFs::read`.
        #[allow(clippy::reversed_empty_ranges)]
        let err = fs
            .read(Path::new("/data.txt"), Some(10..5))
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::InvalidArgument { .. }));
    }

    #[tokio::test]
    async fn stream_error_surfaces_in_hash_file() {
        let fs = MockFs::new("mock")
            .with_file("/broken.bin", vec![0u8; CHUNK_SIZE * 2 + 3])
            .with_stream_error(
                "/broken.bin",
                FsError::Io {
                    operation: FsOperation::Read,
                    path: "/broken.bin".into(),
                    message: "injected".to_string(),
                },
            );
        let err = hash_file(&fs, Path::new("/broken.bin")).await.unwrap_err();
        assert!(matches!(err, FsError::Io { .. }));
    }

    #[tokio::test]
    async fn hash_file_reads_every_chunk() {
        let data = Bytes::from_static(b"hello world");
        let fs = MockFs::new("mock").with_file("/f.txt", data.clone());
        let hash = hash_file(&fs, Path::new("/f.txt")).await.unwrap();
        assert_eq!(hash, hash_bytes(&data));
    }

    #[tokio::test]
    async fn file_handle_reads_to_end() {
        let fs = MockFs::new("mock").with_file("/f.txt", "hello");
        let mut handle =
            fs.open(Path::new("/f.txt"), OpenMode::Read).await.unwrap();
        let mut buf = Vec::new();
        handle.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello");
        // Writing through a read-only handle fails.
        assert!(handle.write_all(b"more").await.is_err());
    }
}
