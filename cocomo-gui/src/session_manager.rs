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
//! Two collaborating entities:
//!
//! - [`SavedSessionStore`] owns everything that outlives a window: the session
//!   directory, the saved-session list, the auto-saved history, and the
//!   default-session marker. There is exactly one per application run,
//!   reachable through [`shared_store`], because every window reads and writes
//!   the same files.
//! - [`GuiSessionManager`] owns one window's open tabs. Each window has its
//!   own manager, which is what makes moving a tab to another window a matter
//!   of handing an [`OpenSession`] from one manager to the next.
//!
//! Tab ids come from a process-wide counter so they stay unique across
//! windows; two managers must never hand out the same id.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result};
use cocomo_lib::{
    Endpoint, FsError, NullPrompter, ProviderRef, Session, SessionConfig,
    SessionSettings, SessionType,
};
use gpui::{
    App, AppContext as _, Context, Entity, Global, Task,
    component::{WindowExt as _, button::ButtonVariant},
};

use crate::{
    log::{self, LogLevel},
    tabview::TabEntity,
    workspace::WindowWorkspace,
};

/// Type-erased node filesystem, matching what [`Session`] accepts.
#[allow(dead_code)]
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

/// Process-wide tab-id source, so ids never collide between windows.
static NEXT_TAB_ID: AtomicUsize = AtomicUsize::new(1);

/// The next unique tab id.
fn next_tab_id() -> usize {
    NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed)
}

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

// ---------------------------------------------------------------------------
// Shared persistence
// ---------------------------------------------------------------------------

/// The window-independent session persistence, shared by every window's
/// [`GuiSessionManager`].
///
/// Saved sessions, the auto-saved history, and the default marker live on
/// disk once per application; keeping them in one entity means two windows
/// cannot disagree about what is saved.
pub struct SavedSessionStore {
    /// Directory where `.bcs` session files are stored.
    session_dir: PathBuf,
    /// Directory where auto-saved history snapshots are stored.
    history_dir: PathBuf,
    /// Cached saved session configs loaded from disk, sorted by name.
    recent_sessions: Vec<SavedSession>,
    /// Auto-saved history of recently opened sessions, newest first.
    history: Vec<SavedSession>,
    /// File name of the saved session marked as default, if any.
    default_name: Option<String>,
}

// `build_session` and `session_dir` are the store's API for later phases;
// nothing calls them yet.
#[allow(dead_code)]
impl SavedSessionStore {
    /// Create a store that persists sessions in the given directory.
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
            recent_sessions: Vec::new(),
            history: Vec::new(),
            default_name: None,
        }
    }

    /// Return the session directory.
    pub fn session_dir(&self) -> &Path {
        &self.session_dir
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

    /// Persist `config` to its file, derived from its name.
    pub fn save_config(
        &self,
        config: &SessionConfig,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let config = config.clone();
        let session_dir = self.session_dir.clone();
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let file_name =
                format!("{}.bcs", sanitize_file_name(&config.name));
            let path = session_dir.join(&file_name);

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

        let _ = cx;
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

        let _ = cx;
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
                        if let Some(store) = weak.upgrade() {
                            store.update(cx, |s, cx| {
                                s.recent_sessions = sessions;
                                s.default_name = default_name;
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
                        if let Some(store) = weak.upgrade() {
                            store.update(cx, |s, cx| {
                                s.history = history;
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
    pub fn record_history(
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
                        if let Some(store) = weak.upgrade() {
                            store.update(cx, |s, cx| s.load_history(cx));
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
                            if let Some(store) = weak.upgrade() {
                                store.update(cx, |s, cx| {
                                    s.recent_sessions = sessions;
                                    s.default_name = default_name;
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
                            if let Some(store) = weak.upgrade() {
                                store.update(cx, |s, cx| {
                                    s.recent_sessions = sessions;
                                    s.default_name = default_name;
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
                        if let Some(store) = weak.upgrade() {
                            store.update(cx, |s, cx| {
                                s.load_recent_sessions(cx)
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

/// Handle wrapper that keeps the single shared store reachable globally.
struct SharedStoreGlobal(Entity<SavedSessionStore>);

impl Global for SharedStoreGlobal {}

/// Return the application's shared session store, creating it on first use.
pub fn shared_store(cx: &mut App) -> Entity<SavedSessionStore> {
    if let Some(global) = cx.try_global::<SharedStoreGlobal>() {
        return global.0.clone();
    }
    let session_dir = default_session_dir();
    let _ = std::fs::create_dir_all(&session_dir);
    let store = cx.new(|_| SavedSessionStore::new(session_dir));
    store.update(cx, |s, cx| {
        s.load_recent_sessions(cx);
        s.load_history(cx);
    });
    cx.set_global(SharedStoreGlobal(store.clone()));
    store
}

// ---------------------------------------------------------------------------
// Per-window tab manager
// ---------------------------------------------------------------------------

/// Manages one window's open tabs.
///
/// This is a gpui [`Entity`] that can be observed by the UI. It holds the
/// list of open sessions (tabs), tracks the active tab, and delegates
/// everything persistent to the shared [`SavedSessionStore`].
pub struct GuiSessionManager {
    /// The application-wide saved-session store.
    store: Entity<SavedSessionStore>,
    /// Currently open sessions.
    open_sessions: Vec<OpenSession>,
    /// Index of the active session in [`Self::open_sessions`].
    active_index: usize,
}

/// The manager's full tab API. A few accessors exist for the views and
/// for commands that arrive in later phases, so the whole block stays
/// allow-listed rather than trimmed to today's call sites.
#[allow(dead_code)]
impl GuiSessionManager {
    /// Create a manager for one window, backed by the shared store.
    pub fn new(store: Entity<SavedSessionStore>) -> Self {
        Self {
            store,
            open_sessions: Vec::new(),
            active_index: 0,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    /// Return the open sessions' tab ids, names, and dirty flags.
    pub fn open_tab_ids(&self) -> Vec<(usize, String, bool)> {
        self.open_sessions
            .iter()
            .map(|s| (s.tab_id, s.config.name.clone(), s.dirty))
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

    /// Whether any open session has unsaved changes.
    pub fn has_dirty(&self) -> bool {
        self.open_sessions.iter().any(|s| s.dirty)
    }

    /// The configs of the sessions with unsaved changes.
    pub fn dirty_configs(&self) -> Vec<SessionConfig> {
        self.open_sessions
            .iter()
            .filter(|s| s.dirty)
            .map(|s| s.config.clone())
            .collect()
    }

    /// Handle to the shared saved-session store.
    #[allow(dead_code)]
    pub fn store(&self) -> &Entity<SavedSessionStore> {
        &self.store
    }

    /// Return the saved sessions loaded from disk, sorted by name.
    pub fn recent_sessions(&self, cx: &App) -> Vec<SavedSession> {
        self.store.read(cx).recent_sessions().to_vec()
    }

    /// Return the auto-saved history of recently opened sessions, newest
    /// first.
    pub fn history(&self, cx: &App) -> Vec<SavedSession> {
        self.store.read(cx).history().to_vec()
    }

    /// Return the file name of the saved session marked as default.
    pub fn default_session_name(&self, cx: &App) -> Option<String> {
        self.store
            .read(cx)
            .default_session_name()
            .map(str::to_owned)
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
            tab_id: next_tab_id(),
            config,
            dirty: false,
            tab_entity: TabEntity::from_home(view),
        };
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
            tab_id: next_tab_id(),
            config,
            dirty: false,
            tab_entity,
        };
        log::push(
            LogLevel::Info,
            format!("Opened session: {}", session.config.name),
            cx,
        );
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Add a text diff tab for the given file endpoints and make it
    /// active. `parent` is the folder session the pair was selected in,
    /// enabling "copy file and open next difference" from the diff tab.
    pub fn add_text_diff_tab(
        &mut self,
        left: Endpoint,
        right: Endpoint,
        parent: Option<gpui::WeakEntity<crate::state::AppState>>,
        cx: &mut Context<Self>,
    ) {
        let mgr = cx.entity().clone();
        let view = crate::text_diff::new_text_diff_view(
            left.clone(),
            right.clone(),
            None,
            parent,
            Some(mgr),
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
            tab_id: next_tab_id(),
            config,
            dirty: false,
            tab_entity,
        };
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Add a text merge tab for the given file endpoints and make it
    /// active. `ancestor` is the common base for a three-way merge; `None`
    /// runs a two-way merge where any difference is a conflict.
    pub fn add_text_merge_tab(
        &mut self,
        left: Endpoint,
        right: Endpoint,
        ancestor: Option<Endpoint>,
        cx: &mut Context<Self>,
    ) {
        let mgr = cx.entity().clone();
        let view = crate::text_merge::new_text_merge_view(
            left.clone(),
            right.clone(),
            ancestor.clone(),
            Some(mgr),
            cx,
        );

        let config = SessionConfig {
            name: "text merge".to_string(),
            session_type: SessionType::TextMerge,
            left: ProviderRef {
                provider: left.provider_ref(),
                path: left.path().to_path_buf(),
            },
            right: ProviderRef {
                provider: right.provider_ref(),
                path: right.path().to_path_buf(),
            },
            center: ancestor.map(|a| ProviderRef {
                provider: a.provider_ref(),
                path: a.path().to_path_buf(),
            }),
            settings: SessionSettings::default(),
        };

        let tab_entity = TabEntity::from_text_merge(view);

        self.record_history(config.clone(), cx);
        let session = OpenSession {
            tab_id: next_tab_id(),
            config,
            dirty: false,
            tab_entity,
        };
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// Add a text diff tab over two in-memory texts ("compare selection
    /// to clipboard") and make it active.
    pub fn add_text_diff_tab_from_texts(
        &mut self,
        left_text: &str,
        right_text: &str,
        title: gpui::SharedString,
        cx: &mut Context<Self>,
    ) {
        let mgr = cx.entity().clone();
        let view = crate::text_diff::new_text_diff_view_from_texts(
            left_text,
            right_text,
            title,
            Some(mgr),
            cx,
        );
        let tab_entity = TabEntity::from_text_diff(view);
        let session = OpenSession {
            tab_id: next_tab_id(),
            config: SessionConfig {
                name: "selection vs clipboard".to_string(),
                session_type: SessionType::TextCompare,
                left: ProviderRef {
                    provider: Default::default(),
                    path: "selection".into(),
                },
                right: ProviderRef {
                    provider: Default::default(),
                    path: "clipboard".into(),
                },
                center: None,
                settings: SessionSettings::default(),
            },
            dirty: false,
            tab_entity,
        };
        self.open_sessions.push(session);
        self.active_index = self.open_sessions.len() - 1;
        cx.notify();
    }

    /// The active tab's text compare session, if it is one.
    pub fn active_text_session(
        &self,
        cx: &gpui::App,
    ) -> Option<gpui::Entity<crate::text_diff::TextDiffSession>> {
        self.open_sessions
            .get(self.active_index)
            .and_then(|s| s.tab_entity.text_state(cx))
    }

    /// The active tab's folder compare session, if it is one.
    pub fn active_folder_session(
        &self,
        cx: &gpui::App,
    ) -> Option<gpui::Entity<crate::state::AppState>> {
        self.open_sessions
            .get(self.active_index)
            .and_then(|s| s.tab_entity.folder_state(cx))
    }

    /// The active tab's text merge session, if it is one.
    pub fn active_merge_session(
        &self,
        cx: &gpui::App,
    ) -> Option<gpui::Entity<crate::text_merge::MergeSession>> {
        self.open_sessions
            .get(self.active_index)
            .and_then(|s| s.tab_entity.merge_state(cx))
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
    /// [`SavedSessionStore::build_session`]); a ref that needs credentials
    /// fails and the caller reports the error.
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
    /// Text-compare and text-merge configs open their respective views;
    /// everything else currently opens as a folder compare.
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
            self.add_text_diff_tab(left, right, None, cx);
            return Ok(());
        }
        if config.session_type == SessionType::TextMerge {
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
            let ancestor = config
                .center
                .as_ref()
                .map(|center| {
                    Endpoint::from_ref(
                        &center.provider,
                        center.path.clone(),
                        &prompter,
                    )
                    .with_context(|| {
                        format!(
                            "failed to resolve ancestor endpoint: {}",
                            center.path.display()
                        )
                    })
                })
                .transpose()?;
            self.add_text_merge_tab(left, right, ancestor, cx);
            return Ok(());
        }
        self.open_session_config(config, cx)
    }

    /// Open a session of `kind` from paths dropped onto a Home card.
    ///
    /// One path is used for both sides, two paths become left and right
    /// (for a merge: left, ancestor, right with three). Dropped items are
    /// always local, so the endpoints are built directly and validated
    /// with a synchronous metadata read.
    pub fn start_dropped_session(
        &mut self,
        kind: SessionType,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        match resolve_dropped_paths(kind, paths) {
            Ok((left, ancestor, right)) => match kind {
                SessionType::TextCompare => {
                    self.add_text_diff_tab(left, right, None, cx);
                }
                SessionType::TextMerge => {
                    self.add_text_merge_tab(left, right, ancestor, cx);
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
        self.adjust_active_index(index);
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
    // Moving tabs between windows
    // -----------------------------------------------------------------------

    /// Return the index of the tab with `tab_id`, if it is open here.
    pub fn session_index(&self, tab_id: usize) -> Option<usize> {
        self.open_sessions.iter().position(|s| s.tab_id == tab_id)
    }

    /// Remove the tab at `index` and hand it to the caller.
    ///
    /// The tab's views still point back at this manager; the receiving
    /// manager re-points them through [`TabEntity::set_manager`].
    pub fn take_session(&mut self, index: usize) -> Option<OpenSession> {
        if index >= self.open_sessions.len() {
            return None;
        }
        let session = self.open_sessions.remove(index);
        self.adjust_active_index(index);
        Some(session)
    }

    /// Add an existing tab, re-pointing its views at this manager.
    ///
    /// `index` is where to insert it; `None` appends. The moved tab becomes
    /// the active one, which is what a "move to new window" should feel
    /// like from the user's side.
    pub fn insert_session(
        &mut self,
        session: OpenSession,
        index: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let mgr = cx.entity().clone();
        session.tab_entity.set_manager(mgr, cx);
        let at = index
            .unwrap_or(self.open_sessions.len())
            .min(self.open_sessions.len());
        self.open_sessions.insert(at, session);
        self.active_index = at;
        cx.notify();
    }

    /// Reorder a tab within this window.
    pub fn move_session(
        &mut self,
        from: usize,
        to: usize,
        cx: &mut Context<Self>,
    ) {
        let len = self.open_sessions.len();
        if from >= len || to >= len || from == to {
            return;
        }
        let active_id = self.open_sessions[self.active_index].tab_id;
        let session = self.open_sessions.remove(from);
        self.open_sessions.insert(to, session);
        if let Some(index) = self
            .open_sessions
            .iter()
            .position(|s| s.tab_id == active_id)
        {
            self.active_index = index;
        }
        cx.notify();
    }

    /// Keep the active index valid after removing the entry at `index`.
    fn adjust_active_index(&mut self, index: usize) {
        if self.open_sessions.is_empty() {
            self.active_index = 0;
        } else if self.active_index >= self.open_sessions.len() {
            self.active_index = self.open_sessions.len() - 1;
        } else if index < self.active_index {
            self.active_index -= 1;
        }
    }

    // -----------------------------------------------------------------------
    // Workspace
    // -----------------------------------------------------------------------

    /// Snapshot this window's tabs for workspace persistence.
    ///
    /// Home tabs are launchers, not sessions, so they are left out; the
    /// active index is recomputed against the filtered list.
    pub fn workspace_snapshot(&self) -> WindowWorkspace {
        let mut tabs = Vec::new();
        let mut active_index = 0;
        for (index, session) in self.open_sessions.iter().enumerate() {
            if session.tab_entity.is_home() {
                continue;
            }
            if index == self.active_index {
                active_index = tabs.len();
            }
            tabs.push(session.config.clone());
        }
        WindowWorkspace::new(tabs, active_index)
    }

    /// Open every tab recorded in `snapshot`, returning how many opened.
    ///
    /// A tab whose endpoints no longer resolve is logged and skipped, so
    /// one stale entry cannot swallow the rest of the workspace.
    pub fn restore_workspace_tabs(
        &mut self,
        snapshot: &WindowWorkspace,
        cx: &mut Context<Self>,
    ) -> usize {
        let mut restored = 0;
        for config in &snapshot.tabs {
            match self.open_saved(config.clone(), cx) {
                Ok(()) => restored += 1,
                Err(e) => {
                    log::push(
                        LogLevel::Warning,
                        format!("Couldn't restore {}: {e}", config.name),
                        cx,
                    );
                }
            }
        }
        if !snapshot.tabs.is_empty() {
            let index =
                snapshot.active_index.min(self.len().saturating_sub(1));
            self.activate_session(index, cx);
        }
        restored
    }

    // -----------------------------------------------------------------------
    // Save / Load (delegated to the shared store)
    // -----------------------------------------------------------------------

    /// Save the active session to its file.
    pub fn save_active_session(
        &self,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let Some(config) = self.active_config() else {
            return cx.background_spawn(async { Ok(()) });
        };
        let config = config.clone();
        self.store
            .update(cx, |store, cx| store.save_config(&config, cx))
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
        let Some(config) = self.active_config() else {
            return cx.background_spawn(async { Ok(()) });
        };
        let config = config.clone();
        self.store
            .update(cx, |store, cx| store.save_config(&config, cx))
    }

    /// Load a session from a `.bcs` file path.
    pub fn load_session_file(
        &self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Task<Result<SessionConfig>> {
        self.store
            .update(cx, |store, cx| store.load_session_file(path, cx))
    }

    /// Delete a saved session file and refresh the saved list.
    pub fn delete_saved_session(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.store
            .update(cx, |store, cx| store.delete_saved_session(path, cx));
    }

    /// Rename a saved session and refresh the saved list.
    pub fn rename_saved_session(
        &mut self,
        old_path: PathBuf,
        new_name: String,
        cx: &mut Context<Self>,
    ) {
        self.store.update(cx, |store, cx| {
            store.rename_saved_session(old_path, new_name, cx);
        });
    }

    /// Mark a saved session as the default, clearing the mark when it is
    /// already the default one.
    pub fn set_default_session(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.store
            .update(cx, |store, cx| store.set_default_session(path, cx));
    }

    /// Record an opened session in the shared history.
    fn record_history(
        &mut self,
        config: SessionConfig,
        cx: &mut Context<Self>,
    ) {
        self.store
            .update(cx, |store, cx| store.record_history(config, cx));
    }
}

/// Create a session manager for a new window, backed by the shared store.
pub fn create_window_manager(cx: &mut App) -> Entity<GuiSessionManager> {
    let store = shared_store(cx);
    cx.new(|_| GuiSessionManager::new(store))
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
/// a merge accepts two (two-way) or three files (left, ancestor, right).
/// Folder kinds require directories, text kinds require files.
fn resolve_dropped_paths(
    kind: SessionType,
    paths: &[PathBuf],
) -> std::result::Result<(Endpoint, Option<Endpoint>, Endpoint), String> {
    let is_merge = kind == SessionType::TextMerge;
    let min = if is_merge { 2 } else { 1 };
    let max = if is_merge { 3 } else { 2 };
    if paths.len() < min || paths.len() > max {
        return Err(if is_merge {
            "Drop two or three files onto the card to start a merge.".into()
        } else {
            "Drop one or two items onto the card to start a session.".into()
        });
    }
    let (left, ancestor, right) = if is_merge {
        if paths.len() == 2 {
            (&paths[0], None, &paths[1])
        } else {
            (&paths[0], Some(&paths[1]), &paths[2])
        }
    } else if paths.len() == 1 {
        (&paths[0], None, &paths[0])
    } else {
        (&paths[0], None, &paths[1])
    };
    let want_dir =
        kind != SessionType::TextCompare && kind != SessionType::TextMerge;
    for path in [Some(left), ancestor, Some(right)].into_iter().flatten() {
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
        ancestor.map(|p| Endpoint::local(p.clone())),
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

    /// A manager over a throwaway store, for tab-list behavior tests.
    fn manager_in(
        cx: &mut TestAppContext,
        tag: &str,
    ) -> Entity<GuiSessionManager> {
        let store =
            cx.new(|_| SavedSessionStore::new(std::env::temp_dir().join(tag)));
        cx.new(|_| GuiSessionManager::new(store))
    }

    #[gpui::test]
    fn manager_new_starts_empty(cx: &mut TestAppContext) {
        let mgr = manager_in(cx, "cocomo-test-empty");
        mgr.update(cx, |m, _| {
            assert!(m.is_empty());
            assert!(m.active_config().is_none());
            assert!(!m.has_dirty());
        });
    }

    #[gpui::test]
    fn open_home_opens_a_home_tab(cx: &mut TestAppContext) {
        let mgr = manager_in(cx, "cocomo-test-home");
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
        let mgr = manager_in(cx, "cocomo-test-home-reuse");
        mgr.update(cx, |m, cx| m.open_home(cx));
        mgr.update(cx, |m, cx| m.open_home(cx));
        mgr.update(cx, |m, _| assert_eq!(m.len(), 1));
    }

    #[gpui::test]
    fn tab_ids_are_unique_across_managers(cx: &mut TestAppContext) {
        let first = manager_in(cx, "cocomo-test-ids-a");
        let second = manager_in(cx, "cocomo-test-ids-b");
        first.update(cx, |m, cx| m.open_home(cx));
        second.update(cx, |m, cx| m.open_home(cx));
        let first_id = first.read_with(cx, |m, _| m.open_tab_ids()[0].0);
        let second_id = second.read_with(cx, |m, _| m.open_tab_ids()[0].0);
        assert_ne!(first_id, second_id, "tab ids must not collide");
    }

    #[gpui::test]
    fn take_and_insert_move_a_tab_between_managers(cx: &mut TestAppContext) {
        let source = manager_in(cx, "cocomo-test-move-src");
        let target = manager_in(cx, "cocomo-test-move-dst");
        source.update(cx, |m, cx| m.open_home(cx));
        let taken = source.update(cx, |m, _| m.take_session(0));
        let taken = taken.expect("the only tab");
        source.update(cx, |m, _| {
            assert!(m.is_empty());
            assert_eq!(m.active_index(), 0);
        });
        target.update(cx, |m, cx| m.insert_session(taken, None, cx));
        target.update(cx, |m, _| {
            assert_eq!(m.len(), 1);
            assert_eq!(m.active_index(), 0);
            assert!(m.active_session().unwrap().tab_entity.is_home());
        });
    }

    #[gpui::test]
    fn take_session_keeps_the_active_tab_selected(cx: &mut TestAppContext) {
        let mgr = manager_in(cx, "cocomo-test-move-active");
        // Two Home-less tabs would need real endpoints; two home tabs are
        // not possible, so use the tab list directly through move_session.
        mgr.update(cx, |m, cx| m.open_home(cx));
        let taken = mgr.update(cx, |m, _| m.take_session(0)).expect("tab");
        mgr.update(cx, |m, cx| m.insert_session(taken, None, cx));
        mgr.update(cx, |m, _| assert_eq!(m.active_index(), 0));
    }

    #[gpui::test]
    fn move_session_reorders_within_a_window(cx: &mut TestAppContext) {
        let mgr = manager_in(cx, "cocomo-test-reorder");
        mgr.update(cx, |m, cx| m.open_home(cx));
        let ids = mgr.read_with(cx, |m, _| m.open_tab_ids());
        assert_eq!(ids.len(), 1);
        // Reordering to the same position is a no-op, not a panic.
        mgr.update(cx, |m, cx| m.move_session(0, 0, cx));
        mgr.update(cx, |m, cx| m.move_session(0, 5, cx));
        mgr.update(cx, |m, _| assert_eq!(m.len(), 1));
    }

    #[gpui::test]
    fn workspace_snapshot_skips_the_home_tab(cx: &mut TestAppContext) {
        let mgr = manager_in(cx, "cocomo-test-snapshot");
        mgr.update(cx, |m, cx| m.open_home(cx));
        let snapshot = mgr.read_with(cx, |m, _| m.workspace_snapshot());
        assert!(snapshot.tabs.is_empty());
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
        let (left, ancestor, right) = resolve_dropped_paths(
            SessionType::DirCompare,
            &[dir.path().to_path_buf()],
        )
        .unwrap();
        assert_eq!(left.path(), dir.path());
        assert!(ancestor.is_none());
        assert_eq!(right.path(), dir.path());
    }

    #[test]
    fn dropped_pair_maps_left_to_right() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        fs_err::create_dir(&left).unwrap();
        fs_err::create_dir(&right).unwrap();
        let (l, ancestor, r) = resolve_dropped_paths(
            SessionType::DirCompare,
            &[left.clone(), right.clone()],
        )
        .unwrap();
        assert_eq!(l.path(), left);
        assert!(ancestor.is_none());
        assert_eq!(r.path(), right);
    }

    #[test]
    fn dropped_merge_pair_has_no_ancestor() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left.txt");
        let right = root.path().join("right.txt");
        fs_err::write(&left, b"a").unwrap();
        fs_err::write(&right, b"b").unwrap();
        let (l, ancestor, r) = resolve_dropped_paths(
            SessionType::TextMerge,
            &[left.clone(), right.clone()],
        )
        .unwrap();
        assert_eq!(l.path(), left);
        assert!(ancestor.is_none());
        assert_eq!(r.path(), right);
    }

    #[test]
    fn dropped_merge_triple_places_the_ancestor_in_the_middle() {
        let root = tempfile::tempdir().unwrap();
        let left = root.path().join("left.txt");
        let ancestor_file = root.path().join("base.txt");
        let right = root.path().join("right.txt");
        fs_err::write(&left, b"a").unwrap();
        fs_err::write(&ancestor_file, b"a").unwrap();
        fs_err::write(&right, b"a").unwrap();
        let (l, ancestor, r) = resolve_dropped_paths(
            SessionType::TextMerge,
            &[left.clone(), ancestor_file.clone(), right.clone()],
        )
        .unwrap();
        assert_eq!(l.path(), left);
        assert_eq!(ancestor.expect("ancestor").path(), ancestor_file);
        assert_eq!(r.path(), right);
    }

    #[test]
    fn dropped_single_file_is_rejected_for_merges() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("f.txt");
        fs_err::write(&file, b"x").unwrap();
        let err = resolve_dropped_paths(
            SessionType::TextMerge,
            std::slice::from_ref(&file),
        )
        .unwrap_err();
        assert!(err.contains("two or three"), "unexpected error: {err}");
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
