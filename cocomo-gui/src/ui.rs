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

use std::ops::Range;

use cocomo_lib::DirEntryStatus;
use gpui::{
    App, Context, Entity, Focusable, FontWeight, KeyBinding, Render,
    SharedString, UniformListScrollHandle, Window, actions, div, prelude::*,
    px, rgb, uniform_list,
};

use crate::state::{AppState, StatusSummary};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    folder_compare,
    [SelectNext, SelectPrev, EnterDir, LeaveDir, Reload]
);

// ---------------------------------------------------------------------------
// Key bindings
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
pub struct FolderCompareView {
    /// Handle to the application state.
    state: Entity<AppState>,
    /// Scroll handle for the entry list.
    scroll_handle: UniformListScrollHandle,
    /// Whether key bindings have been registered.
    bindings_registered: bool,
}

impl FolderCompareView {
    /// Create a new folder compare view.
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Trigger auto-load of comparison if not already loaded.
        state.update(cx, |state, cx| {
            state.trigger_auto_load(cx);
        });

        Self {
            state,
            scroll_handle: UniformListScrollHandle::new(),
            bindings_registered: false,
        }
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

    /// Handle the enter-directory action.
    fn enter_dir(
        &mut self,
        _: &EnterDir,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.navigate_into_subdir(cx);
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
}

impl Render for FolderCompareView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Register key bindings on first render.
        if !self.bindings_registered {
            self.bindings_registered = true;
            cx.bind_keys(folder_compare_bindings());
        }

        // Focus the app state for keyboard navigation.
        window.focus(&self.state.focus_handle(cx));

        div()
            .key_context("FolderCompare")
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .text_sm()
            // Header bar
            .child(self.render_header(cx))
            // Main content area
            .child(self.render_content(window, cx))
            // Status bar
            .child(self.render_status_bar(cx))
            // Action handlers
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::enter_dir))
            .on_action(cx.listener(Self::leave_dir))
            .on_action(cx.listener(Self::reload))
    }
}

impl FolderCompareView {
    /// Render the header bar showing paths.
    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let left_path = state.left_path().to_string_lossy().to_string();
        let right_path = state.right_path().to_string_lossy().to_string();

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_1()
            .bg(rgb(0x181825))
            .border_b_1()
            .border_color(rgb(0x313244))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(0x89b4fa))
                            .child("cocomo"),
                    )
                    .child(div().text_color(rgb(0xa6adc8)).child("/"))
                    .child(
                        div()
                            .truncate()
                            .max_w(px(400.))
                            .text_color(rgb(0xa6e3a1))
                            .child(left_path),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .truncate()
                            .max_w(px(400.))
                            .text_color(rgb(0xf38ba8))
                            .child(right_path),
                    )
                    .child(div().text_color(rgb(0xa6adc8)).child("/"))
                    .child(
                        div()
                            .font_weight(FontWeight(600.))
                            .text_color(rgb(0x89b4fa))
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

        // Clone data needed inside the closure.
        let state_weak = self.state.downgrade();
        let scroll_handle = self.scroll_handle.clone();

        div()
            .flex()
            .flex_1()
            .overflow_hidden()
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x313244))
            .bg(rgb(0x1e1e2e))
            // Column headers
            .child(render_column_headers())
            // Entry list
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when(is_loading, |this| {
                        this.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(rgb(0x6c7086))
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
                                    let mut items =
                                        Vec::with_capacity(range.end - range.start);
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
                                                    items
                                                        .push(EntryRow::new(
                                                            i,
                                                            entry,
                                                            i
                                                                == selected,
                                                        ));
                                                }
                                            }
                                        }
                                    }
                                    items
                                },
                            )
                            .size_full()
                            .track_scroll(scroll_handle),
                        )
                    }),
            )
    }

    /// Render the status bar at the bottom.
    fn render_status_bar(
        &mut self,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let state = self.state.read(cx);

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
            rgb(0x89b4fa)
        } else if state.error().is_some() {
            rgb(0xf38ba8)
        } else if state.comparison().is_some() {
            rgb(0xa6adc8)
        } else {
            rgb(0x6c7086)
        };

        div()
            .flex()
            .justify_between()
            .items_center()
            .px_3()
            .py_0p5()
            .bg(rgb(0x181825))
            .border_t_1()
            .border_color(rgb(0x313244))
            .text_xs()
            .child(div().text_color(text_color).child(status_text))
            .child(
                div()
                    .text_color(rgb(0x6c7086))
                    .child("j/k navigate  Enter open  Backspace up  r reload"),
            )
    }
}

/// Render the column header row.
fn render_column_headers() -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .px_2()
        .py_1()
        .bg(rgb(0x181825))
        .border_b_1()
        .border_color(rgb(0x313244))
        .text_xs()
        .text_color(rgb(0x6c7086))
        .font_weight(FontWeight(600.))
        .child(div().w(px(16.)).child("sel"))
        .child(div().w(px(16.)).child("st"))
        .child(div().w(px(16.)).child("ty"))
        .child(div().flex_1().child("name"))
        .child(div().w(px(80.)).child("size"))
        .child(div().w(px(60.)).child("left"))
        .child(div().w(px(60.)).child("right"))
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
        let is_dir = entry.left.as_ref().map_or(false, |l| l.is_dir)
            || entry.right.as_ref().map_or(false, |r| r.is_dir);

        Self {
            index,
            name: SharedString::from(entry.name.clone()),
            status: entry.status.clone(),
            is_dir,
            is_selected,
            left_size,
            right_size,
            has_left,
            has_right,
        }
    }
}

/// A single row in the entry list, showing one comparison entry.
#[derive(IntoElement)]
struct EntryRow {
    data: EntryRowData,
}

impl EntryRow {
    fn new(
        index: usize,
        entry: &cocomo_lib::DirEntry,
        is_selected: bool,
    ) -> Self {
        Self {
            data: EntryRowData::new(index, entry, is_selected),
        }
    }
}

impl gpui::RenderOnce for EntryRow {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
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
        } = self.data;

        let status_color = status_color(&status);
        let status_char = status_indicator(&status);
        let dir_icon = if is_dir { "D" } else { "F" };
        let selection_indicator = if is_selected { "► " } else { "  " };

        let bg_color = if is_selected {
            rgb(0x313244)
        } else if index % 2 == 0 {
            rgb(0x1e1e2e)
        } else {
            rgb(0x1a1a28)
        };

        // Format size display.
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
                    .w(px(16.))
                    .text_color(rgb(0x89b4fa))
                    .child(selection_indicator),
            )
            .child(
                div()
                    .w(px(16.))
                    .text_color(status_color)
                    .font_weight(FontWeight(700.))
                    .child(status_char),
            )
            .child(div().w(px(16.)).text_color(rgb(0x6c7086)).child(dir_icon))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(if has_left || has_right {
                        rgb(0xcdd6f4)
                    } else {
                        rgb(0x6c7086)
                    })
                    .child(name),
            )
            .child(
                div()
                    .w(px(80.))
                    .text_color(rgb(0x6c7086))
                    .child(size_display),
            )
            .child(
                div()
                    .w(px(60.))
                    .text_color(if has_left {
                        rgb(0xa6e3a1)
                    } else {
                        rgb(0x6c7086)
                    })
                    .child(if has_left { "yes" } else { "—" }),
            )
            .child(
                div()
                    .w(px(60.))
                    .text_color(if has_right {
                        rgb(0xf38ba8)
                    } else {
                        rgb(0x6c7086)
                    })
                    .child(if has_right { "yes" } else { "—" }),
            )
    }
}

/// Get the status color for an entry.
fn status_color(status: &DirEntryStatus) -> gpui::Hsla {
    match status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => gpui::green(),
        DirEntryStatus::Similar => rgb(0xFFAA00).into(),
        DirEntryStatus::Different => rgb(0xFF4444).into(),
        DirEntryStatus::LeftOnly => rgb(0x44AAFF).into(),
        DirEntryStatus::RightOnly => rgb(0xFF6644).into(),
        DirEntryStatus::CenterOnly => rgb(0xAA44FF).into(),
        DirEntryStatus::Mergeable => rgb(0x44FFAA).into(),
        DirEntryStatus::Conflict => rgb(0xFF0000).into(),
        DirEntryStatus::IdenticalNameDifferentType => rgb(0xFF8800).into(),
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
