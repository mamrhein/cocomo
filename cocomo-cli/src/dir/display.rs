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
use cocomo_lib::compare::DirEntryStatus;

use crate::cli::ColorMode;

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Conditionally render text with ANSI color escape codes based on the entry
/// status. When colors are disabled, returns the text unchanged.
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

    /// Wrap `text` with ANSI codes appropriate for the given status.
    pub(super) fn colorize(
        &self,
        text: &str,
        status: DirEntryStatus,
    ) -> String {
        if !self.active {
            return text.to_string();
        }
        let code = color_code(status);
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

/// Return the ANSI color code for a given comparison status.
pub(super) fn color_code(status: DirEntryStatus) -> &'static str {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => "32", // green
        DirEntryStatus::Similar | DirEntryStatus::Different => "33", // yellow
        DirEntryStatus::LeftOnly | DirEntryStatus::CenterOnly => "36", // cyan
        DirEntryStatus::RightOnly => "35",                         // magenta
        DirEntryStatus::Mergeable => "34",                         // blue
        DirEntryStatus::Conflict => "31",                          // red
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
    use cocomo_lib::compare::DirEntryStatus;

    use super::{Colorizer, color_code, format_modified, truncate_name};
    use crate::cli::ColorMode;

    #[test]
    fn colorizer_never_strips_codes() {
        let c = Colorizer::new(ColorMode::Never);
        assert!(!c.is_active());
        assert_eq!(c.colorize("=", DirEntryStatus::Same), "=");
    }

    #[test]
    fn colorizer_always_adds_codes() {
        let c = Colorizer::new(ColorMode::Always);
        assert!(c.is_active());
        let result = c.colorize("=", DirEntryStatus::Same);
        assert!(result.contains("\x1b[32m"));
        assert!(result.ends_with("\x1b[0m"));
    }

    #[test]
    fn colorizer_respects_all_status_codes() {
        let c = Colorizer::new(ColorMode::Always);
        assert_eq!(color_code(DirEntryStatus::Same), "32");
        assert_eq!(color_code(DirEntryStatus::SameBinary), "32");
        assert_eq!(color_code(DirEntryStatus::Similar), "33");
        assert_eq!(color_code(DirEntryStatus::Different), "33");
        assert_eq!(color_code(DirEntryStatus::LeftOnly), "36");
        assert_eq!(color_code(DirEntryStatus::RightOnly), "35");
        assert_eq!(color_code(DirEntryStatus::Mergeable), "34");
        assert_eq!(color_code(DirEntryStatus::Conflict), "31");
        assert_eq!(color_code(DirEntryStatus::CenterOnly), "36");
        // Verify the colored output contains the code.
        assert!(
            c.colorize("X", DirEntryStatus::Conflict)
                .contains("\x1b[31m")
        );
    }

    #[test]
    fn colorizer_color_for_center_only() {
        let c = Colorizer::new(ColorMode::Always);
        assert!(
            c.colorize("|", DirEntryStatus::CenterOnly)
                .contains("\x1b[36m")
        );
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
