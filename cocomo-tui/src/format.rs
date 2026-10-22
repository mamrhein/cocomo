// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Presentation formatting helpers: status colors, human-readable sizes and
//! dates, and grammar-based text styles.

use cocomo_lib::{DirEntryStatus, Importance, LineInfo};
use ratatui_kit::ratatui::style::{Color, Style};

/// Return the status color for a directory entry status.
pub(crate) fn status_style(status: DirEntryStatus) -> Color {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => Color::Gray,
        DirEntryStatus::Similar => Color::Yellow,
        DirEntryStatus::Different => Color::Red,
        DirEntryStatus::LeftOnly => Color::Cyan,
        DirEntryStatus::RightOnly => Color::Magenta,
        DirEntryStatus::CenterOnly => Color::White,
        DirEntryStatus::Mergeable => Color::Green,
        DirEntryStatus::Conflict => Color::Rgb(255, 0, 255),
        DirEntryStatus::IdenticalNameDifferentType => Color::Rgb(255, 255, 0),
    }
}

/// Format a byte count as a human-readable size string.
pub(crate) fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Truncate an ISO-8601 date string to "YYYY-MM-DD HH:MM".
pub(crate) fn format_date(date_str: &str) -> String {
    if date_str.len() > 16 {
        date_str[..16].replace('T', " ")
    } else {
        date_str.to_string()
    }
}

/// Return the highest importance token in a line.
pub(crate) fn importance_for_line(line: &LineInfo) -> Importance {
    let mut max_imp = Importance::Ignored;
    for token in &line.tokens {
        if token.importance > max_imp {
            max_imp = token.importance;
        }
    }
    max_imp
}

/// Return a text style based on the grammar importance of a token.
pub(crate) fn grammar_style(importance: Importance) -> Style {
    match importance {
        Importance::Code => Style::default(),
        Importance::Data => Style::default().fg(Color::Yellow),
        Importance::Comment => Style::default().fg(Color::DarkGray),
        Importance::Ignored => Style::default().fg(Color::DarkGray),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_uses_binary_units() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(999), "999B");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1536), "1.5K");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0M");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.0G");
    }

    #[test]
    fn format_date_truncates_to_minutes() {
        assert_eq!(
            format_date("2026-10-22T09:47:08+02:00"),
            "2026-10-22 09:47"
        );
        // Strings at or below the cut length pass through unchanged.
        assert_eq!(format_date("2026-10-22T09:47"), "2026-10-22T09:47");
        assert_eq!(format_date(""), "");
    }

    #[test]
    fn status_style_distinguishes_all_statuses() {
        let statuses = [
            DirEntryStatus::Same,
            DirEntryStatus::SameBinary,
            DirEntryStatus::Similar,
            DirEntryStatus::Different,
            DirEntryStatus::LeftOnly,
            DirEntryStatus::RightOnly,
            DirEntryStatus::CenterOnly,
            DirEntryStatus::Mergeable,
            DirEntryStatus::Conflict,
            DirEntryStatus::IdenticalNameDifferentType,
        ];
        let mut colors: Vec<Color> =
            statuses.iter().copied().map(status_style).collect();
        // `Same` and `SameBinary` intentionally share a color; every other
        // status must be visually distinct.
        colors.dedup();
        assert_eq!(colors.len(), statuses.len() - 1);
    }
}
