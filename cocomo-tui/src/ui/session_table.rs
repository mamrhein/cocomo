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

/// Props for [`SessionTable`].
#[derive(Default, Props)]
pub(super) struct SessionTableProps {
    /// Session-owned selection mirrored into the table handle.
    pub(super) selected: Option<usize>,
    /// Panel title rendered in the table block's top border.
    pub(super) title: &'static str,
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
/// Owns the selection mirroring ([`use_session_table_state`]), the
/// bordered block with themed title, and the default table styling, so the
/// views only supply columns, rows, and the renderer.  When there are no
/// rows, the `empty_message` is rendered centered in the framed panel
/// instead of an empty table.
#[component]
pub(super) fn SessionTable(
    props: &SessionTableProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let table_state = use_session_table_state(&mut hooks, props.selected);

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
            rows: props.rows.clone(),
            render_row: props.render_row.clone(),
            block: Some(block),
            highlight_style: theme.highlight,
            highlight_symbol: Some(symbols::block::FULL),
            column_spacing: 1u16,
            header_style: theme.title,
        )
    )
    .into_any()
}
