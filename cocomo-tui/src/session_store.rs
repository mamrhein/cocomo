// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Session-file persistence: config conversion, saved-session listing, and
//! the default session directory.

use std::{io, path::PathBuf};

use cocomo_lib::{
    ProviderRef, SessionConfig, SessionSettings, SessionType as LibSessionType,
};

use crate::{
    error::{Result, TuiError},
    state::SessionView,
};

/// Convert a directory comparison session to a serializable config.
pub(crate) fn session_to_config(session: &SessionView) -> SessionConfig {
    SessionConfig {
        name: session.name.clone(),
        session_type: LibSessionType::DirCompare,
        left: ProviderRef {
            provider: "local".to_string(),
            path: session.left_path.clone(),
        },
        right: ProviderRef {
            provider: "local".to_string(),
            path: session.right_path.clone(),
        },
        center: None,
        settings: SessionSettings {
            compare_files: session.compare_content,
            compare_structure: true,
            name_filter: session
                .active_filter
                .as_ref()
                .map(|f| vec![f.clone()])
                .unwrap_or_default(),
            ignore_whitespace: false,
            skip_blank_lines: false,
            skip_comments: false,
            show_same: !session.hide_same,
            show_different: true,
            show_orphans: true,
        },
    }
}

/// List all saved session files in the default session directory.
///
/// A session directory that does not exist yet simply means "no saved
/// sessions"; any other I/O failure is propagated with path context.
pub(crate) async fn list_saved_sessions() -> Result<Vec<PathBuf>> {
    let session_dir = default_session_dir();
    let mut entries = match fs_err::tokio::read_dir(&session_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(TuiError::io(&session_dir, e)),
    };

    let mut sessions = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| TuiError::io(&session_dir, e))?
    {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml") {
            sessions.push(path);
        }
    }
    sessions.sort();
    Ok(sessions)
}

/// Return the default directory for saved session files.
pub(crate) fn default_session_dir() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("cocomo");
    config_dir.join("sessions")
}
