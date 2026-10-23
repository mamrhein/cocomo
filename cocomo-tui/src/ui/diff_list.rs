// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Windowed side-by-side diff list for the text comparison view.
//!
//! A text diff can have tens of thousands of rows; rendering them all with
//! a `Table` would rebuild every row's cells on every frame.  Instead the
//! rows are memoized view models ([`TextDiffRow`]) shared via `Arc`, and
//! only the visible window is rendered, through `VirtualList` and the small
//! native [`DiffLine`] widget that draws one row's two halves.

use std::sync::Arc;

use ratatui_kit::{
    prelude::*,
    ratatui::{
        buffer::Buffer,
        layout::{Alignment, Constraint, Direction, Rect},
        style::{Style, Styled},
        text::{Line, Span},
        widgets::Widget,
    },
};

use super::{rows::TextDiffRow, theme::CocomoTheme};

/// One rendered diff row: two styled half-lines sharing a row style.
///
/// Implemented as a native Ratatui widget (the `VirtualList` item type):
/// the area is split at the midpoint and each half renders one line.
#[derive(Clone)]
struct DiffLine {
    left: Line<'static>,
    right: Line<'static>,
    style: Style,
}

impl Widget for DiffLine {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Widget::render(&self, area, buf);
    }
}

impl Widget for &DiffLine {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let half = area.width / 2;
        let left_area = Rect {
            width: half,
            ..area
        };
        let right_area = Rect {
            x: area.x + half,
            width: area.width - half,
            ..area
        };
        buf.set_style(left_area, self.style);
        buf.set_style(right_area, self.style);
        self.left.clone().render(left_area, buf);
        self.right.clone().render(right_area, buf);
    }
}

/// Props for [`SessionDiffList`].
#[derive(Default, Props)]
pub(super) struct SessionDiffListProps {
    /// Session-owned selection mirrored into the list handle.
    pub(super) selected: Option<usize>,
    /// Panel title rendered centered in the top border.
    pub(super) title: &'static str,
    /// Message rendered when the diff has no rows.
    pub(super) empty_message: &'static str,
    /// Memoized diff rows, shared via `Arc` so per-frame cloning is O(1).
    pub(super) rows: Arc<Vec<TextDiffRow>>,
    /// Left file's name, shown above the left half.
    pub(super) left_name: String,
    /// Right file's name, shown above the right half.
    pub(super) right_name: String,
}

/// Side-by-side diff list with a themed border, file-name header, and a
/// windowed body (F9/F16).
///
/// Mirrors the session-owned selection into a `ListState` handle exactly
/// like the table path mirrors it into a `TableState`: the session is the
/// single owner and the handle only feeds the widget.  The `VirtualList`
/// keeps the selection in view and renders only the visible window.
#[component]
pub(super) fn SessionDiffList(
    props: &SessionDiffListProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let list_state = hooks.use_state(tui_widget_list::ListState::default);
    if list_state.read().selected != props.selected {
        list_state.write_no_update().select(props.selected);
    }

    let title = Line::from(format!(" {} ", props.title));
    if props.rows.is_empty() {
        return element!(
            Border(
                height: Constraint::Fill(1),
                border_style: theme.border,
                top_title: title.set_style(theme.title).centered(),
            ) {
                Text(
                    text: Line::from(props.empty_message),
                    alignment: Alignment::Center,
                )
            }
        );
    }

    let rows = props.rows.clone();
    let highlight = theme.highlight;
    let header = DiffLine {
        left: Line::from(Span::styled(
            props.left_name.clone(),
            theme.left_name,
        )),
        right: Line::from(Span::styled(
            props.right_name.clone(),
            theme.right_name,
        )),
        style: Style::default(),
    };

    element!(
        Border(
            height: Constraint::Fill(1),
            flex_direction: Direction::Vertical,
            border_style: theme.border,
            top_title: title.set_style(theme.title).centered(),
        ) {
            View(height: Constraint::Length(1)) {
                widget(header)
            }
            VirtualList<DiffLine>(
                height: Constraint::Fill(1),
                state: Some(list_state),
                item_count: rows.len(),
                active: false,
                default_index: Some(0),
                render_item: RenderVirtualItem::from(
                    move |ctx: &VirtualItemContext| {
                        let row = &rows[ctx.index];
                        let line = DiffLine {
                            left: row.left.clone(),
                            right: row.right.clone(),
                            style: if ctx.is_selected {
                                highlight
                            } else {
                                Style::default()
                            },
                        };
                        (line, 1u16)
                    },
                ),
            )
        }
    )
}
