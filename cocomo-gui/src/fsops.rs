// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Local filesystem operations backing the folder view's context menu.
//!
//! The comparison and transfer engines live in `cocomo-lib`; the row
//! commands of spec section 4.4 (Touch, Rename, Exchange, New Folder,
//! Copy/Move to Folder) are plain filesystem edits. They run on the
//! shared tokio runtime through `fs_err` so errors carry the path that
//! failed. Remote endpoints are rejected with a clear message rather
//! than silently doing nothing.

use std::{path::Path, time::SystemTime};

use cocomo_lib::Endpoint;

/// Human-readable error for the status bar and the log.
pub(crate) type FsResult = Result<(), String>;

/// Reject non-local endpoints with a message naming the provider.
fn require_local(endpoint: &Endpoint, what: &str) -> Result<(), String> {
    if endpoint.is_local() {
        Ok(())
    } else {
        Err(format!("{what} is only supported for local folders"))
    }
}

/// Set the modification time of `endpoint` to now.
///
/// Directories are opened read-only (a write open fails on macOS);
/// `futimens` on the resulting descriptor updates the timestamp for both
/// files and directories. The open+set pair is a short blocking call,
/// acceptable on the tokio runtime that already serves the comparison.
pub(crate) async fn touch(endpoint: Endpoint) -> FsResult {
    require_local(&endpoint, "Touch")?;
    let path = endpoint.path();
    let is_dir = fs_err::tokio::metadata(path)
        .await
        .map(|m| m.is_dir())
        .map_err(|e| format!("Touch failed: {e}"))?;
    let mut options = std::fs::File::options();
    if is_dir {
        options.read(true);
    } else {
        options.write(true);
    }
    options
        .open(path)
        .and_then(|file| file.set_modified(SystemTime::now()))
        .map_err(|e| format!("Touch failed: {e}"))
}

/// Rename the entry at `endpoint` to `new_name` (same parent).
pub(crate) async fn rename(endpoint: Endpoint, new_name: String) -> FsResult {
    require_local(&endpoint, "Rename")?;
    let from = endpoint.path().to_path_buf();
    let Some(parent) = from.parent() else {
        return Err("Rename failed: the entry has no parent folder".into());
    };
    let to = parent.join(&new_name);
    fs_err::tokio::rename(&from, &to)
        .await
        .map_err(|e| format!("Rename of {} failed: {e}", from.display()))
}

/// Swap the contents of the two endpoints (left ↔ right).
///
/// A three-step rename through a temporary name in the left parent;
/// both sides must be local and both entries must exist.
pub(crate) async fn exchange(left: Endpoint, right: Endpoint) -> FsResult {
    require_local(&left, "Exchange")?;
    require_local(&right, "Exchange")?;
    let a = left.path().to_path_buf();
    let b = right.path().to_path_buf();
    let tmp = a.with_extension("cocomo-exchange-tmp");
    let steps = async {
        fs_err::tokio::rename(&a, &tmp).await?;
        match fs_err::tokio::rename(&b, &a).await {
            Ok(()) => {}
            Err(e) => {
                // Put the first rename back so neither side loses data.
                let _ = fs_err::tokio::rename(&tmp, &a).await;
                return Err(e);
            }
        }
        fs_err::tokio::rename(&tmp, &b).await
    };
    steps.await.map_err(|e| format!("Exchange failed: {e}"))
}

/// Create a uniquely named new folder inside `endpoint`.
///
/// Returns the created name; the caller reloads the comparison.
pub(crate) async fn create_folder(
    endpoint: Endpoint,
) -> Result<String, String> {
    require_local(&endpoint, "New Folder")?;
    let root = endpoint.path().to_path_buf();
    let mut name = "New Folder".to_string();
    let mut n = 1;
    while fs_err::tokio::symlink_metadata(root.join(&name))
        .await
        .is_ok()
    {
        name = format!("New Folder ({n})");
        n += 1;
    }
    fs_err::tokio::create_dir_all(root.join(&name))
        .await
        .map_err(|e| format!("New Folder failed: {e}"))?;
    Ok(name)
}

/// Copy or move the entry at `source` into the folder `dest_dir`.
pub(crate) async fn copy_to_folder(
    source: Endpoint,
    dest_dir: std::path::PathBuf,
    move_entry: bool,
) -> FsResult {
    require_local(&source, if move_entry { "Move" } else { "Copy" })?;
    let from = source.path().to_path_buf();
    let Some(name) = from.file_name().map(|n| n.to_os_string()) else {
        return Err("The entry has no name to copy".into());
    };
    let to = dest_dir.join(name);
    let verb = if move_entry { "Move" } else { "Copy" };
    if move_entry {
        // Prefer a same-filesystem rename; fall back to copy+delete.
        if fs_err::tokio::rename(&from, &to).await.is_ok() {
            return Ok(());
        }
    }
    copy_tree(&from, &to)
        .await
        .map_err(|e| format!("{verb} to folder failed: {e}"))?;
    if move_entry {
        let _ = fs_err::tokio::remove_dir_all(&from).await;
        let _ = fs_err::tokio::remove_file(&from).await;
    }
    Ok(())
}

/// Recursively copy a file or directory tree.
async fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = fs_err::tokio::symlink_metadata(from).await?;
    if meta.is_dir() {
        fs_err::tokio::create_dir_all(to).await?;
        let mut entries = fs_err::tokio::read_dir(from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            Box::pin(copy_tree(&entry.path(), &to.join(name))).await?;
        }
        Ok(())
    } else {
        fs_err::tokio::copy(from, to).await?;
        Ok(())
    }
}

/// Open `endpoint` in the platform's file explorer.
pub(crate) fn reveal_in_explorer(endpoint: &Endpoint) -> FsResult {
    let path = endpoint.path();
    let mut command = {
        #[cfg(target_os = "macos")]
        {
            let mut c = std::process::Command::new("open");
            c.arg(path);
            c
        }
        #[cfg(target_os = "linux")]
        {
            let mut c = std::process::Command::new("xdg-open");
            c.arg(path);
            c
        }
        #[cfg(target_os = "windows")]
        {
            let mut c = std::process::Command::new("explorer");
            c.arg(path);
            c
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )))]
        {
            return Err(
                "Opening a folder is not supported on this platform".into()
            );
        }
    };
    command
        .spawn()
        .map_err(|e| format!("Couldn't open the folder: {e}"))?;
    Ok(())
}
