// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
// details please read the file LICENSE.TXT provided together
// with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Single comparison table for the folder compare view.
//!
//! Replaces the former two mirrored panes with one [`DataTable`] whose
//! columns read left-to-right as the left side's `Name`, `Size`,
//! `Modified`, an unlabeled status symbol, then the right side's `Name`,
//! `Size`, `Modified`. Above the table a base-dir bar shows the left
//! path on the leading edge and the right path on the trailing edge.
//! Rows alternate backgrounds (the table's `stripe`, themed by
//! `table_even`), and nested entries are indented with tree guides and
//! `├─`/`└─` indicators — the GUI equivalent of the CLI's prefixes.
//!
//! [`AppState`] owns rows, expansion, selection, sort, and filters; the
//! [`FolderTableDelegate`] is a thin view over it and stores no data of
//! its own.

use cocomo_lib::{DirEntry, DirEntryStatus, Endpoint};
use gpui::{
    AnyElement, App, ElementId, Entity, ExternalPaths, FontWeight, Hsla,
    Pixels, SharedString, TestSupportExt as _, Window,
    base::ElementExt as _,
    component::{
        ActiveTheme, FocusableExt as _, Sizable as _, Theme,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
        spinner::Spinner,
        table::{Column, DataTable, TableDelegate, TableState},
        v_flex,
    },
    div,
    prelude::*,
    px, rems,
};

use crate::{
    color::{self, Side},
    log::{self, LogLevel},
    state::{AppState, SortKey, TreeRow, entry_endpoints},
    ui::{
        CompareContentsHash, CompareContentsMeta, CopyFilename, CopyToFolder,
        CopyToLeft, CopyToOtherSide, CopyToRight, DeleteLeftSide,
        DeleteRightSide, ExchangeEntry, ExcludeEntry, MoveToFolder,
        MoveToLeft, MoveToRight, NewFolder, OpenDirDiff, OpenFileDiff,
        OpenMerge, QuickCompare, RefreshSelection, RenameEntry,
        RevealInExplorer, SetBaseBoth, SetBaseLeft, SetBaseRight, SetSidePath,
        ShowAttributes, SyncMirrorLeft, SyncMirrorRight, SyncUpdateBoth,
        SyncUpdateLeft, SyncUpdateRight, ToggleIgnored, TouchEntry,
    },
};

/// Indentation per tree depth, in rems.
const INDENT_REMS: f32 = 0.875;

/// Column keys, matched in [`FolderTableDelegate::render_td`].
const COL_NAME_L: &str = "name_l";
const COL_SIZE_L: &str = "size_l";
const COL_DATE_L: &str = "date_l";
const COL_STATUS: &str = "status";
const COL_NAME_R: &str = "name_r";
const COL_SIZE_R: &str = "size_r";
const COL_DATE_R: &str = "date_r";

/// Index of the right side's Name column: the stretchable filler column
/// that makes the row content fill the table's width.
const RIGHT_NAME_IX: usize = 4;

/// Width of the table's trailing empty column, mirrored by
/// [`FolderTableDelegate::render_last_empty_col`] as its minimum.
const LAST_EMPTY_COL: Pixels = px(12.);

/// Comparison table row height, in pixels. Denser than the component
/// library's `xsmall` (26 px) to fit more entries on screen.
const ROW_HEIGHT: Pixels = px(24.);

/// Render the folder comparison: the base-dir bar above the striped
/// comparison table.
///
/// `left_path` and `right_path` are the editable path fields; pressing
/// Enter in one reloads that side of the compared pair. Dropping a
/// folder onto the leading (trailing) half of the table repoints the
/// left (right) side.
pub(crate) fn render_folder_table(
    state: &Entity<AppState>,
    table_state: &Entity<TableState<FolderTableDelegate>>,
    left_path: &Entity<InputState>,
    right_path: &Entity<InputState>,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    // The base-dir bar's left half spans exactly the table's left group
    // (Name, Size, Modified, status lane) so its right half lines up
    // with the table's right group.
    let left_half = table_state.read(cx).delegate().left_half_width();
    let fill_table = table_state.clone();

    v_flex()
        .size_full()
        .min_h_0()
        .bg(theme.background)
        .child(render_base_dir_bar(
            state, left_path, right_path, left_half, &theme, cx,
        ))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .relative()
                // Stretch the right side's Name column to the available
                // width. The hook runs every frame; `set_fill_width`
                // only reports a change when the width actually moved,
                // so a steady table is not repainted.
                .on_prepaint(move |bounds, _, cx| {
                    let avail = bounds.size.width;
                    fill_table.update(cx, |table, cx| {
                        if table.delegate_mut().set_fill_width(avail) {
                            table.refresh(cx);
                            cx.notify();
                        }
                    });
                })
                .child(
                    DataTable::new(table_state)
                        .with_size(ROW_HEIGHT)
                        .stripe(true)
                        .bordered(false)
                        .focus_ring(false),
                )
                .children(drop_zones(state, &theme)),
        )
        .into_any_element()
}

/// The base-dir bar above the table: the left path on the leading edge,
/// the right path on the trailing edge, each editable and with its own
/// history dropdown and new-folder command.
///
/// `left_width` is the table's left-group width; the left half of the
/// bar takes it verbatim so the right half starts where the table's
/// right side's Name column starts.
fn render_base_dir_bar(
    state: &Entity<AppState>,
    left_path: &Entity<InputState>,
    right_path: &Entity<InputState>,
    left_width: Pixels,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    h_flex()
        .items_center()
        .bg(theme.muted)
        .border_b_1()
        .border_color(theme.border)
        .child(path_cell(
            Side::Left,
            state,
            left_path,
            Some(left_width),
            cx,
        ))
        .child(div().h_full().w_px().bg(theme.border).flex_shrink_0())
        .child(path_cell(Side::Right, state, right_path, None, cx))
        .into_any_element()
}

/// One editable side of the base-dir bar.
///
/// `width` fixes the cell's width (used for the left half, to align it
/// with the table columns); `None` lets the cell fill the remaining
/// space.
fn path_cell(
    side: Side,
    state: &Entity<AppState>,
    path_input: &Entity<InputState>,
    width: Option<Pixels>,
    cx: &mut App,
) -> AnyElement {
    let side_name = side_name(side);
    let history: Vec<String> = state.read(cx).side_path_history(side);

    let history_btn = Button::new((
        ElementId::from("path-history"),
        SharedString::from(side_name),
    ))
    .icon(gpui::assets::IconName::ChevronDown)
    .ghost()
    .xsmall()
    .tooltip("Path history")
    .accessibility_label("Path history")
    .dropdown_menu(move |menu, _, _| {
        let mut menu = menu;
        for path in &history {
            let action = Box::new(SetSidePath {
                side,
                path: path.clone(),
            });
            menu = menu.menu(path.clone(), action);
        }
        if history.is_empty() {
            menu = menu.menu("(no history)", Box::new(Noop));
        }
        menu
    });

    let new_folder_btn = Button::new((
        ElementId::from("new-folder"),
        SharedString::from(side_name),
    ))
    .icon(gpui::assets::IconName::Folder)
    .ghost()
    .xsmall()
    .tooltip("New folder")
    .accessibility_label("New folder")
    .on_click({
        let state = state.clone();
        move |_, _, cx| {
            state.update(cx, |state, cx| state.new_folder(side, cx));
        }
    });

    h_flex()
        .when_some(width, |el, w| el.w(w).flex_shrink_0())
        .when(width.is_none(), |el| el.flex_1())
        .min_w_0()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(Input::new(path_input).small()),
        )
        .child(history_btn)
        .child(new_folder_btn)
        .into_any_element()
}

/// The two invisible halves that receive folder drops over the table.
///
/// They carry no click handlers, so row clicks, hover, and scrolling all
/// pass through to the table underneath; only the drop listener on each
/// half claims a dropped folder for its side.
fn drop_zones(state: &Entity<AppState>, theme: &Theme) -> [AnyElement; 2] {
    [Side::Left, Side::Right].map(|side| {
        let drop_state = state.clone();
        let primary = theme.primary;
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .w_1_2()
            .when(side == Side::Right, |this| this.right_0())
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.bg(primary.opacity(0.1))
            })
            .on_drop::<ExternalPaths>(move |paths, _, cx| {
                accept_side_drop(side, &drop_state, paths, cx);
            })
            .into_any_element()
    })
}

/// Point `side` at the first dropped path, when it is a directory.
///
/// A drop that cannot be honored says why in the log and as a
/// notification; silently ignoring a dropped item reads as a broken drop
/// target.
fn accept_side_drop(
    side: Side,
    state: &Entity<AppState>,
    paths: &ExternalPaths,
    cx: &mut App,
) {
    let Some(path) = paths.paths().first() else {
        return;
    };
    match fs_err::metadata(path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => {
            let message = format!("{} is not a directory.", path.display());
            log::push(LogLevel::Error, message.clone(), cx);
            crate::menus::notify_error(message, cx);
            return;
        }
        Err(e) => {
            let message = format!("Couldn't read {}: {e}", path.display());
            log::push(LogLevel::Error, message.clone(), cx);
            crate::menus::notify_error(message, cx);
            return;
        }
    }
    state.update(cx, |state, cx| {
        state.set_side_path(side, &path.to_string_lossy(), cx);
    });
}

/// Stable id part distinguishing a side's controls.
fn side_name(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
    }
}

// ---------------------------------------------------------------------------
// Table delegate
// ---------------------------------------------------------------------------

/// Thin [`TableDelegate`] view over [`AppState`].
///
/// The delegate stores only the column definitions; every cell reads the
/// state lazily, so a [`TableState::refresh`] after a state change is
/// enough to repaint the table.
pub(crate) struct FolderTableDelegate {
    state: Entity<AppState>,
    columns: Vec<Column>,
}

impl FolderTableDelegate {
    pub(crate) fn new(state: Entity<AppState>, rem: gpui::Pixels) -> Self {
        let rem = |r: f32| rems(r).to_pixels(rem);
        let name = |key: &str| {
            Column::new(key, "Name")
                .width(rem(16.))
                .min_width(rem(8.))
                .max_width(rem(40.))
        };
        let size =
            |key: &str| Column::new(key, "Size").width(rem(4.5)).text_right();
        let date = |key: &str| {
            Column::new(key, "Modified").width(rem(7.5)).text_right()
        };
        Self {
            state,
            columns: vec![
                name(COL_NAME_L),
                size(COL_SIZE_L),
                date(COL_DATE_L),
                // The unlabeled status lane: a single glyph, centered.
                Column::new(COL_STATUS, "")
                    .width(rem(1.75))
                    .text_center()
                    .resizable(false)
                    .movable(false),
                name(COL_NAME_R),
                size(COL_SIZE_R),
                date(COL_DATE_R),
            ],
        }
    }

    /// The row data for `row_ix`, or `None` when the index no longer
    /// resolves (rows can shrink between a state change and the repaint).
    fn row_at(&self, row_ix: usize, cx: &App) -> Option<TableRow> {
        let overrides = color::merge_overrides(
            &self.state.read(cx).settings().color_overrides,
            cx,
        );
        let state = self.state.read(cx);
        let row = state.row_at(row_ix)?;
        let entry = state.entry_at(row_ix)?;
        Some(TableRow::new(row_ix, row, entry, state, &overrides))
    }

    /// Width of the table's left half: the left side's `Name`, `Size`,
    /// and `Modified` columns plus the status lane. The base-dir bar
    /// uses it to align its right half with the table's right group.
    pub(crate) fn left_half_width(&self) -> Pixels {
        self.columns[..RIGHT_NAME_IX]
            .iter()
            .fold(px(0.), |sum, col| sum + col.width)
    }

    /// Stretch the right side's Name column so the row content fills
    /// `avail`, leaving [`LAST_EMPTY_COL`] as trailing padding.
    ///
    /// Returns `true` only when the width actually changed, so the
    /// per-frame measurement hook does not repaint a steady table.
    fn set_fill_width(&mut self, avail: Pixels) -> bool {
        let fixed =
            self.columns
                .iter()
                .enumerate()
                .fold(px(0.), |sum, (ix, col)| {
                    if ix == RIGHT_NAME_IX {
                        sum
                    } else {
                        sum + col.width
                    }
                });
        let col = &mut self.columns[RIGHT_NAME_IX];
        let want = (avail - fixed - LAST_EMPTY_COL).max(col.min_width);
        if col.width != want {
            col.width = want;
            true
        } else {
            false
        }
    }
}

impl TableDelegate for FolderTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, cx: &App) -> usize {
        self.state.read(cx).entry_count()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_last_empty_col(
        &mut self,
        _: &mut Window,
        _: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Trailing filler: absorbs any rounding left by the stretched
        // right Name column so row content always reaches the table's
        // edge, and shrinks away when the columns overflow.
        h_flex().flex_1().min_w(LAST_EMPTY_COL).h_full()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        let key = self.columns[col_ix].key.as_ref();
        let state = self.state.clone();
        match key {
            COL_NAME_L | COL_NAME_R => {
                let side = if key == COL_NAME_R { "Right" } else { "Left" };
                sort_header(&state, "Name", side, SortKey::Name, false, cx)
            }
            COL_SIZE_L | COL_SIZE_R => {
                let (side, sort_key) = if key == COL_SIZE_R {
                    ("Right", SortKey::SizeRight)
                } else {
                    ("Left", SortKey::SizeLeft)
                };
                sort_header(&state, "Size", side, sort_key, true, cx)
            }
            COL_DATE_L | COL_DATE_R => {
                let (side, sort_key) = if key == COL_DATE_R {
                    ("Right", SortKey::DateRight)
                } else {
                    ("Left", SortKey::DateLeft)
                };
                sort_header(&state, "Modified", side, sort_key, true, cx)
            }
            _ => div().into_any_element(),
        }
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> gpui::Stateful<gpui::Div> {
        let path = self
            .state
            .read(cx)
            .row_at(row_ix)
            .map(|row| row.path().to_string())
            .unwrap_or_default();
        let is_selected = self.state.read(cx).selected_index() == row_ix;
        // Only paint a background when selected. The table applies this
        // style *after* its own stripe background, so setting a
        // background unconditionally would erase the zebra stripes.
        div()
            .id(ElementId::from(path))
            .when(is_selected, |this| this.bg(cx.theme().list_active))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(data) = self.row_at(row_ix, cx) else {
            return div().into_any_element();
        };
        let theme = cx.theme();
        let key = self.columns[col_ix].key.as_ref();
        match key {
            COL_NAME_L | COL_NAME_R => {
                let side = if key == COL_NAME_R {
                    Side::Right
                } else {
                    Side::Left
                };
                render_name_cell(&data, side, theme, window.rem_size())
                    .into_any_element()
            }
            COL_SIZE_L | COL_SIZE_R => {
                let side = if key == COL_SIZE_R {
                    Side::Right
                } else {
                    Side::Left
                };
                let size = if side == Side::Right {
                    data.right_size
                } else {
                    data.left_size
                };
                let role = side_role(&data, side);
                div()
                    .size_full()
                    .text_color(role.resolve(theme, &data.overrides))
                    .child(size.map(format_size).unwrap_or_default())
                    .into_any_element()
            }
            COL_DATE_L | COL_DATE_R => {
                let side = if key == COL_DATE_R {
                    Side::Right
                } else {
                    Side::Left
                };
                let modified = if side == Side::Right {
                    data.right_modified.as_deref()
                } else {
                    data.left_modified.as_deref()
                };
                let role = side_role(&data, side);
                div()
                    .size_full()
                    .text_color(role.resolve(theme, &data.overrides))
                    .child(
                        modified
                            .map(crate::ui::format_modified)
                            .unwrap_or_default(),
                    )
                    .into_any_element()
            }
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .font_weight(FontWeight(700.))
                .text_color(theme.muted_foreground)
                .when(!data.status_symbol.is_empty(), |this| {
                    this.child(data.status_symbol)
                })
                .into_any_element(),
        }
    }

    fn cell_label(
        &self,
        row_ix: usize,
        col_ix: usize,
        cx: &App,
    ) -> Option<SharedString> {
        let data = self.row_at(row_ix, cx)?;
        let key = self.columns[col_ix].key.as_ref();
        match key {
            COL_NAME_L => Some(data.name.clone()),
            COL_NAME_R => Some(if data.has_right {
                data.name.clone()
            } else {
                SharedString::default()
            }),
            COL_SIZE_L => data.left_size.map(|s| format_size(s).into()),
            COL_SIZE_R => data.right_size.map(|s| format_size(s).into()),
            COL_DATE_L => data
                .left_modified
                .as_deref()
                .map(|m| format_modified_label(m).into()),
            COL_DATE_R => data
                .right_modified
                .as_deref()
                .map(|m| format_modified_label(m).into()),
            COL_STATUS => Some(SharedString::from(data.status_symbol)),
            _ => None,
        }
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(data) = self.row_at(row_ix, cx) else {
            return menu;
        };
        build_row_menu(&data, menu, window, cx)
    }

    fn loading(&self, cx: &App) -> bool {
        self.state.read(cx).is_loading()
    }

    fn render_loading(
        &mut self,
        _: gpui::component::Size,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        let label = self
            .state
            .read(cx)
            .busy_label()
            .unwrap_or("Loading comparison…")
            .to_string();
        h_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .text_color(cx.theme().muted_foreground)
            .child(Spinner::new().small())
            .child(label)
            .into_any_element()
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut gpui::Context<TableState<Self>>,
    ) -> impl IntoElement {
        let has_comparison = self.state.read(cx).comparison().is_some();
        let (title, hint) = if has_comparison {
            ("No entries", "Adjust the display filters to see entries.")
        } else {
            (
                "No comparison loaded",
                "Drop a folder onto either half, or edit a path above.",
            )
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .text_color(cx.theme().muted_foreground)
            .child(div().child(title))
            .child(div().text_xs().child(hint))
            .into_any_element()
    }
}

/// Compact display for a modification stamp, shared by the date cells
/// and their accessibility labels.
fn format_modified_label(modified: &str) -> String {
    crate::ui::format_modified(modified)
}

/// The color role of `side`'s data in `row`.
fn side_role(row: &TableRow, side: Side) -> color::ColorRole {
    match side {
        Side::Left => row.left_role,
        Side::Right => row.right_role,
    }
}

/// One clickable column header, driving [`AppState::set_sort`].
///
/// `side` (`"Left"` / `"Right"`) distinguishes the two halves of the
/// comparison in the element id and the accessible name, since both
/// halves carry the same visible label.
///
/// The table's own sort handling stays off; the sort state lives in
/// `AppState`, which also keeps the selection on the same entry across
/// the reorder.
fn sort_header(
    state: &Entity<AppState>,
    label: &'static str,
    side: &'static str,
    key: SortKey,
    align_end: bool,
    cx: &App,
) -> AnyElement {
    let (sort_key, descending) = {
        let state = state.read(cx);
        (state.sort_key(), state.is_sort_descending())
    };
    let is_active = sort_key == key;
    let state = state.clone();
    h_flex()
        .id((ElementId::from("sort_header"), format!("{side}:{label}")))
        .size_full()
        .items_center()
        .gap_1()
        .when(align_end, |this| this.justify_end())
        .text_color(if is_active {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        })
        .hover(|this| this.bg(cx.theme().accent))
        .role(gpui::Role::Button)
        .aria_label(format!("Sort by {label} ({})", side.to_lowercase()))
        .on_click(move |_, _, cx| {
            state.update(cx, |state, cx| state.set_sort(key, cx));
        })
        .child(label)
        .when(is_active, |this| {
            this.child(div().flex_shrink_0().child(if descending {
                gpui::assets::IconName::ArrowDown
            } else {
                gpui::assets::IconName::ArrowUp
            }))
        })
        .test_support()
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Table row
// ---------------------------------------------------------------------------

/// Everything one comparison row needs to render, for both sides.
#[derive(Clone)]
pub(crate) struct TableRow {
    /// One flag per indent slot (levels `0..depth`): whether a vertical
    /// tree guide is drawn there.
    guides: Vec<bool>,
    /// Whether this row is the last sibling at its depth (draws `└─`).
    last: bool,
    name: SharedString,
    status: DirEntryStatus,
    is_dir: bool,
    expanded: bool,
    ignored: bool,
    left_size: Option<u64>,
    right_size: Option<u64>,
    left_modified: Option<SharedString>,
    right_modified: Option<SharedString>,
    left_role: color::ColorRole,
    right_role: color::ColorRole,
    overrides: cocomo_lib::ColorOverrides,
    /// Coarse status glyph; empty for orphans, which the row coloring
    /// already marks.
    status_symbol: &'static str,
    has_left: bool,
    has_right: bool,
    left_is_dir: bool,
    right_is_dir: bool,
    name_conflict: bool,
    left_endpoint: Endpoint,
    right_endpoint: Endpoint,
}

impl TableRow {
    pub(crate) fn new(
        index: usize,
        row: &TreeRow,
        entry: &DirEntry,
        state: &AppState,
        overrides: &cocomo_lib::ColorOverrides,
    ) -> Self {
        let (guides, last) = tree_guides(state, index, row.depth());
        let ignored = state.is_ignored(&entry.name);
        let (left_endpoint, right_endpoint) =
            entry_endpoints(entry, state.left(), state.right());
        Self {
            guides,
            last,
            name: SharedString::from(entry.name.clone()),
            status: entry.status,
            is_dir: entry.left.as_ref().is_some_and(|l| l.is_dir)
                || entry.right.as_ref().is_some_and(|r| r.is_dir),
            expanded: state.is_expanded(row.path()),
            ignored,
            left_size: entry.left.as_ref().map(|s| s.size),
            right_size: entry.right.as_ref().map(|s| s.size),
            left_modified: entry
                .left
                .as_ref()
                .map(|s| SharedString::from(s.modified.clone())),
            right_modified: entry
                .right
                .as_ref()
                .map(|s| SharedString::from(s.modified.clone())),
            left_role: color::side_role(entry, Side::Left, ignored),
            right_role: color::side_role(entry, Side::Right, ignored),
            overrides: overrides.clone(),
            status_symbol: color::gutter_icon(entry, ignored),
            has_left: entry.left.is_some(),
            has_right: entry.right.is_some(),
            left_is_dir: entry.left.as_ref().is_some_and(|l| l.is_dir),
            right_is_dir: entry.right.as_ref().is_some_and(|r| r.is_dir),
            name_conflict: entry.name_conflict,
            left_endpoint,
            right_endpoint,
        }
    }

    /// The action the row context menu's `Open` dispatches: nested dir
    /// compare for directories, text diff for files.
    pub(crate) fn open_action(&self) -> Box<dyn gpui::Action> {
        if self.is_dir {
            Box::new(OpenDirDiff {
                left_path: self.left_endpoint.clone(),
                right_path: self.right_endpoint.clone(),
            })
        } else {
            Box::new(OpenFileDiff {
                left_path: self.left_endpoint.clone(),
                right_path: self
                    .has_right
                    .then(|| self.right_endpoint.clone()),
            })
        }
    }
}

/// The name cell of one side: tree indent and indicator, expansion
/// chevron, and the entry name.
///
/// The side of an orphan that has no entry renders blank; the row's
/// other side carries the comparison.
fn render_name_cell(
    row: &TableRow,
    side: Side,
    theme: &Theme,
    rem: gpui::Pixels,
) -> impl IntoElement {
    let present = if side == Side::Right {
        row.has_right
    } else {
        row.has_left
    };
    let role = side_role(row, side);
    let row_color: Hsla = role.resolve(theme, &row.overrides);

    h_flex()
        .size_full()
        .items_center()
        .gap_1()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_color(row_color)
        .when(present, |this| {
            // Tree indent: one full-height slot per ancestor level with a
            // vertical line wherever that level continues below this row,
            // then the `├─`/`└─` indicator for the entry itself.
            this.children(row.guides.iter().map(|active| {
                div()
                    .w(rems(INDENT_REMS).to_pixels(rem))
                    .flex_shrink_0()
                    .self_stretch()
                    .flex()
                    .justify_center()
                    .when(*active, |slot| {
                        slot.child(div().w_px().h_full().bg(theme.border))
                    })
            }))
            .when(!row.guides.is_empty(), |this| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if row.last { "└─" } else { "├─" }),
                )
            })
            .child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if row.is_dir {
                        if row.expanded {
                            gpui::assets::IconName::ChevronDown
                        } else {
                            gpui::assets::IconName::ChevronRight
                        }
                    } else {
                        gpui::assets::IconName::File
                    }),
            )
            .child(div().truncate().child(row.name.clone()))
        })
}

/// Which indent slots of the row at `index` carry a vertical tree guide,
/// and whether the row is the last sibling at its depth.
///
/// A slot `l` (0-based, `0..depth`) belongs to the row's ancestor at
/// depth `l`; its guide continues exactly while the row directly below
/// is deeper than `l` — that row is still inside the ancestor's subtree.
/// The `last` flag decides between the `└─` and `├─` indicator: the row
/// is last when the next row at its depth or shallower is strictly
/// shallower (or there is none), i.e. it has no sibling below. Because
/// rows are in depth-first order, descendants are skipped while
/// scanning.
fn tree_guides(
    state: &AppState,
    index: usize,
    depth: usize,
) -> (Vec<bool>, bool) {
    if depth == 0 {
        return (Vec::new(), false);
    }
    let next_depth = state.row_at(index + 1).map(|row| row.depth());
    let guides = (0..depth)
        .map(|level| next_depth.is_some_and(|d| d > level))
        .collect();
    let mut last = true;
    let mut next = index + 1;
    while let Some(row) = state.row_at(next) {
        let row_depth = row.depth();
        if row_depth <= depth {
            last = row_depth < depth;
            break;
        }
        next += 1;
    }
    (guides, last)
}

/// Build the rich row context menu of spec section 4.4.
fn build_row_menu(
    row: &TableRow,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut gpui::Context<TableState<FolderTableDelegate>>,
) -> PopupMenu {
    let differs = row.status != DirEntryStatus::Same
        && row.status != DirEntryStatus::SameBinary;
    let mut menu = menu
        .menu("Open", row.open_action())
        .menu("Quick Compare", Box::new(QuickCompare));
    // A file present on both sides can be merged (two-way: a folder
    // comparison has no ancestor to offer).
    if !row.is_dir && row.has_left && row.has_right {
        menu = menu.menu(
            "Merge…",
            Box::new(OpenMerge {
                left_path: row.left_endpoint.clone(),
                right_path: row.right_endpoint.clone(),
            }),
        );
    }
    // The delegate hands us a `Context<TableState<_>>`, not a
    // `Context<PopupMenu>`, so submenus are built as standalone entities
    // and attached as items.
    let compare_contents = PopupMenu::build(window, cx, |menu, _, _| {
        menu.menu("By content (hash)", Box::new(CompareContentsHash))
            .menu("By size & date", Box::new(CompareContentsMeta))
    });
    let mut menu = menu
        .item(PopupMenuItem::submenu("Compare Contents", compare_contents))
        .separator();
    // Reconcile toward each side, following the entry's shape.
    if differs && !row.name_conflict {
        if row.has_left {
            menu = menu
                .menu("Copy to right", Box::new(CopyToRight))
                .menu("Move to right", Box::new(MoveToRight));
        }
        if row.has_right {
            menu = menu
                .menu("Copy to left", Box::new(CopyToLeft))
                .menu("Move to left", Box::new(MoveToLeft));
        }
        if row.has_left && row.has_right {
            menu = menu.menu("Exchange", Box::new(ExchangeEntry));
        }
    }
    if row.has_left ^ row.has_right {
        menu = menu.menu("Copy to other side", Box::new(CopyToOtherSide));
    }
    menu = menu
        .menu("Copy to folder…", Box::new(CopyToFolder))
        .menu("Move to folder…", Box::new(MoveToFolder));
    if differs && !row.name_conflict {
        if row.has_left {
            menu = menu.menu("Delete from left", Box::new(DeleteLeftSide));
        }
        if row.has_right {
            menu = menu.menu("Delete from right", Box::new(DeleteRightSide));
        }
    }
    menu = menu
        .separator()
        .menu("Rename…", Box::new(RenameEntry))
        .menu("Attributes", Box::new(ShowAttributes))
        .menu("Touch", Box::new(TouchEntry))
        .separator()
        .menu("Exclude", Box::new(ExcludeEntry))
        .menu_with_check("Ignored", row.ignored, Box::new(ToggleIgnored))
        .menu("New Folder", Box::new(NewFolder))
        .menu("Copy Filename", Box::new(CopyFilename))
        .menu("Refresh Selection", Box::new(RefreshSelection));
    let synchronize = PopupMenu::build(window, cx, |menu, _, _| {
        menu.menu("Update Right", Box::new(SyncUpdateRight))
            .menu("Update Left", Box::new(SyncUpdateLeft))
            .menu("Update Both", Box::new(SyncUpdateBoth))
            .separator()
            .menu("Mirror to Right", Box::new(SyncMirrorRight))
            .menu("Mirror to Left", Box::new(SyncMirrorLeft))
    });
    menu = menu.item(PopupMenuItem::submenu("Synchronize", synchronize));
    if row.left_is_dir || row.right_is_dir {
        menu = menu.menu("Explorer", Box::new(RevealInExplorer));
        menu = menu.separator();
        if row.left_is_dir {
            menu =
                menu.menu("Set as base folder (left)", Box::new(SetBaseLeft));
        }
        if row.right_is_dir {
            menu = menu
                .menu("Set as base folder (right)", Box::new(SetBaseRight));
        }
        if row.left_is_dir && row.right_is_dir {
            menu =
                menu.menu("Set as base folder (both)", Box::new(SetBaseBoth));
        }
    } else {
        menu = menu.menu("Explorer", Box::new(RevealInExplorer));
    }
    menu
}

/// Format a file size in human-readable form.
pub(crate) fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Placeholder action for an empty history dropdown.
#[derive(gpui::Action, Clone, Debug, PartialEq)]
#[action(namespace = folder_table, no_json)]
struct Noop;

#[cfg(test)]
mod tests {
    use cocomo_lib::{DirComparison, DirEntryStatus, EntryInfo};
    use gpui::TestAppContext;

    use super::*;
    use crate::state::TreeRow;

    fn info(path: &str, is_dir: bool) -> EntryInfo {
        EntryInfo {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            size: 1,
            modified: String::new(),
            is_dir,
            hash: None,
        }
    }

    fn entry(
        name: &str,
        status: DirEntryStatus,
        left: Option<EntryInfo>,
        right: Option<EntryInfo>,
    ) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            status,
            left,
            right,
            center: None,
            sub_entries: None,
            name_conflict: false,
        }
    }

    /// Build a table row over `entry` against a state rooted at
    /// `/left` ↔ `/right`.
    fn row(entry: &DirEntry, cx: &mut TestAppContext) -> TableRow {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(DirComparison {
                entries: vec![entry.clone()],
                counts: Default::default(),
                errors: Vec::new(),
            });
            state
        });
        cx.update(|cx| {
            let read = state.read(cx);
            TableRow::new(
                0,
                &TreeRow::new_for_test(0, entry.name.as_str()),
                entry,
                read,
                &cocomo_lib::ColorOverrides::default(),
            )
        })
    }

    #[gpui::test]
    fn table_row_endpoints_join_side_paths_onto_the_comparison_root(
        cx: &mut TestAppContext,
    ) {
        let entry = entry(
            "sub",
            DirEntryStatus::Same,
            Some(info("sub", true)),
            Some(info("other/sub", true)),
        );
        let data = row(&entry, cx);
        assert_eq!(data.left_endpoint.display(), "/left/sub");
        assert_eq!(data.right_endpoint.display(), "/right/other/sub");
    }

    #[gpui::test]
    fn table_row_endpoints_fall_back_to_name_for_missing_side(
        cx: &mut TestAppContext,
    ) {
        let entry = entry(
            "orphan.txt",
            DirEntryStatus::LeftOnly,
            Some(info("orphan.txt", false)),
            None,
        );
        let data = row(&entry, cx);
        assert_eq!(data.left_endpoint.display(), "/left/orphan.txt");
        assert_eq!(data.right_endpoint.display(), "/right/orphan.txt");
    }

    #[gpui::test]
    fn open_action_opens_dir_diff_for_directories(cx: &mut TestAppContext) {
        let entry = entry(
            "sub",
            DirEntryStatus::Same,
            Some(info("sub", true)),
            Some(info("sub", true)),
        );
        let expected = OpenDirDiff {
            left_path: Endpoint::local("/left/sub"),
            right_path: Endpoint::local("/right/sub"),
        };
        assert!(row(&entry, cx).open_action().partial_eq(&expected));
    }

    #[gpui::test]
    fn open_action_opens_file_diff_with_both_sides(cx: &mut TestAppContext) {
        let entry = entry(
            "a.txt",
            DirEntryStatus::Different,
            Some(info("a.txt", false)),
            Some(info("a.txt", false)),
        );
        let expected = OpenFileDiff {
            left_path: Endpoint::local("/left/a.txt"),
            right_path: Some(Endpoint::local("/right/a.txt")),
        };
        assert!(row(&entry, cx).open_action().partial_eq(&expected));
    }

    #[gpui::test]
    fn open_action_omits_missing_right_side(cx: &mut TestAppContext) {
        let entry = entry(
            "only-left.txt",
            DirEntryStatus::LeftOnly,
            Some(info("only-left.txt", false)),
            None,
        );
        let expected = OpenFileDiff {
            left_path: Endpoint::local("/left/only-left.txt"),
            right_path: None,
        };
        assert!(row(&entry, cx).open_action().partial_eq(&expected));
    }

    #[gpui::test]
    fn table_row_carries_both_sides_data(cx: &mut TestAppContext) {
        let mut left = info("a.txt", false);
        left.size = 10;
        let mut right = info("a.txt", false);
        right.size = 20;
        let entry =
            entry("a.txt", DirEntryStatus::Different, Some(left), Some(right));
        let data = row(&entry, cx);
        assert_eq!(data.left_size, Some(10));
        assert_eq!(data.right_size, Some(20));
    }

    #[gpui::test]
    fn table_row_marks_the_absent_side_of_an_orphan(cx: &mut TestAppContext) {
        let entry = entry(
            "orphan.txt",
            DirEntryStatus::LeftOnly,
            Some(info("orphan.txt", false)),
            None,
        );
        let data = row(&entry, cx);
        assert!(data.has_left);
        assert!(!data.has_right);
    }

    #[gpui::test]
    fn orphan_rows_have_no_status_symbol(cx: &mut TestAppContext) {
        let left_only = entry(
            "orphan.txt",
            DirEntryStatus::LeftOnly,
            Some(info("orphan.txt", false)),
            None,
        );
        assert_eq!(row(&left_only, cx).status_symbol, "");

        let right_only = entry(
            "orphan.txt",
            DirEntryStatus::RightOnly,
            None,
            Some(info("orphan.txt", false)),
        );
        assert_eq!(row(&right_only, cx).status_symbol, "");

        // A matched difference still carries its glyph.
        let different = entry(
            "a.txt",
            DirEntryStatus::Different,
            Some(info("a.txt", false)),
            Some(info("a.txt", false)),
        );
        assert_eq!(row(&different, cx).status_symbol, "~");
    }

    #[gpui::test]
    fn tree_guides_continue_while_a_level_has_more_siblings(
        cx: &mut TestAppContext,
    ) {
        // Visible tree after expanding `a` and `a/a1`:
        //   a
        //     a1
        //       x
        //     a2
        //   b
        let x_sub = DirComparison {
            entries: vec![entry(
                "x",
                DirEntryStatus::Same,
                Some(info("a/a1/x", false)),
                Some(info("a/a1/x", false)),
            )],
            counts: Default::default(),
            errors: Vec::new(),
        };
        let mut a1 = entry(
            "a1",
            DirEntryStatus::Same,
            Some(info("a/a1", true)),
            Some(info("a/a1", true)),
        );
        a1.sub_entries = Some(std::sync::Arc::new(x_sub));
        let sub = DirComparison {
            entries: vec![
                a1,
                entry(
                    "a2",
                    DirEntryStatus::Same,
                    Some(info("a/a2", false)),
                    Some(info("a/a2", false)),
                ),
            ],
            counts: Default::default(),
            errors: Vec::new(),
        };
        let mut a = entry(
            "a",
            DirEntryStatus::Same,
            Some(info("a", true)),
            Some(info("a", true)),
        );
        a.sub_entries = Some(std::sync::Arc::new(sub));
        let b = entry(
            "b",
            DirEntryStatus::Same,
            Some(info("b", false)),
            Some(info("b", false)),
        );
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(DirComparison {
                entries: vec![a, b],
                counts: Default::default(),
                errors: Vec::new(),
            });
            state
        });
        state.update(cx, |state, cx| {
            state.toggle_expanded("a", cx);
            state.toggle_expanded("a/a1", cx);
        });
        cx.update(|cx| {
            let read = state.read(cx);
            assert_eq!(read.entry_count(), 5);
            // Root rows carry no indent slots.
            assert_eq!(tree_guides(read, 0, 0), (vec![], false));
            // `a1` sits under the still-open `a`: its guide continues,
            // and a sibling (`a2`) follows, so it is not last.
            assert_eq!(tree_guides(read, 1, 1), (vec![true], false));
            // `x` is `a1`'s only child: `a`'s guide continues through
            // its row, `a1`'s child line closes at the `└─`.
            assert_eq!(tree_guides(read, 2, 2), (vec![true, false], true));
            // `a2` is the last child: the `└─` indicator closes the
            // guide, so its slot stays blank.
            assert_eq!(tree_guides(read, 3, 1), (vec![false], true));
            // `b` is a root entry: no indent slots.
            assert_eq!(tree_guides(read, 4, 0), (vec![], false));
        });
    }

    #[gpui::test]
    fn delegate_rows_count_tracks_the_visible_tree(cx: &mut TestAppContext) {
        let state = cx.new(|cx| {
            let mut state = AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            );
            state.set_comparison_for_test(DirComparison {
                entries: vec![entry(
                    "a.txt",
                    DirEntryStatus::Same,
                    Some(info("a.txt", false)),
                    Some(info("a.txt", false)),
                )],
                counts: Default::default(),
                errors: Vec::new(),
            });
            state
        });
        let delegate = cx.update(|_cx| {
            FolderTableDelegate::new(state.clone(), gpui::px(16.0))
        });
        cx.update(|cx| {
            assert_eq!(delegate.rows_count(cx), state.read(cx).entry_count());
            assert_eq!(delegate.columns_count(cx), 7);
        });
    }

    /// A delegate over an empty comparison, with a 16 px rem basis.
    fn test_delegate(cx: &mut TestAppContext) -> FolderTableDelegate {
        let state = cx.new(|cx| {
            AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            )
        });
        cx.update(|_cx| FolderTableDelegate::new(state, gpui::px(16.)))
    }

    #[gpui::test]
    fn left_half_width_covers_the_left_group_and_status_lane(
        cx: &mut TestAppContext,
    ) {
        let delegate = test_delegate(cx);
        // Name (16) + Size (4.5) + Modified (7.5) + status (1.75) rems
        // at a 16 px rem basis.
        assert_eq!(delegate.left_half_width(), px(476.));
    }

    #[gpui::test]
    fn set_fill_width_stretches_the_right_name_column(
        cx: &mut TestAppContext,
    ) {
        let mut delegate = test_delegate(cx);
        // The left group (476 px), the right Size and Modified columns
        // (192 px), and the trailing filler (12 px) stay fixed; the
        // right Name column takes the rest.
        assert!(delegate.set_fill_width(px(1200.)));
        assert_eq!(delegate.columns[RIGHT_NAME_IX].width, px(520.));
        // An unchanged available width is a no-op, so the per-frame
        // hook does not repaint the table.
        assert!(!delegate.set_fill_width(px(1200.)));
        // A too-narrow viewport clamps at the column's minimum and the
        // table scrolls horizontally instead.
        assert!(delegate.set_fill_width(px(200.)));
        assert_eq!(
            delegate.columns[RIGHT_NAME_IX].width,
            delegate.columns[RIGHT_NAME_IX].min_width
        );
    }
}
