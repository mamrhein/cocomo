// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Workspace persistence: the set of open windows and their tabs.
//!
//! A workspace is the serializable shape of a whole application run: every
//! window's open tabs (as [`SessionConfig`]s) plus which tab was active.
//! It is written to the config directory on quit and read back at launch,
//! so both frontends can save and restore the same
//! `workspace.json` (spec section 8, "workspace, persistence").

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    Result, SessionConfig,
    error::{self, FsError, FsOperation},
};

/// Schema version of the workspace file, so a future format change can
/// migrate or discard old files instead of failing to parse them.
pub const WORKSPACE_VERSION: u8 = 1;

/// The full set of open windows, as persisted to disk.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Workspace {
    /// File format version.
    pub version: u8,
    /// One entry per open window, in window-creation order.
    pub windows: Vec<WindowWorkspace>,
}

impl Workspace {
    /// An empty workspace at the current schema version.
    pub fn new() -> Self {
        Self {
            version: WORKSPACE_VERSION,
            windows: Vec::new(),
        }
    }

    /// Whether the workspace holds no tabs in any window.
    pub fn is_empty(&self) -> bool {
        self.windows.iter().all(|w| w.tabs.is_empty())
    }

    /// Total number of tabs across all windows.
    pub fn tab_count(&self) -> usize {
        self.windows.iter().map(|w| w.tabs.len()).sum()
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// One window's tabs, as persisted to disk.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WindowWorkspace {
    /// The window's open tabs, in tab-bar order.
    pub tabs: Vec<SessionConfig>,
    /// Index of the active tab when the workspace was written.
    pub active_index: usize,
}

impl WindowWorkspace {
    /// A window snapshot with the given tabs and active index.
    pub fn new(tabs: Vec<SessionConfig>, active_index: usize) -> Self {
        Self { tabs, active_index }
    }
}

/// Directory holding the application's own state files.
pub fn config_dir() -> PathBuf {
    if let Some(config_dir) = dirs::config_dir() {
        config_dir.join("cocomo")
    } else {
        PathBuf::from(".cocomo")
    }
}

/// Path of the workspace file.
pub fn workspace_path() -> PathBuf {
    config_dir().join("workspace.json")
}

/// Load the persisted workspace, if any.
///
/// A missing, unreadable, unparseable, or version-mismatched file yields
/// `None`: a broken workspace must never prevent the app from starting.
pub fn load_workspace() -> Option<Workspace> {
    load_workspace_from(&workspace_path())
}

/// Load the workspace persisted at `path`, applying the same tolerance as
/// [`load_workspace`].
pub fn load_workspace_from(path: &Path) -> Option<Workspace> {
    let text = fs_err::read_to_string(path).ok()?;
    let workspace: Workspace = serde_json::from_str(&text).ok()?;
    (workspace.version == WORKSPACE_VERSION).then_some(workspace)
}

/// Persist `workspace` to disk, creating the config directory as needed.
///
/// An empty workspace is never written: the last non-empty state stays on
/// disk, so closing every tab does not erase a restorable workspace.
pub fn save_workspace(workspace: &Workspace) -> Result<()> {
    save_workspace_to(workspace, &workspace_path())
}

/// Persist `workspace` at `path`, applying the same rules as
/// [`save_workspace`].
pub fn save_workspace_to(workspace: &Workspace, path: &Path) -> Result<()> {
    if workspace.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent).map_err(|e| {
            error::wrap(e, FsOperation::CreateDir, parent.to_path_buf())
        })?;
    }
    let text = serde_json::to_string_pretty(workspace).map_err(|e| {
        FsError::InvalidArgument {
            operation: FsOperation::Write,
            path: path.to_path_buf(),
            message: format!("failed to serialize the workspace: {e}"),
        }
    })?;
    fs_err::write(path, text)
        .map_err(|e| error::wrap(e, FsOperation::Write, path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{ProviderRef, SessionSettings, SessionType};

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
    fn workspace_round_trips_through_json() {
        let workspace = Workspace {
            version: WORKSPACE_VERSION,
            windows: vec![
                WindowWorkspace::new(
                    vec![config_named("alpha"), config_named("beta")],
                    1,
                ),
                WindowWorkspace::new(vec![config_named("gamma")], 0),
            ],
        };
        let text = serde_json::to_string(&workspace).unwrap();
        let back: Workspace = serde_json::from_str(&text).unwrap();
        assert_eq!(back.version, WORKSPACE_VERSION);
        assert_eq!(back.tab_count(), 3);
        assert_eq!(back.windows[0].active_index, 1);
        assert_eq!(back.windows[1].tabs[0].name, "gamma");
    }

    #[test]
    fn empty_workspace_reports_no_tabs() {
        let mut workspace = Workspace::new();
        assert!(workspace.is_empty());
        workspace.windows.push(WindowWorkspace::new(Vec::new(), 0));
        assert!(workspace.is_empty());
        workspace
            .windows
            .push(WindowWorkspace::new(vec![config_named("alpha")], 0));
        assert!(!workspace.is_empty());
    }

    #[test]
    fn workspace_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("workspace.json");
        let workspace = Workspace {
            version: WORKSPACE_VERSION,
            windows: vec![WindowWorkspace::new(
                vec![config_named("alpha")],
                0,
            )],
        };
        save_workspace_to(&workspace, &path).unwrap();
        let back = load_workspace_from(&path).unwrap();
        assert_eq!(back.tab_count(), 1);
        assert_eq!(back.windows[0].tabs[0].name, "alpha");
    }

    #[test]
    fn saving_an_empty_workspace_writes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        save_workspace_to(&Workspace::new(), &path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn loading_a_missing_workspace_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_workspace_from(&dir.path().join("nope.json")).is_none());
    }

    #[test]
    fn loading_a_foreign_workspace_version_yields_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workspace.json");
        let mut workspace = Workspace::new();
        workspace
            .windows
            .push(WindowWorkspace::new(vec![config_named("alpha")], 0));
        workspace.version = WORKSPACE_VERSION + 1;
        fs_err::write(&path, serde_json::to_string(&workspace).unwrap())
            .unwrap();
        assert!(load_workspace_from(&path).is_none());
    }
}
