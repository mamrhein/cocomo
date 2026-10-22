// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Pure diff analysis over a text-compare session: which rows contain
//! differences, how they group into contiguous blocks, and where the cursor
//! sits relative to those groups.

use std::collections::HashSet;

use cocomo_lib::TextDifference;

use crate::state::SessionView;

/// Build a set of line numbers that are part of a difference.
pub fn diff_line_numbers(session: &SessionView) -> HashSet<usize> {
    let mut numbers = HashSet::new();

    if let Some(ref diff) = session.text_diff {
        for td in &diff.differences {
            match td {
                TextDifference::LineDifferent(left, right) => {
                    numbers.insert(left.number);
                    numbers.insert(right.number);
                }
                TextDifference::LinesAdded(_, lines) => {
                    for l in lines {
                        numbers.insert(l.number);
                    }
                }
                TextDifference::LinesRemoved(_, lines) => {
                    for l in lines {
                        numbers.insert(l.number);
                    }
                }
                TextDifference::LinesChanged(_, removed, added) => {
                    for l in removed {
                        numbers.insert(l.number);
                    }
                    for l in added {
                        numbers.insert(l.number);
                    }
                }
                TextDifference::BlankLine => {}
            }
        }
    }

    numbers
}

/// Return the first row index of each contiguous difference group.
///
/// Each element is the row index (0-based into the merged view) where a
/// difference group starts. These are the targets for n/N navigation.
pub fn diff_group_starts(session: &SessionView) -> Vec<usize> {
    let diff_numbers = diff_line_numbers(session);

    let max_lines =
        std::cmp::max(session.left_lines.len(), session.right_lines.len());

    // Find all rows that contain at least one diff line.
    let diff_rows: Vec<usize> = (0..max_lines)
        .filter(|&i| {
            let left_is_diff = session
                .left_lines
                .get(i)
                .is_some_and(|l| diff_numbers.contains(&l.number));
            let right_is_diff = session
                .right_lines
                .get(i)
                .is_some_and(|l| diff_numbers.contains(&l.number));
            left_is_diff || right_is_diff
        })
        .collect();

    // Group contiguous rows and return the first index of each group.
    let mut groups = Vec::new();
    let mut i = 0;
    while i < diff_rows.len() {
        groups.push(diff_rows[i]);
        while i + 1 < diff_rows.len() && diff_rows[i + 1] == diff_rows[i] + 1 {
            i += 1;
        }
        i += 1;
    }

    groups
}

/// Return the current diff group index (1-based) and total count.
///
/// Scans backward from the current selection to find which contiguous
/// difference group the cursor is on. Returns `(current, total)` where
/// `current` is 1-based. Returns `(0, total)` if no group is selected.
pub fn current_diff_position(session: &SessionView) -> (usize, usize) {
    let groups = diff_group_starts(session);
    let total = groups.len();
    if total == 0 {
        return (0, 0);
    }

    let selected = session.table_state.selected().unwrap_or(0);

    // Find the group whose start is <= selected.
    for (i, &start) in groups.iter().enumerate().rev() {
        if selected >= start {
            return (i + 1, total);
        }
    }

    // Selected is before the first group.
    (0, total)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use cocomo_lib::{Grammar, LineInfo, TextDiff, TextDifference};

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
            PathBuf::from("/left"),
            PathBuf::from("/right"),
            left_lines,
            right_lines,
            diff,
            Grammar::plain_text(),
        )
    }

    #[test]
    fn diff_line_numbers_collects_all_difference_lines() {
        let numbers = diff_line_numbers(&session());
        for n in [3, 4, 5, 7] {
            assert!(numbers.contains(&n), "missing line {n}");
        }
        assert!(!numbers.contains(&1));
        assert!(!numbers.contains(&10));
    }

    #[test]
    fn diff_group_starts_merges_contiguous_rows() {
        // Diff rows 2, 3, 4 (0-based) form one group, row 6 another.
        assert_eq!(diff_group_starts(&session()), vec![2, 6]);
    }

    #[test]
    fn current_diff_position_counts_groups_one_based() {
        let mut session = session();
        // Before the first group.
        session.table_state.select(Some(0));
        assert_eq!(current_diff_position(&session), (0, 2));
        session.table_state.select(Some(3));
        assert_eq!(current_diff_position(&session), (1, 2));
        session.table_state.select(Some(9));
        assert_eq!(current_diff_position(&session), (2, 2));
    }

    #[test]
    fn empty_diff_has_no_groups() {
        let mut session = session();
        session.text_diff = Some(TextDiff::default());
        assert!(diff_group_starts(&session).is_empty());
        assert_eq!(current_diff_position(&session), (0, 0));
    }
}
