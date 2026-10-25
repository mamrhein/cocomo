// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Thin adapters from a [`SessionView`] to the diff-group model in
//! `cocomo-lib`.
//!
//! The difference analysis itself (which lines differ, how they group into
//! contiguous blocks, and where the cursor sits relative to those groups)
//! lives on [`TextDiff`] in `cocomo-lib` and is tested there. These helpers
//! only bridge the session's `Option<TextDiff>` and its table selection to
//! those methods, so the view layer never has to unwrap the diff itself.

use std::collections::HashSet;

use cocomo_lib::{DirEntryStatus, TextDiff};

use crate::state::{SessionContent, SessionView};

/// Build the set of line numbers that are part of a difference.
pub fn diff_line_numbers(session: &SessionView) -> HashSet<usize> {
    session
        .text()
        .and_then(|text| text.text_diff.as_ref())
        .map_or_else(HashSet::new, TextDiff::difference_line_numbers)
}

/// Return the first row index of each contiguous difference group.
///
/// Each element is the row index (0-based into the merged view) where a
/// difference group starts. These are the targets for n/N navigation.
///
/// Text sessions group their differing lines into blocks; directory
/// sessions treat every entry that is not identical (`Same` /
/// `SameBinary`) as its own group, so n/N step through the differing
/// entries one by one.
pub fn diff_group_starts(session: &SessionView) -> Vec<usize> {
    match &session.content {
        SessionContent::Dir(_) => session
            .filtered_entries()
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                !matches!(
                    entry.status,
                    DirEntryStatus::Same | DirEntryStatus::SameBinary
                )
            })
            .map(|(i, _)| i)
            .collect(),
        SessionContent::Text(_) => session
            .text()
            .and_then(|text| text.text_diff.as_ref())
            .map_or_else(Vec::new, TextDiff::group_starts),
    }
}

/// Return the current diff group index (1-based) and total count.
///
/// Returns `(current, total)` where `current` is 1-based, `(0, total)` if the
/// cursor precedes the first group, and `(0, 0)` for a session with no text
/// diff.
pub fn current_diff_position(session: &SessionView) -> (usize, usize) {
    match session.text().and_then(|text| text.text_diff.as_ref()) {
        Some(diff) => {
            let selected = session.ui.table_state.selected().unwrap_or(0);
            diff.position_of(selected)
        }
        None => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cocomo_lib::{Endpoint, Grammar, LineInfo, TextDiff, TextDifference};

    use super::*;

    /// Text session with 10 merged rows: line 3 differs, lines 4 and 5 were
    /// added on the right, line 7 differs.
    fn session() -> SessionView {
        let left_lines: Vec<LineInfo> = (1..=10)
            .map(|n| LineInfo::new(n, format!("left {n}")))
            .collect();
        let right_lines: Vec<LineInfo> = (1..=10)
            .map(|n| LineInfo::new(n, format!("right {n}")))
            .collect();
        let diff = TextDiff {
            differences: vec![
                TextDifference::LineDifferent(
                    LineInfo::new(3, "a".into()),
                    LineInfo::new(3, "b".into()),
                ),
                TextDifference::LinesAdded(
                    4,
                    vec![
                        LineInfo::new(4, "c".into()),
                        LineInfo::new(5, "d".into()),
                    ],
                ),
                TextDifference::LineDifferent(
                    LineInfo::new(7, "e".into()),
                    LineInfo::new(7, "f".into()),
                ),
            ],
            ..Default::default()
        };
        SessionView::new_text_compare(
            Endpoint::local(PathBuf::from("/left")),
            Endpoint::local(PathBuf::from("/right")),
            left_lines,
            right_lines,
            diff,
            Grammar::plain_text(),
        )
    }

    #[test]
    fn diff_line_numbers_delegates_to_lib() {
        let numbers = diff_line_numbers(&session());
        for n in [3, 4, 5, 7] {
            assert!(numbers.contains(&n), "missing line {n}");
        }
        assert!(!numbers.contains(&1));
    }

    #[test]
    fn diff_group_starts_delegates_to_lib() {
        // Diff rows 2, 3, 4 (0-based) form one group, row 6 another.
        assert_eq!(diff_group_starts(&session()), vec![2, 6]);
    }

    #[test]
    fn current_diff_position_uses_the_selected_row() {
        let mut session = session();
        session.ui.table_state.select(Some(0));
        assert_eq!(current_diff_position(&session), (0, 2));
        session.ui.table_state.select(Some(3));
        assert_eq!(current_diff_position(&session), (1, 2));
        session.ui.table_state.select(Some(9));
        assert_eq!(current_diff_position(&session), (2, 2));
    }

    #[test]
    fn session_without_a_text_diff_is_empty() {
        let mut session = session();
        session.text_mut().unwrap().text_diff = None;
        assert!(diff_line_numbers(&session).is_empty());
        assert!(diff_group_starts(&session).is_empty());
        assert_eq!(current_diff_position(&session), (0, 0));
    }
}
