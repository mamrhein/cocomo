// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Table compare view for delimited text files (UI spec section 7, new
//! session types).
//!
//! [`TableDiffSession`] owns one structured comparison: the parsed grids
//! of both files and the row-aligned diff model derived from them.
//! [`TableDiffView`] renders the aligned rows with per-cell diff coloring
//! and a status bar.
//!
//! The comparison renders through a [`DataTable`] (the same delegate
//! pattern as the folder table): one body row per [`AlignedTableRow`],
//! and one column per side per logical column — the left group followed
//! by the right group. The union of both header rows defines the logical
//! columns (see [`table_column_names`]). A cell whose text differs from
//! its counterpart is tinted with the important-difference color
//! ([`ColorRole::Important`]); a one-sided row tints its present side
//! with the orphan color ([`ColorRole::Orphan`]). The table's own cell
//! padding is zeroed ([`Column::p_0`]) so those backgrounds fill the
//! whole cell, with [`CELL_PAD`] applied inside them.

use std::{sync::Arc, time::Instant};

use cocomo_lib::{ColorOverrides, Endpoint, FsError, NodeFileSystem};
use gpui::{
    AnyElement, App, Context, ElementId, Entity, FocusHandle, Focusable,
    IntoElement, Pixels, Render, SharedString, Subscription, Window,
    component::{
        ActiveTheme, FocusableExt as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        spinner::Spinner,
        status_bar::StatusBar,
        table::{Column, DataTable, TableDelegate, TableState},
        v_flex,
    },
    div,
    prelude::*,
    px, rems,
};

use crate::{
    color::{self, ColorRole},
    log::{self, LogLevel},
    session_manager::GuiSessionManager,
};

/// Comparison table row height, in pixels. Matches the folder table's
/// density.
const ROW_HEIGHT: Pixels = px(24.);

/// Horizontal padding inside a data cell. The table's own cell padding
/// is zeroed (see [`Column::p_0`]) so diff backgrounds can fill the
/// whole cell; this pads the text inside the background.
const CELL_PAD: Pixels = px(6.);

/// Alpha of the diff/orphan cell background tint, so row stripes and
/// cell content stay readable underneath it.
const CELL_BG_OPACITY: f32 = 0.25;

// ---------------------------------------------------------------------------
// Table model
// ---------------------------------------------------------------------------

/// A parsed delimited-text grid: a header row plus data rows.
#[derive(Clone, Debug, Default)]
pub(crate) struct TableGrid {
    /// Column names from the first row.
    pub(crate) header: Vec<String>,
    /// The remaining rows, each with one cell per column.
    pub(crate) rows: Vec<Vec<String>>,
}

/// Pick the delimiter for a delimited-text file by extension: `.tsv` and
/// `.tab` are tab-separated, everything else comma-separated.
pub(crate) fn delimiter_for(extension: &str) -> char {
    match extension {
        "tsv" | "tab" => '\t',
        _ => ',',
    }
}

/// Parse a delimited-text buffer into a grid.
///
/// Quoted fields (RFC 4180 double quotes) are supported; a parse problem
/// degrades to a best-effort split rather than failing the comparison.
pub(crate) fn parse_table(text: &str, delimiter: char) -> TableGrid {
    let mut grid = TableGrid::default();
    let mut lines = text.lines();
    let Some(header_line) = lines.next() else {
        return grid;
    };
    grid.header = split_delimited(header_line, delimiter);
    for line in lines {
        grid.rows.push(split_delimited(line, delimiter));
    }
    grid
}

/// Split one line on `delimiter`, honoring double-quoted fields.
fn split_delimited(line: &str, delimiter: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes && chars.peek() == Some(&'"') => {
                current.push('"');
                chars.next();
            }
            '"' => in_quotes = !in_quotes,
            c if c == delimiter && !in_quotes => {
                fields.push(std::mem::take(&mut current));
            }
            c => current.push(c),
        }
    }
    fields.push(current);
    fields
}

/// How one aligned row pair relates between the two grids.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TableRowStatus {
    /// Both sides carry the same cells.
    Same,
    /// The row exists on both sides with at least one differing cell.
    Different,
    /// The row exists only on the left.
    LeftOnly,
    /// The row exists only on the right.
    RightOnly,
}

/// One row of the aligned comparison.
#[derive(Clone, Debug)]
pub(crate) struct AlignedTableRow {
    /// Left row index, or `None` for a right-only row.
    pub(crate) left: Option<usize>,
    /// Right row index, or `None` for a left-only row.
    pub(crate) right: Option<usize>,
    /// How the pair relates.
    pub(crate) status: TableRowStatus,
}

/// Align two grids' rows into a shared display order.
///
/// Rows are matched by content in order (a longest-common-subsequence
/// alignment); unmatched rows become one-sided entries at their position.
pub(crate) fn align_table_rows(
    left: &[Vec<String>],
    right: &[Vec<String>],
) -> Vec<AlignedTableRow> {
    // LCS over row contents. Quadratic in the row count, which is fine
    // for the grid sizes a table compare targets; very large inputs are
    // truncated to a positional alignment instead.
    if left.len().saturating_mul(right.len()) > 4_000_000 {
        let mut rows = Vec::new();
        for ix in 0..left.len().max(right.len()) {
            let l = left.get(ix);
            let r = right.get(ix);
            let status = match (l, r) {
                (Some(a), Some(b)) if a == b => TableRowStatus::Same,
                (Some(_), Some(_)) => TableRowStatus::Different,
                (Some(_), None) => TableRowStatus::LeftOnly,
                (None, Some(_)) => TableRowStatus::RightOnly,
                (None, None) => unreachable!(),
            };
            rows.push(AlignedTableRow {
                left: l.map(|_| ix),
                right: r.map(|_| ix),
                status,
            });
        }
        return rows;
    }
    let n = left.len();
    let m = right.len();
    // `lcs[i][j]`: LCS length of left[i..] and right[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if left[i] == right[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut rows = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if left[i] == right[j] {
            rows.push(AlignedTableRow {
                left: Some(i),
                right: Some(j),
                status: TableRowStatus::Same,
            });
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            // Consume the left row; if the right row at the same display
            // slot is its counterpart candidate, pair them as different
            // only when neither side advances toward the other's LCS.
            rows.push(AlignedTableRow {
                left: Some(i),
                right: None,
                status: TableRowStatus::LeftOnly,
            });
            i += 1;
        } else {
            rows.push(AlignedTableRow {
                left: None,
                right: Some(j),
                status: TableRowStatus::RightOnly,
            });
            j += 1;
        }
    }
    while i < n {
        rows.push(AlignedTableRow {
            left: Some(i),
            right: None,
            status: TableRowStatus::LeftOnly,
        });
        i += 1;
    }
    while j < m {
        rows.push(AlignedTableRow {
            left: None,
            right: Some(j),
            status: TableRowStatus::RightOnly,
        });
        j += 1;
    }
    pair_replaced_rows(&mut rows);
    rows
}

/// Merge runs of left-only rows followed by right-only rows into
/// `Different` pairs: a replaced row reads better side by side than as
/// two one-sided rows.
pub(crate) fn pair_replaced_rows(rows: &mut Vec<AlignedTableRow>) {
    let mut i = 0;
    while i < rows.len() {
        if rows[i].status != TableRowStatus::LeftOnly {
            i += 1;
            continue;
        }
        let left_run_start = i;
        while i < rows.len() && rows[i].status == TableRowStatus::LeftOnly {
            i += 1;
        }
        let right_run_start = i;
        while i < rows.len() && rows[i].status == TableRowStatus::RightOnly {
            i += 1;
        }
        let left_run = right_run_start - left_run_start;
        let right_run = i - right_run_start;
        let paired = left_run.min(right_run);
        for k in 0..paired {
            let left = rows[left_run_start + k].left;
            let right = rows[right_run_start + k].right;
            rows[left_run_start + k] = AlignedTableRow {
                left,
                right,
                status: TableRowStatus::Different,
            };
        }
        // Drop the right rows that were folded into the paired entries.
        let drop_from = right_run_start;
        let drop_to = right_run_start + paired;
        rows.drain(drop_from..drop_to);
        // Resume past the whole left run: its rows are either paired in
        // place or stay left-only (a run with no right counterpart must
        // not restart the scan at its head).
        i = left_run_start + left_run;
    }
}

/// Header names for the comparison table: the left group's names and
/// the right group's names, one per logical column.
///
/// The union of both header rows defines the columns: one per position
/// up to the longer header. When both files carry the same number of
/// columns the left header wins, so both groups show the left names
/// (falling back to the right name, then to `column N`, where the left
/// entry is empty). When the header lengths differ the two sides cannot
/// share a name, so each group labels its columns positionally:
/// `left.N` / `right.N` (1-based) wherever that side has no header
/// entry of its own.
pub(crate) fn table_column_names(
    left: &[String],
    right: &[String],
) -> (Vec<String>, Vec<String>) {
    let count = left.len().max(right.len());
    let same_len = left.len() == right.len();
    let named = |header: &[String], i: usize| {
        header.get(i).filter(|name| !name.is_empty()).cloned()
    };
    let left_names: Vec<String> = (0..count)
        .map(|i| {
            if same_len {
                named(left, i)
                    .or_else(|| named(right, i))
                    .unwrap_or_else(|| format!("column {}", i + 1))
            } else {
                named(left, i).unwrap_or_else(|| format!("left.{}", i + 1))
            }
        })
        .collect();
    let right_names: Vec<String> = (0..count)
        .map(|i| {
            if same_len {
                // The left header wins for both groups.
                left_names[i].clone()
            } else {
                named(right, i).unwrap_or_else(|| format!("right.{}", i + 1))
            }
        })
        .collect();
    (left_names, right_names)
}

/// The diff color role of one displayed cell, given the cell texts of
/// both sides. A side is `None` when the aligned row is one-sided.
///
/// Paired cells with different text are an important difference; a cell
/// whose counterpart side is missing entirely is an orphan. Equal texts
/// stay plain, and an empty cell opposite an empty one stays plain too.
pub(crate) fn cell_role(
    left: Option<&str>,
    right: Option<&str>,
) -> Option<ColorRole> {
    match (left, right) {
        (Some(a), Some(b)) if a != b => Some(ColorRole::Important),
        (Some(_), None) | (None, Some(_)) => Some(ColorRole::Orphan),
        _ => None,
    }
}

/// Status-bar row counts: total, different, left-only, right-only.
pub(crate) fn row_counts(
    rows: &[AlignedTableRow],
) -> (usize, usize, usize, usize) {
    let mut different = 0;
    let mut left_only = 0;
    let mut right_only = 0;
    for row in rows {
        match row.status {
            TableRowStatus::Same => {}
            TableRowStatus::Different => different += 1,
            TableRowStatus::LeftOnly => left_only += 1,
            TableRowStatus::RightOnly => right_only += 1,
        }
    }
    (rows.len(), different, left_only, right_only)
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// One structured comparison: the parsed grids of both files and the
/// aligned row model derived from them.
pub struct TableDiffSession {
    /// Focus handle shared with the tab entity.
    focus_handle: FocusHandle,
    /// Left file endpoint.
    left: Endpoint,
    /// Right file endpoint.
    right: Endpoint,
    /// Tab title.
    title: SharedString,
    /// The owning window's session manager, for follow-up tabs.
    manager: Option<Entity<GuiSessionManager>>,
    /// Parsed left grid once loaded.
    left_grid: Option<Arc<TableGrid>>,
    /// Parsed right grid once loaded.
    right_grid: Option<Arc<TableGrid>>,
    /// Aligned rows once computed.
    aligned: Arc<Vec<AlignedTableRow>>,
    /// Whether a load is in flight.
    loading: bool,
    /// Load failure, shown in the view instead of the grid.
    error: Option<String>,
    /// Counter that invalidates superseded loads.
    load_revision: u64,
}

impl TableDiffSession {
    /// Create a session for the given file pair; nothing is read until
    /// [`Self::ensure_loaded`].
    pub fn new(
        left: Endpoint,
        right: Endpoint,
        title: SharedString,
        manager: Option<Entity<GuiSessionManager>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            left,
            right,
            title,
            manager,
            left_grid: None,
            right_grid: None,
            aligned: Arc::new(Vec::new()),
            loading: false,
            error: None,
            load_revision: 0,
        }
    }

    /// The session's focus handle.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// The tab title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// The compared endpoints, for "Compare files using ▸".
    pub fn pair_endpoints(&self) -> (Endpoint, Endpoint) {
        (self.left.clone(), self.right.clone())
    }

    /// Re-point the session at a new owning manager (tab moved between
    /// windows).
    pub fn set_manager(&mut self, manager: Entity<GuiSessionManager>) {
        self.manager = Some(manager);
    }

    /// The parsed left grid, if any.
    pub fn left_grid(&self) -> Option<&Arc<TableGrid>> {
        self.left_grid.as_ref()
    }

    /// The parsed right grid, if any.
    pub fn right_grid(&self) -> Option<&Arc<TableGrid>> {
        self.right_grid.as_ref()
    }

    /// The aligned comparison rows.
    pub fn aligned(&self) -> &Arc<Vec<AlignedTableRow>> {
        &self.aligned
    }

    /// Whether a load is in flight.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The load failure, if the last load failed.
    pub fn error(&self) -> Option<&String> {
        self.error.as_ref()
    }

    /// Start the initial load unless grids are already loaded or in
    /// flight.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.left_grid.is_none() && !self.loading && self.error.is_none() {
            self.load(cx);
        }
    }

    /// Re-read both files and rebuild the aligned model.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.load_revision += 1;
        let revision = self.load_revision;
        self.error = None;
        cx.notify();

        let left = self.left.clone();
        let right = self.right.clone();
        let delimiter = delimiter_for(
            &left
                .path()
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        );
        let weak = cx.entity().downgrade();

        // File I/O runs on the tokio runtime because the providers use
        // tokio-based IO internally.
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let started = Instant::now();
            let read_side = |endpoint: Endpoint| async move {
                match endpoint.fs().read_path(endpoint.path(), None).await {
                    Ok(bytes) => Ok::<_, anyhow::Error>(bytes.to_vec()),
                    Err(FsError::NotFound { .. }) => Ok(Vec::new()),
                    Err(e) => Err(anyhow::anyhow!("{e}")),
                }
            };
            let left = read_side(left).await?;
            let right = read_side(right).await?;
            let left_text = String::from_utf8_lossy(&left).into_owned();
            let right_text = String::from_utf8_lossy(&right).into_owned();
            let left_grid = parse_table(&left_text, delimiter);
            let right_grid = parse_table(&right_text, delimiter);
            Ok::<_, anyhow::Error>((left_grid, right_grid, started.elapsed()))
        });

        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    let Some(entity) = weak.upgrade() else { return };
                    entity.update(cx, |session, cx| {
                        if revision != session.load_revision {
                            return;
                        }
                        match result {
                            Ok((left_grid, right_grid, elapsed)) => {
                                let aligned = Arc::new(align_table_rows(
                                    &left_grid.rows,
                                    &right_grid.rows,
                                ));
                                session.left_grid = Some(Arc::new(left_grid));
                                session.right_grid =
                                    Some(Arc::new(right_grid));
                                session.aligned = aligned;
                                log::push(
                                    LogLevel::Info,
                                    format!(
                                        "Table comparison completed in {} ms",
                                        elapsed.as_millis()
                                    ),
                                    cx,
                                );
                            }
                            Err(e) => {
                                session.error =
                                    Some(format!("Comparison failed: {e}"));
                                log::push(
                                    LogLevel::Error,
                                    format!("Table comparison failed: {e}"),
                                    cx,
                                );
                            }
                        }
                        session.loading = false;
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }
}

impl Focusable for TableDiffSession {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// DataTable delegate
// ---------------------------------------------------------------------------

/// [`DataTable`] delegate over a [`TableDiffSession`].
///
/// A thin view like the folder table's delegate: all data lives in the
/// session entity, and the delegate only maps aligned rows and the
/// union header onto table coordinates. Columns are built lazily from
/// the session because the grids arrive asynchronously.
struct TableDiffDelegate {
    session: Entity<TableDiffSession>,
    /// Width of one data column, derived from the window's rem size at
    /// construction.
    col_width: Pixels,
}

impl TableDiffDelegate {
    fn new(session: Entity<TableDiffSession>, rem: Pixels) -> Self {
        Self {
            session,
            col_width: rems(10.).to_pixels(rem),
        }
    }

    /// The logical column count: the longer of the two header rows.
    fn logical_count(&self, cx: &App) -> usize {
        let session = self.session.read(cx);
        let left = session.left_grid().map(|g| g.header.len()).unwrap_or(0);
        let right = session.right_grid().map(|g| g.header.len()).unwrap_or(0);
        left.max(right)
    }

    /// The two groups' header names, from the loaded grids.
    fn group_names(&self, cx: &App) -> (Vec<String>, Vec<String>) {
        let session = self.session.read(cx);
        let empty = Vec::new();
        let left = session.left_grid().map(|g| &g.header).unwrap_or(&empty);
        let right = session.right_grid().map(|g| &g.header).unwrap_or(&empty);
        table_column_names(left, right)
    }

    /// The two sides' cell texts for logical column `i` of aligned row
    /// `row_ix`. Each side is `None` when the aligned row is one-sided;
    /// a paired row whose side lacks the column reads as an empty cell.
    fn cell_texts<'a>(
        &self,
        row_ix: usize,
        i: usize,
        cx: &'a App,
    ) -> Option<(Option<&'a str>, Option<&'a str>)> {
        let session = self.session.read(cx);
        let row = session.aligned().get(row_ix)?;
        let left = row.left.map(|li| {
            session
                .left_grid()
                .and_then(|g| g.rows.get(li))
                .and_then(|r| r.get(i))
                .map(String::as_str)
                .unwrap_or("")
        });
        let right = row.right.map(|ri| {
            session
                .right_grid()
                .and_then(|g| g.rows.get(ri))
                .and_then(|r| r.get(i))
                .map(String::as_str)
                .unwrap_or("")
        });
        Some((left, right))
    }
}

impl TableDelegate for TableDiffDelegate {
    fn columns_count(&self, cx: &App) -> usize {
        self.logical_count(cx) * 2
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.session.read(cx).aligned().len()
    }

    fn column(&self, col_ix: usize, cx: &App) -> Column {
        let count = self.logical_count(cx);
        let (left_names, right_names) = self.group_names(cx);
        let (key, name) = if col_ix < count {
            (format!("l{col_ix}"), left_names[col_ix].clone())
        } else {
            let i = col_ix - count;
            (format!("r{i}"), right_names[i].clone())
        };
        Column::new(key, name)
            .width(self.col_width)
            .min_width(px(64.))
            // The table pads each cell around the delegate's content,
            // which would leave an uncolored frame around a diff-tinted
            // cell. Zero the padding and let `render_td` pad inside its
            // background instead.
            .p_0()
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> gpui::Stateful<gpui::Div> {
        // Stable row identity: the aligned pair's model indices, not
        // the display position.
        let key = self
            .session
            .read(cx)
            .aligned()
            .get(row_ix)
            .map(|row| {
                format!(
                    "table-diff-{}-{}",
                    row.left.unwrap_or(usize::MAX),
                    row.right.unwrap_or(usize::MAX),
                )
            })
            .unwrap_or_else(|| format!("table-diff-empty-{row_ix}"));
        let is_selected = cx.entity().read(cx).selected_row() == Some(row_ix);
        // Only paint a background when selected; setting one
        // unconditionally would erase the table's stripe backgrounds.
        div()
            .id(ElementId::Name(key.into()))
            .when(is_selected, |this| this.bg(cx.theme().list_active))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        let count = self.logical_count(cx);
        if count == 0 {
            return div().into_any_element();
        }
        let is_left = col_ix < count;
        let i = if is_left { col_ix } else { col_ix - count };
        let Some((left, right)) = self.cell_texts(row_ix, i, cx) else {
            return div().into_any_element();
        };
        let shown = if is_left { left } else { right }.unwrap_or("");
        let overrides = color::merge_overrides(&ColorOverrides::default(), cx);
        let bg = cell_role(left, right).map(|role| {
            role.resolve(cx.theme(), &overrides)
                .opacity(CELL_BG_OPACITY)
        });
        div()
            .size_full()
            .flex()
            .items_center()
            .px(CELL_PAD)
            .when_some(bg, |this, bg| this.bg(bg))
            .child(shown.to_string())
            .into_any_element()
    }

    fn cell_label(
        &self,
        row_ix: usize,
        col_ix: usize,
        cx: &App,
    ) -> Option<SharedString> {
        let count = self.logical_count(cx);
        let is_left = col_ix < count;
        let i = if is_left {
            col_ix
        } else {
            col_ix.checked_sub(count)?
        };
        let (left, right) = self.cell_texts(row_ix, i, cx)?;
        Some((if is_left { left } else { right }?).into())
    }

    fn loading(&self, cx: &App) -> bool {
        self.session.read(cx).is_loading()
    }

    fn render_loading(
        &mut self,
        _: gpui::component::Size,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        h_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(cx.theme().muted_foreground)
            .child(Spinner::new().small())
            .child("Parsing files…")
            .into_any_element()
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .text_color(cx.theme().muted_foreground)
            .child("No rows")
            .child(div().text_xs().child("Both files have no data rows."))
            .into_any_element()
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// The table compare view.
pub struct TableDiffView {
    /// The compare session this view renders.
    state: Entity<TableDiffSession>,
    /// The comparison table's state, created on first render because
    /// [`TableState::new`] needs the window for its rem size.
    table_state: Option<Entity<TableState<TableDiffDelegate>>>,
    /// Re-render whenever the session notifies.
    _state_sub: Subscription,
}

impl TableDiffView {
    /// Create a view over an existing session; starts the initial load.
    pub fn new(
        state: Entity<TableDiffSession>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _state_sub = cx.observe(&state, |this, _, cx| {
            // The table caches its column layout; rebuild it whenever
            // the session's model changes (initial load, reload).
            if let Some(table) = this.table_state.as_ref() {
                table.update(cx, |table, cx| {
                    table.refresh(cx);
                    cx.notify();
                });
            }
            cx.notify();
        });
        state.update(cx, |session, cx| session.ensure_loaded(cx));
        Self {
            state,
            table_state: None,
            _state_sub,
        }
    }

    /// The session's focus handle, shared with the tab entity.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle()
    }

    /// The tab title.
    pub fn title(&self, cx: &App) -> SharedString {
        self.state.read(cx).title().clone()
    }

    /// The compared endpoints, for "Compare files using ▸".
    pub fn pair_endpoints(&self, cx: &App) -> (Endpoint, Endpoint) {
        self.state.read(cx).pair_endpoints()
    }

    /// Re-point the underlying session at a new owning manager.
    pub fn set_manager(
        &self,
        manager: Entity<GuiSessionManager>,
        cx: &mut App,
    ) {
        self.state
            .update(cx, |session, _| session.set_manager(manager));
    }

    /// Render the view's toolbar row (row 2) for the window root.
    pub fn render_view_toolbar(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = self.state.clone();
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .child(
                Button::new("table_reload")
                    .icon(gpui::assets::IconName::RefreshCw)
                    .label("Reload")
                    .ghost()
                    .small()
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |session, cx| session.load(cx));
                    }),
            )
            .into_any_element()
    }

    /// Create the comparison table's state on first use.
    fn ensure_table_state(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TableState<TableDiffDelegate>> {
        if let Some(table) = &self.table_state {
            return table.clone();
        }
        let session = self.state.clone();
        let table = cx.new(|cx| {
            TableState::new(
                TableDiffDelegate::new(session, window.rem_size()),
                window,
                cx,
            )
            .row_selectable(true)
            .col_selectable(false)
            .cell_selectable(false)
            .sortable(false)
            .col_movable(false)
            .loop_selection(false)
        });
        self.table_state = Some(table.clone());
        table
    }
}

impl Render for TableDiffView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let table_state = self.ensure_table_state(window, cx);
        let (theme_bg, theme_fg, muted, mono) = {
            let theme = cx.theme();
            (
                theme.background,
                theme.foreground,
                theme.muted_foreground,
                theme.mono_font_family.clone(),
            )
        };
        let session = self.state.read(cx);
        let focus_handle = session.focus_handle();

        // Loading and the empty grid are the table's own states (its
        // delegate reports them); only a load failure replaces the
        // table entirely.
        let body = if let Some(err) = session.error() {
            v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(muted)
                .child("The comparison could not be loaded.")
                .child(div().text_xs().child(err.clone()))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                // Table data reads best in the theme's monospace font;
                // the rest of the UI keeps the proportional font.
                .font_family(mono)
                .child(
                    DataTable::new(&table_state)
                        .with_size(ROW_HEIGHT)
                        .stripe(true)
                        .bordered(false)
                        .focus_ring(false),
                )
                .into_any_element()
        };

        let (total, different, left_only, right_only) =
            row_counts(session.aligned());
        let left_name = base_name(&session.left);
        let right_name = base_name(&session.right);
        div()
            .key_context("TableCompare")
            .track_focus(&focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme_bg)
            .text_color(theme_fg)
            .text_sm()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(left_name)
                    .child(right_name),
            )
            .child(body)
            .child(
                StatusBar::new().left(
                    h_flex()
                        .items_center()
                        .gap_3()
                        .child("Table Compare")
                        .child(format!("{total} rows"))
                        .child(format!("{different} different"))
                        .child(format!("{left_only} left-only"))
                        .child(format!("{right_only} right-only")),
                ),
            )
    }
}

/// File name of an endpoint, for the tab title.
fn base_name(endpoint: &Endpoint) -> String {
    endpoint
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| endpoint.display())
}

// ---------------------------------------------------------------------------
// Tab factory
// ---------------------------------------------------------------------------

/// Create a table compare tab view for the given file pair.
pub(crate) fn new_table_diff_view(
    left: Endpoint,
    right: Endpoint,
    manager: Option<Entity<GuiSessionManager>>,
    cx: &mut App,
) -> Entity<TableDiffView> {
    let title = format!("{} vs {}", base_name(&left), base_name(&right));
    let session = cx.new(|cx| {
        TableDiffSession::new(left, right, title.into(), manager, cx)
    });
    cx.new(|cx| TableDiffView::new(session, cx))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_fields() {
        let grid = parse_table("a,b\n\"x,1\",2\n", ',');
        assert_eq!(grid.header, vec!["a", "b"]);
        assert_eq!(grid.rows, vec![vec!["x,1".to_string(), "2".to_string()]]);
    }

    #[test]
    fn delimiter_follows_extension() {
        assert_eq!(delimiter_for("tsv"), '\t');
        assert_eq!(delimiter_for("csv"), ',');
        assert_eq!(delimiter_for(""), ',');
    }

    #[test]
    fn align_marks_same_and_one_sided_rows() {
        let left = vec![
            vec!["1".to_string()],
            vec!["2".to_string()],
            vec!["3".to_string()],
        ];
        let right = vec![vec!["1".to_string()], vec!["3".to_string()]];
        let rows = align_table_rows(&left, &right);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, TableRowStatus::Same);
        assert_eq!(rows[1].status, TableRowStatus::LeftOnly);
        assert_eq!(rows[2].status, TableRowStatus::Same);
    }

    #[test]
    fn replaced_rows_pair_as_different() {
        let left = vec![
            vec!["1".to_string()],
            vec!["2".to_string()],
            vec!["3".to_string()],
        ];
        let right = vec![
            vec!["1".to_string()],
            vec!["x".to_string()],
            vec!["3".to_string()],
        ];
        let rows = align_table_rows(&left, &right);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, TableRowStatus::Same);
        assert_eq!(rows[1].status, TableRowStatus::Different);
        assert_eq!(rows[1].left, Some(1));
        assert_eq!(rows[1].right, Some(1));
        assert_eq!(rows[2].status, TableRowStatus::Same);
    }

    #[test]
    fn column_names_left_wins_on_equal_length() {
        let left = vec!["id".to_string(), "name".to_string()];
        let right = vec!["key".to_string(), "value".to_string()];
        let (left_names, right_names) = table_column_names(&left, &right);
        assert_eq!(left_names, vec!["id", "name"]);
        assert_eq!(right_names, vec!["id", "name"]);
    }

    #[test]
    fn column_names_positional_when_lengths_differ() {
        let left = vec!["id".to_string()];
        let right = vec!["id".to_string(), "name".to_string(), "".to_string()];
        let (left_names, right_names) = table_column_names(&left, &right);
        assert_eq!(left_names, vec!["id", "left.2", "left.3"]);
        assert_eq!(right_names, vec!["id", "name", "right.3"]);
    }

    #[test]
    fn trailing_left_only_run_terminates_and_stays_one_sided() {
        // A left-only run with no right counterpart used to restart the
        // pairing scan at its head and spin forever.
        let left = vec![
            vec!["1".to_string()],
            vec!["2".to_string()],
            vec!["3".to_string()],
        ];
        let right = vec![vec!["1".to_string()]];
        let rows = align_table_rows(&left, &right);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, TableRowStatus::Same);
        assert_eq!(rows[1].status, TableRowStatus::LeftOnly);
        assert_eq!(rows[2].status, TableRowStatus::LeftOnly);
    }

    #[test]
    fn column_names_fall_back_on_empty_headers() {
        // An empty left entry falls back to the right name on equal
        // length.
        let left = vec!["".to_string()];
        let right = vec!["key".to_string()];
        let (left_names, right_names) = table_column_names(&left, &right);
        assert_eq!(left_names, vec!["key"]);
        assert_eq!(right_names, vec!["key"]);
        // No headers at all means no columns.
        let (left_names, right_names) = table_column_names(&[], &[]);
        assert!(left_names.is_empty());
        assert!(right_names.is_empty());
    }

    #[test]
    fn cell_role_marks_differing_and_orphan_cells() {
        assert_eq!(cell_role(Some("a"), Some("a")), None);
        assert_eq!(
            cell_role(Some("a"), Some("b")),
            Some(ColorRole::Important)
        );
        assert_eq!(cell_role(Some("a"), None), Some(ColorRole::Orphan));
        assert_eq!(cell_role(None, Some("b")), Some(ColorRole::Orphan));
        assert_eq!(cell_role(None, None), None);
        // A paired row whose side lacks the column reads as an empty
        // cell, so it differs from a non-empty counterpart.
        assert_eq!(cell_role(Some(""), Some("x")), Some(ColorRole::Important));
        assert_eq!(cell_role(Some(""), Some("")), None);
    }

    #[test]
    fn row_counts_split_by_status() {
        let rows = vec![
            AlignedTableRow {
                left: Some(0),
                right: Some(0),
                status: TableRowStatus::Same,
            },
            AlignedTableRow {
                left: Some(1),
                right: Some(1),
                status: TableRowStatus::Different,
            },
            AlignedTableRow {
                left: Some(2),
                right: None,
                status: TableRowStatus::LeftOnly,
            },
            AlignedTableRow {
                left: None,
                right: Some(2),
                status: TableRowStatus::RightOnly,
            },
        ];
        assert_eq!(row_counts(&rows), (4, 1, 1, 1));
    }
}
