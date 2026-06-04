// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

#![allow(async_fn_in_trait)]

use std::{
    collections::HashMap,
    io,
    path::{Component, Path, PathBuf},
    sync::RwLock,
    time::SystemTime,
};

use crate::backends::{DirEntry, DirEntryKind, Metadata, VfsBackend};
use crate::vfs::VfsError;

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum InodeKind {
    File(Vec<u8>),
    Directory,
    Symlink(PathBuf),
}

#[derive(Debug, Clone)]
struct Inode {
    kind: InodeKind,
    modified: SystemTime,
    readonly: bool,
}

// ---------------------------------------------------------------------------
// InMemoryFS
// ---------------------------------------------------------------------------

/// A [`VfsBackend`] that stores all data in memory.
///
/// No actual I/O is performed.  Useful for testing and mocking.
#[derive(Debug)]
pub struct InMemoryFS {
    inodes: RwLock<HashMap<PathBuf, Inode>>,
}

impl Default for InMemoryFS {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryFS {
    /// Creates a new empty in-memory filesystem with a root directory `/`.
    pub fn new() -> Self {
        let mut inodes = HashMap::new();
        inodes.insert(
            PathBuf::from("/"),
            Inode {
                kind: InodeKind::Directory,
                modified: SystemTime::UNIX_EPOCH,
                readonly: false,
            },
        );
        Self {
            inodes: RwLock::new(inodes),
        }
    }

    fn inode_meta(inode: &Inode) -> Metadata {
        let kind = match &inode.kind {
            InodeKind::File(_) => DirEntryKind::File,
            InodeKind::Directory => DirEntryKind::Directory,
            InodeKind::Symlink(_) => DirEntryKind::Symlink,
        };
        let len = match &inode.kind {
            InodeKind::File(c) => c.len() as u64,
            _ => 0,
        };
        Metadata {
            kind,
            len,
            modified: Some(inode.modified),
            readonly: inode.readonly,
        }
    }

    fn not_found(path: &Path) -> VfsError {
        VfsError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no such entry: {}", path.display()),
        ))
    }

    fn is_dir(inode: &Inode) -> bool {
        matches!(inode.kind, InodeKind::Directory)
    }

    fn is_file(inode: &Inode) -> bool {
        matches!(inode.kind, InodeKind::File(_))
    }

    fn resolve_target(&self, from: &Path, to: &Path) -> PathBuf {
        let inodes = self.inodes.read().unwrap();
        if let Some(tgt) = inodes.get(to)
            && Self::is_dir(tgt)
            && let Some(name) = from.file_name()
        {
            return to.join(name);
        }
        to.to_path_buf()
    }

    fn copy_file_inodes(
        &self,
        from: &Path,
        to: &Path,
    ) -> Result<(), VfsError> {
        let src = {
            let inodes = self.inodes.read().unwrap();
            inodes
                .get(from)
                .ok_or_else(|| Self::not_found(from))?
                .clone()
        };
        let mut inodes = self.inodes.write().unwrap();
        inodes.insert(to.to_path_buf(), src);
        Ok(())
    }

    fn copy_dir_recursive(&self, from: &Path, to: &Path) -> Result<(), VfsError> {
        let snapshot = {
            let inodes = self.inodes.read().unwrap();
            inodes
                .iter()
                .filter(|(p, _)| p.starts_with(from))
                .map(|(p, i)| {
                    let rel = p.strip_prefix(from).unwrap_or(p);
                    (to.join(rel), i.clone())
                })
                .collect::<Vec<_>>()
        };
        let mut inodes = self.inodes.write().unwrap();
        for (path, inode) in snapshot {
            inodes.insert(path, inode);
        }
        Ok(())
    }

    fn remove_tree(&self, path: &Path) {
        let mut inodes = self.inodes.write().unwrap();
        let targets: Vec<PathBuf> = inodes
            .keys()
            .filter(|p| *p == path || p.starts_with(path))
            .cloned()
            .collect();
        for p in targets {
            inodes.remove(&p);
        }
    }

    fn collect_children(&self, dir: &Path) -> Vec<PathBuf> {
        let inodes = self.inodes.read().unwrap();
        let mut children: Vec<PathBuf> = inodes
            .keys()
            .filter(|p| p.parent() == Some(dir))
            .cloned()
            .collect();
        children.sort();
        children
    }
}

// ---------------------------------------------------------------------------
// VfsBackend implementation
// ---------------------------------------------------------------------------

impl VfsBackend for InMemoryFS {
    async fn symlink_metadata(&self, path: &Path) -> Result<Metadata, VfsError> {
        let inodes = self.inodes.read().unwrap();
        inodes
            .get(path)
                .map(Self::inode_meta)
            .ok_or_else(|| Self::not_found(path))
    }

    async fn metadata(&self, path: &Path) -> Result<Metadata, VfsError> {
        let inodes = self.inodes.read().unwrap();
        let mut current = path.to_path_buf();
        let mut hops = 0;
        while hops < 32 {
            match inodes.get(current.as_path()) {
                Some(Inode {
                    kind: InodeKind::Symlink(target),
                    ..
                }) => {
                    current = if target.is_absolute() {
                        target.clone()
                    } else {
                        let parent = current.parent().unwrap_or(Path::new("/"));
                        parent.join(target)
                    };
                    hops += 1;
                }
                Some(inode) => return Ok(Self::inode_meta(inode)),
                None => return Err(Self::not_found(path)),
            }
        }
        Err(VfsError::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "too many symlink hops",
        )))
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let inodes = self.inodes.read().unwrap();
        match inodes.get(path) {
            Some(Inode {
                kind: InodeKind::Symlink(target),
                ..
            }) => Ok(target.clone()),
            _ => Err(Self::not_found(path)),
        }
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let meta = self.symlink_metadata(path).await?;
        if meta.kind != DirEntryKind::Directory {
            return Err(VfsError::NotADirectory(path.to_path_buf()));
        }
        let children = self.collect_children(path);
        let mut entries = Vec::with_capacity(children.len());
        let inodes = self.inodes.read().unwrap();
        for child_path in &children {
            if let Some(inode) = inodes.get(child_path) {
                let name = child_path.file_name().unwrap_or(child_path.as_os_str());
                entries.push(DirEntry {
                    name: name.to_os_string(),
                    path: child_path.clone(),
                    metadata: Self::inode_meta(inode),
                    mime_type: None,
                });
            }
        }
        Ok(entries)
    }

    async fn read_to_string(&self, path: &Path) -> Result<String, VfsError> {
        let inodes = self.inodes.read().unwrap();
        match inodes.get(path) {
            Some(Inode {
                kind: InodeKind::File(content),
                ..
            }) => {
                let s = String::from_utf8_lossy(content).into_owned();
                Ok(s)
            }
            Some(_) => Err(VfsError::Unsupported(format!(
                "not a file: {}",
                path.display()
            ))),
            None => Err(Self::not_found(path)),
        }
    }

    async fn read_head(&self, path: &Path, n: usize) -> Result<Vec<u8>, VfsError> {
        let inodes = self.inodes.read().unwrap();
        match inodes.get(path) {
            Some(Inode {
                kind: InodeKind::File(content),
                ..
            }) => {
                let end = n.min(content.len());
                Ok(content[..end].to_vec())
            }
            Some(_) => Err(VfsError::Unsupported(format!(
                "not a file: {}",
                path.display()
            ))),
            None => Err(Self::not_found(path)),
        }
    }

    async fn canonicalize(&self, path: &Path) -> Result<PathBuf, VfsError> {
        let inodes = self.inodes.read().unwrap();
        // Simple check: if path (or its normalized form) exists, return it.
        if inodes.contains_key(path) {
            return Ok(path.to_path_buf());
        }
        // Try normalising . and .. components.
        let normalized = normalize_path(path);
        if inodes.contains_key(&normalized) {
            return Ok(normalized);
        }
        Err(Self::not_found(path))
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<(), VfsError> {
        let src_is_dir = {
            let inodes = self.inodes.read().unwrap();
            let src = inodes
                .get(from)
                .ok_or_else(|| Self::not_found(from))?;
            Self::is_dir(src)
        };
        let target = self.resolve_target(from, to);
        if src_is_dir {
            self.copy_dir_recursive(from, &target)
        } else {
            self.copy_file_inodes(from, &target)
        }
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<(), VfsError> {
        // Ensure source exists.
        {
            let inodes = self.inodes.read().unwrap();
            inodes
                .get(from)
                .ok_or_else(|| Self::not_found(from))?;
        }
        let target = self.resolve_target(from, to);
        // Move all entries rooted at `from` to `target`.
        let snapshot = {
            let inodes = self.inodes.read().unwrap();
            inodes
                .iter()
                .filter(|(p, _)| p.starts_with(from))
                .map(|(p, i)| {
                    let rel = p.strip_prefix(from).unwrap_or(p);
                    (p.clone(), target.join(rel), i.clone())
                })
                .collect::<Vec<_>>()
        };
        let mut inodes = self.inodes.write().unwrap();
        for (old_path, new_path, inode) in snapshot {
            inodes.remove(&old_path);
            inodes.insert(new_path, inode);
        }
        Ok(())
    }

    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        let mut inodes = self.inodes.write().unwrap();
        match inodes.get(path) {
            Some(Inode {
                kind: InodeKind::File(_),
                ..
            }) => {
                inodes.remove(path);
                Ok(())
            }
            Some(_) => Err(VfsError::Unsupported(format!(
                "not a file: {}",
                path.display()
            ))),
            None => Err(Self::not_found(path)),
        }
    }

    async fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        let meta = self.symlink_metadata(path).await?;
        if meta.kind != DirEntryKind::Directory {
            return Err(VfsError::Unsupported(format!(
                "not a directory: {}",
                path.display()
            )));
        }
        self.remove_tree(path);
        Ok(())
    }

    async fn create_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        let mut inodes = self.inodes.write().unwrap();
        let mut current = PathBuf::new();
        for component in path.components() {
            match component {
                Component::RootDir => {
                    current.push("/");
                }
                Component::CurDir => {}
                Component::ParentDir => {
                    current.pop();
                }
                Component::Normal(_) => {
                    current.push(component);
                    if !inodes.contains_key(&current) {
                        inodes.insert(
                            current.clone(),
                            Inode {
                                kind: InodeKind::Directory,
                                modified: SystemTime::now(),
                                readonly: false,
                            },
                        );
                    }
                }
                Component::Prefix(_) => {
                    current.push(component.as_os_str());
                }
            }
        }
        Ok(())
    }
}

/// Normalise a path by resolving `.` and `..` components.
fn normalize_path(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => result.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            Component::Normal(c) => result.push(c),
            Component::Prefix(_) => result.push(component.as_os_str()),
        }
    }
    result
}
