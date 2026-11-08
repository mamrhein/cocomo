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

use std::sync::Arc;

use cocomo_lib::{
    Importance, LineInfo, MergedDiffRow, MergedRowStatus, format_date,
    format_size,
};
use ratatui_kit::{
    prelude::*,
    ratatui::{
        style::Style,
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
/// The name cell carries the box-drawing tree prefix computed by
/// [`crate::state::DirRow::tree_prefix`] (the same indicators as
/// `cocomo-cli`'s tree output); directories are marked with a `/` suffix.
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
                // The absent side of an orphan entry stays blank.
                .unwrap_or_default();

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
                .unwrap_or_default();

            // Directories carry a `/` suffix, as in the CLI tree output.
            let display_name = if row.is_dir {
                format!("{}/", entry.name)
            } else {
                entry.name.clone()
            };

            CellRow {
                index,
                cells: Arc::from(vec![
                    TableCell::new(format!(
                        "{}{}",
                        row.tree_prefix, display_name
                    )),
                    TableCell::new(left_meta),
                    // The status color rides on the cell style: the
                    // themed wrapper composes the selection background
                    // under it, so the fg survives on the selected row.
                    TableCell::new(entry.status.symbol().0)
                        .style(status_style),
                    TableCell::new(right_meta),
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

/// Build the memoized text-diff rows from the aligned diff rows.
///
/// Each merged row contributes one display row: its two (possibly absent)
/// line halves are pre-styled, and the gutter status follows the row's
/// alignment — `<` when the right side carries new or changed content
/// (inserted or changed), `>` when only the left side does (deleted), and
/// `=` for equal context.
///
/// The result is `Arc`-wrapped so the component clones it in O(1) per
/// frame; the rows themselves are only rebuilt when the underlying diff
/// changes.
pub(super) fn text_diff_rows(
    merged: &[MergedDiffRow],
    theme: &CocomoTheme,
) -> Arc<Vec<TextDiffRow>> {
    Arc::new(
        merged
            .iter()
            .map(|row| {
                let gutter = match row.status {
                    MergedRowStatus::Same => DiffGutter::Equal,
                    MergedRowStatus::Changed | MergedRowStatus::Inserted => {
                        // The right side carries the newer content.
                        DiffGutter::RightNewer
                    }
                    MergedRowStatus::Deleted => DiffGutter::LeftNewer,
                };
                let is_diff = row.status.is_difference();
                TextDiffRow {
                    left: diff_side_line(
                        row.left.as_ref(),
                        is_diff,
                        theme,
                        theme.diff_removed,
                    ),
                    right: diff_side_line(
                        row.right.as_ref(),
                        is_diff,
                        theme,
                        theme.diff_added,
                    ),
                    gutter,
                }
            })
            .collect(),
    )
}

/// Width of the line-number column on each side.
const LINE_NUMBER_WIDTH: usize = 6;

/// Render one side of a diff row: line number, diff marker, and content.
///
/// The absent side of a filler row stays fully blank: no number, no
/// marker, no highlight, so filler rows read as gaps rather than as line
/// 0.
///
/// Diff lines are highlighted with `diff_style` (red for the left, green
/// for the right side); otherwise the grammar classification of the line
/// decides the style.
fn diff_side_line(
    line: Option<&LineInfo>,
    is_diff: bool,
    theme: &CocomoTheme,
    diff_style: Style,
) -> Line<'static> {
    let Some(line) = line else {
        return Line::from(vec![
            Span::raw(" ".repeat(LINE_NUMBER_WIDTH)),
            Span::raw("  "),
            Span::raw(""),
        ]);
    };
    let content_style = if is_diff {
        diff_style
    } else {
        theme.grammar_style(importance_for_line(line))
    };
    let marker: &str = if is_diff { "\u{25B6} " } else { "  " };
    let number = format!("{:<width$}", line.number, width = LINE_NUMBER_WIDTH);
    Line::from(vec![
        Span::styled(number, theme.dim),
        Span::styled(marker, content_style),
        Span::styled(line.content.clone(), content_style),
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
    use cocomo_lib::{DirEntry, DirEntryStatus, EntryInfo};

    use super::*;

    #[test]
    fn selected_row_keeps_the_cell_foreground() {
        use ratatui_kit::{
            ratatui::{
                layout::Constraint,
                style::{Color, Modifier},
            },
            test_util::render_frames,
        };

        use super::super::{
            session_table::SessionTable, theme::cocomo_palette,
        };

        let palette = cocomo_palette();
        let theme = CocomoTheme::default();
        let style = theme.status_style(DirEntryStatus::LeftNewer);
        let rows = vec![
            CellRow {
                index: 0,
                cells: Arc::from(vec![TableCell::new("alpha").style(style)]),
            },
            CellRow {
                index: 1,
                cells: Arc::from(vec![TableCell::new("beta").style(style)]),
            },
        ];
        let buf = render_frames(
            element!(PaletteProvider(palette: palette) {
                SessionTable(
                    selected: Some(0),
                    selection_range: None,
                    title: "t".to_string(),
                    columns: vec![TableColumn::new(
                        "name",
                        Constraint::Fill(1),
                    )],
                    rows: rows,
                    render_row: None,
                    empty_message: Line::default(),
                    on_select: Handler::default(),
                    on_extend: Handler::default(),
                )
            }),
            40,
            10,
            1,
        );
        // Body rows start after the block border and the header row.
        let selected = &buf[(1, 2)];
        assert_eq!(selected.symbol(), "a");
        // The selection marks the row by background while the cell's own
        // foreground (and its bold) survive.
        assert_eq!(selected.fg, palette.error);
        assert_eq!(selected.bg, palette.selection);
        assert!(selected.modifier.contains(Modifier::BOLD));
        // The unselected row keeps its foreground and no background.
        let plain = &buf[(1, 3)];
        assert_eq!(plain.symbol(), "b");
        assert_eq!(plain.fg, palette.error);
        assert_eq!(plain.bg, Color::Reset);
    }

    fn file_entry(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            status: DirEntryStatus::Same,
            left: None,
            right: None,
            center: None,
            sub_entries: None,
            name_conflict: false,
        }
    }

    fn dir_entry(name: &str) -> DirEntry {
        let info = || EntryInfo {
            name: name.to_string(),
            path: name.to_string(),
            is_dir: true,
            ..Default::default()
        };
        DirEntry {
            left: Some(info()),
            right: Some(info()),
            ..file_entry(name)
        }
    }

    fn name_cell(row: &CellRow) -> String {
        let cells = row.cells();
        cells[0]
            .line
            .spans
            .first()
            .map(|s| s.content.to_string())
            .unwrap_or_default()
    }

    #[test]
    fn dir_rows_render_cli_style_names() {
        let entries = [dir_entry("sub"), file_entry("x.txt")];
        let rows = vec![
            DirRow {
                entry: &entries[0],
                rel_path: "sub".into(),
                depth: 0,
                is_dir: true,
                expanded: true,
                tree_prefix: String::new(),
            },
            DirRow {
                entry: &entries[1],
                rel_path: "sub/x.txt".into(),
                depth: 1,
                is_dir: false,
                expanded: false,
                tree_prefix: "\u{2514}\u{2500} ".into(),
            },
        ];
        let cell_rows = dir_rows(&rows, &CocomoTheme::default());
        // Directories carry the `/` suffix; the tree prefix passes through
        // unchanged; plain files carry neither.
        assert_eq!(name_cell(&cell_rows[0]), "sub/");
        assert_eq!(name_cell(&cell_rows[1]), "\u{2514}\u{2500} x.txt");
    }

    #[test]
    fn text_diff_rows_gutter_points_at_the_behind_side() {
        // Line 2 changed on both sides, line 4 exists only on the left.
        let diff = cocomo_lib::compare_texts(
            "a\nb\nc\nd\n",
            "a\nx\nc\n",
            &cocomo_lib::TextCompareSettings::new(),
        );
        let rows =
            text_diff_rows(&diff.merged_rows(), &CocomoTheme::default());
        let gutters: Vec<&str> =
            rows.iter().map(|r| r.gutter.symbol()).collect();
        assert_eq!(gutters, vec!["=", "<", "=", ">"]);
    }

    #[test]
    fn text_diff_rows_fill_blank_cells_without_markers() {
        let diff = cocomo_lib::compare_texts(
            "a\n",
            "a\nb\n",
            &cocomo_lib::TextCompareSettings::new(),
        );
        let rows =
            text_diff_rows(&diff.merged_rows(), &CocomoTheme::default());
        assert_eq!(rows.len(), 2);
        // The inserted row's left half is a blank placeholder: no diff
        // marker on the absent side, while the present side is marked.
        assert_eq!(rows[1].left.spans[1].content.as_ref(), "  ");
        assert_eq!(rows[1].right.spans[1].content.as_ref(), "\u{25B6} ");
        assert_eq!(rows[1].gutter, DiffGutter::RightNewer);
    }
}
