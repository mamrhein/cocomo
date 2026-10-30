// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Two-pane mirrored tree layout for the folder compare view.
//!
//! Implements the layout of UI spec section 4.1: a left and a right tree
//! pane separated by a splitter, each with its own editable path bar and
//! a history dropdown. Both panes render the *same* [`TreeRow`] list from
//! [`AppState`], so mirrored rows align by construction; the left pane's
//! rows additionally carry the status column with the connector line and
//! the coarse status symbol of spec section 4.2, and nested rows are
//! indented with vertical tree guides in place of the CLI's `├─`/`└─`
//! prefixes.

use std::ops::Range;

use cocomo_lib::{DirEntry, DirEntryStatus, Endpoint};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, Entity, ExternalPaths,
    FocusHandle, FontWeight, Hsla, MouseButton, Pixels, SharedString,
    TestSupportExt as _, UniformListScrollHandle, Window,
    component::{
        ActiveTheme, Sizable as _, Theme,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputState},
        menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu},
        resizable::{h_resizable, resizable_panel},
        spinner::Spinner,
        v_flex,
    },
    div,
    prelude::*,
    rems, uniform_list,
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

/// Shared plumbing for both panes: the synchronized scroll handle and
/// the view's keyboard focus target.
struct PaneEnv<'a> {
    scroll_handle: &'a UniformListScrollHandle,
    focus_handle: &'a FocusHandle,
}

/// Render the two mirrored tree panes with their path bars.
///
/// The panes share one [`UniformListScrollHandle`], so scrolling either
/// list scrolls the other and the gutter. `left_path` and `right_path`
/// are the editable path fields; pressing Enter in one reloads that side
/// of the compared pair.
pub(crate) fn render_folder_panes(
    state: &Entity<AppState>,
    scroll_handle: &UniformListScrollHandle,
    focus_handle: &FocusHandle,
    left_path: &Entity<InputState>,
    right_path: &Entity<InputState>,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme().clone();
    let env = PaneEnv {
        scroll_handle,
        focus_handle,
    };

    let left_pane =
        render_pane(Side::Left, state, &env, left_path, &theme, cx);
    let right_pane =
        render_pane(Side::Right, state, &env, right_path, &theme, cx);

    h_resizable("folder-panes")
        .child(resizable_panel().child(left_pane))
        .child(resizable_panel().child(right_pane))
        .into_any_element()
}

/// Point `side` at the first dropped path, when it is a directory.
///
/// A drop that cannot be honored says why in the log and as a notification;
/// silently ignoring a dropped item reads as a broken drop target.
fn accept_pane_drop(
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

/// One pane: path bar, column headers, and the tree list.
fn render_pane(
    side: Side,
    state: &Entity<AppState>,
    env: &PaneEnv<'_>,
    path_input: &Entity<InputState>,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let (entry_count, is_loading) = {
        let state = state.read(cx);
        (state.entry_count(), state.is_loading())
    };
    let bar = render_path_bar(side, state, path_input, theme, cx);
    let headers = render_pane_headers(side, state, theme, cx);
    let list = render_pane_list(side, state, env, entry_count, theme);

    let side_key = if side == Side::Left { "left" } else { "right" };
    let primary = theme.primary;
    let drop_state = state.clone();

    v_flex()
        .id(ElementId::Name(format!("folder-pane-{side_key}").into()))
        .size_full()
        .min_w_0()
        .bg(theme.background)
        // Dropping a folder onto a pane repoints that side, the
        // pointer-first equivalent of editing its path bar.
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.border_color(primary)
        })
        .on_drop::<ExternalPaths>(move |paths, _, cx| {
            accept_pane_drop(side, &drop_state, paths, cx);
        })
        .child(bar)
        .child(headers)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .when(is_loading, |this| {
                    this.child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .text_color(theme.muted_foreground)
                            .child(Spinner::new().small())
                            .child("Loading comparison…"),
                    )
                })
                .when(!is_loading, |this| this.child(list)),
        )
        .into_any_element()
}

/// The per-pane path bar: editable path, history dropdown, new folder.
fn render_path_bar(
    side: Side,
    state: &Entity<AppState>,
    path_input: &Entity<InputState>,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let history: Vec<String> = state.read(cx).side_path_history(side);

    let history_btn = Button::new((
        ElementId::from("path-history"),
        SharedString::from(format!("{side:?}")),
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
        SharedString::from(format!("{side:?}")),
    ))
    .icon(gpui::assets::IconName::Folder)
    .ghost()
    .xsmall()
    .tooltip("New folder")
    .accessibility_label("New folder")
    .on_click({
        let state = state.clone();
        move |_, _, cx| {
            state.update(cx, |state, cx| match side {
                Side::Left => state.new_folder(Side::Left, cx),
                Side::Right => state.new_folder(Side::Right, cx),
            });
        }
    });

    h_flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .bg(theme.muted)
        .border_b_1()
        .border_color(theme.border)
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

/// Sortable column headers of one pane: Name, Size, Modified.
fn render_pane_headers(
    side: Side,
    state: &Entity<AppState>,
    theme: &Theme,
    cx: &mut App,
) -> AnyElement {
    let (size_key, date_key) = match side {
        Side::Left => (SortKey::SizeLeft, SortKey::DateLeft),
        Side::Right => (SortKey::SizeRight, SortKey::DateRight),
    };

    h_flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .bg(theme.muted)
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .font_weight(FontWeight(600.))
        .child(sort_header_cell(
            side,
            "Name",
            SortKey::Name,
            state,
            theme,
            false,
            cx,
        ))
        .child(sort_header_cell(
            side, "Size", size_key, state, theme, true, cx,
        ))
        .child(sort_header_cell(
            side, "Modified", date_key, state, theme, true, cx,
        ))
        .when(side == Side::Left, |this| this.child(gutter_lane()))
        .into_any_element()
}

/// One clickable column header cell, keyed per side so the two panes can
/// both offer the same column without element-id collisions.
fn sort_header_cell(
    side: Side,
    label: &'static str,
    key: SortKey,
    state: &Entity<AppState>,
    theme: &Theme,
    align_end: bool,
    cx: &mut App,
) -> AnyElement {
    let (sort_key, descending) = {
        let state = state.read(cx);
        (state.sort_key(), state.is_sort_descending())
    };
    let is_active = sort_key == key;
    let state = state.clone();
    header_lane(label, align_end)
        .id((ElementId::from("sort_header"), format!("{side:?}:{label}")))
        .when(align_end, |this| this.justify_end())
        .text_color(if is_active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .hover(|this| this.bg(theme.accent))
        .role(gpui::Role::Button)
        .aria_label(format!("Sort by {label}"))
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

/// Header lane matching the row lanes of a pane.
///
/// The lanes are the very builders the rows use, so a header label and its
/// column values share the same alignment spine by construction.
fn header_lane(label: &'static str, align_end: bool) -> gpui::Div {
    let lane = match label {
        "Name" => name_lane(),
        "Size" => size_lane(),
        _ => date_lane(),
    };
    if align_end { lane.justify_end() } else { lane }
}

/// Name lane: chevron, type icon, and the entry name.
fn name_lane() -> gpui::Div {
    div().flex_1().min_w_0().flex().items_center().gap_1p5()
}

/// Mirrored size lane, right-aligned like the table it replaces.
fn size_lane() -> gpui::Div {
    div()
        .w_20()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .text_right()
        .text_xs()
}

/// Mirrored modification-date lane.
fn date_lane() -> gpui::Div {
    div()
        .w_32()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .text_right()
        .text_xs()
}

/// The status column closing the left pane's rows: a connector line for
/// matched entries and the coarse status symbol in its own sub-column.
fn gutter_lane() -> gpui::Div {
    div()
        .w_10()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
}

/// The tree list of one pane, rendering the shared row range.
fn render_pane_list(
    side: Side,
    state: &Entity<AppState>,
    env: &PaneEnv<'_>,
    entry_count: usize,
    theme: &Theme,
) -> AnyElement {
    let state_weak = state.downgrade();
    let scroll_handle = env.scroll_handle.clone();
    let focus_handle = env.focus_handle.clone();
    let theme = theme.clone();

    uniform_list(
        (
            ElementId::from("pane-list"),
            SharedString::from(format!("{side:?}")),
        ),
        entry_count,
        move |range: Range<usize>, window: &mut Window, app: &mut App| {
            let mut items = Vec::with_capacity(range.len());
            if let Some(state_entity) = state_weak.upgrade() {
                // Resolve the global color overrides once per batch, not
                // once per row.
                let overrides = color::merge_overrides(
                    &state_entity.read(app).settings().color_overrides,
                    app,
                );
                let app_state = state_entity.read(app);
                for i in range {
                    let (Some(row), Some(entry)) =
                        (app_state.row_at(i), app_state.entry_at(i))
                    else {
                        continue;
                    };
                    let data = PaneRow::new(
                        i, side, row, entry, app_state, &overrides,
                    );
                    items.push(
                        data.render_row(
                            &theme,
                            window.rem_size(),
                            state_entity.clone(),
                            focus_handle.clone(),
                        )
                        .into_any_element(),
                    );
                }
            }
            items
        },
    )
    .size_full()
    .track_scroll(&scroll_handle)
    .into_any_element()
}

// ---------------------------------------------------------------------------
// Pane row
// ---------------------------------------------------------------------------

/// Everything one pane row of one side needs to render.
#[derive(Clone)]
pub(crate) struct PaneRow {
    index: usize,
    side: Side,
    /// Whether this side actually has an entry for the row. An orphan's
    /// empty side renders blank lanes to keep the panes mirrored.
    present: bool,
    /// One flag per indent slot (levels `0..=depth`): whether a vertical
    /// tree guide is drawn there.
    guides: Vec<bool>,
    path: SharedString,
    name: SharedString,
    status: DirEntryStatus,
    is_dir: bool,
    expanded: bool,
    ignored: bool,
    is_selected: bool,
    size: Option<u64>,
    modified: Option<SharedString>,
    role: color::ColorRole,
    overrides: cocomo_lib::ColorOverrides,
    gutter: &'static str,
    has_left: bool,
    has_right: bool,
    left_is_dir: bool,
    right_is_dir: bool,
    name_conflict: bool,
    left_endpoint: Endpoint,
    right_endpoint: Endpoint,
}

impl PaneRow {
    pub(crate) fn new(
        index: usize,
        side: Side,
        row: &TreeRow,
        entry: &DirEntry,
        state: &AppState,
        overrides: &cocomo_lib::ColorOverrides,
    ) -> Self {
        let side_info = match side {
            Side::Left => entry.left.as_ref(),
            Side::Right => entry.right.as_ref(),
        };
        let (left_endpoint, right_endpoint) =
            entry_endpoints(entry, state.left(), state.right());
        Self {
            index,
            side,
            present: side_info.is_some(),
            guides: tree_guides(state, index, row.depth()),
            path: SharedString::from(row.path().to_string()),
            name: SharedString::from(entry.name.clone()),
            status: entry.status,
            is_dir: entry.left.as_ref().is_some_and(|l| l.is_dir)
                || entry.right.as_ref().is_some_and(|r| r.is_dir),
            expanded: state.is_expanded(row.path()),
            ignored: state.is_ignored(&entry.name),
            is_selected: index == state.selected_index(),
            size: side_info.map(|s| s.size),
            modified: side_info
                .map(|s| SharedString::from(s.modified.clone())),
            role: color::side_role(entry, side, state.is_ignored(&entry.name)),
            overrides: overrides.clone(),
            gutter: color::gutter_icon(entry, state.is_ignored(&entry.name)),
            has_left: entry.left.is_some(),
            has_right: entry.right.is_some(),
            left_is_dir: entry.left.as_ref().is_some_and(|l| l.is_dir),
            right_is_dir: entry.right.as_ref().is_some_and(|r| r.is_dir),
            name_conflict: entry.name_conflict,
            left_endpoint,
            right_endpoint,
        }
    }

    /// The action a double-click or Enter opens: nested dir compare for
    /// directories, text diff for files.
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

    fn render_row(
        self,
        theme: &Theme,
        rem_size: Pixels,
        state: Entity<AppState>,
        focus_handle: FocusHandle,
    ) -> impl IntoElement {
        let data = self.clone();
        let PaneRow {
            index,
            side,
            present,
            guides,
            path,
            name,
            is_dir,
            expanded,
            is_selected,
            size,
            modified,
            role,
            overrides,
            gutter,
            ..
        } = self;

        let row_color: Hsla = role.resolve(theme, &overrides);
        let row_id = (
            ElementId::from("pane_row"),
            SharedString::from(format!("{}/{}", side_key(side), path)),
        );
        let open_data = data.clone();
        let expand_data = data.clone();
        let click_state = state.clone();
        let click_focus = focus_handle.clone();

        div()
            .id(row_id)
            .flex()
            // Lanes stretch so the tree guides can span the full row
            // height and connect across rows; each lane carries the
            // vertical padding instead.
            .items_stretch()
            .gap_1()
            .px_2()
            .text_color(row_color)
            .when(is_selected, |this| this.bg(theme.list_active))
            .hover(|this| {
                if is_selected {
                    this
                } else {
                    this.bg(theme.list_hover)
                }
            })
            .role(gpui::Role::ListItem)
            .aria_selected(is_selected)
            .on_click(move |event: &ClickEvent, window, cx| {
                click_focus.focus(window, cx);
                let double_click = event.click_count() == 2;
                let action = if double_click && expand_data.is_dir {
                    // Explorer-style: a double-click on a directory
                    // expands or collapses it in place.
                    let path = expand_data.path.to_string();
                    click_state.update(cx, |state, cx| {
                        state.select_row(index, cx);
                        state.toggle_expanded(&path, cx);
                    });
                    None
                } else {
                    click_state.update(cx, |state, cx| {
                        state.select_row(index, cx);
                        double_click.then(|| open_data.open_action())
                    })
                };
                if let Some(action) = action {
                    window.dispatch_action(action, cx);
                }
            })
            .on_mouse_down(MouseButton::Right, {
                let state = state.clone();
                let focus = focus_handle.clone();
                move |_, window, cx| {
                    focus.focus(window, cx);
                    state.update(cx, |state, cx| {
                        state.select_row(index, cx);
                    });
                }
            })
            .context_menu(move |menu, window, cx| {
                build_row_menu(&data, menu, window, cx)
            })
            // Tree indent guides: one full-height slot per level, with a
            // vertical line wherever that level continues below this row.
            .when(present && !guides.is_empty(), |this| {
                this.child(
                    div().flex_shrink_0().flex().items_stretch().children(
                        guides.iter().map(|active| {
                            div()
                                .w(rems(INDENT_REMS) * rem_size)
                                .flex_shrink_0()
                                .flex()
                                .justify_center()
                                .when(*active, |slot| {
                                    slot.child(
                                        div().w_px().h_full().bg(theme.border),
                                    )
                                })
                        }),
                    ),
                )
            })
            // Name lane: expansion chevron, type icon, and the entry name.
            // An orphan's empty side stays blank; its row only keeps the
            // mirrored panes aligned.
            .child(
                name_lane()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .py_0p5()
                    .when(present, |this| {
                        this.child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(if is_dir {
                                    if expanded {
                                        gpui::assets::IconName::ChevronDown
                                    } else {
                                        gpui::assets::IconName::ChevronRight
                                    }
                                } else {
                                    gpui::assets::IconName::File
                                }),
                        )
                        .child(div().truncate().child(name))
                    })
                    // The blank side of an orphan still needs a text line
                    // box: the virtual list assumes one uniform row height.
                    .when(!present, |this| this.child(" ")),
            )
            .child(
                size_lane()
                    .py_0p5()
                    .child(size.map(format_size).unwrap_or_default()),
            )
            .child(
                date_lane().py_0p5().child(
                    modified
                        .as_deref()
                        .map(crate::ui::format_modified)
                        .unwrap_or_default(),
                ),
            )
            // The left pane's rows end with the status column: a connector
            // line for matched rows and the coarse status symbol in its own
            // sub-column.
            .when(side == Side::Left, |this| {
                this.child(
                    gutter_lane()
                        .py_0p5()
                        .when(!matches!(gutter, "|"), |this| {
                            this.child(div().flex_1().h_px().bg(theme.border))
                        })
                        .child(
                            div()
                                .w_4()
                                .flex_shrink_0()
                                .text_center()
                                .font_weight(FontWeight(700.))
                                .text_color(theme.muted_foreground)
                                .child(gutter),
                        ),
                )
            })
    }
}

/// Which indent slots of the row at `index` carry a vertical tree guide.
///
/// A guide at level `l` (0-based, `0..=depth`) stays active while the
/// ancestor at that level still has a sibling below the current row — the
/// GUI equivalent of the CLI's `│` continuation. Because rows are in
/// depth-first order, the first following row with `depth <= l` decides:
/// equal depth means a sibling follows, shallower means the subtree ended.
fn tree_guides(state: &AppState, index: usize, depth: usize) -> Vec<bool> {
    let mut guides = vec![false; depth + 1];
    let mut open: Vec<usize> = (0..=depth).collect();
    let mut next = index + 1;
    while !open.is_empty() {
        let Some(row) = state.row_at(next) else { break };
        let depth = row.depth();
        open.retain(|&level| {
            if depth <= level {
                guides[level] = depth == level;
                false
            } else {
                true
            }
        });
        next += 1;
    }
    guides
}

/// Stable id part distinguishing the two panes' rows.
fn side_key(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
    }
}

/// Build the rich row context menu of spec section 4.4.
fn build_row_menu(
    row: &PaneRow,
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut gpui::Context<PopupMenu>,
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
    let mut menu = menu
        .submenu("Compare Contents", window, cx, |menu: PopupMenu, _, _| {
            menu.menu("By content (hash)", Box::new(CompareContentsHash))
                .menu("By size & date", Box::new(CompareContentsMeta))
        })
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
        .menu("Refresh Selection", Box::new(RefreshSelection))
        .submenu("Synchronize", window, cx, |menu: PopupMenu, _, _| {
            menu.menu("Update Right", Box::new(SyncUpdateRight))
                .menu("Update Left", Box::new(SyncUpdateLeft))
                .menu("Update Both", Box::new(SyncUpdateBoth))
                .separator()
                .menu("Mirror to Right", Box::new(SyncMirrorRight))
                .menu("Mirror to Left", Box::new(SyncMirrorLeft))
        });
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
#[action(namespace = folder_panes, no_json)]
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

    /// Build a pane row over `entry` against a state rooted at
    /// `/left` ↔ `/right`.
    fn row(side: Side, entry: &DirEntry, cx: &mut TestAppContext) -> PaneRow {
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
            PaneRow::new(
                0,
                side,
                &TreeRow::new_for_test(0, entry.name.as_str()),
                entry,
                read,
                &cocomo_lib::ColorOverrides::default(),
            )
        })
    }

    #[gpui::test]
    fn pane_row_endpoints_join_side_paths_onto_the_comparison_root(
        cx: &mut TestAppContext,
    ) {
        let entry = entry(
            "sub",
            DirEntryStatus::Same,
            Some(info("sub", true)),
            Some(info("other/sub", true)),
        );
        let data = row(Side::Left, &entry, cx);
        assert_eq!(data.left_endpoint.display(), "/left/sub");
        assert_eq!(data.right_endpoint.display(), "/right/other/sub");
    }

    #[gpui::test]
    fn pane_row_endpoints_fall_back_to_name_for_missing_side(
        cx: &mut TestAppContext,
    ) {
        let entry = entry(
            "orphan.txt",
            DirEntryStatus::LeftOnly,
            Some(info("orphan.txt", false)),
            None,
        );
        let data = row(Side::Right, &entry, cx);
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
        assert!(
            row(Side::Left, &entry, cx)
                .open_action()
                .partial_eq(&expected)
        );
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
        assert!(
            row(Side::Left, &entry, cx)
                .open_action()
                .partial_eq(&expected)
        );
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
        assert!(
            row(Side::Left, &entry, cx)
                .open_action()
                .partial_eq(&expected)
        );
    }

    #[gpui::test]
    fn pane_row_side_data_follows_the_requested_side(cx: &mut TestAppContext) {
        let mut left = info("a.txt", false);
        left.size = 10;
        let mut right = info("a.txt", false);
        right.size = 20;
        let entry =
            entry("a.txt", DirEntryStatus::Different, Some(left), Some(right));
        assert_eq!(row(Side::Left, &entry, cx).size, Some(10));
        assert_eq!(row(Side::Right, &entry, cx).size, Some(20));
    }

    #[gpui::test]
    fn pane_row_is_absent_on_an_orphans_empty_side(cx: &mut TestAppContext) {
        let entry = entry(
            "orphan.txt",
            DirEntryStatus::LeftOnly,
            Some(info("orphan.txt", false)),
            None,
        );
        assert!(row(Side::Left, &entry, cx).present);
        assert!(!row(Side::Right, &entry, cx).present);
    }

    #[gpui::test]
    fn tree_guides_continue_while_a_level_has_more_siblings(
        cx: &mut TestAppContext,
    ) {
        // Visible tree after expanding `a`:
        //   a
        //     a1
        //     a2
        //   b
        let sub = DirComparison {
            entries: vec![
                entry(
                    "a1",
                    DirEntryStatus::Same,
                    Some(info("a/a1", false)),
                    Some(info("a/a1", false)),
                ),
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
        state.update(cx, |state, cx| state.toggle_expanded("a", cx));
        cx.update(|cx| {
            let read = state.read(cx);
            assert_eq!(read.entry_count(), 4);
            // `a` connects down to `b`, and `a1` to `a2`.
            assert_eq!(tree_guides(read, 0, 0), vec![true]);
            assert_eq!(tree_guides(read, 1, 1), vec![true, true]);
            // `a2` is the last child: its own slot ends, but the `a`-to-`b`
            // continuation still runs through the row.
            assert_eq!(tree_guides(read, 2, 1), vec![true, false]);
            // `b` is the last root entry: no guide below it.
            assert_eq!(tree_guides(read, 3, 0), vec![false]);
        });
    }
}
