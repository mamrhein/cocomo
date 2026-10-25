// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared table scaffolding: the session-selection mirror and the themed
//! `Table` wrapper used by the directory comparison view.
//!
//! The wrapper also windows the rows: ratatui-kit's `Table` has no scroll
//! offset (it renders from the top of its area and clips at the bottom),
//! so the component slices out the viewport itself and keeps the selection
//! inside it.

use ratatui_kit::{
    prelude::*,
    ratatui::{
        layout::{Alignment, Constraint},
        style::Styled,
        symbols,
        text::{Line, Span},
        widgets::{Block, Borders},
    },
};

use super::{rows::CellRow, theme::CocomoTheme};

/// Mirror the session-owned table selection into a component-local
/// `State<TableState>` for the `Table` component.
///
/// The session (`SessionView::table_state`) is the single owner of the
/// selection: key handling and async completions mutate it under the
/// `APP_STATE` write lock. The `Table` component, however, only accepts a
/// reactive `State` handle, so each view mirrors the owner's value into
/// this handle on every render — the session value always wins, and the
/// handle exists purely to feed the widget.
///
/// `write_no_update` is deliberate: the atom write that changed the
/// selection already woke the subscribed component, so the mirror must
/// not trigger a second redraw.
fn use_session_table_state(
    hooks: &mut Hooks<'_, '_>,
    selected: Option<usize>,
) -> State<TableState> {
    let table_state = hooks.use_state(TableState::default);
    if table_state.read().selected() != selected {
        table_state.write_no_update().select(selected);
    }
    table_state
}

/// Rows the table spends above the first body row: the block's top and
/// bottom border plus the single header row.
///
/// The header separator renders zero rows under [`TableBorderMode::None`],
/// so it costs nothing here.
const CHROME_ROWS: u16 = 3;

/// Compute the first visible row of the table window.
///
/// The window slides just enough to keep `selected` inside the
/// `body_height` visible rows and keeps the previous offset whenever the
/// selection is already visible. A `body_height` of `0` (the area has not
/// been measured yet) or a list that fits entirely renders all rows from
/// the top.
fn window_offset(
    total: usize,
    body_height: usize,
    prev_offset: usize,
    selected: Option<usize>,
) -> usize {
    if body_height == 0 || total <= body_height {
        return 0;
    }

    let mut offset = prev_offset;
    if let Some(selected) = selected {
        if selected < offset {
            offset = selected;
        } else if selected >= offset + body_height {
            offset = selected + 1 - body_height;
        }
    }
    offset.min(total - body_height)
}

/// Props for [`SessionTable`].
#[derive(Default, Props)]
pub(super) struct SessionTableProps {
    /// Session-owned selection mirrored into the table handle.
    pub(super) selected: Option<usize>,
    /// Panel title rendered in the table block's top border.
    pub(super) title: String,
    /// Column definitions.
    pub(super) columns: Vec<TableColumn>,
    /// Pre-rendered rows to display (memoized by the owning view).
    pub(super) rows: Vec<CellRow>,
    /// Per-row cell renderer.
    pub(super) render_row: Option<RenderTableRow<CellRow>>,
    /// Message rendered instead of the table when there are no rows.
    pub(super) empty_message: Line<'static>,
}

/// Themed table shared by the comparison views (F16).
///
/// Owns the selection mirroring ([`use_session_table_state`]), the row
/// windowing ([`window_offset`]), the bordered block with themed title,
/// and the default table styling, so the views only supply columns, rows,
/// and the renderer.  The session owns the *absolute* selection; the
/// `Table` receives only the visible slice with a selection relative to
/// that slice.  When there are no rows, the `empty_message` is rendered
/// centered in the framed panel instead of an empty table.
#[component]
pub(super) fn SessionTable(
    props: &SessionTableProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // The window math needs the body height *before* the `Table` renders,
    // and the previous frame's draw area is the only measurement that
    // accounts for the surrounding layout (menu bar, tab bar, status bar,
    // help bar).
    let area = hooks.use_previous_size();
    let mut scroll_offset = hooks.use_state(|| 0usize);
    // `use_previous_size` lags one frame behind a resize; this tick re-runs
    // the update once after the terminal size changes so the window
    // converges to the new area without waiting for the next key press.
    let (term_width, term_height) = hooks.use_terminal_size();
    let mut resize_tick = hooks.use_state(|| 0u32);
    hooks.use_effect(
        move || {
            resize_tick.set(resize_tick.get() + 1);
        },
        (term_width, term_height),
    );

    // Slice the rows to the viewport and make the selection relative to
    // the slice; `TableWrapMode::Truncate` below guarantees one row is
    // exactly one line, which keeps this math exact.
    let total = props.rows.len();
    let body_height = usize::from(area.height.saturating_sub(CHROME_ROWS));
    let offset =
        window_offset(total, body_height, scroll_offset.get(), props.selected);
    if scroll_offset.get() != offset {
        scroll_offset.set_no_update(offset);
    }
    let visible = if body_height == 0 {
        total
    } else {
        body_height.min(total)
    };
    let rows = props.rows[offset..offset + visible].to_vec();
    let selected = props.selected.map(|index| index.saturating_sub(offset));
    let table_state = use_session_table_state(&mut hooks, selected);

    let title = Line::from(format!(" {} ", props.title));
    if props.rows.is_empty() {
        return element!(
            Border(
                height: Constraint::Fill(1),
                border_style: theme.border,
                top_title: title.set_style(theme.title).centered(),
            ) {
                Text(
                    text: props.empty_message.clone(),
                    alignment: Alignment::Center,
                )
            }
        )
        .into_any();
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(format!(" {} ", props.title), theme.title))
        .border_style(theme.border);

    element!(
        Table<CellRow>(
            height: Constraint::Fill(1),
            state: Some(table_state),
            active: false,
            default_index: Some(0),
            columns: props.columns.clone(),
            rows,
            render_row: props.render_row.clone(),
            block: Some(block),
            // The block already draws the outer border; the table must not
            // draw its own. With `Outer`, ratatui-kit 0.12 reserves (n-1)
            // cells for inter-column separators in the border lines but
            // writes none in the content rows, so the rows' right border
            // ends short of the panel's right edge. `None` keeps the
            // layout math and the rendered rows consistent, filling the
            // block's inner width exactly.
            border_mode: TableBorderMode::None,
            // One row, one line: long names are truncated instead of
            // wrapped, which is what keeps the windowing math above
            // (one row per viewport line) exact.
            wrap_mode: TableWrapMode::Truncate,
            highlight_style: theme.highlight,
            highlight_symbol: Some(symbols::block::FULL),
            // No reserved leading gutter: the reversed `highlight_style`
            // already marks the selected row, so the first column can sit
            // flush against the border.
            highlight_spacing: HighlightSpacing::Never,
            // Zero cell padding: columns are separated only by
            // `column_spacing` (one char) and the outer columns hug the
            // block border.
            cell_padding: 0u16,
            column_spacing: 1u16,
            header_style: theme.title,
        )
    )
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_windowing_before_the_area_is_measured() {
        // A `body_height` of 0 means the draw area is unknown; render
        // everything from the top and let the table clip.
        assert_eq!(window_offset(100, 0, 0, Some(50)), 0);
    }

    #[test]
    fn lists_that_fit_render_from_the_top() {
        assert_eq!(window_offset(3, 10, 4, Some(2)), 0);
        assert_eq!(window_offset(10, 10, 0, Some(9)), 0);
    }

    #[test]
    fn selection_scrolls_down_minimally() {
        // Rows 0..=9 visible; selecting row 10 slides the window by one.
        assert_eq!(window_offset(100, 10, 0, Some(10)), 1);
        // A jump (G) slides just enough to show the last row.
        assert_eq!(window_offset(100, 10, 0, Some(99)), 90);
    }

    #[test]
    fn selection_inside_the_window_keeps_the_offset() {
        assert_eq!(window_offset(100, 10, 5, Some(5)), 5);
        assert_eq!(window_offset(100, 10, 5, Some(8)), 5);
        assert_eq!(window_offset(100, 10, 5, Some(14)), 5);
    }

    #[test]
    fn selection_scrolls_up_minimally() {
        assert_eq!(window_offset(100, 10, 5, Some(4)), 4);
        assert_eq!(window_offset(100, 10, 5, Some(0)), 0);
    }

    #[test]
    fn stale_offset_shrinks_with_the_list() {
        // After filtering, the previous offset may exceed the last valid
        // window; the result must keep the window inside the rows.
        assert_eq!(window_offset(12, 10, 50, Some(11)), 2);
    }

    #[test]
    fn no_selection_keeps_the_offset() {
        assert_eq!(window_offset(100, 10, 7, None), 7);
    }
}
