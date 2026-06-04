// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

pub(crate) mod error;
pub(crate) mod fs;
pub(crate) mod fsitem;
pub(crate) mod traits;

pub use crate::backends::localfs::LocalFs;
pub use crate::backends::VfsBackend;
pub use error::VfsError;
pub use fs::VfsImpl;
pub use fsitem::{FSItem, FSItemKind};
pub use traits::{Vfs, VfsItem};
