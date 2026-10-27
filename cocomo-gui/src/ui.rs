// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! UI components for the folder compare view.
//!
//! This module provides the directory comparison view, including
//! status indicators, navigation, and entry rendering.

use std::{ops::Range, path::PathBuf};

use cocomo_lib::{DirEntryStatus, Endpoint};
use gpui::{
    Action, App, Context, Entity, FocusHandle, FontWeight, Hsla, KeyBinding,
    Render, SharedString, UniformListScrollHandle, Window, actions,
    component::{ActiveTheme, Theme},
    div,
    prelude::*,
    uniform_list,
};

use crate::{
    menus::{ReloadCompare, SaveSession},
    session_manager::GuiSessionManager,
    state::{AppState, StatusSummary},
};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    folder_compare,
    [SelectNext, SelectPrev, EnterDir, LeaveDir, Reload]
);

/// Action to open a text diff for a file in a new tab.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = folder_compare, no_json)]
pub struct OpenFileDiff {
    /// Left file endpoint.
    pub left_path: Endpoint,
    /// Right file endpoint (optional).
    pub right_path: Option<Endpoint>,
}

/// Action to open a directory compare in a new tab.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = folder_compare, no_json)]
pub struct OpenDirDiff {
    /// Left directory endpoint.
    pub left_path: Endpoint,
    /// Right directory endpoint.
    pub right_path: Endpoint,
}
// ---------------------------------------------------------------------------

/// Register key bindings for folder comparison navigation.
pub fn folder_compare_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("j", SelectNext, None),
        KeyBinding::new("k", SelectPrev, None),
        KeyBinding::new("down", SelectNext, None),
        KeyBinding::new("up", SelectPrev, None),
        KeyBinding::new("enter", EnterDir, None),
        KeyBinding::new("backspace", LeaveDir, None),
        KeyBinding::new("r", Reload, None),
    ]
}

// ---------------------------------------------------------------------------
// Folder Compare View
// ---------------------------------------------------------------------------

/// The main folder comparison view.
///
/// Renders only its own regions (path header, entry list, status bar).
/// The window chrome (tab bar, toolbar) and keyboard focus are owned by
/// [`crate::tabview::WindowRoot`]; this view never requests focus from
/// `render`.
pub struct FolderCompareView {
    /// Handle to the application state.
    state: Entity<AppState>,
    /// Handle to the session manager.
    session_manager: Entity<GuiSessionManager>,
    /// Scroll handle for the entry list.
    scroll_handle: UniformListScrollHandle,
}

impl FolderCompareView {
    /// Create a new folder compare view.
    pub fn new(
        state: Entity<AppState>,
        session_manager: Entity<GuiSessionManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Trigger auto-load of comparison if not already loaded.
        state.update(cx, |state, cx| {
            state.trigger_auto_load(cx);
        });

        Self {
            state,
            session_manager,
            scroll_handle: UniformListScrollHandle::new(),
        }
    }

    /// Return the focus handle for keyboard navigation.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx).clone()
    }

    /// Handle the select-next action.
    fn select_next(
        &mut self,
        _: &SelectNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.select_next(cx);
        });
    }

    /// Handle the select-previous action.
    fn select_prev(
        &mut self,
        _: &SelectPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.select_previous(cx);
        });
    }

    /// Handle the enter-directory action: open a new dir-diff tab.
    fn enter_dir(
        &mut self,
        _: &EnterDir,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entry = match self.state.read(cx).selected_entry() {
            Some(e) => e,
            None => return,
        };

        // Determine sub-endpoints for both sides. An entry's `path` is
        // relative to the current comparison root, so it must be joined onto
        // the endpoint path rather than used as the endpoint path directly
        // (which would drop the root and resolve to a non-existent location).
        let left_sub = match &entry.left {
            Some(left_info) => {
                self.state.read(cx).left().join(&left_info.path)
            }
            None => self.state.read(cx).left().join(&entry.name),
        };
        let right_sub = match &entry.right {
            Some(right_info) => {
                self.state.read(cx).right().join(&right_info.path)
            }
            None => self.state.read(cx).right().join(&entry.name),
        };

        // Dispatch action to open a new dir-diff tab.
        cx.dispatch_action(&crate::ui::OpenDirDiff {
            left_path: left_sub,
            right_path: right_sub,
        });
    }

    /// Handle the leave-directory action.
    fn leave_dir(
        &mut self,
        _: &LeaveDir,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.navigate_up(cx);
        });
    }

    /// Handle the reload action.
    fn reload(&mut self, _: &Reload, _: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.reload(cx);
        });
    }

    /// Handle the ReloadCompare action dispatched from menus or toolbar.
    fn handle_reload_compare(
        &mut self,
        _: &ReloadCompare,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.reload(cx);
        });
    }

    /// Handle the SaveSession action dispatched from menus or toolbar.
    fn handle_save_session(
        &mut self,
        _: &SaveSession,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.state.read(cx).title().to_string();
        let config = self.state.read(cx).to_config(name.clone());

        self.session_manager.update(cx, |mgr, cx| {
            mgr.update_active_config(config, cx);
        });
    }

    /// Handle OpenFileDiff: dispatch to session manager to create a text diff
    /// tab.
    fn handle_open_file_diff(
        &mut self,
        action: &OpenFileDiff,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let right_path = action.right_path.clone().unwrap_or_else(|| {
            // Fallback: use the same file on the other side.
            action.left_path.clone()
        });
        self.session_manager.update(cx, |mgr, cx| {
            mgr.add_text_diff_tab(action.left_path.clone(), right_path, cx);
        });
    }

    /// Handle OpenDirDiff: dispatch to session manager to create a dir diff
    /// tab.
    fn handle_open_dir_diff(
        &mut self,
        action: &OpenDirDiff,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.session_manager.update(cx, |mgr, cx| {
            mgr.add_dir_diff_tab(
                action.left_path.clone(),
                action.right_path.clone(),
                cx,
            );
        });
    }

    // -----------------------------------------------------------------------
    // Session callbacks (used by TabBar)
    // -----------------------------------------------------------------------

    /// Save the current session.
    #[allow(dead_code)]
    fn save_session(&mut self, cx: &mut Context<Self>) {
        let name = self.state.read(cx).title().to_string();
        let config = self.state.read(cx).to_config(name.clone());

        self.session_manager.update(cx, |mgr, cx| {
            mgr.update_active_config(config, cx);
        });
    }
}

impl Render for FolderCompareView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Key bindings are registered globally at startup; keyboard focus is
        // acquired by `WindowRoot` on tab activation. Tracking the focus
        // handle here makes this region (with its `key_context` and
        // `on_action` handlers) the focused region for keyboard input.
        let focus_handle = self.focus_handle(cx);

        div()
            .key_context("FolderCompare")
            .track_focus(&focus_handle)
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            // Header bar
            .child(self.render_header(cx))
            // Main content area
            .child(self.render_content(window, cx))
            // Status bar
            .child(self.render_status_bar(cx))
            // View-scoped action handlers (keyboard shortcuts).
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::enter_dir))
            .on_action(cx.listener(Self::leave_dir))
            .on_action(cx.listener(Self::reload))
            // Menu-dispatched actions that need view context.
            .on_action(cx.listener(Self::handle_reload_compare))
            .on_action(cx.listener(Self::handle_save_session))
            // File/dir diff actions.
            .on_action(cx.listener(Self::handle_open_file_diff))
            .on_action(cx.listener(Self::handle_open_dir_diff))
    }
}

impl FolderCompareView {
    /// Render the header bar showing paths.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let left_path = state.left().display();
        let right_path = state.right().display();
        let theme = cx.theme();

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_1()
            .bg(theme.muted)
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .font_weight(FontWeight(600.))
                            .text_color(theme.muted_foreground)
                            .child("cocomo"),
                    )
                    .child(div().text_color(theme.muted_foreground).child("/"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(left_path),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.foreground)
                            .child(right_path),
                    )
                    .child(div().text_color(theme.muted_foreground).child("/"))
                    .child(
                        div()
                            .font_weight(FontWeight(600.))
                            .text_color(theme.muted_foreground)
                            .child("compare"),
                    ),
            )
    }

    /// Render the main content area with the entry list.
    fn render_content(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.read(cx);
        let entry_count = state.entry_count();
        let is_loading = state.is_loading();
        let theme = cx.theme();

        // Clone data needed inside the closure.
        let state_weak = self.state.downgrade();
        let scroll_handle = self.scroll_handle.clone();

        div()
            .flex()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            // Column headers
            .child(render_column_headers(theme))
            // Entry list
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
                                .text_color(theme.muted_foreground)
                                .child("Loading comparison..."),
                        )
                    })
                    .when(!is_loading, |this| {
                        this.child(
                            uniform_list(
                                "entries",
                                entry_count,
                                move |range: Range<usize>,
                                      _: &mut Window,
                                      app: &mut App| {
                                    let mut items: Vec<gpui::AnyElement> =
                                        Vec::with_capacity(range.end - range.start);
                                    let theme = app.theme();
                                    if let Some(state_entity) =
                                        state_weak.upgrade()
                                    {
                                        let app_state =
                                            state_entity.read(app);
                                        if let Some(comparison) =
                                            app_state.comparison()
                                        {
                                            let selected =
                                                app_state.selected_index();
                                            for i in range {
                                                if let Some(entry) =
                                                    comparison
                                                        .entries
                                                        .get(i)
                                                {
                                                    items.push(
                                                        EntryRowData::new(
                                                            i,
                                                            entry,
                                                            i == selected,
                                                        )
                                                        .render_row(theme)
                                                        .into_any_element(),
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    items
                                },
                            )
                            .size_full()
                            .track_scroll(&scroll_handle),
                        )
                    }),
            )
    }

    /// Render the status bar at the bottom.
    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let theme = cx.theme();

        let status_text: SharedString = if state.is_loading() {
            SharedString::from("Loading comparison...")
        } else if let Some(err) = state.error() {
            SharedString::from(err.to_string())
        } else if let Some(comparison) = state.comparison() {
            let summary = StatusSummary::from_comparison(comparison);
            SharedString::from(format!(
                "{} entries  |  {} same  |  {} different  |  {} orphan",
                summary.total,
                summary.same,
                summary.different,
                summary.orphans
            ))
        } else {
            SharedString::from("Ready. Press r to compare.")
        };

        let text_color = if state.is_loading() {
            theme.muted_foreground
        } else if state.error().is_some() {
            theme.danger
        } else if state.comparison().is_some() {
            theme.secondary_foreground
        } else {
            theme.muted_foreground
        };

        div()
            .flex()
            .justify_between()
            .items_center()
            .px_3()
            .py_0p5()
            .bg(theme.status_bar)
            .border_t_1()
            .border_color(theme.status_bar_border)
            .text_xs()
            .child(div().text_color(text_color).child(status_text))
            .child(
                div()
                    .text_color(theme.muted_foreground)
                    .child("j/k navigate  Enter open  Backspace up  r reload"),
            )
    }
}

/// Render the column header row.
///
/// The fixed lane widths are shared with [`EntryRowData::render_row`] so
/// headers and values stay aligned; they are functional data lanes, not
/// decorative sizes.
fn render_column_headers(theme: &Theme) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .bg(theme.muted)
        .border_b_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted_foreground)
        .font_weight(FontWeight(600.))
        .child(div().w_4().child("sel"))
        .child(div().w_4().child("st"))
        .child(div().w_4().child("ty"))
        .child(div().flex_1().child("name"))
        .child(div().w_20().child("size"))
        .child(div().w_16().child("left"))
        .child(div().w_16().child("right"))
}

// ---------------------------------------------------------------------------
// Entry Row
// ---------------------------------------------------------------------------

/// Data for a single row in the entry list.
#[derive(Clone)]
struct EntryRowData {
    /// Row index in the full list.
    index: usize,
    /// Entry name.
    name: SharedString,
    /// Entry status.
    status: DirEntryStatus,
    /// Whether this is a directory.
    is_dir: bool,
    /// Whether this row is selected.
    is_selected: bool,
    /// Size from left side, if available.
    left_size: Option<u64>,
    /// Size from right side, if available.
    right_size: Option<u64>,
    /// Whether entry exists on left side.
    has_left: bool,
    /// Whether entry exists on right side.
    has_right: bool,
    /// Left file path (for double-click to open text diff).
    left_path: Option<PathBuf>,
    /// Right file path (for double-click to open text diff).
    right_path: Option<PathBuf>,
}

impl EntryRowData {
    /// Create entry row data from a comparison entry.
    fn new(
        index: usize,
        entry: &cocomo_lib::DirEntry,
        is_selected: bool,
    ) -> Self {
        let has_left = entry.left.is_some();
        let has_right = entry.right.is_some();
        let left_size = entry.left.as_ref().map(|l| l.size);
        let right_size = entry.right.as_ref().map(|r| r.size);
        let is_dir = entry.left.as_ref().is_some_and(|l| l.is_dir)
            || entry.right.as_ref().is_some_and(|r| r.is_dir);
        let left_path = entry.left.as_ref().map(|l| PathBuf::from(&l.path));
        let right_path = entry.right.as_ref().map(|r| PathBuf::from(&r.path));

        Self {
            index,
            name: SharedString::from(entry.name.clone()),
            status: entry.status,
            is_dir,
            is_selected,
            left_size,
            right_size,
            has_left,
            has_right,
            left_path,
            right_path,
        }
    }
}

impl EntryRowData {
    /// Render a row as a div (for use in uniform_list).
    fn render_row(self, theme: &Theme) -> impl IntoElement {
        let EntryRowData {
            index,
            name,
            status,
            is_dir,
            is_selected,
            left_size,
            right_size,
            has_left,
            has_right,
            left_path: _left_path,
            right_path: _right_path,
        } = self;

        let status_char = status_indicator(&status);
        let dir_icon = if is_dir { "D" } else { "F" };
        let selection_indicator = if is_selected { "► " } else { "  " };

        let bg_color = if is_selected {
            theme.accent
        } else if index % 2 == 0 {
            theme.background
        } else {
            theme.muted
        };

        let size_display = if has_left && has_right {
            if left_size == right_size {
                format_size(left_size.unwrap_or(0))
            } else {
                format!(
                    "{}/{}",
                    format_size(left_size.unwrap_or(0)),
                    format_size(right_size.unwrap_or(0))
                )
            }
        } else if has_left || has_right {
            format_size(left_size.or(right_size).unwrap_or(0))
        } else {
            String::new()
        };

        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .bg(bg_color)
            .cursor_pointer()
            .child(
                div()
                    .w_4()
                    .text_color(theme.accent_foreground)
                    .child(selection_indicator),
            )
            .child(
                div()
                    .w_4()
                    .text_color(status_role(&status).color(theme))
                    .font_weight(FontWeight(700.))
                    .child(status_char),
            )
            .child(
                div()
                    .w_4()
                    .text_color(theme.muted_foreground)
                    .child(dir_icon),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(if has_left || has_right {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(name),
            )
            .child(
                div()
                    .w_20()
                    .text_color(theme.muted_foreground)
                    .child(size_display),
            )
            .child(
                div()
                    .w_16()
                    .text_color(if has_left {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(if has_left { "yes" } else { "—" }),
            )
            .child(
                div()
                    .w_16()
                    .text_color(if has_right {
                        theme.foreground
                    } else {
                        theme.muted_foreground
                    })
                    .child(if has_right { "yes" } else { "—" }),
            )
    }
}

/// Semantic color role for a directory entry status.
///
/// Statuses map onto a small set of semantic roles instead of a fixed RGB
/// palette, so entry colors follow the active theme (light, dark, and
/// custom). The glyph from [`status_indicator`] remains as a secondary,
/// color-independent cue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusRole {
    /// Entry matches on both sides.
    Success,
    /// Entry differs in a way that may still be benign.
    Warning,
    /// Entry differs or cannot be reconciled.
    Danger,
    /// Entry exists on only one side, or needs a three-way decision.
    Info,
}

impl StatusRole {
    /// Resolve this role to a color from the active theme.
    fn color(self, theme: &Theme) -> Hsla {
        match self {
            Self::Success => theme.success,
            Self::Warning => theme.warning,
            Self::Danger => theme.danger,
            Self::Info => theme.info,
        }
    }
}

/// Map an entry status to its semantic color role.
fn status_role(status: &DirEntryStatus) -> StatusRole {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => {
            StatusRole::Success
        }
        DirEntryStatus::Similar
        | DirEntryStatus::IdenticalNameDifferentType => StatusRole::Warning,
        DirEntryStatus::Different | DirEntryStatus::Conflict => {
            StatusRole::Danger
        }
        DirEntryStatus::LeftOnly
        | DirEntryStatus::RightOnly
        | DirEntryStatus::CenterOnly
        | DirEntryStatus::Mergeable => StatusRole::Info,
    }
}

/// Get the status indicator character for an entry.
fn status_indicator(status: &DirEntryStatus) -> &'static str {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => "=",
        DirEntryStatus::Similar => "~",
        DirEntryStatus::Different => "!",
        DirEntryStatus::LeftOnly => "<",
        DirEntryStatus::RightOnly => ">",
        DirEntryStatus::CenterOnly => "^",
        DirEntryStatus::Mergeable => "M",
        DirEntryStatus::Conflict => "C",
        DirEntryStatus::IdenticalNameDifferentType => "T",
    }
}

/// Format a file size in human-readable form.
fn format_size(bytes: u64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_role_maps_every_status() {
        assert_eq!(status_role(&DirEntryStatus::Same), StatusRole::Success);
        assert_eq!(
            status_role(&DirEntryStatus::SameBinary),
            StatusRole::Success
        );
        assert_eq!(status_role(&DirEntryStatus::Similar), StatusRole::Warning);
        assert_eq!(
            status_role(&DirEntryStatus::IdenticalNameDifferentType),
            StatusRole::Warning
        );
        assert_eq!(
            status_role(&DirEntryStatus::Different),
            StatusRole::Danger
        );
        assert_eq!(status_role(&DirEntryStatus::Conflict), StatusRole::Danger);
        assert_eq!(status_role(&DirEntryStatus::LeftOnly), StatusRole::Info);
        assert_eq!(status_role(&DirEntryStatus::RightOnly), StatusRole::Info);
        assert_eq!(status_role(&DirEntryStatus::CenterOnly), StatusRole::Info);
        assert_eq!(status_role(&DirEntryStatus::Mergeable), StatusRole::Info);
    }
}
