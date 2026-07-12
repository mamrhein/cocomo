// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Side-by-side text diff view.
//!
//! Displays a line-by-line comparison of two text files with status
//! indicators. Opens as a new session tab when a file is double-clicked
//! in the folder compare view.

use std::path::PathBuf;

use cocomo_lib::{LineInfo, TextCompareSettings, TextDiff, compare_texts};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Render, SharedString,
    Window, actions, div, prelude::*, px, rgb,
};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(text_diff, [SelectNext, SelectPrev]);

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Application state for a text diff session.
pub struct TextDiffState {
    /// Focus handle for keyboard navigation.
    focus_handle: FocusHandle,
    /// Window / tab title.
    title: SharedString,
    /// Left file path.
    left_path: PathBuf,
    /// Right file path.
    right_path: PathBuf,
    /// Text diff result.
    diff: Option<TextDiff>,
    /// Left file lines.
    left_lines: Vec<LineInfo>,
    /// Right file lines.
    right_lines: Vec<LineInfo>,
    /// Selected row index in the diff list.
    selected_index: usize,
    /// Whether the diff is currently being computed.
    loading: bool,
    /// Error message, if any.
    error: Option<String>,
}

#[allow(dead_code)]
impl TextDiffState {
    /// Create a new text diff state for the given file paths.
    pub fn new(
        left_path: PathBuf,
        right_path: PathBuf,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left_path,
            right_path,
            diff: None,
            left_lines: Vec::new(),
            right_lines: Vec::new(),
            selected_index: 0,
            loading: false,
            error: None,
        }
    }

    /// Return the focus handle.
    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Return the tab title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Whether the diff is currently being computed.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Return the error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Return the diff result, if loaded.
    pub fn diff(&self) -> Option<&TextDiff> {
        self.diff.as_ref()
    }

    /// Return the left lines.
    pub fn left_lines(&self) -> &[LineInfo] {
        &self.left_lines
    }

    /// Return the right lines.
    pub fn right_lines(&self) -> &[LineInfo] {
        &self.right_lines
    }

    /// Return the selected index.
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// Navigate to the next diff row.
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        let max = self
            .diff
            .as_ref()
            .map_or(0, |d| d.differences.len().saturating_sub(1));
        if self.selected_index < max {
            self.selected_index += 1;
            cx.notify();
        }
    }

    /// Navigate to the previous diff row.
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            cx.notify();
        }
    }

    /// Compute the text diff asynchronously.
    pub fn load_diff(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }

        self.loading = true;
        self.error = None;
        self.diff = None;
        self.left_lines.clear();
        self.right_lines.clear();
        self.selected_index = 0;
        cx.notify();

        let left_path = self.left_path.clone();
        let right_path = self.right_path.clone();
        let _entity = cx.entity().downgrade();

        let task = cx.background_spawn(async move {
            let left_content = tokio::fs::read_to_string(&left_path)
                .await
                .unwrap_or_default();
            let right_content = tokio::fs::read_to_string(&right_path)
                .await
                .unwrap_or_default();

            let settings = TextCompareSettings::new();

            let left_lines: Vec<LineInfo> = left_content
                .lines()
                .enumerate()
                .map(|(i, line)| LineInfo::new(i + 1, line.to_string()))
                .collect();

            let right_lines: Vec<LineInfo> = right_content
                .lines()
                .enumerate()
                .map(|(i, line)| LineInfo::new(i + 1, line.to_string()))
                .collect();

            let left_text = left_lines
                .iter()
                .map(|l| l.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let right_text = right_lines
                .iter()
                .map(|l| l.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            let diff = compare_texts(&left_text, &right_text, &settings);

            (diff, left_lines, right_lines)
        });

        cx.spawn(
            |this: gpui::WeakEntity<TextDiffState>,
             cx: &mut gpui::AsyncApp| {
                let async_app = cx.clone();
                async move {
                    let result = task.await;
                    async_app.update(|cx| {
                        if let Some(state) = this.upgrade() {
                            state.update(cx, |state, _| {
                                state.loading = false;
                                let (diff, left_lines, right_lines) = result;
                                state.diff = Some(diff);
                                state.left_lines = left_lines;
                                state.right_lines = right_lines;
                            });
                        }
                    });
                }
            },
        )
        .detach();
    }

    /// Trigger auto-load if not already loaded.
    pub fn trigger_auto_load(&mut self, cx: &mut Context<Self>) {
        if !self.loading && self.diff.is_none() && self.error.is_none() {
            self.load_diff(cx);
        }
    }
}

impl gpui::Focusable for TextDiffState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// Side-by-side text diff view.
pub struct TextDiffView {
    /// Handle to the diff state.
    state: Entity<TextDiffState>,
    /// Whether key bindings have been registered.
    bindings_registered: bool,
}

impl TextDiffView {
    /// Create a new text diff view.
    pub fn new(state: Entity<TextDiffState>, cx: &mut Context<Self>) -> Self {
        state.update(cx, |s, cx| {
            s.trigger_auto_load(cx);
        });

        Self {
            state,
            bindings_registered: false,
        }
    }

    /// Return the focus handle.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx).clone()
    }

    /// Return the tab title.
    pub fn title(&self, cx: &App) -> SharedString {
        self.state.read(cx).title().clone()
    }

    /// Handle select-next action.
    fn select_next(
        &mut self,
        _: &SelectNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.select_next(cx);
        });
    }

    /// Handle select-previous action.
    fn select_prev(
        &mut self,
        _: &SelectPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.select_previous(cx);
        });
    }
}

impl Render for TextDiffView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !self.bindings_registered {
            self.bindings_registered = true;
            cx.bind_keys(vec![
                gpui::KeyBinding::new("j", SelectNext, None),
                gpui::KeyBinding::new("k", SelectPrev, None),
                gpui::KeyBinding::new("down", SelectNext, None),
                gpui::KeyBinding::new("up", SelectPrev, None),
            ]);
        }

        window.focus(&self.state.read(cx).focus_handle(cx), cx);

        let state = self.state.read(cx);
        let title = state.title().to_string();

        div()
            .key_context("TextDiff")
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .text_xs()
            // Title bar.
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_1()
                    .bg(rgb(0x181825))
                    .border_b_1()
                    .border_color(rgb(0x313244))
                    .child(SharedString::from(title))
                    .child(
                        div()
                            .text_color(rgb(0x6c7086))
                            .child(SharedString::from("text diff")),
                    ),
            )
            // Content area.
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .p_2()
                    .child(self.render_content(cx)),
            )
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
    }
}

impl TextDiffView {
    /// Render the content area (loading, error, or diff).
    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        // Clone state data to avoid borrow issues.
        let state = self.state.read(cx);
        let loading = state.is_loading();
        let error = state.error().map(|s| s.to_string());
        let left_lines: Vec<(usize, String)> = state
            .left_lines()
            .iter()
            .map(|l| (l.number, l.content.clone()))
            .collect();
        let right_lines: Vec<(usize, String)> = state
            .right_lines()
            .iter()
            .map(|l| (l.number, l.content.clone()))
            .collect();
        let has_diff = state.diff().is_some();
        let selected_index = state.selected_index();

        if loading {
            div()
                .text_color(rgb(0x6c7086))
                .child(SharedString::from("loading diff..."))
        } else if let Some(err) = error {
            div()
                .text_color(rgb(0xf38ba8))
                .child(SharedString::from(err))
        } else if !has_diff {
            div()
                .text_color(rgb(0xa6e3a1))
                .child(SharedString::from("files are identical"))
        } else {
            div()
                .flex()
                .size_full()
                .gap_2()
                .child(Self::render_pane("left", &left_lines, selected_index))
                .child(Self::render_pane(
                    "right",
                    &right_lines,
                    selected_index,
                ))
        }
    }

    /// Render a single pane (left or right) as a simple list.
    fn render_pane(
        label: &str,
        lines: &[(usize, String)],
        selected_index: usize,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .bg(rgb(0x11111b))
            .rounded_md()
            .border_1()
            .border_color(rgb(0x313244))
            .overflow_hidden()
            // Header.
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .py_0p5()
                    .bg(rgb(0x181825))
                    .border_b_1()
                    .border_color(rgb(0x313244))
                    .child(SharedString::from(label))
                    .child(SharedString::from(format!(
                        "{} lines",
                        lines.len()
                    ))),
            )
            // Lines.
            .child(div().flex_1().overflow_hidden().children(
                lines.iter().enumerate().map(|(i, (line_num, content))| {
                    let bg = if i == selected_index {
                        rgb(0x313244)
                    } else {
                        rgb(0x1e1e2e)
                    };
                    div()
                        .flex()
                        .w_full()
                        .py_0p5()
                        .bg(bg)
                        .child(
                            div()
                                .w(px(40.))
                                .text_right()
                                .px_1()
                                .text_color(rgb(0x6c7086))
                                .font(gpui::Font::default())
                                .child(SharedString::from(format!(
                                    "{:>4}",
                                    line_num
                                ))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .px_1()
                                .overflow_hidden()
                                .font(gpui::Font::default())
                                .child(SharedString::from(content)),
                        )
                }),
            ))
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create a new text diff view entity for the given file paths.
pub fn new_text_diff_view(
    left_path: PathBuf,
    right_path: PathBuf,
    _grammar: Option<cocomo_lib::Grammar>,
    cx: &mut App,
) -> Entity<TextDiffView> {
    let left_name = left_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "left".to_string());
    let right_name = right_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "right".to_string());
    let title = format!("{left_name} vs {right_name}");

    let state = cx.new(|cx| {
        TextDiffState::new(
            left_path,
            right_path,
            SharedString::from(title),
            cx,
        )
    });

    cx.new(|cx| TextDiffView::new(state, cx))
}
