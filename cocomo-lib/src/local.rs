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
    fs, io,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
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
    error::{Result, wrap},
    file::FsFile,
    fs::{DirEntryMeta, DirStream, FileSystem, OpenMode},
    meta::Metadata,
    node::UserPermissions,
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

/// Local filesystem provider.
///
/// Uses `fs_err::tokio` for filesystem operations to get rich error context,
/// and `walkdir` for directory traversal with inode-based cycle detection.
pub struct LocalFs {
    /// Human-readable label for this provider instance.
    label: String,
}

impl LocalFs {
    /// Create a new `LocalFs` with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
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
        range: Option<Range<usize>>,
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
            if (r.start as u64) >= file_size {
                return Ok(Bytes::new());
            }
            let end = std::cmp::min(r.end as u64, file_size);
            let len = (end - (r.start as u64)) as usize;

            use tokio::io::AsyncSeekExt;
            file.seek(io::SeekFrom::Start(r.start as u64))
                .await
                .map_err(|e| {
                    wrap(
                        e,
                        crate::error::FsOperation::Read,
                        path.to_path_buf(),
                    )
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
        _range: Option<Range<usize>>,
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
}
