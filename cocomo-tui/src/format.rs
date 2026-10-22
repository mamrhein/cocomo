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
