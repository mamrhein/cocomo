// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Async file handle trait. The path is resolved at construction time; all
//! subsequent operations go through this handle to avoid TOCTOU races.

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::meta::Metadata;

/// A handle to an opened file. The path is resolved at construction time;
/// all subsequent operations go through this handle to avoid TOCTOU races.
///
/// Implementors must also satisfy `AsyncRead` and `AsyncWrite` so that
/// consumers can read and write through the same abstraction regardless of
/// the underlying provider.
#[async_trait]
pub trait FsFile: Send + Unpin + AsyncRead + AsyncWrite {
    /// Cached metadata captured at open time.
    fn metadata(&self) -> &Metadata;

    /// Flush any buffered writes.
    async fn flush(&mut self) -> crate::error::Result<()>;
}
