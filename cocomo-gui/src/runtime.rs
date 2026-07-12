// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Tokio runtime integration for the GUI.
//!
//! Gpui uses its own executor, but `cocomo-lib` relies on `tokio::fs` for
//! non-blocking IO. This module stores the tokio runtime handle globally and
//! provides helpers to bridge between tokio tasks and gpui tasks.
//!
//! The key insight is that awaiting a tokio `JoinHandle` does not require a
//! tokio reactor — it just polls the task that is running on tokio's
//! executor. So we can spawn the work on tokio and then await the handle
//! inside a gpui `Task`, unifying both executors without blocking.

use std::sync::OnceLock;

use anyhow::Result;
use tokio::runtime::Handle;

/// Global tokio runtime handle, initialized at startup.
static HANDLE: OnceLock<Handle> = OnceLock::new();

/// Store the current tokio runtime handle for later use.
///
/// Must be called once before any `spawn_on_tokio` calls, typically at
/// application startup while a tokio runtime is active on the current
/// thread (i.e., inside `rt.enter()` scope).
pub fn set_handle() {
    let _ = HANDLE.set(Handle::current());
}

/// Return the globally stored tokio runtime handle.
///
/// Panics if `set_handle` was not called previously.
pub fn handle() -> &'static Handle {
    HANDLE.get().expect("tokio runtime handle not initialized")
}

/// Spawn a future on the tokio runtime and return a `JoinHandle` that can
/// be awaited from within a gpui task.
///
/// The future runs on tokio's executor where the reactor is available for
/// `tokio::fs` operations. The returned `JoinHandle` can be awaited inside
/// a gpui `Task` (e.g., via `cx.background_spawn`), because awaiting a
/// `JoinHandle` does not require a tokio reactor — it just polls the task
/// running on tokio's executor.
pub fn spawn_on_tokio<T, F>(future: F) -> tokio::task::JoinHandle<Result<T>>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T>> + Send + 'static,
{
    handle().spawn(future)
}
