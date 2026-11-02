// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::io::IsTerminal;

use chrono::DateTime;
use cocomo_lib::compare::StatusColor;

use crate::cli::ColorMode;

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Conditionally render text with ANSI color escape codes based on the
/// status color role. When colors are disabled, returns the text
/// unchanged.
pub(super) struct Colorizer {
    active: bool,
}

impl Colorizer {
    /// Create a new `Colorizer` from a [`ColorMode`].
    pub(super) fn new(mode: ColorMode) -> Self {
        let active = match mode {
            ColorMode::Always => true,
            ColorMode::Never => false,
            ColorMode::Auto => std::io::stdout().is_terminal(),
        };
        Self { active }
    }

    /// Return `true` when ANSI color codes will be emitted.
    #[allow(dead_code)]
    pub(super) fn is_active(&self) -> bool {
        self.active
    }

    /// Wrap `text` with ANSI codes appropriate for the given color role.
    ///
    /// The role is resolved from a status bitmap via the lib's
    /// `DirEntryStatus::symbol`.
    pub(super) fn colorize(&self, text: &str, role: StatusColor) -> String {
        if !self.active {
            return text.to_string();
        }
        let code = color_code(role);
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

/// Return the ANSI color code for a status color role.
pub(super) fn color_code(role: StatusColor) -> &'static str {
    match role {
        StatusColor::Equal => "90", // bright black (grey)
        StatusColor::Different => "31", // red
        StatusColor::Orphan => "34", // blue
    }
}

/// Parse an RFC 3339 timestamp string and reformat it as `yyyy-mm-dd
/// hh:mm:ss`. Falls back to the original string when parsing fails, so non-RFC
/// 3339 values (such as the `"2024-03-15 14:30"` used in tests) are preserved.
pub(super) fn format_modified(rfc3339: &str) -> String {
    DateTime::parse_from_rfc3339(rfc3339)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| rfc3339.to_string())
}

/// Return the terminal width in columns, but only when stdout is a TTY.
/// Returns `None` for piped or redirected output so that automated tests
/// and CI get deterministic results.
pub(super) fn terminal_width() -> Option<usize> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    terminal_size::terminal_size().map(|size| size.0.0 as usize)
}

/// Truncate a name to fit within `width` display columns. When the name is
/// too long, show the first `(width - 6)` characters, an ellipsis (`…`),
/// and the last 5 characters. When `width` is too small for this pattern,
/// fall back to a simple character-boundary truncation.
pub(super) fn truncate_name(name: &str, width: usize) -> String {
    let name_len = name.chars().count();
    if name_len <= width {
        return name.to_string();
    }
    if width < 7 {
        return name.chars().take(width).collect();
    }
    let head_len = width - 6;
    let tail_start = name_len - 5;
    let mut result = String::with_capacity(width);
    result.extend(name.chars().take(head_len));
    result.push('\u{2026}');
    result.extend(name.chars().skip(tail_start));
    result
}

/// Format a file size as a human-readable string with binary units.
pub(crate) fn format_size(size: u64) -> String {
    if size >= 1_073_741_824 {
        format!("{:.1}G", size as f64 / 1_073_741_824.0)
    } else if size >= 1_048_576 {
        format!("{:.1}M", size as f64 / 1_048_576.0)
    } else if size >= 1024 {
        format!("{:.1}K", size as f64 / 1024.0)
    } else {
        format!("{}B", size)
    }
}

#[cfg(test)]
mod tests {
    use cocomo_lib::compare::{DirEntryStatus, StatusColor};

    use super::{Colorizer, color_code, format_modified, truncate_name};
    use crate::cli::ColorMode;

    #[test]
    fn colorizer_never_strips_codes() {
        let c = Colorizer::new(ColorMode::Never);
        assert!(!c.is_active());
        assert_eq!(c.colorize("=", StatusColor::Equal), "=");
    }

    #[test]
    fn colorizer_always_adds_codes() {
        let c = Colorizer::new(ColorMode::Always);
        assert!(c.is_active());
        let result = c.colorize("=", StatusColor::Equal);
        assert!(result.contains("\x1b[90m"));
        assert!(result.ends_with("\x1b[0m"));
    }

    #[test]
    fn colorizer_respects_all_status_codes() {
        let c = Colorizer::new(ColorMode::Always);
        assert_eq!(color_code(StatusColor::Equal), "90");
        assert_eq!(color_code(StatusColor::Different), "31");
        assert_eq!(color_code(StatusColor::Orphan), "34");
        // Verify the colored output contains the code.
        assert!(c.colorize("!", StatusColor::Different).contains("\x1b[31m"));
    }

    #[test]
    fn colorizer_color_for_orphan_role() {
        let c = Colorizer::new(ColorMode::Always);
        assert!(
            c.colorize("\u{25C0}", StatusColor::Orphan)
                .contains("\x1b[34m")
        );
    }

    #[test]
    fn color_code_follows_status_symbol_role() {
        // Every status bitmap must colorize via the role its `symbol()`
        // resolves to.
        let cases = [
            (DirEntryStatus::Same, StatusColor::Equal),
            (DirEntryStatus::Equal, StatusColor::Equal),
            (DirEntryStatus::LeftNewer, StatusColor::Different),
            (DirEntryStatus::RightNewer, StatusColor::Different),
            (DirEntryStatus::DIFFERENT, StatusColor::Different),
            (
                DirEntryStatus::Equal | DirEntryStatus::LeftOrphan,
                StatusColor::Different,
            ),
            (DirEntryStatus::LeftOrphan, StatusColor::Orphan),
            (DirEntryStatus::RightOrphan, StatusColor::Orphan),
        ];
        for (status, role) in cases {
            assert_eq!(status.symbol().1, role, "role mismatch: {status}");
        }
    }

    #[test]
    fn format_modified_parses_rfc3339() {
        assert_eq!(
            format_modified("2024-03-15T14:30:00+00:00"),
            "2024-03-15 14:30:00"
        );
    }

    #[test]
    fn format_modified_strips_microseconds_and_tz() {
        let rfc = "2024-03-15T14:30:45.905309080+00:00";
        assert_eq!(format_modified(rfc), "2024-03-15 14:30:45");
    }

    #[test]
    fn format_modified_falls_back_for_non_rfc3339() {
        assert_eq!(format_modified("2024-03-15 14:30"), "2024-03-15 14:30");
        assert_eq!(format_modified("garbage"), "garbage");
    }

    #[test]
    fn truncate_name_short_unchanged() {
        assert_eq!(truncate_name("file.txt", 40), "file.txt");
    }

    #[test]
    fn truncate_name_long_uses_head_tail_pattern() {
        // width = 20: head_len = 14, ellipsis = 1 char, tail = 5 -> total 20.
        let long_name = "a".repeat(30);
        let result = truncate_name(&long_name, 20);
        assert_eq!(result.chars().count(), 20);
        // First 14 chars are a, last 5 chars are a.
        assert!(result.starts_with("aaaaaaaaaaaaaa"));
        assert!(result.ends_with("aaaaa"));
    }

    #[test]
    fn truncate_name_width_too_small_falls_back() {
        // width < 7: simple truncation, no ellipsis pattern.
        assert_eq!(truncate_name("verylongname", 3), "ver");
        assert_eq!(truncate_name("abcdef", 2), "ab");
    }
}
