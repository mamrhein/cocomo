// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Filesystem operations backing the folder view's context menu.
//!
//! The comparison and transfer engines live in `cocomo-lib`; the row
//! commands of spec section 4.4 (Touch, Rename, Exchange, New Folder,
//! Copy/Move to Folder) are plain filesystem edits. They run on the
//! shared tokio runtime so errors carry the path that failed: local
//! sides go through `fs_err`, remote sides through the `cocomo-lib`
//! provider traits. An operation a provider cannot support (setting a
//! modification time, exchanging across providers) is rejected with a
//! clear message rather than silently doing nothing.

use std::{ffi::OsStr, fs::Metadata, path::Path, time::SystemTime};

use cocomo_lib::{
    CopyOptions, DirId, Endpoint, FsError, NodeFileSystem, NodeId, NodeKind,
    Provider, WritableFileSystem,
};

/// Human-readable error for the status bar and the log.
pub(crate) type FsResult = Result<(), String>;

/// Reject operations no provider implements for remote sides.
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
///
/// The provider traits expose no set-modification-time operation, so a
/// remote side reports a clear error instead of a silent no-op.
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
///
/// Local sides rename through the OS; a remote side resolves its path to
/// a node and renames the node in place, which every writable provider
/// (`FTP`, `SFTP`) implements.
pub(crate) async fn rename(endpoint: Endpoint, new_name: String) -> FsResult {
    if !endpoint.is_local() {
        let fs = endpoint.fs();
        let id = fs
            .resolve_path(endpoint.path())
            .await
            .map_err(|e| rename_err(&endpoint, &e))?;
        return fs
            .rename_node(id, OsStr::new(&new_name))
            .await
            .map_err(|e| rename_err(&endpoint, &e));
    }
    let from = endpoint.path().to_path_buf();
    let Some(parent) = from.parent() else {
        return Err("Rename failed: the entry has no parent folder".into());
    };
    let to = parent.join(&new_name);
    fs_err::tokio::rename(&from, &to)
        .await
        .map_err(|e| format!("Rename of {} failed: {e}", from.display()))
}

/// Rename failure carrying the endpoint's display path.
fn rename_err(endpoint: &Endpoint, e: &FsError) -> String {
    format!("Rename of {} failed: {e}", endpoint.display())
}

/// Swap the contents of the two endpoints (left ↔ right).
///
/// A three-step rename through a temporary name in the left parent.
/// Renaming across two different providers has no atomic equivalent, so
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
    let mut name = "New Folder".to_string();
    let mut n = 1;
    while child_exists(&endpoint, &name).await? {
        name = format!("New Folder ({n})");
        n += 1;
    }
    create_folder_named(endpoint, &name).await?;
    Ok(name)
}

/// Create a folder with exactly `name` inside `endpoint`, failing when
/// the name is taken. Backs the both-sides command, which must not
/// silently create a second, differently named folder.
pub(crate) async fn create_folder_named(
    endpoint: Endpoint,
    name: &str,
) -> FsResult {
    if child_exists(&endpoint, name).await? {
        return Err(format!(
            "New Folder failed: {} already exists",
            endpoint.join(name).display()
        ));
    }
    if endpoint.is_local() {
        return fs_err::tokio::create_dir_all(endpoint.path().join(name))
            .await
            .map_err(|e| format!("New Folder failed: {e}"));
    }
    let fs = endpoint.fs();
    let parent = fs
        .resolve_path(endpoint.path())
        .await
        .map_err(|e| format!("New Folder failed: {e}"))?;
    fs.create_dir_node(DirId::new(*parent.get()), OsStr::new(name))
        .await
        .map(|_| ())
        .map_err(|e| format!("New Folder failed: {e}"))
}

/// Whether `name` already addresses an entry inside `endpoint`.
///
/// The local probe is a `symlink_metadata` (it must not follow a
/// dangling link); the remote probe resolves the child through the
/// provider and treats `NotFound` as free.
async fn child_exists(
    endpoint: &Endpoint,
    name: &str,
) -> Result<bool, String> {
    if endpoint.is_local() {
        let path = endpoint.path().join(name);
        return match fs_err::tokio::symlink_metadata(&path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(format!("New Folder failed: {e}")),
        };
    }
    let child = endpoint.join(name);
    match endpoint.fs().metadata_path(child.path()).await {
        Ok(_) => Ok(true),
        Err(FsError::NotFound { .. }) => Ok(false),
        Err(e) => Err(format!("New Folder failed: {e}")),
    }
}

/// Copy or move the entry at `source` into the local folder `dest_dir`.
///
/// A local source prefers a same-filesystem rename for a move, then
/// falls back to a recursive copy. A remote source is pulled through the
/// provider: the node tree is walked with `read_dir_node` and every file
/// is read and written locally; a move removes the remote node once the
/// copy succeeded. The copy honors `copy_options` (links, timestamps,
/// mode bits); a move carries content over unchanged instead.
pub(crate) async fn copy_to_folder(
    source: Endpoint,
    dest_dir: std::path::PathBuf,
    move_entry: bool,
    copy_options: CopyOptions,
) -> FsResult {
    let verb = if move_entry { "Move" } else { "Copy" };
    let Some(name) = source.path().file_name().map(|n| n.to_os_string())
    else {
        return Err("The entry has no name to copy".into());
    };
    let to = dest_dir.join(name);
    if !source.is_local() {
        let fs = source.fs_arc();
        let id = fs
            .resolve_path(source.path())
            .await
            .map_err(|e| format!("{verb} to folder failed: {e}"))?;
        pull_node(&fs, id, &to, &copy_options)
            .await
            .map_err(|e| format!("{verb} to folder failed: {e}"))?;
        if move_entry {
            fs.remove_all_node(id)
                .await
                .map_err(|e| format!("{verb} to folder failed: {e}"))?;
        }
        return Ok(());
    }
    let from = source.path().to_path_buf();
    if move_entry {
        // Prefer a same-filesystem rename; fall back to copy+delete.
        if fs_err::tokio::rename(&from, &to).await.is_ok() {
            return Ok(());
        }
    }
    copy_tree(&from, &to, &copy_options)
        .await
        .map_err(|e| format!("{verb} to folder failed: {e}"))?;
    if move_entry {
        let _ = fs_err::tokio::remove_dir_all(&from).await;
        let _ = fs_err::tokio::remove_file(&from).await;
    }
    Ok(())
}

/// Recursively copy a provider node into the local path `to`.
///
/// `read_dir_node` populates the cached node's child names, so the node
/// is read again after the listing. Errors are the plain strings the
/// row commands report.
async fn pull_node(
    fs: &Provider,
    id: NodeId<u64>,
    to: &Path,
    options: &CopyOptions,
) -> Result<(), String> {
    let node = fs.get_node(id).map_err(|e| e.to_string())?;
    if node.kind().is_symlink() && options.preserve_links {
        // Recreate the link verbatim instead of pulling its target.
        if let NodeKind::Symlink { target } = node.kind() {
            create_local_symlink(target.path(), to)
                .await
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if !node.kind().is_directory() {
        let bytes = fs
            .read_node(cocomo_lib::FileId::new(*id.get()), None)
            .await
            .map_err(|e| e.to_string())?;
        fs_err::tokio::write(to, bytes)
            .await
            .map_err(|e| e.to_string())?;
        if options.preserve_timestamps {
            preserve_modified(to, node.metadata().modified);
        }
        return Ok(());
    }
    let dir = DirId::new(*id.get());
    fs.read_dir_node(dir).await.map_err(|e| e.to_string())?;
    let names: Vec<String> = fs
        .get_node(id)
        .map_err(|e| e.to_string())?
        .kind()
        .children()
        .map(|children| children.to_vec())
        .unwrap_or_default();
    fs_err::tokio::create_dir_all(to)
        .await
        .map_err(|e| e.to_string())?;
    for name in names {
        let child_path = node.path().join(&name);
        let child = fs
            .resolve_path(&child_path)
            .await
            .map_err(|e| e.to_string())?;
        Box::pin(pull_node(fs, child, &to.join(name), options)).await?;
    }
    // The directory's own time goes last: writing the children above
    // bumped it.
    if options.preserve_timestamps {
        preserve_modified(to, node.metadata().modified);
    }
    Ok(())
}

/// Best-effort restore of a provider node's modification time onto the
/// local `path`. Directories are opened read-only (a write open fails on
/// macOS), the same trick as [`touch`].
fn preserve_modified(path: &Path, modified: chrono::DateTime<chrono::Utc>) {
    let is_dir = fs_err::metadata(path).is_ok_and(|m| m.is_dir());
    let mut options = std::fs::File::options();
    if is_dir {
        options.read(true);
    } else {
        options.write(true);
    }
    if let Ok(file) = options.open(path) {
        let _ = file.set_modified(modified.into());
    }
}

/// Copy a file, symlink, or directory tree to `to`, honoring `options`.
///
/// With `preserve_links` a symlink is recreated as a symlink; otherwise
/// the linked file or tree is copied instead (the behavior from before
/// the copy options existed). Timestamps and mode bits are best-effort:
/// the destination is complete even where the platform refuses to
/// restore them.
pub(crate) async fn copy_tree(
    from: &Path,
    to: &Path,
    options: &CopyOptions,
) -> std::io::Result<()> {
    let meta = fs_err::tokio::symlink_metadata(from).await?;
    copy_tree_at(from, to, &meta, options).await
}

/// [`copy_tree`] over a source whose metadata is already known.
async fn copy_tree_at(
    from: &Path,
    to: &Path,
    meta: &Metadata,
    options: &CopyOptions,
) -> std::io::Result<()> {
    if meta.file_type().is_symlink() {
        if options.preserve_links {
            let target = fs_err::tokio::read_link(from).await?;
            create_local_symlink(&target, to).await?;
            if options.preserve_timestamps {
                preserve_timestamps(to, meta);
            }
            return Ok(());
        }
        // Dereference: `canonicalize` resolves every link, so the target
        // is never a symlink and cannot re-enter the link branch.
        let target = fs_err::tokio::canonicalize(from).await?;
        let target_meta = fs_err::tokio::symlink_metadata(&target).await?;
        return Box::pin(copy_tree_at(&target, to, &target_meta, options))
            .await;
    }
    if meta.is_dir() {
        fs_err::tokio::create_dir_all(to).await?;
        let mut entries = fs_err::tokio::read_dir(from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            Box::pin(copy_tree(&entry.path(), &to.join(name), options))
                .await?;
        }
    } else {
        // Content-only copy: `std::fs::copy` also duplicates mode and (on
        // macOS, via `copyfile`) timestamps and xattrs, which would
        // silently override the `preserve_*` options applied below.
        let mut reader = fs_err::tokio::File::open(from).await?;
        let mut writer = fs_err::tokio::File::create(to).await?;
        tokio::io::copy(&mut reader, &mut writer).await?;
    }
    // Attributes go last: writing the children above bumped a directory's
    // own mtime, and a restrictive source mode applied first would block
    // the child copies.
    if options.preserve_timestamps {
        preserve_timestamps(to, meta);
    }
    if options.preserve_mode {
        preserve_mode(to, meta);
    }
    Ok(())
}

/// Create a symlink at `link` pointing at `target`, verbatim.
///
/// On Windows the link flavor is picked from what the target currently
/// is; a missing target is treated as a file link.
async fn create_local_symlink(
    target: &Path,
    link: &Path,
) -> std::io::Result<()> {
    #[cfg(unix)]
    let created = fs_err::tokio::symlink(target, link).await;
    #[cfg(windows)]
    let created = if fs_err::tokio::metadata(target)
        .await
        .is_ok_and(|m| m.is_dir())
    {
        fs_err::tokio::symlink_dir(target, link).await
    } else {
        fs_err::tokio::symlink_file(target, link).await
    };
    created
}

/// Best-effort restore of the timestamps `meta` describes onto `dst`.
///
/// `meta` must describe `dst`'s source itself (from `symlink_metadata`)
/// so a symlink is timed without touching its target. Timestamps the
/// source does not expose are skipped; the creation time has no portable
/// setter and is not restored.
fn preserve_timestamps(dst: &Path, meta: &Metadata) {
    let atime = meta.accessed().ok();
    let mtime = meta.modified().ok();
    let mut times = std::fs::FileTimes::new();
    if let Some(a) = atime {
        times = times.set_accessed(a);
    }
    if let Some(m) = mtime {
        times = times.set_modified(m);
    }
    let set = match (atime, mtime) {
        (None, None) => Ok(()),
        (Some(_), Some(_)) if meta.is_symlink() => {
            std::fs::set_times_nofollow(dst, times)
        }
        // A filesystem that hides one timestamp must still not block
        // restoring the other; symlinks only expose both times together.
        _ if !meta.is_symlink() => std::fs::set_times(dst, times),
        _ => Ok(()),
    };
    let _ = set;
}

/// Best-effort copy of the source mode bits onto `dst`.
///
/// Mode bits the kernel rejects are skipped, never fatal. ACLs and
/// extended attributes are carried by the transfer engine's copy; this
/// context-menu fallback restores the mode bits only.
fn preserve_mode(dst: &Path, meta: &Metadata) {
    let _ = fs_err::set_permissions(dst, meta.permissions());
}

/// Spawn the platform's "open this path" helper.
///
/// The same command backs both operations: on macOS and Linux `open` /
/// `xdg-open` shows a folder in the file manager and opens a file in its
/// associated application; on Windows `explorer` does the same.
fn open_via_os(
    path: &std::path::Path,
    what: &str,
) -> std::result::Result<(), String> {
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
            let _ = (path, what);
            return Err(format!(
                "Opening a {what} is not supported on this platform"
            ));
        }
    };
    command
        .spawn()
        .map_err(|e| format!("Couldn't open the {what}: {e}"))?;
    Ok(())
}

/// Open `endpoint` in the platform's file explorer with the entry
/// selected.
///
/// A native shell context menu cannot be invoked from a third-party
/// application (it requires a shell extension, see the spec's platform
/// limits), so the closest platform-correct command is a reveal that
/// selects the item. Remote sides have no local path to reveal and
/// report that clearly.
pub(crate) fn reveal_in_explorer(endpoint: &Endpoint) -> FsResult {
    require_local(endpoint, "Reveal")?;
    let path = endpoint.path();
    let mut command = {
        #[cfg(target_os = "macos")]
        {
            // `-R` reveals the item in Finder with it selected.
            let mut c = std::process::Command::new("open");
            c.arg("-R").arg(path);
            c
        }
        #[cfg(target_os = "windows")]
        {
            // `/select,` opens the parent with the item highlighted; the
            // flag and path are one argument with Windows separators.
            let mut c = std::process::Command::new("explorer");
            c.arg(format!("/select,{}", path.display()));
            c
        }
        #[cfg(target_os = "linux")]
        {
            // No desktop-neutral selection flag exists, so the closest
            // fallback opens the containing folder.
            let mut c = std::process::Command::new("xdg-open");
            c.arg(path.parent().unwrap_or(path));
            c
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "linux",
            target_os = "windows"
        )))]
        {
            return Err("Reveal is not supported on this platform".into());
        }
    };
    command
        .spawn()
        .map_err(|e| format!("Couldn't reveal the folder: {e}"))?;
    Ok(())
}

/// The reveal command's label in the platform's own vocabulary.
///
/// The command means the same thing everywhere; only the file manager's
/// name differs, so the row menu and the command palette use the name
/// the user's OS actually gives it.
pub(crate) fn reveal_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Reveal in Finder"
    }
    #[cfg(target_os = "windows")]
    {
        "Show in Explorer"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Open in Files"
    }
}

/// Open `endpoint` with the operating system's associated application.
///
/// Files launch their registered handler; folders open in the file
/// manager (the closest an OS opener gets to "reveal" without extra
/// per-platform flags).
pub(crate) fn open_with_associated_app(endpoint: &Endpoint) -> FsResult {
    open_via_os(endpoint.path(), "file")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tokio::runtime::Runtime;

    use super::*;

    /// A fresh temporary directory, removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("cocomo-fsops-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn runtime() -> Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    #[test]
    fn create_folder_picks_a_name_the_side_does_not_carry_yet() {
        let dir = TempDir::new("unique");
        let rt = runtime();
        let endpoint = Endpoint::local(dir.0.clone());
        assert_eq!(
            rt.block_on(create_folder(endpoint.clone())).unwrap(),
            "New Folder"
        );
        assert_eq!(
            rt.block_on(create_folder(endpoint)).unwrap(),
            "New Folder (1)"
        );
        assert!(dir.0.join("New Folder").is_dir());
        assert!(dir.0.join("New Folder (1)").is_dir());
    }

    #[test]
    fn create_folder_named_refuses_a_taken_name() {
        let dir = TempDir::new("named");
        let rt = runtime();
        let endpoint = Endpoint::local(dir.0.clone());
        rt.block_on(create_folder_named(endpoint.clone(), "docs"))
            .unwrap();
        let err = rt
            .block_on(create_folder_named(endpoint, "docs"))
            .unwrap_err();
        assert!(err.contains("already exists"), "unexpected error: {err}");
    }

    #[test]
    fn reveal_names_the_platform_file_manager() {
        assert!(!reveal_label().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_recreates_symlinks_by_default() {
        let dir = TempDir::new("links");
        let rt = runtime();
        std::fs::create_dir_all(dir.0.join("src")).unwrap();
        std::fs::write(dir.0.join("src/file.txt"), b"content").unwrap();
        std::os::unix::fs::symlink("file.txt", dir.0.join("src/link.txt"))
            .unwrap();
        rt.block_on(copy_tree(
            &dir.0.join("src"),
            &dir.0.join("dst"),
            &CopyOptions::default(),
        ))
        .unwrap();
        let link = dir.0.join("dst/link.txt");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the default options must not dereference the link"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("file.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_follows_symlinks_when_links_are_not_preserved() {
        let dir = TempDir::new("follow-links");
        let rt = runtime();
        std::fs::write(dir.0.join("src.txt"), b"content").unwrap();
        std::os::unix::fs::symlink("src.txt", dir.0.join("link.txt")).unwrap();
        let options = CopyOptions {
            preserve_links: false,
            ..CopyOptions::default()
        };
        rt.block_on(copy_tree(
            &dir.0.join("link.txt"),
            &dir.0.join("dst.txt"),
            &options,
        ))
        .unwrap();
        assert!(
            !std::fs::symlink_metadata(dir.0.join("dst.txt"))
                .unwrap()
                .file_type()
                .is_symlink(),
            "without preserve_links the copy holds the linked content"
        );
        assert_eq!(std::fs::read(dir.0.join("dst.txt")).unwrap(), b"content");
    }

    #[test]
    fn copy_tree_restores_timestamps_only_when_asked() {
        let dir = TempDir::new("times");
        let rt = runtime();
        let src = dir.0.join("src.txt");
        std::fs::write(&src, b"x").unwrap();
        let old = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(1_700_000_000);
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(old)
            .unwrap();
        rt.block_on(copy_tree(
            &src,
            &dir.0.join("kept.txt"),
            &CopyOptions::default(),
        ))
        .unwrap();
        assert_eq!(
            std::fs::metadata(dir.0.join("kept.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            old,
            "timestamps are preserved by default"
        );
        let options = CopyOptions {
            preserve_timestamps: false,
            ..CopyOptions::default()
        };
        rt.block_on(copy_tree(&src, &dir.0.join("fresh.txt"), &options))
            .unwrap();
        assert_ne!(
            std::fs::metadata(dir.0.join("fresh.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            old,
            "without preserve_timestamps the copy is freshly timed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_carries_mode_bits_when_preserved() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("mode");
        let rt = runtime();
        let src = dir.0.join("src.sh");
        std::fs::write(&src, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o750))
            .unwrap();
        let options = CopyOptions {
            preserve_mode: true,
            ..CopyOptions::default()
        };
        rt.block_on(copy_tree(&src, &dir.0.join("kept.sh"), &options))
            .unwrap();
        assert_eq!(
            std::fs::metadata(dir.0.join("kept.sh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o750,
            "preserve_mode must carry the source mode bits"
        );
    }
}
