// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! GUI session management.
//!
//! Wraps [`cocomo_lib::SessionManager`] in a gpui [`Model`] so the UI can
//! observe changes reactively. Manages open sessions as a tab list, persists
//! sessions to `.bcs` files, and provides recent-session browsing.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, Result};
use cocomo_lib::{
    LocalFs, ProviderRef, Session, SessionConfig, SessionSettings, SessionType,
};
use gpui::{App, AppContext, Context, Entity, Task, WeakEntity};

/// A single open tab in the GUI.
///
/// Stores the serializable config plus a weak handle to the runtime
/// [`AppState`] so the tab can be closed independently.
#[derive(Clone, Debug)]
pub struct OpenSession {
    /// Unique index for this tab (stable across saves).
    pub tab_id: usize,
    /// Session configuration (serializable form).
    pub config: SessionConfig,
    /// Whether the session has unsaved changes.
    pub dirty: bool,
}

impl OpenSession {
    /// Create a new open session from a config.
    pub fn new(tab_id: usize, config: SessionConfig) -> Self {
        Self {
            tab_id,
            config,
            dirty: false,
        }
    }

    /// Create a new empty dir-compare session config.
    pub fn empty_compare() -> Self {
        Self::new(
            0,
            SessionConfig {
                name: "untitled".to_string(),
                session_type: SessionType::DirCompare,
                left: ProviderRef {
                    provider: "local".to_string(),
                    path: PathBuf::from("/"),
                },
                right: ProviderRef {
                    provider: "local".to_string(),
                    path: PathBuf::from("/"),
                },
                center: None,
                settings: SessionSettings::default(),
            },
        )
    }
}

/// Manages open sessions and session persistence for the GUI.
///
/// This is a gpui [`Entity`] that can be observed by the UI. It holds the list
/// of open sessions (tabs), tracks the active tab, and handles loading/saving
/// session files.
pub struct GuiSessionManager {
    /// Directory where `.bcs` session files are stored.
    session_dir: PathBuf,
    /// Next unique tab ID.
    next_tab_id: usize,
    /// Currently open sessions.
    open_sessions: Vec<OpenSession>,
    /// Index of the active session in [`Self::open_sessions`].
    active_index: usize,
    /// Cached recent session configs loaded from disk.
    recent_sessions: Vec<SessionConfig>,
}

// TODO: remove this
#[allow(dead_code)]
impl GuiSessionManager {
    /// Create a new session manager that stores sessions in the given
    /// directory.
    pub fn new(session_dir: PathBuf) -> Self {
        Self {
            session_dir,
            next_tab_id: 1,
            open_sessions: Vec::new(),
            active_index: 0,
            recent_sessions: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Return the open sessions.
    #[allow(dead_code)]
    pub fn open_sessions(&self) -> &[OpenSession] {
        &self.open_sessions
    }

    /// Return the active session, if any.
    pub fn active_session(&self) -> Option<&OpenSession> {
        self.open_sessions.get(self.active_index)
    }

    /// Return the active session index.
    #[allow(dead_code)]
    pub fn active_index(&self) -> usize {
        self.active_index
    }

    /// Return the number of open sessions.
    pub fn len(&self) -> usize {
        self.open_sessions.len()
    }

    /// Whether there are no open sessions.
    pub fn is_empty(&self) -> bool {
        self.open_sessions.is_empty()
    }

    /// Return the recent sessions loaded from disk.
    #[allow(dead_code)]
    pub fn recent_sessions(&self) -> &[SessionConfig] {
        &self.recent_sessions
    }

    /// Return the session directory.
    #[allow(dead_code)]
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    // -----------------------------------------------------------------------
    // Session operations
    // -----------------------------------------------------------------------

    /// Add a new empty dir-compare session and make it active.
    pub fn add_new_session(&mut self, cx: &mut Context<Self>) {
        let mut session = OpenSession::empty_compare();
        session.tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Open a session config as a new tab.
    pub fn open_session(
        &mut self,
        config: SessionConfig,
        cx: &mut Context<Self>,
    ) {
        let mut session = OpenSession::new(self.next_tab_id, config);
        session.tab_id = self.next_tab_id;
        self.next_tab_id += 1;
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Close the session at the given index.
    pub fn close_session(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.open_sessions.len() {
            return;
        }

        self.open_sessions.remove(index);

        // Adjust active index.
        if self.open_sessions.is_empty() {
            self.active_index = 0;
        } else if self.active_index >= self.open_sessions.len() {
            self.active_index = self.open_sessions.len() - 1;
        } else if index < self.active_index {
            self.active_index -= 1;
        }

        cx.notify();
    }

    /// Close the active session.
    pub fn close_active_session(&mut self, cx: &mut Context<Self>) {
        if !self.open_sessions.is_empty() {
            self.close_session(self.active_index, cx);
        }
    }

    /// Switch to the session at the given index.
    pub fn activate_session(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.open_sessions.len() {
            self.active_index = index;
            cx.notify();
        }
    }

    /// Mark the active session as dirty.
    pub fn mark_dirty(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.open_sessions.get_mut(self.active_index) {
            session.dirty = true;
            cx.notify();
        }
    }

    /// Mark the active session as clean.
    pub fn mark_clean(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.open_sessions.get_mut(self.active_index) {
            session.dirty = false;
            cx.notify();
        }
    }

    /// Update the active session's config (paths, name, etc.).
    pub fn update_active_config(
        &mut self,
        config: SessionConfig,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.open_sessions.get_mut(self.active_index) {
            session.config = config;
            session.dirty = true;
            cx.notify();
        }
    }

    // -----------------------------------------------------------------------
    // Save / Load
    // -----------------------------------------------------------------------

    /// Save the active session to its file.
    pub fn save_active_session(
        &self,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let config = match self.active_session() {
            Some(s) => s.config.clone(),
            None => return cx.background_spawn(async { Ok(()) }),
        };
        let session_dir = self.session_dir.clone();
        let name = config.name.clone();

        cx.background_spawn(async move {
            let file_name = if name == "untitled" || name.is_empty() {
                "untitled.bcs".to_string()
            } else {
                // Sanitize the name to create a valid file name.
                let sanitized: String = name
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect();
                format!("{}.bcs", sanitized)
            };

            let path = session_dir.join(&file_name);

            // Ensure the session directory exists.
            tokio::fs::create_dir_all(&session_dir).await.with_context(
                || {
                    format!(
                        "failed to create session directory: {:?}",
                        session_dir
                    )
                },
            )?;

            config.save_to_file(&path).await.with_context(|| {
                format!("failed to save session to {:?}", path)
            })?;

            Ok(())
        })
    }

    /// Save the active session with a specific name.
    pub fn save_as(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        if let Some(session) = self.open_sessions.get_mut(self.active_index) {
            session.config.name = name.clone();
        }
        let config = self.active_session().map(|s| s.config.clone());
        let session_dir = self.session_dir.clone();

        cx.background_spawn(async move {
            let config = match config {
                Some(c) => c,
                None => return Ok(()),
            };
            let file_name = format!("{}.bcs", config.name);
            let path = session_dir.join(&file_name);
            tokio::fs::create_dir_all(
                &path.parent().unwrap_or(Path::new(".")),
            )
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            config
                .save_to_file(&path)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
    }

    /// Load a session from a `.bcs` file path.
    pub fn load_session_file(
        &self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<Result<SessionConfig>> {
        cx.background_spawn(async move {
            SessionConfig::load_from_file(&path)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
    }

    /// Load recent sessions from disk.
    pub fn load_recent_sessions(&self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let session_dir = self.session_dir.clone();

        cx.spawn(
            |this: WeakEntity<GuiSessionManager>, cx: &mut gpui::AsyncApp| {
                // Clone async_app OUTSIDE the async block.
                let async_app = cx.clone();
                async move {
                    let _ = this;
                    let configs: Vec<SessionConfig> =
                        match tokio::fs::read_dir(&session_dir).await {
                            Ok(mut entries) => {
                                let mut found = Vec::new();
                                while let Ok(Some(entry)) =
                                    entries.next_entry().await
                                {
                                    let path = entry.path();
                                    if path
                                        .extension()
                                        .and_then(|e| e.to_str())
                                        == Some("bcs")
                                        && let Ok(config) =
                                            SessionConfig::load_from_file(
                                                &path,
                                            )
                                            .await
                                    {
                                        found.push(config);
                                    }
                                }
                                found
                            }
                            Err(_) => Vec::new(),
                        };

                    async_app.update(|cx| {
                        if let Some(manager) = weak.upgrade() {
                            manager.update(cx, |m, cx| {
                                m.recent_sessions = configs;
                                cx.notify();
                            });
                        }
                    });
                }
            },
        )
        .detach();
    }

    /// Build a runtime [`Session`] from a config using local filesystem
    /// providers.
    pub async fn build_session(
        &self,
        config: &SessionConfig,
    ) -> Result<Session> {
        let left_fs: Arc<
            dyn cocomo_lib::fs::NodeFileSystem<
                    Nid = u64,
                    FsId = u64,
                    Error = cocomo_lib::FsError,
                >,
        > = Arc::new(LocalFs::new("local"));

        let right_fs: Arc<
            dyn cocomo_lib::fs::NodeFileSystem<
                    Nid = u64,
                    FsId = u64,
                    Error = cocomo_lib::FsError,
                >,
        > = Arc::new(LocalFs::new("local"));

        let center_fs: Option<
            Arc<
                dyn cocomo_lib::fs::NodeFileSystem<
                        Nid = u64,
                        FsId = u64,
                        Error = cocomo_lib::FsError,
                    >,
            >,
        > = config.center.as_ref().map(|_| {
            Arc::new(LocalFs::new("local"))
                as Arc<
                    dyn cocomo_lib::fs::NodeFileSystem<
                            Nid = u64,
                            FsId = u64,
                            Error = cocomo_lib::FsError,
                        >,
                >
        });

        Session::from_config_and_providers(
            config, left_fs, right_fs, center_fs,
        )
        .await
        .with_context(|| format!("failed to build session: {}", config.name))
    }
}

/// Create a new session manager with the default session directory.
pub fn create_default_manager(cx: &mut App) -> Entity<GuiSessionManager> {
    let session_dir = default_session_dir();
    let _ = std::fs::create_dir_all(&session_dir);

    cx.new(|cx| {
        let mgr = GuiSessionManager::new(session_dir);
        mgr.load_recent_sessions(cx);
        mgr
    })
}

/// Return the default session directory path.
fn default_session_dir() -> PathBuf {
    if let Some(config_dir) = dirs::config_dir() {
        config_dir.join("cocomo").join("sessions")
    } else {
        PathBuf::from(".cocomo/sessions")
    }
}

/// Resolve the default session directory, creating it if necessary.
#[allow(dead_code)]
pub fn ensure_session_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| {
        format!("failed to create session directory: {:?}", dir)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_session_new_starts_empty() {
        let mgr = GuiSessionManager::new(PathBuf::from("/tmp/test_sessions"));
        assert!(mgr.is_empty());
        assert!(mgr.active_session().is_none());
    }

    #[test]
    fn open_session_empty_compare_has_defaults() {
        let session = OpenSession::empty_compare();
        assert_eq!(session.config.name, "untitled");
        assert_eq!(session.config.session_type, SessionType::DirCompare);
        assert_eq!(session.config.left.provider, "local");
        assert!(!session.dirty);
    }
}
