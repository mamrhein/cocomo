// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Session management for comparison, merge, and sync workspaces.
//!
//! A session captures everything needed to restore a workspace: the providers
//! (left, right, optionally center), their paths, and all comparison settings.
//! Sessions are serialized to TOML files (`.bcs` extension) and can be shared
//! between users or cloned to create variant workspaces.
//!
//! # Architecture
//!
//! Serialization uses paths and provider schemes because they are
//! human-readable and portable. On load, paths are resolved to [`NodeId`]
//! values via
//! [`NodeFileSystem::resolve_path`](crate::fs::NodeFileSystem::resolve_path)
//! so active operations use the node-based API.
//!
//! The [`SessionConfig`] struct is the serializable form. The [`Session`]
//! struct is the runtime form that holds live provider references and resolved
//! node IDs.

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Result,
    error::FsError,
    fs::NodeFileSystem,
    identity::NodeId,
    text::{TextCompareSettings, WhitespaceMode},
};

/// Type alias for a type-erased node-based filesystem.
///
/// All current backends use `u64` for both filesystem and node IDs, and
/// `FsError` as the error type. This alias enables trait objects so sessions
/// can hold heterogeneous providers.
type AnyNodeFs = dyn NodeFileSystem<Nid = u64, FsId = u64, Error = FsError>;

/// Type alias for a resolved node ID in a session.
type SessionNodeId = NodeId<u64>;

// ---------------------------------------------------------------------------
// Session types
// ---------------------------------------------------------------------------

/// The kind of workspace a session represents.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SessionType {
    /// Dashboard / session browser.
    Home,
    /// Two-way folder comparison.
    DirCompare,
    /// Three-way folder merge.
    DirMerge,
    /// Folder synchronization.
    DirSync,
    /// Two-way text diff.
    TextCompare,
    /// Three-way text merge.
    TextMerge,
    /// Single-file editor.
    TextEdit,
    /// Apply a patch file.
    TextPatch,
    /// Structured data (table) comparison.
    TableCompare,
    /// Binary comparison.
    HexCompare,
    /// Image comparison.
    PixCompare,
}

impl std::fmt::Display for SessionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Home => write!(f, "Home"),
            Self::DirCompare => write!(f, "DirCompare"),
            Self::DirMerge => write!(f, "DirMerge"),
            Self::DirSync => write!(f, "DirSync"),
            Self::TextCompare => write!(f, "TextCompare"),
            Self::TextMerge => write!(f, "TextMerge"),
            Self::TextEdit => write!(f, "TextEdit"),
            Self::TextPatch => write!(f, "TextPatch"),
            Self::TableCompare => write!(f, "TableCompare"),
            Self::HexCompare => write!(f, "HexCompare"),
            Self::PixCompare => write!(f, "PixCompare"),
        }
    }
}

// ---------------------------------------------------------------------------
// Serializable provider reference
// ---------------------------------------------------------------------------

/// A serializable reference to a provider and a path within it.
///
/// Stored in session files. On load, this is resolved to a live provider
/// instance and a [`NodeId`] via the provider registry and
/// [`NodeFileSystem::resolve_path`](crate::fs::NodeFileSystem::resolve_path).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderRef {
    /// Provider scheme (e.g., `"file"`, `"s3"`, `"ftp"`).
    pub provider: String,
    /// Absolute path within the provider.
    pub path: PathBuf,
}

// ---------------------------------------------------------------------------
// Serializable session configuration
// ---------------------------------------------------------------------------

/// Serializable settings for a session. Written to `.bcs` files.
///
/// This is the on-disk representation. To create a runtime [`Session`],
/// use [`Session::from_config`] after resolving providers and paths.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionConfig {
    /// Human-readable session name.
    pub name: String,
    /// Session kind.
    #[serde(rename = "type")]
    pub session_type: SessionType,
    /// Left provider and path.
    pub left: ProviderRef,
    /// Right provider and path.
    pub right: ProviderRef,
    /// Optional center provider and path (for 3-way operations).
    #[serde(default)]
    pub center: Option<ProviderRef>,
    /// Per-session settings.
    #[serde(default)]
    pub settings: SessionSettings,
}

/// Per-session comparison and display settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionSettings {
    /// Compare file contents (not just metadata).
    #[serde(default = "default_true")]
    pub compare_files: bool,
    /// Compare directory structure.
    #[serde(default = "default_true")]
    pub compare_structure: bool,
    /// Name filter patterns (glob).
    #[serde(default)]
    pub name_filter: Vec<String>,
    /// Ignore whitespace differences in text comparison.
    #[serde(default)]
    pub ignore_whitespace: bool,
    /// Skip blank lines in text comparison.
    #[serde(default)]
    pub skip_blank_lines: bool,
    /// Skip comment lines in text comparison.
    #[serde(default)]
    pub skip_comments: bool,
    /// Show identical entries in the comparison view.
    #[serde(default = "default_true")]
    pub show_same: bool,
    /// Show different entries.
    #[serde(default = "default_true")]
    pub show_different: bool,
    /// Show orphan entries (exist on one side only).
    #[serde(default = "default_true")]
    pub show_orphans: bool,
}

impl Default for SessionSettings {
    fn default() -> Self {
        Self {
            compare_files: true,
            compare_structure: true,
            name_filter: Vec::new(),
            ignore_whitespace: false,
            skip_blank_lines: false,
            skip_comments: false,
            show_same: true,
            show_different: true,
            show_orphans: true,
        }
    }
}

fn default_true() -> bool {
    true
}

impl SessionConfig {
    /// Serialize this config to a TOML string.
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| crate::error::FsError::Io {
            operation: crate::error::FsOperation::Write,
            path: PathBuf::new(),
            message: format!("failed to serialize session: {e}"),
        })
    }

    /// Deserialize a session config from a TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self> {
        toml::from_str(toml_str).map_err(|e| crate::error::FsError::Io {
            operation: crate::error::FsOperation::Read,
            path: PathBuf::new(),
            message: format!("failed to parse session: {e}"),
        })
    }

    /// Serialize this config to a file.
    pub async fn save_to_file(&self, path: &Path) -> Result<()> {
        let content = self.to_toml()?;
        fs_err::tokio::write(path, content).await.map_err(|e| {
            crate::error::FsError::Io {
                operation: crate::error::FsOperation::Write,
                path: path.to_path_buf(),
                message: format!("failed to write session file: {e}"),
            }
        })
    }

    /// Load a session config from a file.
    pub async fn load_from_file(path: &Path) -> Result<Self> {
        let content =
            fs_err::tokio::read_to_string(path).await.map_err(|e| {
                crate::error::FsError::Io {
                    operation: crate::error::FsOperation::Read,
                    path: path.to_path_buf(),
                    message: format!("failed to read session file: {e}"),
                }
            })?;
        Self::from_toml(&content)
    }
}

// ---------------------------------------------------------------------------
// Runtime session
// ---------------------------------------------------------------------------

/// A runtime session holding live provider references and resolved node IDs.
///
/// This is the in-memory representation used by the TUI/GUI. Created by
/// loading a [`SessionConfig`] and resolving providers + paths. The session
/// stores both the original paths (for display and serialization) and the
/// resolved node IDs (for efficient I/O).
pub struct Session {
    /// Unique session identifier.
    pub id: Uuid,
    /// Human-readable session name.
    pub name: String,
    /// Session kind.
    pub session_type: SessionType,
    /// Left provider instance.
    pub left_provider: Arc<AnyNodeFs>,
    /// Right provider instance.
    pub right_provider: Arc<AnyNodeFs>,
    /// Optional center provider (for 3-way operations).
    pub center_provider: Option<Arc<AnyNodeFs>>,
    /// Left path (for display and serialization).
    pub left_path: PathBuf,
    /// Right path (for display and serialization).
    pub right_path: PathBuf,
    /// Optional center path.
    pub center_path: Option<PathBuf>,
    /// Resolved left node ID. Valid as long as the node exists in the cache.
    pub left_node_id: Option<SessionNodeId>,
    /// Resolved right node ID.
    pub right_node_id: Option<SessionNodeId>,
    /// Optional resolved center node ID.
    pub center_node_id: Option<SessionNodeId>,
    /// Per-session settings.
    pub settings: SessionSettings,
    /// When this session was created.
    pub created_at: DateTime<Utc>,
    /// When this session was last modified.
    pub modified_at: DateTime<Utc>,
}

impl Session {
    /// Create a new session from resolved providers and paths.
    ///
    /// This constructor does not resolve paths; the caller must provide
    /// pre-resolved node IDs. Use [`Session::from_config_and_providers`]
    /// to create a session by resolving paths asynchronously.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        session_type: SessionType,
        left_provider: Arc<AnyNodeFs>,
        right_provider: Arc<AnyNodeFs>,
        center_provider: Option<Arc<AnyNodeFs>>,
        left_path: PathBuf,
        right_path: PathBuf,
        center_path: Option<PathBuf>,
        left_node_id: Option<SessionNodeId>,
        right_node_id: Option<SessionNodeId>,
        center_node_id: Option<SessionNodeId>,
        settings: SessionSettings,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            name,
            session_type,
            left_provider,
            right_provider,
            center_provider,
            left_path,
            right_path,
            center_path,
            left_node_id,
            right_node_id,
            center_node_id,
            settings,
            created_at: now,
            modified_at: now,
        }
    }

    /// Create a session from a config and live providers.
    ///
    /// Resolves paths to node IDs asynchronously. Returns an error if any
    /// path resolution fails.
    pub async fn from_config_and_providers(
        config: &SessionConfig,
        left_provider: Arc<AnyNodeFs>,
        right_provider: Arc<AnyNodeFs>,
        center_provider: Option<Arc<AnyNodeFs>>,
    ) -> Result<Self> {
        let left_node_id =
            left_provider.resolve_path(&config.left.path).await.ok();
        let right_node_id =
            right_provider.resolve_path(&config.right.path).await.ok();
        let center_node_id =
            if let (Some(cp), Some(cr)) = (&center_provider, &config.center) {
                cp.resolve_path(&cr.path).await.ok()
            } else {
                None
            };

        Ok(Self {
            id: Uuid::new_v4(),
            name: config.name.clone(),
            session_type: config.session_type,
            left_provider,
            right_provider,
            center_provider,
            left_path: config.left.path.clone(),
            right_path: config.right.path.clone(),
            center_path: config.center.as_ref().map(|c| c.path.clone()),
            left_node_id,
            right_node_id,
            center_node_id,
            settings: config.settings.clone(),
            created_at: Utc::now(),
            modified_at: Utc::now(),
        })
    }

    /// Convert this session back to a serializable config.
    pub fn to_config(&self) -> SessionConfig {
        SessionConfig {
            name: self.name.clone(),
            session_type: self.session_type,
            left: ProviderRef {
                provider: self.left_provider.label_node().to_string(),
                path: self.left_path.clone(),
            },
            right: ProviderRef {
                provider: self.right_provider.label_node().to_string(),
                path: self.right_path.clone(),
            },
            center: self.center_path.clone().map(|path| ProviderRef {
                provider: self
                    .center_provider
                    .as_ref()
                    .map(|p| p.label_node().to_string())
                    .unwrap_or_default(),
                path,
            }),
            settings: self.settings.clone(),
        }
    }

    /// Save this session to a `.bcs` file.
    pub async fn save(&self, path: &Path) -> Result<()> {
        self.to_config().save_to_file(path).await
    }

    /// Update the modified timestamp.
    pub fn touch(&mut self) {
        self.modified_at = Utc::now();
    }

    /// Clone this session with a new name and optionally swapped paths.
    pub fn clone_session(&self, new_name: String) -> Self {
        let mut cloned = self.clone();
        cloned.id = Uuid::new_v4();
        cloned.name = new_name;
        cloned.created_at = Utc::now();
        cloned.modified_at = cloned.created_at;
        cloned
    }

    /// Build [`TextCompareSettings`] from the session settings.
    pub fn text_compare_settings(&self) -> TextCompareSettings {
        TextCompareSettings {
            whitespace_mode: if self.settings.ignore_whitespace {
                WhitespaceMode::Insensitive
            } else {
                WhitespaceMode::Sensitive
            },
            ignore_blank_lines: self.settings.skip_blank_lines,
            ignore_comments: self.settings.skip_comments,
            ..TextCompareSettings::default()
        }
    }
}

impl Clone for Session {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            session_type: self.session_type,
            left_provider: self.left_provider.clone(),
            right_provider: self.right_provider.clone(),
            center_provider: self.center_provider.clone(),
            left_path: self.left_path.clone(),
            right_path: self.right_path.clone(),
            center_path: self.center_path.clone(),
            left_node_id: self.left_node_id,
            right_node_id: self.right_node_id,
            center_node_id: self.center_node_id,
            settings: self.settings.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
        }
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("session_type", &self.session_type)
            .field("left_label", &self.left_provider.label_node())
            .field("right_label", &self.right_provider.label_node())
            .field(
                "center_label",
                &self.center_provider.as_ref().map(|p| p.label_node()),
            )
            .field("left_path", &self.left_path)
            .field("right_path", &self.right_path)
            .field("center_path", &self.center_path)
            .field("left_node_id", &self.left_node_id)
            .field("right_node_id", &self.right_node_id)
            .field("center_node_id", &self.center_node_id)
            .field("settings", &self.settings)
            .field("created_at", &self.created_at)
            .field("modified_at", &self.modified_at)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

/// Manages a collection of sessions.
///
/// Provides persistence of recent sessions and session listing.
pub struct SessionManager {
    /// Directory where `.bcs` session files are stored.
    session_dir: PathBuf,
    /// Currently loaded sessions.
    sessions: Vec<Session>,
}

impl SessionManager {
    /// Create a new session manager that stores sessions in the given
    /// directory.
    pub fn new(session_dir: PathBuf) -> Self {
        Self {
            session_dir,
            sessions: Vec::new(),
        }
    }

    /// Add a session to the manager.
    pub fn add_session(&mut self, session: Session) {
        self.sessions.push(session);
    }

    /// Return all managed sessions.
    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    /// Find a session by ID.
    pub fn get_session(&self, id: &Uuid) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == *id)
    }

    /// Find a mutable session by ID.
    pub fn get_session_mut(&mut self, id: &Uuid) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == *id)
    }

    /// Remove a session by ID. Returns `true` if the session existed.
    pub fn remove_session(&mut self, id: &Uuid) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|s| s.id != *id);
        self.sessions.len() < before
    }

    /// Save a session to the session directory.
    pub async fn save_session(&self, session: &Session) -> Result<()> {
        let path = self.session_dir.join(format!("{}.bcs", session.id));
        session.save(&path).await
    }

    /// List all `.bcs` session files in the session directory.
    pub async fn list_session_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        if !self.session_dir.exists() {
            return Ok(files);
        }

        let mut entries = fs_err::tokio::read_dir(&self.session_dir)
            .await
            .map_err(|e| crate::error::FsError::Io {
                operation: crate::error::FsOperation::ReadDir,
                path: self.session_dir.clone(),
                message: format!("failed to read session directory: {e}"),
            })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            crate::error::FsError::Io {
                operation: crate::error::FsOperation::ReadDir,
                path: self.session_dir.clone(),
                message: format!("failed to read directory entry: {e}"),
            }
        })? {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("bcs") {
                files.push(path);
            }
        }

        Ok(files)
    }

    /// Load all session configs from the session directory.
    pub async fn load_all_configs(&self) -> Result<Vec<SessionConfig>> {
        let files = self.list_session_files().await?;
        let mut configs = Vec::new();
        for file in files {
            if let Ok(config) = SessionConfig::load_from_file(&file).await {
                configs.push(config);
            }
            // Silently skip corrupted session files.
        }
        Ok(configs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_type_display() {
        assert_eq!(format!("{}", SessionType::DirCompare), "DirCompare");
        assert_eq!(format!("{}", SessionType::TextMerge), "TextMerge");
        assert_eq!(format!("{}", SessionType::PixCompare), "PixCompare");
    }

    #[test]
    fn session_config_roundtrip() {
        let config = SessionConfig {
            name: "Test Session".to_string(),
            session_type: SessionType::DirCompare,
            left: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/home/user/left"),
            },
            right: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/home/user/right"),
            },
            center: None,
            settings: SessionSettings::default(),
        };

        let toml_str = config.to_toml().unwrap();
        let loaded = SessionConfig::from_toml(&toml_str).unwrap();
        assert_eq!(config, loaded);
    }

    #[test]
    fn session_config_with_center() {
        let config = SessionConfig {
            name: "3-way merge".to_string(),
            session_type: SessionType::DirMerge,
            left: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/a"),
            },
            right: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/b"),
            },
            center: Some(ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/c"),
            }),
            settings: SessionSettings::default(),
        };

        let toml_str = config.to_toml().unwrap();
        let loaded = SessionConfig::from_toml(&toml_str).unwrap();
        assert_eq!(config, loaded);
        assert!(loaded.center.is_some());
    }

    #[test]
    fn session_settings_default() {
        let settings = SessionSettings::default();
        assert!(settings.compare_files);
        assert!(settings.compare_structure);
        assert!(!settings.ignore_whitespace);
        assert!(settings.show_same);
        assert!(settings.show_orphans);
    }

    #[test]
    fn session_config_serialize_settings() {
        let config = SessionConfig {
            name: "Filtered".to_string(),
            session_type: SessionType::DirCompare,
            left: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/x"),
            },
            right: ProviderRef {
                provider: "local".to_string(),
                path: PathBuf::from("/y"),
            },
            center: None,
            settings: SessionSettings {
                name_filter: vec!["*.rs".to_string(), "*.toml".to_string()],
                ignore_whitespace: true,
                show_same: false,
                ..SessionSettings::default()
            },
        };

        let toml_str = config.to_toml().unwrap();
        assert!(toml_str.contains("ignore_whitespace = true"));
        assert!(toml_str.contains("show_same = false"));
    }

    #[test]
    fn provider_ref_equality() {
        let a = ProviderRef {
            provider: "local".to_string(),
            path: PathBuf::from("/a/b"),
        };
        let b = ProviderRef {
            provider: "local".to_string(),
            path: PathBuf::from("/a/b"),
        };
        let c = ProviderRef {
            provider: "s3".to_string(),
            path: PathBuf::from("/a/b"),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn session_to_config_roundtrip() {
        use crate::local::LocalFs;

        let fs: Arc<AnyNodeFs> = Arc::new(LocalFs::new("local"));
        let session = Session::new(
            "Test".to_string(),
            SessionType::DirCompare,
            fs.clone(),
            fs.clone(),
            None,
            PathBuf::from("/left"),
            PathBuf::from("/right"),
            None,
            None,
            None,
            None,
            SessionSettings::default(),
        );

        let config = session.to_config();
        assert_eq!(config.name, "Test");
        assert_eq!(config.session_type, SessionType::DirCompare);
        assert_eq!(config.left.path, PathBuf::from("/left"));
    }

    #[test]
    fn session_clone_creates_new_id() {
        use crate::local::LocalFs;

        let fs: Arc<AnyNodeFs> = Arc::new(LocalFs::new("local"));
        let session = Session::new(
            "Original".to_string(),
            SessionType::DirCompare,
            fs.clone(),
            fs.clone(),
            None,
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            None,
            None,
            None,
            None,
            SessionSettings::default(),
        );

        let cloned = session.clone_session("Clone".to_string());
        assert_ne!(session.id, cloned.id);
        assert_eq!(cloned.name, "Clone");
        assert_eq!(session.name, "Original");
    }

    #[test]
    fn session_manager_add_and_get() {
        let mut mgr = SessionManager::new(PathBuf::from("/tmp/sessions"));
        let fs: Arc<AnyNodeFs> = Arc::new(crate::local::LocalFs::new("local"));
        let session = Session::new(
            "Test".to_string(),
            SessionType::DirCompare,
            fs.clone(),
            fs.clone(),
            None,
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            None,
            None,
            None,
            None,
            SessionSettings::default(),
        );
        let id = session.id;

        mgr.add_session(session);
        assert_eq!(mgr.sessions().len(), 1);
        assert!(mgr.get_session(&id).is_some());
    }

    #[test]
    fn session_manager_remove() {
        let mut mgr = SessionManager::new(PathBuf::from("/tmp/sessions"));
        let fs: Arc<AnyNodeFs> = Arc::new(crate::local::LocalFs::new("local"));
        let session = Session::new(
            "Test".to_string(),
            SessionType::DirCompare,
            fs.clone(),
            fs.clone(),
            None,
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            None,
            None,
            None,
            None,
            SessionSettings::default(),
        );
        let id = session.id;

        mgr.add_session(session);
        assert!(mgr.remove_session(&id));
        assert!(!mgr.remove_session(&id));
        assert_eq!(mgr.sessions().len(), 0);
    }
}
