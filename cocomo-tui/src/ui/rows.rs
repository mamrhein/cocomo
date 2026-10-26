// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Row view models for the comparison tables and the diff list.
//!
//! These types are the memoized view models of F9: they are built from the
//! `cocomo-lib` data only when that data changes (keyed by the session's
//! revision counters), and cloned cheaply per frame afterwards — the cell
//! rows share an `Arc<[TableCell]>`, and a diff row is a single pair of
//! pre-styled lines plus a gutter status.

use std::{collections::HashSet, sync::Arc};

use cocomo_lib::{
    Importance, LineInfo, TransferAction, TransferItem, format_date,
    format_size,
};
use ratatui_kit::{
    prelude::*,
    ratatui::{
        style::{Modifier, Style},
        text::{Line, Span},
    },
};

use super::theme::CocomoTheme;
use crate::state::DirRow;

/// A pre-rendered table row: the cells are computed once when the
/// underlying data changes and shared through an `Arc` across frames.
///
/// The `index` is the row's absolute position in the table, so the themed
/// wrapper in [`super::session_table`] can highlight drag-range and hover
/// rows without re-deriving the window offset in its renderer.
#[derive(Clone)]
pub(super) struct CellRow {
    /// Absolute row index in the table (independent of the visible window).
    pub index: usize,
    cells: Arc<[TableCell]>,
}

impl CellRow {
    /// Return the row's cells for the `Table` renderer.
    pub fn cells(&self) -> Vec<TableCell> {
        self.cells.to_vec()
    }
}

/// The `Table` row renderer for pre-rendered [`CellRow`]s: it only hands
/// out the row's already-computed cells, so no styling work happens per
/// frame.
pub(super) fn cell_row_renderer() -> RenderTableRow<CellRow> {
    Arc::new(|row: &CellRow, _is_selected: bool| row.cells())
}

/// Build the directory-comparison rows from the visible tree rows.
///
/// The name cell carries the tree indent and the `+`/`-` toggle indicator;
/// file rows use a same-width blank so all names start at the same column
/// within a level.
pub(super) fn dir_rows(rows: &[DirRow], theme: &CocomoTheme) -> Vec<CellRow> {
    rows.iter()
        .enumerate()
        .map(|(index, row)| {
            let entry = row.entry;
            let status_style = theme.status_style(entry.status);

            let left_meta = entry
                .left
                .as_ref()
                .map(|l| {
                    format!(
                        "{:>6} {}",
                        format_size(l.size),
                        format_date(&l.modified)
                    )
                })
                .unwrap_or_else(|| "-             ".to_string());

            let right_meta = entry
                .right
                .as_ref()
                .map(|r| {
                    format!(
                        "{:>6} {}",
                        format_size(r.size),
                        format_date(&r.modified)
                    )
                })
                .unwrap_or_else(|| "-             ".to_string());

            let toggle = match (row.is_dir, row.expanded) {
                (true, true) => "-",
                (true, false) => "+",
                (false, _) => " ",
            };
            let indent = "  ".repeat(row.depth);

            CellRow {
                index,
                cells: Arc::from(vec![
                    TableCell::new(format!("{indent}{toggle} {}", entry.name)),
                    TableCell::new(left_meta),
                    TableCell::new(entry.status.symbol()).style(status_style),
                    TableCell::new(right_meta),
                ]),
            }
        })
        .collect()
}

/// Build the sync-preview rows from the planned transfer items.
pub(super) fn sync_rows(
    items: &[TransferItem],
    theme: &CocomoTheme,
) -> Vec<CellRow> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let action_color = match item.action {
                TransferAction::CopyLeft | TransferAction::CopyRight => {
                    theme.transfer_copy
                }
                TransferAction::CopyCenter => theme.transfer_copy_center,
                TransferAction::DeleteLeft | TransferAction::DeleteRight => {
                    theme.transfer_delete
                }
                TransferAction::MoveLeft | TransferAction::MoveRight => {
                    theme.transfer_move
                }
            };
            let dir_indicator = if item.is_dir { "\u{1F4C4} " } else { "  " };
            CellRow {
                index,
                cells: Arc::from(vec![
                    TableCell::new(item.action.label().to_string()).style(
                        Style::new()
                            .fg(action_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    TableCell::new(format!(
                        "{}{}",
                        dir_indicator, item.rel_path
                    )),
                ]),
            }
        })
        .collect()
}

/// Per-row status rendered as the one-character gutter between the two
/// diff panes (requirements §11.3).
///
/// The arrow points at the side that is behind: `<` when the right side
/// has new or changed content, `>` when the left side has content the
/// right side lacks. This mirrors the dir view's status symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DiffGutter {
    /// Both sides show the same line.
    Equal,
    /// The right side has new or changed content (`<`).
    RightNewer,
    /// The left side has content the right side lacks (`>`).
    LeftNewer,
}

impl DiffGutter {
    /// The single-character gutter symbol.
    pub fn symbol(self) -> &'static str {
        match self {
            DiffGutter::Equal => "=",
            DiffGutter::RightNewer => "<",
            DiffGutter::LeftNewer => ">",
        }
    }
}

/// One row of the side-by-side text diff: the two fully styled half-lines
/// (line number, diff marker, content) and the center gutter status.
#[derive(Clone)]
pub(super) struct TextDiffRow {
    /// Left half-line.
    pub left: Line<'static>,
    /// Right half-line.
    pub right: Line<'static>,
    /// Status shown in the gutter between the panes.
    pub gutter: DiffGutter,
}

/// Build the memoized text-diff rows from the two files' lines.
///
/// `left_diff_numbers` and `right_diff_numbers` are the per-side sets of
/// differing line numbers; a row's gutter is `<` when its right line is
/// part of a difference (changed or added), `>` when only its left line
/// is (removed), and `=` otherwise.
///
/// The result is `Arc`-wrapped so the component clones it in O(1) per
/// frame; the rows themselves are only rebuilt when the underlying diff
/// changes.
pub(super) fn text_diff_rows(
    left_lines: &[LineInfo],
    right_lines: &[LineInfo],
    left_diff_numbers: &HashSet<usize>,
    right_diff_numbers: &HashSet<usize>,
    theme: &CocomoTheme,
) -> Arc<Vec<TextDiffRow>> {
    let max_lines = std::cmp::max(left_lines.len(), right_lines.len());
    Arc::new(
        (0..max_lines)
            .map(|i| {
                let left = left_lines.get(i);
                let right = right_lines.get(i);
                let left_diff = left
                    .is_some_and(|l| left_diff_numbers.contains(&l.number));
                let right_diff = right
                    .is_some_and(|l| right_diff_numbers.contains(&l.number));
                let gutter = if right_diff {
                    // A changed or added right line is the newer content.
                    DiffGutter::RightNewer
                } else if left_diff {
                    DiffGutter::LeftNewer
                } else {
                    DiffGutter::Equal
                };
                TextDiffRow {
                    left: diff_side_line(
                        left,
                        left_diff_numbers,
                        theme,
                        theme.diff_removed,
                    ),
                    right: diff_side_line(
                        right,
                        right_diff_numbers,
                        theme,
                        theme.diff_added,
                    ),
                    gutter,
                }
            })
            .collect(),
    )
}

/// Render one side of a diff row: line number, diff marker, and content,
/// or a blank placeholder when the file has no line at this index.
///
/// `diff_numbers` is the *side's own* set of differing line numbers, so a
/// line is only marked when it participates in a difference on its own
/// side. Diff lines are highlighted with `diff_style` (red for the left,
/// green for the right side); otherwise the grammar classification of the
/// line decides the style.
fn diff_side_line(
    line: Option<&LineInfo>,
    diff_numbers: &HashSet<usize>,
    theme: &CocomoTheme,
    diff_style: Style,
) -> Line<'static> {
    let number = line.map(|l| l.number).unwrap_or(0);
    let content = line.map(|l| l.content.clone()).unwrap_or_default();
    let is_diff = line.is_some_and(|l| diff_numbers.contains(&l.number));
    let content_style = if is_diff {
        diff_style
    } else {
        theme.grammar_style(
            line.map(importance_for_line).unwrap_or(Importance::Ignored),
        )
    };
    let marker: &str = if is_diff { "\u{25B6} " } else { "  " };
    Line::from(vec![
        Span::styled(format!("{number:<6}"), theme.dim),
        Span::styled(marker, content_style),
        Span::styled(content, content_style),
    ])
}

/// Return the highest importance token in a line.
fn importance_for_line(line: &LineInfo) -> Importance {
    let mut max_imp = Importance::Ignored;
    for token in &line.tokens {
        if token.importance > max_imp {
            max_imp = token.importance;
        }
    }
    max_imp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(contents: &[&str]) -> Vec<LineInfo> {
        contents
            .iter()
            .enumerate()
            .map(|(i, c)| LineInfo::new(i + 1, (*c).to_string()))
            .collect()
    }

    fn numbers(nums: &[usize]) -> HashSet<usize> {
        nums.iter().copied().collect()
    }

    #[test]
    fn text_diff_rows_gutter_points_at_the_behind_side() {
        // Line 2 changed on both sides, line 4 exists only on the left.
        let left = lines(&["a", "b", "c", "d"]);
        let right = lines(&["a", "x", "c"]);
        let rows = text_diff_rows(
            &left,
            &right,
            &numbers(&[2, 4]),
            &numbers(&[2]),
            &CocomoTheme::default(),
        );
        let gutters: Vec<&str> =
            rows.iter().map(|r| r.gutter.symbol()).collect();
        assert_eq!(gutters, vec!["=", "<", "=", ">"]);
    }

    #[test]
    fn text_diff_rows_mark_only_their_own_side() {
        let left = lines(&["a"]);
        let right = lines(&["b"]);
        // Only the right line participates in a difference: the left line
        // must not carry a diff marker even though line 1 differs on the
        // other side.
        let rows = text_diff_rows(
            &left,
            &right,
            &numbers(&[]),
            &numbers(&[1]),
            &CocomoTheme::default(),
        );
        assert_eq!(rows[0].left.spans[1].content.as_ref(), "  ");
        assert_eq!(rows[0].right.spans[1].content.as_ref(), "\u{25B6} ");
        assert_eq!(rows[0].gutter, DiffGutter::RightNewer);
    }
}
