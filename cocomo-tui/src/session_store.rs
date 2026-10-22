// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Converting a [`SessionView`] into a serializable [`SessionConfig`].
//!
//! The session-file location and listing helpers live in `cocomo-lib`
//! ([`cocomo_lib::default_session_dir`], [`cocomo_lib::list_saved_sessions`])
//! so all frontends share them; only the TUI-specific view-to-config mapping
//! stays here.

use cocomo_lib::{
    ProviderRef, SessionConfig, SessionSettings, SessionType as LibSessionType,
};

use crate::state::SessionView;

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
