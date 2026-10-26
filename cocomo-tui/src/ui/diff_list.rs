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
//! native [`DiffLine`] widget that draws one row's two halves separated by
//! a one-character status gutter (`<`, `=`, `>`) at the split.
//!
//! The gutter arrow points at the side that is behind: `<` when the right
//! side has new or changed content, `>` when the left side has content the
//! right side lacks, and `=` for identical lines (requirements §11.3).

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

use super::{
    rows::{DiffGutter, TextDiffRow},
    theme::CocomoTheme,
};

/// One rendered diff row: two styled half-lines and a one-character status
/// gutter at the split, sharing a row style.
///
/// Implemented as a native Ratatui widget (the `VirtualList` item type):
/// the area is split at the midpoint, one column at the split carries the
/// gutter symbol, and each remaining half renders one line.
#[derive(Clone)]
struct DiffLine {
    left: Line<'static>,
    right: Line<'static>,
    /// Gutter symbol rendered in the column at the split.
    gutter: &'static str,
    /// Style for the gutter symbol.
    gutter_style: Style,
    style: Style,
}

impl Widget for DiffLine {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Widget::render(&self, area, buf);
    }
}

impl Widget for &DiffLine {
    fn render(self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, self.style);
        if area.width == 0 {
            return;
        }
        let half = area.width / 2;
        let left_area = Rect {
            width: half,
            ..area
        };
        self.left.clone().render(left_area, buf);
        // One column at the split carries the status gutter; the right
        // half starts after it.
        let gutter_x = area.x + half;
        Span::styled(self.gutter, self.gutter_style).render(
            Rect {
                x: gutter_x,
                width: 1,
                ..area
            },
            buf,
        );
        let right_x = gutter_x + 1;
        let right_width = (area.x + area.width).saturating_sub(right_x);
        if right_width > 0 {
            let right_area = Rect {
                x: right_x,
                width: right_width,
                ..area
            };
            self.right.clone().render(right_area, buf);
        }
    }
}

/// Style for a row's gutter symbol: dim for identical lines, and the
/// color of the newer side's diff content otherwise (green `diff_added`
/// for `<`, red `diff_removed` for `>`).
fn gutter_style(theme: &CocomoTheme, gutter: DiffGutter) -> Style {
    match gutter {
        DiffGutter::Equal => theme.dim,
        DiffGutter::RightNewer => theme.diff_added,
        DiffGutter::LeftNewer => theme.diff_removed,
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
    // The header row has no status of its own; a dim bar keeps the gutter
    // column visually continuous with the rows below.
    let header = DiffLine {
        left: Line::from(Span::styled(
            props.left_name.clone(),
            theme.left_name,
        )),
        right: Line::from(Span::styled(
            props.right_name.clone(),
            theme.right_name,
        )),
        gutter: "\u{2502}",
        gutter_style: theme.dim,
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
                            gutter: row.gutter.symbol(),
                            gutter_style: gutter_style(&theme, row.gutter),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_line(gutter: &'static str) -> DiffLine {
        DiffLine {
            left: Line::from("L"),
            right: Line::from("R"),
            gutter,
            gutter_style: Style::new(),
            style: Style::new(),
        }
    }

    #[test]
    fn diff_line_renders_the_gutter_at_the_split() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 1));
        test_line("<").render(buf.area, &mut buf);
        // Width 10 splits at column 5: the gutter occupies it, the right
        // half starts one column later.
        assert_eq!(buf[(0, 0)].symbol(), "L");
        assert_eq!(buf[(5, 0)].symbol(), "<");
        assert_eq!(buf[(6, 0)].symbol(), "R");
    }

    #[test]
    fn diff_line_survives_a_too_narrow_area() {
        // Width 2 fits the left half and the gutter but not the right
        // half; rendering must not panic.
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        test_line("=").render(buf.area, &mut buf);
        assert_eq!(buf[(0, 0)].symbol(), "L");
        assert_eq!(buf[(1, 0)].symbol(), "=");
    }

    #[test]
    fn gutter_style_distinguishes_the_statuses() {
        let theme = CocomoTheme::default();
        assert_eq!(gutter_style(&theme, DiffGutter::Equal), theme.dim);
        assert_eq!(
            gutter_style(&theme, DiffGutter::RightNewer),
            theme.diff_added
        );
        assert_eq!(
            gutter_style(&theme, DiffGutter::LeftNewer),
            theme.diff_removed
        );
    }
}
