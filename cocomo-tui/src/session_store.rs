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
//!
//! Both session kinds round-trip through the shared lib format: folder
//! sessions persist as `DirCompare`, text sessions as `TextCompare` with
//! their rule toggles in [`SessionSettings`], so a session file saved by
//! the TUI opens in the GUI (and vice versa).

use cocomo_lib::{
    ProviderRef, SessionConfig, SessionSettings, SessionType as LibSessionType,
};

use crate::state::{SessionContent, SessionView};

/// Convert a comparison session to a serializable config.
///
/// Folder sessions map to `DirCompare` with their comparison and display
/// settings; text sessions map to `TextCompare` with their rule toggles
/// (see [`crate::state::TextRules`]).
pub(crate) fn session_to_config(session: &SessionView) -> SessionConfig {
    let (session_type, settings) = match &session.content {
        SessionContent::Dir(dir) => (
            LibSessionType::DirCompare,
            SessionSettings {
                compare_files: dir.compare_content,
                compare_structure: true,
                name_filter: session
                    .ui
                    .name_filter
                    .as_ref()
                    .map(|f| vec![f.pattern.clone()])
                    .unwrap_or_default(),
                ignore_whitespace: false,
                skip_blank_lines: false,
                skip_comments: false,
                show_hidden: dir.display.show_hidden,
                // The per-session copy overrides; `None` fields are
                // omitted from the TOML so the session keeps following the
                // global copy defaults.
                copy_preserve_links: dir.copy_preserve_links,
                copy_preserve_mode: dir.copy_preserve_mode,
                copy_preserve_timestamps: dir.copy_preserve_timestamps,
                // Same for the follow-links override.
                follow_links: dir.follow_links,
                // The shown status bits by name, so the TOML stays
                // human-readable like the old coarse booleans.
                show_statuses: dir
                    .display
                    .show_statuses
                    .iter()
                    .map(|bit| bit.bit_name().to_string())
                    .collect(),
                ..SessionSettings::default()
            },
        ),
        SessionContent::Text(text) => (
            LibSessionType::TextCompare,
            SessionSettings {
                compare_files: true,
                ignore_whitespace: text.source.rules.ignore_whitespace,
                skip_blank_lines: text.source.rules.skip_blank_lines,
                skip_comments: text.source.rules.skip_comments,
                ..SessionSettings::default()
            },
        ),
    };
    SessionConfig {
        name: session.core.name.clone(),
        session_type,
        left: ProviderRef {
            provider: session.core.left.provider_ref(),
            path: session.core.left.path().to_path_buf(),
        },
        right: ProviderRef {
            provider: session.core.right.provider_ref(),
            path: session.core.right.path().to_path_buf(),
        },
        center: None,
        settings,
    }
}

#[cfg(test)]
mod tests {
    use cocomo_lib::{Endpoint, Grammar, compare_texts};

    use super::*;
    use crate::state::{TextRules, TextSource};

    fn dir_session() -> SessionView {
        SessionView::new_dir_compare(
            Endpoint::local("/left"),
            Endpoint::local("/right"),
            true,
        )
    }

    fn text_session(rules: TextRules) -> SessionView {
        let left = "a\n\n  b\n";
        let right = "a\nb\n";
        let settings = rules.compare_settings(&Grammar::plain_text());
        let diff = compare_texts(left, right, &settings);
        SessionView::new_text_compare(
            Endpoint::local("/left/a.txt"),
            Endpoint::local("/right/a.txt"),
            diff,
            Grammar::plain_text(),
            TextSource {
                left: left.to_string(),
                right: right.to_string(),
                rules,
            },
        )
    }

    #[test]
    fn dir_session_maps_to_dir_compare_config() {
        let config = session_to_config(&dir_session());
        assert_eq!(config.session_type, LibSessionType::DirCompare);
        assert_eq!(config.left.path, std::path::PathBuf::from("/left"));
        assert_eq!(config.right.path, std::path::PathBuf::from("/right"));
        assert!(config.settings.compare_files);
    }

    #[test]
    fn dir_copy_overrides_round_trip_through_toml() {
        // Cross-frontend portability: per-session copy overrides must
        // survive the shared TOML format; an unset override must stay
        // unset so the session keeps following the global copy defaults.
        let mut session = dir_session();
        if let Some(dir) = session.dir_mut() {
            dir.copy_preserve_links = Some(false);
            dir.copy_preserve_mode = Some(true);
            dir.follow_links = Some(true);
        }
        let config = session_to_config(&session);
        let loaded =
            SessionConfig::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(loaded.settings.copy_preserve_links, Some(false));
        assert_eq!(loaded.settings.copy_preserve_mode, Some(true));
        assert_eq!(loaded.settings.copy_preserve_timestamps, None);
        assert_eq!(loaded.settings.follow_links, Some(true));
    }

    #[test]
    fn text_session_maps_to_text_compare_config() {
        let rules = TextRules {
            ignore_whitespace: true,
            skip_blank_lines: false,
            skip_comments: true,
        };
        let config = session_to_config(&text_session(rules));
        assert_eq!(config.session_type, LibSessionType::TextCompare);
        assert_eq!(config.left.path, std::path::PathBuf::from("/left/a.txt"));
        assert!(config.settings.ignore_whitespace);
        assert!(!config.settings.skip_blank_lines);
        assert!(config.settings.skip_comments);
    }

    #[test]
    fn text_session_rules_round_trip_through_toml() {
        // Cross-frontend portability: the rules must survive the shared
        // TOML format so a TUI-saved text session opens identically in
        // the GUI (and the other way around).
        let rules = TextRules {
            ignore_whitespace: true,
            skip_blank_lines: true,
            skip_comments: false,
        };
        let config = session_to_config(&text_session(rules));
        let loaded =
            SessionConfig::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(loaded.session_type, LibSessionType::TextCompare);
        assert_eq!(TextRules::from_settings(&loaded.settings), rules);
    }

    #[test]
    fn default_rules_round_trip_too() {
        // A text session with no rules set (endpoints as files) must
        // serialize without extra fields.
        let config = session_to_config(&text_session(TextRules::default()));
        let loaded =
            SessionConfig::from_toml(&config.to_toml().unwrap()).unwrap();
        assert_eq!(
            TextRules::from_settings(&loaded.settings),
            TextRules::default()
        );
    }
}
