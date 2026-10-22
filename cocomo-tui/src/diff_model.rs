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
pub(crate) fn diff_line_numbers(session: &SessionView) -> HashSet<usize> {
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
pub(crate) fn diff_group_starts(session: &SessionView) -> Vec<usize> {
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
pub(crate) fn current_diff_position(session: &SessionView) -> (usize, usize) {
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
