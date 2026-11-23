// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! GUI-side workspace persistence.
//!
//! The workspace file itself (`workspace.json`, the set of open windows
//! and their tabs) is owned by [`cocomo_lib::workspace`] and re-exported
//! here, so the GUI and the TUI save and restore the same file.
//!
//! On top of that, this module handles crash recovery: a *running* marker
//! file records that the process did not shut down cleanly, and periodic
//! session snapshots (`autosave/*.bcs`) capture unsaved work, so the next
//! launch can offer to restore the tabs that were open when the app died.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
pub use cocomo_lib::workspace::{
    WindowWorkspace, Workspace, load_workspace, save_workspace, workspace_path,
};
use cocomo_lib::{SessionConfig, workspace::config_dir};

/// Record that a cocomo run is live and report whether the previous run
/// left its marker behind.
///
/// The marker is removed on a clean quit; if it survives until this call,
/// the previous run crashed. Checking and writing happen in one step on
/// purpose: a launch that wrote the marker first would mistake its own
/// marker for a crash and offer recovery on every start.
pub fn mark_running() -> bool {
    mark_running_in(&config_dir())
}

fn mark_running_in(dir: &Path) -> bool {
    let path = dir.join(".running");
    let crashed = path.exists();
    let _ = fs_err::create_dir_all(dir);
    let _ = fs_err::write(&path, "running");
    crashed
}

/// Remove the running marker after a clean shutdown.
pub fn clear_running_marker() {
    let _ = fs_err::remove_file(running_marker_path());
}

/// Path of the marker file that records a live (or crashed) session.
fn running_marker_path() -> PathBuf {
    config_dir().join(".running")
}

/// Path of the auto-saved session snapshots used for crash recovery.
///
/// Snapshots are plain `.bcs` configs, so the history machinery of the
/// session store can read them; the directory lives next to the workspace
/// file rather than inside the user's session directory.
pub fn autosave_dir() -> PathBuf {
    config_dir().join("autosave")
}

/// Write a snapshot of every dirty session config into [`autosave_dir`].
///
/// Called periodically and on window close while unsaved work is open, so
/// a crash loses at most one interval of edits.
pub fn snapshot_sessions(configs: &[SessionConfig]) -> Result<()> {
    snapshot_sessions_to(&autosave_dir(), configs)
}

/// Write a snapshot of `configs` into `dir`, replacing its contents.
///
/// Stale snapshots of sessions that have since closed must not reappear in
/// the recovery prompt, so the directory is cleared before writing.
pub fn snapshot_sessions_to(
    dir: &Path,
    configs: &[SessionConfig],
) -> Result<()> {
    fs_err::create_dir_all(dir).with_context(|| {
        format!("failed to create autosave directory: {:?}", dir)
    })?;
    if let Ok(entries) = fs_err::read_dir(dir) {
        for entry in entries.flatten() {
            let _ = fs_err::remove_file(entry.path());
        }
    }
    for (index, config) in configs.iter().enumerate() {
        let path = dir.join(format!("session-{index}.bcs"));
        let text = config.to_toml().with_context(|| {
            format!("failed to serialize session: {}", config.name)
        })?;
        fs_err::write(&path, text).with_context(|| {
            format!("failed to write session snapshot: {:?}", path)
        })?;
    }
    Ok(())
}

/// Read back the auto-saved session snapshots, newest file first.
pub fn load_snapshot_sessions() -> Vec<SessionConfig> {
    load_snapshot_sessions_from(&autosave_dir())
}

/// Read every `.bcs` snapshot in `dir`, newest file first.
pub fn load_snapshot_sessions_from(dir: &Path) -> Vec<SessionConfig> {
    let Ok(entries) = fs_err::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (mtime, path)
        })
        .filter(|(_, path)| {
            path.extension().and_then(|e| e.to_str()) == Some("bcs")
        })
        .collect();
    found.sort_by_key(|(mtime, _)| std::cmp::Reverse(*mtime));
    found
        .into_iter()
        .filter_map(|(_, path)| read_bcs(&path).ok())
        .collect()
}

/// Read a `.bcs` session file synchronously.
fn read_bcs(path: &Path) -> Result<SessionConfig> {
    let text = fs_err::read_to_string(path)
        .with_context(|| format!("failed to read session file: {:?}", path))?;
    SessionConfig::from_toml(&text)
        .map_err(|e| anyhow::anyhow!("failed to parse {:?}: {e}", path))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use cocomo_lib::{ProviderRef, SessionSettings, SessionType};

    use super::*;

    fn config_named(name: &str) -> SessionConfig {
        SessionConfig {
            name: name.to_string(),
            session_type: SessionType::DirCompare,
            left: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/left"),
            },
            right: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/right"),
            },
            center: None,
            settings: SessionSettings::default(),
        }
    }

    #[test]
    fn snapshots_round_trip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let configs = vec![config_named("alpha"), config_named("beta")];
        snapshot_sessions_to(dir.path(), &configs).unwrap();
        let loaded = load_snapshot_sessions_from(dir.path());
        let names: Vec<&str> =
            loaded.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["beta", "alpha"]);

        // Rewriting replaces the set, so a closed session disappears.
        snapshot_sessions_to(dir.path(), &configs[..1]).unwrap();
        let loaded = load_snapshot_sessions_from(dir.path());
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "alpha");
    }

    #[test]
    fn loading_snapshots_tolerates_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            load_snapshot_sessions_from(&dir.path().join("nope")).is_empty()
        );
    }

    #[test]
    fn mark_running_reports_the_previous_run_as_crashed() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(".running");
        // A first launch finds no marker: the run before it ended cleanly.
        assert!(!mark_running_in(dir.path()));
        // The marker exists while the run is live...
        assert!(marker.exists());
        // ...so a relaunch without a clean quit reads as a crash.
        assert!(mark_running_in(dir.path()));
        // After a clean quit removes the marker, the next launch is clean.
        fs_err::remove_file(&marker).unwrap();
        assert!(!mark_running_in(dir.path()));
    }
}
