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
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use cocomo_lib::{
    Endpoint, FsError, NullPrompter, ProviderRef, Session, SessionConfig,
    SessionSettings, SessionType,
};
use gpui::{
    App, AppContext as _, Context, Entity, Task,
    component::{WindowExt as _, button::ButtonVariant},
};

use crate::{
    log::{self, LogLevel},
    tabview::TabEntity,
};

/// Type-erased node filesystem, matching what [`Session`] accepts.
type AnyNodeFs =
    dyn cocomo_lib::fs::NodeFileSystem<Nid = u64, FsId = u64, Error = FsError>;

/// A single open tab in the GUI.
///
/// Stores the serializable config plus a type-erased [`TabEntity`] so
/// the tab can be rendered and closed independently.
pub struct OpenSession {
    /// Unique index for this tab (stable across saves).
    pub tab_id: usize,
    /// Session configuration (serializable form).
    pub config: SessionConfig,
    /// Whether the session has unsaved changes.
    pub dirty: bool,
    /// The rendered content for this tab.
    pub tab_entity: TabEntity,
}

/// How many auto-saved history entries are kept on disk.
const HISTORY_LIMIT: usize = 10;

/// Name of the marker file in the session directory that records which
/// saved session the user marked as the default.
const DEFAULT_MARKER_FILE: &str = "default";

/// A saved session config and the `.bcs` file it was loaded from.
///
/// The path is part of the identity: two files may hold configs with the
/// same name, and rename/delete operations act on the file.
#[derive(Clone)]
pub struct SavedSession {
    config: SessionConfig,
    path: PathBuf,
}

impl SavedSession {
    /// The saved session configuration.
    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// The `.bcs` file this session was loaded from.
    pub fn path(&self) -> &Path {
        &self.path
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
    /// Directory where auto-saved history snapshots are stored.
    history_dir: PathBuf,
    /// Next unique tab ID.
    next_tab_id: usize,
    /// Currently open sessions.
    open_sessions: Vec<OpenSession>,
    /// Index of the active session in [`Self::open_sessions`].
    active_index: usize,
    /// Cached saved session configs loaded from disk, sorted by name.
    recent_sessions: Vec<SavedSession>,
    /// Auto-saved history of recently opened sessions, newest first.
    history: Vec<SavedSession>,
    /// File name of the saved session marked as default, if any.
    default_name: Option<String>,
}

// TODO: remove this
#[allow(dead_code)]
impl GuiSessionManager {
    /// Create a new session manager that stores sessions in the given
    /// directory.
    pub fn new(session_dir: PathBuf) -> Self {
        // History lives next to the saved sessions, in its own directory,
        // so pruning snapshots never touches user-saved files.
        let history_dir = session_dir
            .parent()
            .map(|parent| parent.join("history"))
            .unwrap_or_else(|| session_dir.join("history"));
        Self {
            session_dir,
            history_dir,
            next_tab_id: 1,
            open_sessions: Vec::new(),
            active_index: 0,
            recent_sessions: Vec::new(),
            history: Vec::new(),
            default_name: None,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Return the open sessions' tab IDs and names.
    pub fn open_tab_ids(&self) -> Vec<(usize, String)> {
        self.open_sessions
            .iter()
            .map(|s| (s.tab_id, s.config.name.clone()))
            .collect()
    }

    /// Return the active session's config, if any.
    pub fn active_config(&self) -> Option<&SessionConfig> {
        self.open_sessions.get(self.active_index).map(|s| &s.config)
    }

    /// Return the active session, if any.
    pub fn active_session(&self) -> Option<&OpenSession> {
        self.open_sessions.get(self.active_index)
    }

    /// Return the active session index.
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

    /// Return the saved sessions loaded from disk, sorted by name.
    pub fn recent_sessions(&self) -> &[SavedSession] {
        &self.recent_sessions
    }

    /// Return the auto-saved history of recently opened sessions, newest
    /// first.
    pub fn history(&self) -> &[SavedSession] {
        &self.history
    }

    /// Return the file name of the saved session marked as default.
    pub fn default_session_name(&self) -> Option<&str> {
        self.default_name.as_deref()
    }

    /// Return the session directory.
    #[allow(dead_code)]
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    // -----------------------------------------------------------------------
    // Session operations
    // -----------------------------------------------------------------------

    /// Open the Home tab, activating the existing one when present.
    ///
    /// The Home tab is a launcher, not a comparison: its config is a
    /// placeholder that is never saved, so it never becomes dirty and its
    /// save commands stay disabled in the menu bar.
    pub fn open_home(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self
            .open_sessions
            .iter()
            .position(|s| s.tab_entity.is_home())
        {
            self.activate_session(index, cx);
            return;
        }
        let manager = cx.entity().clone();
        let view = cx.new(|cx| crate::home::HomeView::new(manager, cx));
        let left = Endpoint::local("/");
        let right = Endpoint::local("/");
        let config = SessionConfig {
            name: "Home".to_string(),
            session_type: SessionType::Home,
            left: ProviderRef {
                provider: left.provider_ref(),
                path: left.path().to_path_buf(),
            },
            right: ProviderRef {
                provider: right.provider_ref(),
                path: right.path().to_path_buf(),
            },
            center: None,
            settings: SessionSettings::default(),
        };
        let session = OpenSession {
            tab_id: self.next_tab_id,
            config,
            dirty: false,
            tab_entity: TabEntity::from_home(view),
        };
        self.next_tab_id += 1;
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Add a new empty dir-compare tab and make it active.
    pub fn add_new_folder_tab(&mut self, cx: &mut Context<Self>) {
        let left = Endpoint::local("/");
        let right = Endpoint::local("/");
        let config = SessionConfig {
            name: "untitled".to_string(),
            session_type: SessionType::DirCompare,
            left: ProviderRef {
                provider: left.provider_ref(),
                path: left.path().to_path_buf(),
            },
            right: ProviderRef {
                provider: right.provider_ref(),
                path: right.path().to_path_buf(),
            },
            center: None,
            settings: SessionSettings::default(),
        };
        self.open_folder_tab(config, left, right, cx);
    }

    /// Open a folder-compare tab from a session config and its rebuilt
    /// endpoints, and make it active.
    fn open_folder_tab(
        &mut self,
        config: SessionConfig,
        left: Endpoint,
        right: Endpoint,
        cx: &mut Context<Self>,
    ) {
        // The tab title is the session name; the window title is composed
        // by `WindowRoot` from the compared folder names, so the product
        // name is not baked into the state.
        let title = gpui::SharedString::from(config.name.clone());

        let state = cx.new(|cx| {
            crate::state::AppState::from_config(
                &config, left, right, title, cx,
            )
        });

        let mgr = cx.entity().clone();
        let view =
            cx.new(|cx| crate::ui::FolderCompareView::new(state, mgr, cx));

        let tab_entity = TabEntity::from_folder(view);

        self.record_history(config.clone(), cx);
        let session = OpenSession {
            tab_id: self.next_tab_id,
            config,
            dirty: false,
            tab_entity,
        };
        log::push(
            LogLevel::Info,
            format!("Opened session: {}", session.config.name),
            cx,
        );
        self.next_tab_id += 1;
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Add a text diff tab for the given file endpoints and make it
    /// active.
    pub fn add_text_diff_tab(
        &mut self,
        left: Endpoint,
        right: Endpoint,
        cx: &mut Context<Self>,
    ) {
        let view = crate::text_diff::new_text_diff_view(
            left.clone(),
            right.clone(),
            None,
            cx,
        );

        let config = SessionConfig {
            name: "text diff".to_string(),
            session_type: SessionType::TextCompare,
            left: ProviderRef {
                provider: left.provider_ref(),
                path: left.path().to_path_buf(),
            },
            right: ProviderRef {
                provider: right.provider_ref(),
                path: right.path().to_path_buf(),
            },
            center: None,
            settings: SessionSettings::default(),
        };

        let tab_entity = TabEntity::from_text_diff(view);

        self.record_history(config.clone(), cx);
        let session = OpenSession {
            tab_id: self.next_tab_id,
            config,
            dirty: false,
            tab_entity,
        };
        self.next_tab_id += 1;
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Open a directory compare in a new tab.
    pub fn add_dir_diff_tab(
        &mut self,
        left: Endpoint,
        right: Endpoint,
        cx: &mut Context<Self>,
    ) {
        self.add_folder_tab(left, right, true, cx);
    }

    /// Open a folder-compare tab for the given directories and make it
    /// active.
    ///
    /// `content` enables file-content comparison (hashing); without it
    /// the comparison uses metadata only.
    pub fn add_folder_tab(
        &mut self,
        left: Endpoint,
        right: Endpoint,
        content: bool,
        cx: &mut Context<Self>,
    ) {
        let config = SessionConfig {
            name: "dir diff".to_string(),
            session_type: SessionType::DirCompare,
            left: ProviderRef {
                provider: left.provider_ref(),
                path: left.path().to_path_buf(),
            },
            right: ProviderRef {
                provider: right.provider_ref(),
                path: right.path().to_path_buf(),
            },
            center: None,
            settings: SessionSettings {
                compare_files: content,
                ..SessionSettings::default()
            },
        };
        self.open_folder_tab(config, left, right, cx);
    }

    /// Open a tab for a loaded session config, rebuilding both endpoints
    /// from the provider refs recorded in it.
    ///
    /// Remote refs re-resolve without prompting (same rule as
    /// [`Self::build_session`]); a ref that needs credentials fails and the
    /// caller reports the error.
    pub fn open_session_config(
        &mut self,
        config: SessionConfig,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let prompter = NullPrompter;
        let left = Endpoint::from_ref(
            &config.left.provider,
            config.left.path.clone(),
            &prompter,
        )
        .with_context(|| {
            format!(
                "failed to resolve left endpoint: {}",
                config.left.path.display()
            )
        })?;
        let right = Endpoint::from_ref(
            &config.right.provider,
            config.right.path.clone(),
            &prompter,
        )
        .with_context(|| {
            format!(
                "failed to resolve right endpoint: {}",
                config.right.path.display()
            )
        })?;
        self.open_folder_tab(config, left, right, cx);
        Ok(())
    }

    /// Open a tab for a saved session, dispatching on its session type.
    ///
    /// Text-compare configs open the text diff view; everything else
    /// currently opens as a folder compare.
    pub fn open_saved(
        &mut self,
        config: SessionConfig,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        if config.session_type == SessionType::TextCompare {
            let prompter = NullPrompter;
            let left = Endpoint::from_ref(
                &config.left.provider,
                config.left.path.clone(),
                &prompter,
            )
            .with_context(|| {
                format!(
                    "failed to resolve left endpoint: {}",
                    config.left.path.display()
                )
            })?;
            let right = Endpoint::from_ref(
                &config.right.provider,
                config.right.path.clone(),
                &prompter,
            )
            .with_context(|| {
                format!(
                    "failed to resolve right endpoint: {}",
                    config.right.path.display()
                )
            })?;
            self.add_text_diff_tab(left, right, cx);
            return Ok(());
        }
        self.open_session_config(config, cx)
    }

    /// Open a session of `kind` from paths dropped onto a Home card.
    ///
    /// One path is used for both sides, two paths become left and right.
    /// Dropped items are always local, so the endpoints are built directly
    /// and validated with a synchronous metadata read.
    pub fn start_dropped_session(
        &mut self,
        kind: SessionType,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        match resolve_dropped_paths(kind, paths) {
            Ok((left, right)) => match kind {
                SessionType::TextCompare => {
                    self.add_text_diff_tab(left, right, cx);
                }
                _ => {
                    let config = SessionConfig {
                        name: "dir diff".to_string(),
                        session_type: SessionType::DirCompare,
                        left: ProviderRef {
                            provider: left.provider_ref(),
                            path: left.path().to_path_buf(),
                        },
                        right: ProviderRef {
                            provider: right.provider_ref(),
                            path: right.path().to_path_buf(),
                        },
                        center: None,
                        settings: SessionSettings::default(),
                    };
                    self.open_folder_tab(config, left, right, cx);
                }
            },
            Err(message) => {
                log::push(LogLevel::Error, message.clone(), cx);
                crate::menus::notify_error(message, cx);
            }
        }
    }

    /// Return the active tab's content, if any.
    pub fn active_tab_content(&self) -> Option<&TabEntity> {
        self.open_sessions
            .get(self.active_index)
            .map(|s| &s.tab_entity)
    }

    /// Return clones of all tab entities.
    pub fn tab_entities(&self) -> Vec<TabEntity> {
        self.open_sessions
            .iter()
            .map(|s| s.tab_entity.clone())
            .collect()
    }

    /// Update the active session's endpoints.
    pub fn set_active_paths(&mut self, left: Endpoint, right: Endpoint) {
        if let Some(session) = self.open_sessions.get_mut(self.active_index) {
            session.config.left.provider = left.provider_ref();
            session.config.left.path = left.path().to_path_buf();
            session.config.right.provider = right.provider_ref();
            session.config.right.path = right.path().to_path_buf();
        }
    }

    /// Replace the active tab's entity.
    pub fn replace_active_tab(&mut self, entity: TabEntity) {
        if let Some(session) = self.open_sessions.get_mut(self.active_index) {
            session.tab_entity = entity;
        }
    }

    /// Close the session at the given index.
    pub fn close_session(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.open_sessions.len() {
            return;
        }

        let removed = self.open_sessions.remove(index);
        log::push(
            LogLevel::Info,
            format!("Closed session: {}", removed.config.name),
            cx,
        );

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

    /// Close the session at `index`, prompting first when it has unsaved
    /// changes.
    ///
    /// The prompt is an alert dialog because discarding unsaved session
    /// edits cannot be undone.
    pub fn request_close_session(
        &mut self,
        index: usize,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.open_sessions.get(index) else {
            return;
        };
        if !session.dirty {
            self.close_session(index, cx);
            return;
        }
        let name = session.config.name.clone();
        let mgr = cx.entity().clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            // Clone per builder call: the builder is an `Fn` closure that
            // may run on every layer rebuild.
            let mgr = mgr.clone();
            alert
                .confirm()
                .title(format!("Close \"{name}\"?"))
                .description(
                    "This session has unsaved changes. Closing discards them.",
                )
                .ok_text("Close without saving")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    mgr.update(cx, |m, cx| m.close_session(index, cx));
                    true
                })
        });
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
        let config = match self.active_config() {
            Some(c) => c.clone(),
            None => return cx.background_spawn(async { Ok(()) }),
        };
        let session_dir = self.session_dir.clone();
        let name = config.name.clone();

        // Spawn the work on tokio and await the JoinHandle inside a gpui
        // Task. Awaiting a JoinHandle does not require a tokio reactor.
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let file_name = format!("{}.bcs", sanitize_file_name(&name));

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
        });

        cx.background_spawn(async move {
            join_handle
                .await
                .map_err(|e| anyhow::anyhow!("task panicked: {e}"))?
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
        let config = self.active_config().cloned();
        let session_dir = self.session_dir.clone();

        let join_handle = crate::runtime::spawn_on_tokio(async move {
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
        });

        cx.background_spawn(async move {
            join_handle
                .await
                .map_err(|e| anyhow::anyhow!("task panicked: {e}"))?
        })
    }

    /// Load a session from a `.bcs` file path.
    pub fn load_session_file(
        &self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<Result<SessionConfig>> {
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            SessionConfig::load_from_file(&path)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        });

        cx.background_spawn(async move {
            join_handle
                .await
                .map_err(|e| anyhow::anyhow!("task panicked: {e}"))?
        })
    }

    /// Load saved sessions from disk, together with the default marker.
    pub fn load_recent_sessions(&self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let session_dir = self.session_dir.clone();

        // Spawn the filesystem I/O on the tokio runtime.
        let load_task = crate::runtime::spawn_on_tokio(async move {
            Ok::<_, anyhow::Error>(reload_saved(&session_dir).await)
        });

        // Observe the I/O task and update the GUI when complete.
        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                // Silently ignore errors when loading saved sessions.
                if let Ok(Ok((sessions, default_name))) = load_task.await {
                    async_app.update(|cx| {
                        if let Some(manager) = weak.upgrade() {
                            manager.update(cx, |m, cx| {
                                m.recent_sessions = sessions;
                                m.default_name = default_name;
                                cx.notify();
                            });
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// Load the auto-saved history of recently opened sessions, newest
    /// first.
    pub fn load_history(&self, cx: &mut Context<Self>) {
        let weak = cx.entity().downgrade();
        let history_dir = self.history_dir.clone();

        let load_task = crate::runtime::spawn_on_tokio(async move {
            let mut entries = read_bcs_sessions(&history_dir).await;
            entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
            Ok::<_, anyhow::Error>(
                entries
                    .into_iter()
                    .take(HISTORY_LIMIT)
                    .map(|(_, saved)| saved)
                    .collect::<Vec<_>>(),
            )
        });

        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                if let Ok(Ok(history)) = load_task.await {
                    async_app.update(|cx| {
                        if let Some(manager) = weak.upgrade() {
                            manager.update(cx, |m, cx| {
                                m.history = history;
                                cx.notify();
                            });
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// Persist a snapshot of an opened session into the history directory,
    /// keeping only the newest [`HISTORY_LIMIT`] entries.
    ///
    /// Placeholder sessions (Home, untitled) are not worth reopening, so
    /// they are never recorded.
    fn record_history(
        &mut self,
        config: SessionConfig,
        cx: &mut Context<Self>,
    ) {
        if config.session_type == SessionType::Home
            || config.name == "untitled"
            || config.name.is_empty()
        {
            return;
        }
        let history_dir = self.history_dir.clone();
        let path = history_dir.join(format!(
            "{}-{}.bcs",
            sanitize_file_name(&config.name),
            unix_now_secs()
        ));
        let task_config = config.clone();
        let saved = SavedSession {
            config,
            path: path.clone(),
        };

        let join_handle = crate::runtime::spawn_on_tokio(async move {
            fs_err::tokio::create_dir_all(&history_dir)
                .await
                .with_context(|| {
                    format!(
                        "failed to create history directory: {:?}",
                        history_dir
                    )
                })?;
            task_config.save_to_file(&path).await.with_context(|| {
                format!("failed to save history snapshot to {:?}", path)
            })?;
            prune_history(&history_dir).await
        });

        // Reflect the snapshot immediately; a failed write is corrected by
        // reloading from disk when the task reports the error.
        self.history.insert(0, saved);
        self.history.truncate(HISTORY_LIMIT);
        cx.notify();

        let weak = cx.entity().downgrade();
        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let cx = cx.clone();
            async move {
                let result = join_handle
                    .await
                    .map_err(|e| anyhow::anyhow!("task panicked: {e}"))
                    .and_then(|r| r);
                if let Err(e) = result {
                    cx.update(|cx| {
                        log::push(
                            LogLevel::Error,
                            format!("Failed to record session history: {e}"),
                            cx,
                        );
                        if let Some(manager) = weak.upgrade() {
                            manager.update(cx, |m, cx| m.load_history(cx));
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// Delete a saved session file and refresh the saved list.
    pub fn delete_saved_session(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let name = self
            .recent_sessions
            .iter()
            .find(|s| s.path == path)
            .map(|s| s.config.name.clone())
            .unwrap_or_else(|| path.display().to_string());
        let session_dir = self.session_dir.clone();

        let join_handle = crate::runtime::spawn_on_tokio(async move {
            fs_err::tokio::remove_file(&path).await.with_context(|| {
                format!("failed to delete session file: {:?}", path)
            })?;
            Ok::<_, anyhow::Error>(reload_saved(&session_dir).await)
        });

        let weak = cx.entity().downgrade();
        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let cx = cx.clone();
            async move {
                match join_handle
                    .await
                    .map_err(|e| anyhow::anyhow!("task panicked: {e}"))
                    .and_then(|r| r)
                {
                    Ok((sessions, default_name)) => {
                        cx.update(|cx| {
                            log::push(
                                LogLevel::Info,
                                format!("Deleted session: {name}"),
                                cx,
                            );
                            if let Some(manager) = weak.upgrade() {
                                manager.update(cx, |m, cx| {
                                    m.recent_sessions = sessions;
                                    m.default_name = default_name;
                                    cx.notify();
                                });
                            }
                        });
                    }
                    Err(e) => {
                        let message =
                            format!("Couldn't delete the session. {e}");
                        cx.update(|cx| {
                            log::push(LogLevel::Error, message.clone(), cx);
                            crate::menus::notify_error(message, cx);
                        });
                    }
                }
            }
        })
        .detach();
    }

    /// Rename a saved session: rewrite the config under a sanitized file
    /// name, remove the old file, and refresh the saved list.
    pub fn rename_saved_session(
        &mut self,
        old_path: PathBuf,
        new_name: String,
        cx: &mut Context<Self>,
    ) {
        let session_dir = self.session_dir.clone();
        let task_name = new_name.clone();

        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let mut config = SessionConfig::load_from_file(&old_path)
                .await
                .with_context(|| {
                    format!("failed to load session file: {:?}", old_path)
                })?;
            config.name = task_name.clone();
            let new_path = session_dir
                .join(format!("{}.bcs", sanitize_file_name(&task_name)));
            config.save_to_file(&new_path).await.with_context(|| {
                format!("failed to save renamed session to {:?}", new_path)
            })?;
            if new_path != old_path {
                let _ = fs_err::tokio::remove_file(&old_path).await;
                // Keep the default marker pointing at the renamed file.
                let old_file = old_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if read_default_marker(&session_dir).await == Some(old_file) {
                    let new_file = new_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let _ = fs_err::tokio::write(
                        session_dir.join(DEFAULT_MARKER_FILE),
                        new_file,
                    )
                    .await;
                }
            }
            Ok::<_, anyhow::Error>(reload_saved(&session_dir).await)
        });

        let weak = cx.entity().downgrade();
        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let cx = cx.clone();
            async move {
                match join_handle
                    .await
                    .map_err(|e| anyhow::anyhow!("task panicked: {e}"))
                    .and_then(|r| r)
                {
                    Ok((sessions, default_name)) => {
                        cx.update(|cx| {
                            log::push(
                                LogLevel::Info,
                                format!("Renamed session to: {new_name}"),
                                cx,
                            );
                            if let Some(manager) = weak.upgrade() {
                                manager.update(cx, |m, cx| {
                                    m.recent_sessions = sessions;
                                    m.default_name = default_name;
                                    cx.notify();
                                });
                            }
                        });
                    }
                    Err(e) => {
                        let message =
                            format!("Couldn't rename the session. {e}");
                        cx.update(|cx| {
                            log::push(LogLevel::Error, message.clone(), cx);
                            crate::menus::notify_error(message, cx);
                        });
                    }
                }
            }
        })
        .detach();
    }

    /// Mark a saved session as the default, clearing the mark when it is
    /// already the default one.
    pub fn set_default_session(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(file_name) =
            path.file_name().map(|n| n.to_string_lossy().into_owned())
        else {
            return;
        };
        let is_current = self.default_name.as_deref() == Some(&file_name[..]);
        let new_default = (!is_current).then(|| file_name.clone());
        let marker_path = self.session_dir.join(DEFAULT_MARKER_FILE);
        let write_value = new_default.clone();

        let join_handle = crate::runtime::spawn_on_tokio(async move {
            match write_value {
                Some(name) => {
                    fs_err::tokio::write(&marker_path, name)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to write default marker: {:?}",
                                marker_path
                            )
                        })?;
                }
                None => {
                    let _ = fs_err::tokio::remove_file(&marker_path).await;
                }
            }
            Ok::<_, anyhow::Error>(())
        });

        // Reflect the change immediately; a failed write is corrected by
        // reloading the marker from disk when the task reports the error.
        self.default_name = new_default;
        cx.notify();

        let weak = cx.entity().downgrade();
        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let cx = cx.clone();
            async move {
                let result = join_handle
                    .await
                    .map_err(|e| anyhow::anyhow!("task panicked: {e}"))
                    .and_then(|r| r);
                if let Err(e) = result {
                    cx.update(|cx| {
                        log::push(
                            LogLevel::Error,
                            format!("Failed to set the default session: {e}"),
                            cx,
                        );
                        if let Some(manager) = weak.upgrade() {
                            manager.update(cx, |m, cx| {
                                m.load_recent_sessions(cx)
                            });
                        }
                    });
                }
            }
        })
        .detach();
    }

    /// Build a runtime [`Session`] from a config, rebuilding the providers
    /// from the refs recorded in it.
    ///
    /// Remote refs re-resolve without prompting: the GUI has no reliable
    /// terminal, so credentials must come from a profile, the environment,
    /// or the keychain.
    pub async fn build_session(
        &self,
        config: &SessionConfig,
    ) -> Result<Session> {
        let prompter = NullPrompter;
        let left = Endpoint::from_ref(
            &config.left.provider,
            config.left.path.clone(),
            &prompter,
        )?;
        let right = Endpoint::from_ref(
            &config.right.provider,
            config.right.path.clone(),
            &prompter,
        )?;

        let left_fs: Arc<AnyNodeFs> = left.fs_arc();
        let right_fs: Arc<AnyNodeFs> = right.fs_arc();
        let center_fs: Option<Arc<AnyNodeFs>> = match &config.center {
            Some(center) => Some(
                Endpoint::from_ref(
                    &center.provider,
                    center.path.clone(),
                    &prompter,
                )?
                .fs_arc(),
            ),
            None => None,
        };

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
        mgr.load_history(cx);
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

/// Turn a session name into a safe file-name stem.
fn sanitize_file_name(name: &str) -> String {
    if name == "untitled" || name.is_empty() {
        return "untitled".to_string();
    }
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Current wall-clock seconds since the Unix epoch, used to keep history
/// file names unique.
fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Read every loadable `.bcs` file in `dir` with its modification time.
async fn read_bcs_sessions(dir: &Path) -> Vec<(SystemTime, SavedSession)> {
    let mut found = Vec::new();
    let Ok(mut entries) = fs_err::tokio::read_dir(dir).await else {
        return found;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("bcs") {
            continue;
        }
        if let Ok(config) = SessionConfig::load_from_file(&path).await {
            let mtime = entry
                .metadata()
                .await
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            found.push((mtime, SavedSession { config, path }));
        }
    }
    found
}

/// Read the default-session marker, if present.
async fn read_default_marker(session_dir: &Path) -> Option<String> {
    let name =
        fs_err::tokio::read_to_string(session_dir.join(DEFAULT_MARKER_FILE))
            .await
            .ok()?;
    let name = name.trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Reload the saved sessions (sorted by name) and the default marker.
async fn reload_saved(
    session_dir: &Path,
) -> (Vec<SavedSession>, Option<String>) {
    let mut sessions = read_bcs_sessions(session_dir).await;
    sessions.sort_by(|a, b| a.1.config.name.cmp(&b.1.config.name));
    let default_name = read_default_marker(session_dir).await;
    (
        sessions.into_iter().map(|(_, saved)| saved).collect(),
        default_name,
    )
}

/// Remove history snapshots beyond the newest [`HISTORY_LIMIT`].
async fn prune_history(dir: &Path) -> Result<()> {
    let mut entries = read_bcs_sessions(dir).await;
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    for (_, stale) in entries.iter().skip(HISTORY_LIMIT) {
        let _ = fs_err::tokio::remove_file(&stale.path).await;
    }
    Ok(())
}

/// Validate dropped paths for a session kind and build local endpoints.
///
/// One path is used for both sides, two paths become left and right;
/// anything else is rejected. Folder kinds require directories, text kinds
/// require files.
fn resolve_dropped_paths(
    kind: SessionType,
    paths: &[PathBuf],
) -> std::result::Result<(Endpoint, Endpoint), String> {
    if paths.is_empty() || paths.len() > 2 {
        return Err(
            "Drop one or two items onto the card to start a session.".into()
        );
    }
    let (left, right) = if paths.len() == 1 {
        (&paths[0], &paths[0])
    } else {
        (&paths[0], &paths[1])
    };
    let want_dir = kind != SessionType::TextCompare;
    for path in [left, right] {
        let metadata = fs_err::metadata(path)
            .map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
        if want_dir && !metadata.is_dir() {
            return Err(format!("{} is not a directory.", path.display()));
        }
        if !want_dir && !metadata.is_file() {
            return Err(format!("{} is not a file.", path.display()));
        }
    }
    Ok((
        Endpoint::local(left.clone()),
        Endpoint::local(right.clone()),
    ))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui::TestAppContext;

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
    fn manager_new_starts_empty() {
        let mgr = GuiSessionManager::new(PathBuf::from("/tmp/test_sessions"));
        assert!(mgr.is_empty());
        assert!(mgr.active_config().is_none());
    }

    #[gpui::test]
    fn open_home_opens_a_home_tab(cx: &mut TestAppContext) {
        let mgr = cx.new(|_| {
            GuiSessionManager::new(
                std::env::temp_dir().join("cocomo-test-home"),
            )
        });
        mgr.update(cx, |m, cx| m.open_home(cx));
        mgr.update(cx, |m, _| {
            assert_eq!(m.len(), 1);
            assert_eq!(m.active_index(), 0);
            let config = m.active_config().unwrap();
            assert_eq!(config.session_type, SessionType::Home);
            assert!(!m.active_session().unwrap().dirty);
        });
    }

    #[gpui::test]
    fn open_home_reuses_the_existing_home_tab(cx: &mut TestAppContext) {
        let mgr = cx.new(|_| {
            GuiSessionManager::new(
                std::env::temp_dir().join("cocomo-test-home-reuse"),
            )
        });
        mgr.update(cx, |m, cx| m.open_home(cx));
        mgr.update(cx, |m, cx| m.open_home(cx));
        mgr.update(cx, |m, _| assert_eq!(m.len(), 1));
    }

    #[test]
    fn sanitize_file_name_replaces_unsafe_characters() {
        assert_eq!(sanitize_file_name("my session/v1"), "my_session_v1");
        assert_eq!(sanitize_file_name("keep-me_2"), "keep-me_2");
        assert_eq!(sanitize_file_name("untitled"), "untitled");
        assert_eq!(sanitize_file_name(""), "untitled");
    }

    #[test]
    fn dropped_single_directory_serves_both_sides() {
        let dir = tempfile::tempdir().unwrap();
        let (left, right) = resolve_dropped_paths(
            SessionType::DirCompare,
            &[dir.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(left.path(), dir.path());
        assert_eq!(right.path(), dir.path());
    }

    #[test]
    fn dropped_pair_maps_left_to_right() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        fs_err::create_dir(&left).unwrap();
        fs_err::create_dir(&right).unwrap();
        let (l, r) = resolve_dropped_paths(
            SessionType::DirCompare,
            &[left.clone(), right.clone()],
        )
        .unwrap();
        assert_eq!(l.path(), left);
        assert_eq!(r.path(), right);
    }

    #[test]
    fn dropped_files_are_rejected_for_folder_sessions() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("f.txt");
        fs_err::write(&file, b"x").unwrap();
        let err = resolve_dropped_paths(
            SessionType::DirCompare,
            std::slice::from_ref(&file),
        )
        .unwrap_err();
        assert!(err.contains("not a directory"), "unexpected error: {err}");
    }

    #[test]
    fn dropped_directories_are_rejected_for_text_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve_dropped_paths(
            SessionType::TextCompare,
            &[dir.path().to_path_buf()],
        )
        .unwrap_err();
        assert!(err.contains("not a file"), "unexpected error: {err}");
    }

    #[test]
    fn too_many_dropped_paths_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let paths: Vec<PathBuf> =
            (0..3).map(|i| root.path().join(format!("p{i}"))).collect();
        let err = resolve_dropped_paths(SessionType::DirCompare, &paths)
            .unwrap_err();
        assert!(err.contains("one or two items"), "unexpected error: {err}");
    }

    #[tokio::test]
    async fn prune_history_keeps_the_newest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let base = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for i in 0..(HISTORY_LIMIT as u64 + 2) {
            let path = dir.path().join(format!("s{i}.bcs"));
            config_named(&format!("s{i}"))
                .save_to_file(&path)
                .await
                .unwrap();
            let file =
                std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_modified(base + Duration::from_secs(i)).unwrap();
        }

        prune_history(dir.path()).await.unwrap();

        let remaining = fs_err::read_dir(dir.path()).unwrap().count();
        assert_eq!(remaining, HISTORY_LIMIT);
        // The two oldest snapshots are pruned; the newest survives.
        assert!(!dir.path().join("s0.bcs").exists());
        assert!(!dir.path().join("s1.bcs").exists());
        assert!(
            dir.path()
                .join(format!("s{}.bcs", HISTORY_LIMIT + 1))
                .exists()
        );
    }

    #[tokio::test]
    async fn reload_saved_sorts_by_name_and_reads_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        config_named("zeta")
            .save_to_file(&dir.path().join("zeta.bcs"))
            .await
            .unwrap();
        config_named("alpha")
            .save_to_file(&dir.path().join("alpha.bcs"))
            .await
            .unwrap();
        fs_err::write(dir.path().join(DEFAULT_MARKER_FILE), "alpha.bcs\n")
            .unwrap();

        let (sessions, default_name) = reload_saved(dir.path()).await;
        let names: Vec<&str> =
            sessions.iter().map(|s| s.config().name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
        assert_eq!(default_name.as_deref(), Some("alpha.bcs"));
    }

    #[tokio::test]
    async fn reload_saved_tolerates_a_missing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        let (sessions, default_name) = reload_saved(&missing).await;
        assert!(sessions.is_empty());
        assert_eq!(default_name, None);
    }
}
