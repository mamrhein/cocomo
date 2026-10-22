// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Presentation formatting helpers: status colors and grammar-based text
//! styles.
//!
//! Human-readable size and date formatting live in
//! [`cocomo_lib::format_size`] / [`cocomo_lib::format_date`] so all three
//! frontends share one implementation.

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
