// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

pub(crate) mod error;
pub(crate) mod items;
pub(crate) mod traits;
pub(crate) mod types;

pub use crate::backends::localfs::LocalFs;
pub use error::VfsError;
pub use items::{DirItem, FileItem, FSItem, InvalidItem, SpecialItem, SymlinkItem};
pub use traits::{Vfs, VfsBackend, VfsDirectory, VfsFile, VfsItem, VfsSpecial, VfsSymlink};
pub use types::{DirEntry, FileKind, FSItemKind, Metadata};
