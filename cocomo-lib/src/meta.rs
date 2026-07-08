// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Metadata returned by stat-like operations. For local filesystems this
//! includes inode and device information for hard-link detection and
//! cycle-safe traversal.

use chrono::{DateTime, Utc};

use crate::node::UserPermissions;

/// File or directory metadata captured at stat time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Metadata {
    /// Size in bytes. For directories this is filesystem-dependent and may
    /// not reflect the total size of contents.
    pub size: u64,
    /// Creation time. `None` for remote providers that do not expose creation
    /// timestamps.
    pub created: Option<DateTime<Utc>>,
    /// Last modification time.
    pub modified: DateTime<Utc>,
    /// True when this entry is a directory.
    pub is_dir: bool,
    /// True when this entry is a symbolic link.
    pub is_symlink: bool,
    /// Inode number. Platform-specific; `None` for remote providers that
    /// do not expose inode information.
    pub inode: Option<u64>,
    /// Device ID. Platform-specific; used to track filesystem boundaries
    /// so inode-based cycle detection is scoped correctly per mount.
    pub device_id: Option<u64>,
    /// Access rights evaluated for the calling user/context.
    pub permissions: UserPermissions,
}

impl Metadata {
    /// Create new metadata with the given size and modification time for a
    /// regular file.
    pub fn file(size: u64, modified: DateTime<Utc>) -> Self {
        Self {
            size,
            created: None,
            modified,
            is_dir: false,
            is_symlink: false,
            inode: None,
            device_id: None,
            permissions: UserPermissions::READ | UserPermissions::WRITE,
        }
    }

    /// Create new metadata for a directory.
    pub fn dir(modified: DateTime<Utc>) -> Self {
        Self {
            size: 0,
            created: None,
            modified,
            is_dir: true,
            is_symlink: false,
            inode: None,
            device_id: None,
            permissions: UserPermissions::READ
                | UserPermissions::WRITE
                | UserPermissions::EXEC,
        }
    }

    /// Return `true` when this entry is a regular file (not a directory or
    /// symlink).
    pub fn is_file(&self) -> bool {
        !self.is_dir && !self.is_symlink
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_metadata() {
        let m = Metadata::file(100, Utc::now());
        assert!(m.is_file());
        assert!(!m.is_dir);
        assert!(!m.is_symlink);
        assert_eq!(m.size, 100);
    }

    #[test]
    fn dir_metadata() {
        let m = Metadata::dir(Utc::now());
        assert!(m.is_dir);
        assert!(!m.is_file());
    }
}
