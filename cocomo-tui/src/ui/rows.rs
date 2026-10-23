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
//! pre-styled lines.

use std::{collections::HashSet, sync::Arc};

use cocomo_lib::{
    DirEntry, Importance, LineInfo, TransferAction, TransferItem, format_date,
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

/// A pre-rendered table row: the cells are computed once when the
/// underlying data changes and shared through an `Arc` across frames.
#[derive(Clone)]
pub(super) struct CellRow(Arc<[TableCell]>);

impl CellRow {
    /// Return the row's cells for the `Table` renderer.
    pub fn cells(&self) -> Vec<TableCell> {
        self.0.to_vec()
    }
}

/// The `Table` row renderer for pre-rendered [`CellRow`]s: it only hands
/// out the row's already-computed cells, so no styling work happens per
/// frame.
pub(super) fn cell_row_renderer() -> RenderTableRow<CellRow> {
    Arc::new(|row: &CellRow, _is_selected: bool| row.cells())
}

/// Build the directory-comparison rows from the filtered entries.
pub(super) fn dir_rows(
    entries: &[&DirEntry],
    theme: &CocomoTheme,
) -> Vec<CellRow> {
    entries
        .iter()
        .map(|entry| {
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

            let dir_indicator =
                if entry.left.as_ref().is_some_and(|l| l.is_dir)
                    || entry.right.as_ref().is_some_and(|r| r.is_dir)
                {
                    "\u{1F5C0}"
                } else {
                    " "
                };

            CellRow(Arc::from(vec![
                TableCell::new(format!("{} {}", dir_indicator, entry.name)),
                TableCell::new(left_meta),
                TableCell::new(entry.status.symbol()).style(status_style),
                TableCell::new(right_meta),
            ]))
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
        .map(|item| {
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
            CellRow(Arc::from(vec![
                TableCell::new(item.action.label().to_string()).style(
                    Style::new().fg(action_color).add_modifier(Modifier::BOLD),
                ),
                TableCell::new(format!("{}{}", dir_indicator, item.rel_path)),
            ]))
        })
        .collect()
}

/// One row of the side-by-side text diff: the two fully styled half-lines
/// (line number, diff marker, content).
#[derive(Clone)]
pub(super) struct TextDiffRow {
    /// Left half-line.
    pub left: Line<'static>,
    /// Right half-line.
    pub right: Line<'static>,
}

/// Build the memoized text-diff rows from the two files' lines.
///
/// The result is `Arc`-wrapped so the component clones it in O(1) per
/// frame; the rows themselves are only rebuilt when the underlying diff
/// changes.
pub(super) fn text_diff_rows(
    left_lines: &[LineInfo],
    right_lines: &[LineInfo],
    diff_numbers: &HashSet<usize>,
    theme: &CocomoTheme,
) -> Arc<Vec<TextDiffRow>> {
    let max_lines = std::cmp::max(left_lines.len(), right_lines.len());
    Arc::new(
        (0..max_lines)
            .map(|i| TextDiffRow {
                left: diff_side_line(
                    left_lines.get(i),
                    diff_numbers,
                    theme,
                    theme.diff_removed,
                ),
                right: diff_side_line(
                    right_lines.get(i),
                    diff_numbers,
                    theme,
                    theme.diff_added,
                ),
            })
            .collect(),
    )
}

/// Render one side of a diff row: line number, diff marker, and content,
/// or a blank placeholder when the file has no line at this index.
///
/// Diff lines are highlighted with `diff_style` (red for the left, green
/// for the right side); otherwise the grammar classification of the line
/// decides the style.
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
