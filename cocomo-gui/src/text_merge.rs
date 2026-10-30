// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Three-way text merge view (UI spec section 6).
//!
//! [`MergeSession`] owns one merge: the loaded texts of the left, right, and
//! optional ancestor files, the [`MergeResult`] from `cocomo_lib`, the
//! per-section resolutions, and the editable output model. [`MergeView`]
//! renders the sources as read-only panes (left, ancestor, right) with
//! section-level change bars, an editable output pane below with conflict
//! fills, a row-2 toolbar for the take commands, and a status bar with the
//! conflict count and current section.
//!
//! The output is constructed automatically from the merge; conflicts
//! tentatively contribute the left side. Taking a section records a
//! resolution and rebuilds the output. Manual edits in the output pane
//! invalidate the section-to-output mapping, so the take commands stay
//! disabled until the next reload or take.

use std::{collections::HashMap, sync::Arc, time::Instant};

use cocomo_lib::{
    Endpoint, FsError, LineEnding, MergeKind, MergeSection, NodeFileSystem,
    TextEncoding, merge_texts,
};
use gpui::{
    Action, AnyElement, App, Context, ElementId, Entity, FocusHandle, Hsla,
    IntoElement, KeyBinding, ListAlignment, ListState, MouseButton, Pixels,
    Render, SharedString, Subscription, Window, actions,
    assets::IconName,
    base::Disableable as _,
    component::{
        ActiveTheme, Sizable as _, Theme, WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{
            Editor, EditorState, InputEvent, RangeDecoration,
            RangeDecorationCollection, RangeDecorationStyle,
        },
        menu::ContextMenuExt as _,
        resizable::{h_resizable, resizable_panel, v_resizable},
        spinner::Spinner,
        status_bar::StatusBar,
        toolbar::{Toolbar, ToolbarGroup},
        v_flex,
    },
    div, list,
    prelude::*,
    px,
};

use crate::{
    log::{self, LogLevel},
    menus::{ReloadCompare, ToggleMergeAncestor, ToggleWordWrap},
    session_manager::GuiSessionManager,
    text_diff::{decode_bytes, encode_text, normalize_endings},
};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    text_merge,
    [
        TakeLeft,
        TakeRight,
        TakeAncestor,
        NextConflict,
        PrevConflict,
        NextMergeSection,
        PrevMergeSection,
        SaveMergeOutput,
    ]
);

/// Which side's lines a merge section should contribute to the output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeChoice {
    /// Take the left side's lines.
    Left,
    /// Take the right side's lines.
    Right,
    /// Take the ancestor's lines (three-way merges only).
    Ancestor,
}

/// Resolve a specific merge section, from the row context menu.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = text_merge, no_json)]
pub(crate) struct TakeSection {
    /// The section index to resolve.
    pub(crate) section: usize,
    /// The side to take.
    pub(crate) choice: MergeChoice,
}

// ---------------------------------------------------------------------------
// Session model
// ---------------------------------------------------------------------------

/// One three-way (or two-way) text merge session.
pub struct MergeSession {
    /// Focus handle shared with the view.
    focus_handle: FocusHandle,
    /// Tab title.
    title: SharedString,
    /// Left file endpoint.
    left: Endpoint,
    /// Right file endpoint.
    right: Endpoint,
    /// Ancestor file endpoint (three-way merges only).
    ancestor: Option<Endpoint>,
    /// Raw left bytes from the last load.
    left_raw: Option<Vec<u8>>,
    /// Raw right bytes from the last load.
    right_raw: Option<Vec<u8>>,
    /// Raw ancestor bytes from the last load.
    ancestor_raw: Option<Vec<u8>>,
    /// Decoded left lines.
    left_lines: Arc<Vec<String>>,
    /// Decoded right lines.
    right_lines: Arc<Vec<String>>,
    /// Decoded ancestor lines (empty in two-way mode).
    ancestor_lines: Arc<Vec<String>>,
    /// Merge sections in document order.
    sections: Arc<Vec<MergeSection>>,
    /// Left line index → section index.
    left_line_section: Arc<Vec<usize>>,
    /// Right line index → section index.
    right_line_section: Arc<Vec<usize>>,
    /// Ancestor line index → section index.
    ancestor_line_section: Arc<Vec<usize>>,
    /// Section index → first output line of the section.
    section_output_ranges: Arc<Vec<(usize, usize)>>,
    /// Section index → first left line of the section.
    left_starts: Arc<Vec<usize>>,
    /// Section index → first right line of the section.
    right_starts: Arc<Vec<usize>>,
    /// Section index → first ancestor line of the section.
    ancestor_starts: Arc<Vec<usize>>,
    /// Section index → explicit resolution.
    resolutions: HashMap<usize, MergeChoice>,
    /// The constructed output lines.
    output: Vec<String>,
    /// The output editor was edited manually; the section-to-output mapping
    /// is no longer trustworthy, so take commands are disabled.
    output_stale: bool,
    /// The output editor still needs re-seeding from the model.
    output_reseed: bool,
    /// The editable output editor, created by the view.
    output_editor: Option<Entity<EditorState>>,
    /// Subscription tracking the output editor's change events.
    _editor_sub: Option<Subscription>,
    /// Selected section index.
    selected: Option<usize>,
    /// Whether a load is in flight.
    loading: bool,
    /// Last load error, if any.
    error: Option<String>,
    /// Duration of the last successful load, in milliseconds.
    load_ms: Option<u128>,
    /// Word-wrap the source panes and the output editor.
    word_wrap: bool,
    /// Whether the ancestor pane is visible.
    show_ancestor: bool,
    /// Bumps whenever the section or output model changes.
    model_rev: usize,
    /// The session manager, for marking the tab dirty.
    manager: Option<Entity<GuiSessionManager>>,
}

impl MergeSession {
    /// Create a session for the given endpoints; loads on demand.
    pub fn new(
        left: Endpoint,
        right: Endpoint,
        ancestor: Option<Endpoint>,
        title: SharedString,
        manager: Option<Entity<GuiSessionManager>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left,
            right,
            ancestor,
            left_raw: None,
            right_raw: None,
            ancestor_raw: None,
            left_lines: Arc::new(Vec::new()),
            right_lines: Arc::new(Vec::new()),
            ancestor_lines: Arc::new(Vec::new()),
            sections: Arc::new(Vec::new()),
            left_line_section: Arc::new(Vec::new()),
            right_line_section: Arc::new(Vec::new()),
            ancestor_line_section: Arc::new(Vec::new()),
            section_output_ranges: Arc::new(Vec::new()),
            left_starts: Arc::new(Vec::new()),
            right_starts: Arc::new(Vec::new()),
            ancestor_starts: Arc::new(Vec::new()),
            resolutions: HashMap::new(),
            output: Vec::new(),
            output_stale: false,
            output_reseed: false,
            output_editor: None,
            _editor_sub: None,
            selected: None,
            loading: false,
            error: None,
            load_ms: None,
            word_wrap: false,
            show_ancestor: true,
            model_rev: 0,
            manager,
        }
    }

    /// Build a session from in-memory texts, without any file I/O. Used by
    /// tests.
    #[cfg(test)]
    pub(crate) fn from_texts(
        ancestor: Option<&str>,
        left_text: &str,
        right_text: &str,
        title: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut session = Self::new(
            Endpoint::local("/left.txt"),
            Endpoint::local("/right.txt"),
            ancestor.map(|_| Endpoint::local("/ancestor.txt")),
            title,
            None,
            cx,
        );
        session.left_raw = Some(left_text.as_bytes().to_vec());
        session.right_raw = Some(right_text.as_bytes().to_vec());
        session.ancestor_raw = ancestor.map(|a| a.as_bytes().to_vec());
        session.load_ms = Some(1);
        session.rebuild_model();
        session
    }

    /// Return the focus handle.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Return the tab title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Whether a load is in flight.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Return the last error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The status bar load label, e.g. `Loaded in 12 ms`.
    pub fn load_label(&self) -> Option<String> {
        self.load_ms.map(|ms| format!("Loaded in {ms} ms"))
    }

    /// Whether this is a three-way merge (an ancestor was provided).
    pub fn has_ancestor(&self) -> bool {
        self.ancestor.is_some()
    }

    /// Whether the ancestor pane is visible.
    pub fn show_ancestor(&self) -> bool {
        self.show_ancestor && self.ancestor.is_some()
    }

    /// Toggle the ancestor pane visibility.
    pub fn toggle_ancestor(&mut self, cx: &mut Context<Self>) {
        if self.ancestor.is_some() {
            self.show_ancestor = !self.show_ancestor;
            cx.notify();
        }
    }

    /// Whether the sources and output wrap long lines.
    pub fn is_word_wrap(&self) -> bool {
        self.word_wrap
    }

    /// Toggle word wrap across the source panes and the output editor.
    pub fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.word_wrap = !self.word_wrap;
        cx.notify();
    }

    /// The number of merge sections.
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// The number of conflicting sections.
    pub fn conflict_count(&self) -> usize {
        self.sections
            .iter()
            .filter(|s| s.kind() == MergeKind::Conflict)
            .count()
    }

    /// The number of conflicts without an explicit resolution.
    pub fn unresolved_count(&self) -> usize {
        self.sections
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                s.kind() == MergeKind::Conflict
                    && !self.resolutions.contains_key(i)
            })
            .count()
    }

    /// The selected section index.
    #[cfg(test)]
    pub fn selected_section(&self) -> Option<usize> {
        self.selected
    }

    /// Whether a take command can run right now.
    pub fn can_take(&self) -> bool {
        self.selected.is_some() && !self.output_stale && !self.loading
    }

    /// The snapshot the view renders; cheap Arc clones.
    pub fn snapshot(&self) -> MergeSnapshot {
        MergeSnapshot {
            left_lines: self.left_lines.clone(),
            right_lines: self.right_lines.clone(),
            ancestor_lines: self.ancestor_lines.clone(),
            sections: self.sections.clone(),
            left_line_section: self.left_line_section.clone(),
            right_line_section: self.right_line_section.clone(),
            ancestor_line_section: self.ancestor_line_section.clone(),
            selected: self.selected,
            word_wrap: self.word_wrap,
            show_ancestor: self.show_ancestor(),
            has_ancestor: self.has_ancestor(),
        }
    }

    /// The `(left, right, ancestor)` first-line index of a section, for
    /// scrolling the panes to reveal it.
    pub fn section_reveal_lines(
        &self,
        section: usize,
    ) -> (Option<usize>, Option<usize>, Option<usize>) {
        let at = |starts: &Vec<usize>| starts.get(section).copied();
        (
            at(&self.left_starts),
            at(&self.right_starts),
            at(&self.ancestor_starts),
        )
    }

    /// The output byte range of a section, for conflict fills.
    pub fn section_output_byte_range(&self, section: usize) -> (usize, usize) {
        let (lo, hi) = self
            .section_output_ranges
            .get(section)
            .copied()
            .unwrap_or((0, 0));
        let byte_start = output_byte_offset(&self.output, lo);
        let byte_end = output_byte_offset(&self.output, hi);
        (byte_start, byte_end)
    }

    /// The byte ranges of all unresolved conflicts, empty once the output
    /// was edited manually (the ranges would point at the wrong text).
    pub fn conflict_fill_ranges(&self) -> Vec<(usize, usize)> {
        if self.output_stale {
            return Vec::new();
        }
        self.sections
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                s.kind() == MergeKind::Conflict
                    && !self.resolutions.contains_key(i)
            })
            .map(|(i, _)| self.section_output_byte_range(i))
            .collect()
    }

    /// The byte offset of an output line, for revealing it in the editor.
    pub fn output_byte_offset(&self, line: usize) -> usize {
        output_byte_offset(&self.output, line)
    }

    /// The first output line of a section.
    pub fn section_output_start(&self, section: usize) -> Option<usize> {
        self.section_output_ranges.get(section).map(|(lo, _)| *lo)
    }

    // -- Selection and navigation ------------------------------------------

    /// Select a section; returns it for the caller's reveal.
    pub fn select_section(
        &mut self,
        section: usize,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        if section >= self.sections.len() {
            return None;
        }
        self.selected = Some(section);
        cx.notify();
        Some(section)
    }

    /// Step the selection by `delta` sections, wrapping around.
    pub fn section_delta(
        &mut self,
        delta: isize,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let n = self.sections.len();
        if n == 0 {
            return None;
        }
        let cur = self.selected.unwrap_or(usize::MAX);
        let next = if cur == usize::MAX {
            if delta >= 0 { 0 } else { n - 1 }
        } else {
            (((cur as isize + delta).rem_euclid(n as isize)) as usize)
                .min(n - 1)
        };
        self.selected = Some(next);
        cx.notify();
        Some(next)
    }

    /// Step the selection between conflicts, wrapping around.
    pub fn conflict_delta(
        &mut self,
        delta: isize,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let conflicts: Vec<usize> = self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind() == MergeKind::Conflict)
            .map(|(i, _)| i)
            .collect();
        if conflicts.is_empty() {
            return None;
        }
        let pos = self
            .selected
            .and_then(|sel| conflicts.iter().position(|&c| c == sel));
        let next = match pos {
            Some(p) => {
                conflicts[((p as isize + delta)
                    .rem_euclid(conflicts.len() as isize)
                    as usize)
                    .min(conflicts.len() - 1)]
            }
            None => {
                if delta >= 0 {
                    // Jump to the first conflict at or after the selection.
                    let from = self.selected.unwrap_or(usize::MAX);
                    *conflicts
                        .iter()
                        .find(|&&c| c > from)
                        .unwrap_or(&conflicts[0])
                } else {
                    *conflicts
                        .iter()
                        .rev()
                        .find(|&&c| c < self.selected.unwrap_or(usize::MAX))
                        .unwrap_or(conflicts.last().expect("non-empty"))
                }
            }
        };
        self.selected = Some(next);
        cx.notify();
        Some(next)
    }

    // -- Resolving ----------------------------------------------------------

    /// Resolve the selected section.
    pub fn take(&mut self, choice: MergeChoice, cx: &mut Context<Self>) {
        let Some(section) = self.selected else { return };
        self.take_at(section, choice, cx);
    }

    /// Resolve a specific section and rebuild the output.
    ///
    /// Rebuilding reseeds the output editor, which clears its undo history;
    /// that is the documented trade-off for keeping the section-to-output
    /// mapping exact after every take.
    pub fn take_at(
        &mut self,
        section: usize,
        choice: MergeChoice,
        cx: &mut Context<Self>,
    ) {
        if self.output_stale || self.loading {
            return;
        }
        if choice == MergeChoice::Ancestor && self.ancestor.is_none() {
            return;
        }
        if section >= self.sections.len() {
            return;
        }
        self.resolutions.insert(section, choice);
        self.rebuild_output();
        if let Some(manager) = &self.manager {
            manager.update(cx, |m, cx| m.mark_dirty(cx));
        }
        cx.notify();
    }

    // -- Output editor -------------------------------------------------------

    /// The output editor, once the view has created it.
    pub fn output_editor(&self) -> Option<&Entity<EditorState>> {
        self.output_editor.as_ref()
    }

    /// Whether the output editor still needs re-seeding from the model.
    pub fn take_output_reseed(&mut self) -> bool {
        std::mem::take(&mut self.output_reseed)
    }

    /// Adopt the view's output editor; manual edits mark the output stale
    /// and the tab dirty.
    pub fn attach_output_editor(
        &mut self,
        editor: Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        self._editor_sub =
            Some(cx.subscribe(&editor, |this, _editor, event, cx| {
                // `set_value` emits no change event, so any Change here is
                // a user edit.
                if matches!(event, InputEvent::Change) && !this.output_stale {
                    this.output_stale = true;
                    if let Some(manager) = &this.manager {
                        manager.update(cx, |m, cx| m.mark_dirty(cx));
                    }
                    cx.notify();
                }
            }));
        self.output_editor = Some(editor);
    }

    /// The current output text: the editor buffer once attached, else the
    /// model.
    pub fn output_text(&self, cx: &App) -> String {
        if let Some(editor) = &self.output_editor {
            return editor.read(cx).value().to_string();
        }
        join_lines(&self.output)
    }

    // -- Load and save -------------------------------------------------------

    /// (Re)read all inputs and recompute the merge.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        cx.notify();

        let left = self.left.clone();
        let right = self.right.clone();
        let ancestor = self.ancestor.clone();
        let weak = cx.entity().downgrade();

        // File I/O runs on the tokio runtime because the providers use
        // tokio-based IO internally. A missing file yields empty content.
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let started = Instant::now();
            let read_side = |endpoint: Endpoint| async move {
                let bytes =
                    match endpoint.fs().read_path(endpoint.path(), None).await
                    {
                        Ok(bytes) => bytes.to_vec(),
                        Err(FsError::NotFound { .. }) => Vec::new(),
                        Err(e) => return Err(anyhow::anyhow!("{e}")),
                    };
                Ok::<_, anyhow::Error>(bytes)
            };
            let left = read_side(left).await?;
            let right = read_side(right).await?;
            let ancestor = match ancestor {
                Some(endpoint) => Some(read_side(endpoint).await?),
                None => None,
            };
            Ok::<_, anyhow::Error>((
                left,
                right,
                ancestor,
                started.elapsed().as_millis(),
            ))
        });

        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    if let Some(entity) = weak.upgrade() {
                        entity.update(cx, |session, cx| {
                            session.loading = false;
                            match result {
                                Ok((lb, rb, ab, ms)) => {
                                    session.left_raw = Some(lb);
                                    session.right_raw = Some(rb);
                                    session.ancestor_raw = ab;
                                    session.load_ms = Some(ms);
                                    session.resolutions.clear();
                                    session.rebuild_model();
                                }
                                Err(e) => {
                                    session.error =
                                        Some(format!("Merge failed: {e}"));
                                }
                            }
                            cx.notify();
                        });
                    }
                });
            }
        })
        .detach();
    }

    /// Start the initial load unless the model is already populated.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.left_raw.is_none() && !self.loading && self.error.is_none() {
            self.load(cx);
        }
    }

    /// Prompt for a target path and write the merge output, confirming
    /// first when the target already exists (data-loss warning per spec).
    pub fn save_output(&mut self, cx: &mut Context<Self>) {
        let text = self.output_text(cx);
        let bytes = encode_text(&text, LineEnding::Lf, TextEncoding::Utf8);
        let path = self.left.path().to_path_buf();
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "merged.txt".to_string());
        let receiver = cx.prompt_for_new_path(&dir, Some(&name));
        let entity = cx.entity().clone();
        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let target = match receiver.await {
                    Ok(Ok(Some(p))) => p,
                    // The user cancelled the save dialog.
                    Ok(Ok(None)) => return,
                    Ok(Err(e)) => {
                        cx.update(|cx| {
                            log::push(
                                LogLevel::Error,
                                format!("Save failed: {e}"),
                                cx,
                            );
                        });
                        return;
                    }
                    Err(_) => return,
                };
                // Overwriting an existing file needs an explicit
                // confirmation: the merge output replaces its contents.
                if fs_err::tokio::metadata(&target).await.is_ok() {
                    let (entity, bytes) = (entity.clone(), bytes.clone());
                    let shown = cx.update(|cx| {
                        Self::confirm_replace(
                            entity,
                            bytes,
                            target.clone(),
                            cx,
                        )
                    });
                    if shown {
                        return;
                    }
                }
                Self::finish_save(entity, bytes, target, &mut cx).await;
            }
        })
        .detach();
    }

    /// The confirm button of the data-loss dialog: closes the dialog and
    /// writes the output on the tokio runtime.
    fn replace_button(
        entity: Entity<Self>,
        bytes: Vec<u8>,
        target: std::path::PathBuf,
    ) -> Button {
        Button::new("merge_save_replace")
            .label("Replace")
            .danger()
            .on_click(move |_, window, cx| {
                window.close_dialog(cx);
                // Clone per click: the button closure may run more than
                // once.
                let entity = entity.clone();
                let bytes = bytes.clone();
                let target = target.clone();
                cx.spawn(move |cx: &mut gpui::AsyncApp| {
                    let mut cx = cx.clone();
                    async move {
                        Self::finish_save(entity, bytes, target, &mut cx)
                            .await;
                    }
                })
                .detach();
            })
    }

    /// Open the data-loss confirmation for an existing save target and
    /// take over the save flow; returns whether the dialog was shown.
    fn confirm_replace(
        entity: Entity<Self>,
        bytes: Vec<u8>,
        target: std::path::PathBuf,
        cx: &mut App,
    ) -> bool {
        let Some(window) = cx.active_window() else {
            return false;
        };
        window
            .update(cx, |_, window, cx| {
                window.open_dialog(cx, move |dialog, _, _| {
                    dialog
                        .title("Replace existing file?")
                        .child(
                            v_flex()
                                .gap_2()
                                .text_sm()
                                .child(format!(
                                    "{} already exists.",
                                    target.display()
                                ))
                                .child(
                                    "Saving the merge output will replace \
                                     its current contents. This cannot be \
                                     undone.",
                                ),
                        )
                        .footer(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("merge_save_cancel")
                                        .label("Cancel")
                                        .on_click(|_, window, cx| {
                                            window.close_dialog(cx);
                                        }),
                                )
                                .child(Self::replace_button(
                                    entity.clone(),
                                    bytes.clone(),
                                    target.clone(),
                                )),
                        )
                });
            })
            .is_ok()
    }

    /// Write the output bytes and settle the tab's dirty state.
    async fn finish_save(
        entity: Entity<MergeSession>,
        bytes: Vec<u8>,
        target: std::path::PathBuf,
        cx: &mut gpui::AsyncApp,
    ) {
        let for_log = target.display().to_string();
        let result = crate::runtime::spawn_on_tokio(async move {
            fs_err::tokio::write(&target, &bytes)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await;
        entity.update(cx, |session, cx| {
            match result {
                Ok(Ok(())) => {
                    if let Some(manager) = &session.manager {
                        manager.update(cx, |m, cx| m.mark_clean(cx));
                    }
                    log::push(
                        LogLevel::Info,
                        format!("Saved merge output: {for_log}"),
                        cx,
                    );
                }
                Ok(Err(e)) => {
                    log::push(
                        LogLevel::Error,
                        format!("Save failed: {e}"),
                        cx,
                    );
                    crate::menus::notify_error(
                        format!("Couldn't save {for_log}. {e}"),
                        cx,
                    );
                }
                Err(e) => {
                    log::push(
                        LogLevel::Error,
                        format!("Save task panicked: {e}"),
                        cx,
                    );
                }
            }
            cx.notify();
        });
    }

    // -- Model rebuild
    // --------------------------------------------------------

    /// Decode the raw inputs, run the merge, and rebuild every derived
    /// model.
    fn rebuild_model(&mut self) {
        let decode = |raw: &Option<Vec<u8>>| -> String {
            normalize_endings(
                &decode_bytes(
                    raw.as_deref().unwrap_or(&[]),
                    TextEncoding::Utf8,
                ),
                LineEnding::Lf,
            )
        };
        let left_text = decode(&self.left_raw);
        let right_text = decode(&self.right_raw);
        let ancestor_text =
            self.ancestor.as_ref().map(|_| decode(&self.ancestor_raw));
        self.left_lines = Arc::new(split_lines(&left_text));
        self.right_lines = Arc::new(split_lines(&right_text));
        self.ancestor_lines = Arc::new(
            ancestor_text
                .as_deref()
                .map(split_lines)
                .unwrap_or_default(),
        );
        let result =
            merge_texts(ancestor_text.as_deref(), &left_text, &right_text);
        self.sections = Arc::new(result.sections().to_vec());
        self.left_line_section =
            Arc::new(build_line_map(&self.sections, |s| s.left()));
        self.right_line_section =
            Arc::new(build_line_map(&self.sections, |s| s.right()));
        self.ancestor_line_section =
            Arc::new(build_line_map(&self.sections, |s| s.ancestor()));
        self.left_starts = Arc::new(build_starts(&self.left_line_section));
        self.right_starts = Arc::new(build_starts(&self.right_line_section));
        self.ancestor_starts =
            Arc::new(build_starts(&self.ancestor_line_section));
        if self.selected.is_some_and(|s| s >= self.sections.len()) {
            self.selected = None;
        }
        self.rebuild_output();
    }

    /// Rebuild the output lines and their per-section ranges from the
    /// sections and the recorded resolutions.
    fn rebuild_output(&mut self) {
        let mut output: Vec<String> = Vec::new();
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for (i, section) in self.sections.iter().enumerate() {
            let choice = self
                .resolutions
                .get(&i)
                .copied()
                .unwrap_or_else(|| default_choice(section.kind()));
            let lines: &[String] = match choice {
                MergeChoice::Left => section.left(),
                MergeChoice::Right => section.right(),
                MergeChoice::Ancestor => section.ancestor(),
            };
            ranges.push((output.len(), output.len() + lines.len()));
            output.extend_from_slice(lines);
        }
        self.output = output;
        self.section_output_ranges = Arc::new(ranges);
        self.output_stale = false;
        self.output_reseed = true;
        self.model_rev += 1;
    }

    /// The model revision, for the view's decoration refresh.
    pub fn model_rev(&self) -> usize {
        self.model_rev
    }

    /// The current output model lines.
    pub fn output_lines(&self) -> &Vec<String> {
        &self.output
    }

    /// The output model lines as plain strs, for tests.
    #[cfg(test)]
    fn output_text_lines(&self) -> Vec<&str> {
        self.output.iter().map(String::as_str).collect()
    }
}

/// The cheap-Arc snapshot the list closures render from.
pub struct MergeSnapshot {
    pub left_lines: Arc<Vec<String>>,
    pub right_lines: Arc<Vec<String>>,
    pub ancestor_lines: Arc<Vec<String>>,
    pub sections: Arc<Vec<MergeSection>>,
    pub left_line_section: Arc<Vec<usize>>,
    pub right_line_section: Arc<Vec<usize>>,
    pub ancestor_line_section: Arc<Vec<usize>>,
    pub selected: Option<usize>,
    pub word_wrap: bool,
    pub show_ancestor: bool,
    pub has_ancestor: bool,
}

/// The side a section contributes when it has no explicit resolution.
fn default_choice(kind: MergeKind) -> MergeChoice {
    match kind {
        MergeKind::RightChanged => MergeChoice::Right,
        _ => MergeChoice::Left,
    }
}

/// Split decoded text into lines (dropping the trailing newline).
fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// Join model lines back into file text with a trailing newline.
fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Build a line-index → section-index map by concatenating each section's
/// side contents (which reproduce the full side text in order).
fn build_line_map(
    sections: &[MergeSection],
    get: impl Fn(&MergeSection) -> &[String],
) -> Vec<usize> {
    let mut map = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        for _ in get(section) {
            map.push(i);
        }
    }
    map
}

/// From a line → section map, derive each section's first line index.
fn build_starts(line_section: &[usize]) -> Vec<usize> {
    let mut starts: Vec<usize> = Vec::new();
    for (line, &section) in line_section.iter().enumerate() {
        while starts.len() <= section {
            starts.push(line);
        }
    }
    starts
}

/// Byte offset of line `line` within the joined output text.
fn output_byte_offset(lines: &[String], line: usize) -> usize {
    lines
        .iter()
        .take(line.min(lines.len()))
        .map(|l| l.len() + 1)
        .sum()
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// The merge view: source panes above, editable output below.
pub struct MergeView {
    /// The merge session this view renders.
    state: Entity<MergeSession>,
    /// Left source pane list.
    left_list: ListState,
    /// Right source pane list.
    right_list: ListState,
    /// Ancestor source pane list.
    ancestor_list: ListState,
    /// Conflict fills for the output editor.
    output_decs: Option<RangeDecorationCollection>,
    /// Model revision the decorations were last rebuilt for.
    decoration_rev: usize,
    /// Line counts the lists were last reset to.
    list_counts: (usize, usize, usize),
    /// Wrap mode the lists were last reset to.
    list_wrap: bool,
    /// Re-render whenever the session notifies.
    _state_sub: Subscription,
}

impl MergeView {
    /// Create a view over an existing session; starts the initial load.
    pub fn new(state: Entity<MergeSession>, cx: &mut Context<Self>) -> Self {
        // Section stepping and conflict walking are scoped to the merge
        // view's key context so they never shadow the other views' bindings.
        cx.bind_keys([
            KeyBinding::new("j", NextMergeSection, Some("TextMerge")),
            KeyBinding::new("k", PrevMergeSection, Some("TextMerge")),
            KeyBinding::new("down", NextMergeSection, Some("TextMerge")),
            KeyBinding::new("up", PrevMergeSection, Some("TextMerge")),
            KeyBinding::new("cmd-shift-]", NextConflict, Some("TextMerge")),
            KeyBinding::new("cmd-shift-[", PrevConflict, Some("TextMerge")),
        ]);

        let _state_sub = cx.observe(&state, |_, _, cx| cx.notify());
        state.update(cx, |session, cx| session.ensure_loaded(cx));
        Self {
            state,
            left_list: ListState::new(0, ListAlignment::Top, px(100.)),
            right_list: ListState::new(0, ListAlignment::Top, px(100.)),
            ancestor_list: ListState::new(0, ListAlignment::Top, px(100.)),
            output_decs: None,
            decoration_rev: usize::MAX,
            list_counts: (0, 0, 0),
            list_wrap: false,
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

    /// The underlying session, for the window root's menu sync.
    pub fn merge_state(&self) -> Entity<MergeSession> {
        self.state.clone()
    }

    /// Scroll the source panes so the section's lines are visible.
    fn reveal_section(&self, section: usize, cx: &App) {
        let (l, r, a) = self.state.read(cx).section_reveal_lines(section);
        if let Some(ix) = l {
            self.left_list.scroll_to_reveal_item(ix);
        }
        if let Some(ix) = r {
            self.right_list.scroll_to_reveal_item(ix);
        }
        if let Some(ix) = a {
            self.ancestor_list.scroll_to_reveal_item(ix);
        }
    }

    /// Place the output editor's caret at the section's first line, which
    /// also scrolls it into view.
    fn reveal_output_section(&self, section: usize, cx: &mut App) {
        let byte = {
            let session = self.state.read(cx);
            match session.section_output_start(section) {
                Some(line) => session.output_byte_offset(line),
                None => return,
            }
        };
        if let Some(editor) = self.state.read(cx).output_editor().cloned() {
            editor.update(cx, |ed, cx| {
                ed.set_selected_range(byte..byte, cx);
            });
        }
    }

    // -- Action handlers -----------------------------------------------------

    fn handle_take_left(
        &mut self,
        _: &TakeLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.take(MergeChoice::Left, cx));
    }

    fn handle_take_right(
        &mut self,
        _: &TakeRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.take(MergeChoice::Right, cx));
    }

    fn handle_take_ancestor(
        &mut self,
        _: &TakeAncestor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.take(MergeChoice::Ancestor, cx));
    }

    fn handle_take_section(
        &mut self,
        action: &TakeSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| {
            s.select_section(action.section, cx);
            s.take_at(action.section, action.choice, cx);
        });
    }

    fn handle_next_conflict(
        &mut self,
        _: &NextConflict,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.conflict_delta(1, cx));
        if let Some(section) = target {
            self.reveal_section(section, cx);
            self.reveal_output_section(section, cx);
        }
    }

    fn handle_prev_conflict(
        &mut self,
        _: &PrevConflict,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.conflict_delta(-1, cx));
        if let Some(section) = target {
            self.reveal_section(section, cx);
            self.reveal_output_section(section, cx);
        }
    }

    fn handle_next_section(
        &mut self,
        _: &NextMergeSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.section_delta(1, cx));
        if let Some(section) = target {
            self.reveal_section(section, cx);
        }
    }

    fn handle_prev_section(
        &mut self,
        _: &PrevMergeSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.section_delta(-1, cx));
        if let Some(section) = target {
            self.reveal_section(section, cx);
        }
    }

    fn handle_save_output(
        &mut self,
        _: &SaveMergeOutput,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.save_output(cx));
    }

    fn handle_toggle_wrap(
        &mut self,
        _: &ToggleWordWrap,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.toggle_wrap(cx));
    }

    fn handle_toggle_ancestor(
        &mut self,
        _: &ToggleMergeAncestor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.toggle_ancestor(cx));
    }

    fn handle_reload(
        &mut self,
        _: &ReloadCompare,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.load(cx));
    }

    // -- Toolbar
    // ---------------------------------------------------------------

    /// Render the view's toolbar row (row 2) for the window root.
    pub fn render_view_toolbar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (can_take, has_ancestor, has_conflicts) = {
            let session = self.state.read(cx);
            (
                session.can_take(),
                session.has_ancestor(),
                session.conflict_count() > 0,
            )
        };
        let (word_wrap, show_ancestor) = {
            let session = self.state.read(cx);
            (session.is_word_wrap(), session.show_ancestor())
        };

        let take_btn = |id: &'static str,
                        label: &'static str,
                        icon: IconName,
                        choice: MergeChoice,
                        state: Entity<MergeSession>,
                        disabled: bool| {
            Button::new(id)
                .icon(icon)
                .label(label)
                .ghost()
                .small()
                .disabled(disabled)
                .tooltip(format!(
                    "Take the selected section's {} lines",
                    label.to_lowercase()
                ))
                .accessibility_label(format!("Take {label}"))
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.take(choice, cx));
                })
        };

        let state = self.state.clone();
        let take_left = take_btn(
            "merge_take_left",
            "Take Left",
            IconName::ArrowLeft,
            MergeChoice::Left,
            state.clone(),
            !can_take,
        );
        let take_right = take_btn(
            "merge_take_right",
            "Take Right",
            IconName::ArrowRight,
            MergeChoice::Right,
            state.clone(),
            !can_take,
        );
        let take_ancestor = take_btn(
            "merge_take_ancestor",
            "Take Ancestor",
            IconName::ArrowRight,
            MergeChoice::Ancestor,
            state.clone(),
            !can_take || !has_ancestor,
        );

        let prev_conflict_btn = Button::new("merge_prev_conflict")
            .icon(IconName::ChevronsUp)
            .ghost()
            .small()
            .disabled(!has_conflicts)
            .tooltip("Previous conflict")
            .accessibility_label("Previous conflict")
            .on_click(|_, window, cx| {
                // Dispatch through the focus path so the view also reveals
                // the conflict in the panes and the output.
                window.dispatch_action(Box::new(PrevConflict), cx);
            });
        let next_conflict_btn = Button::new("merge_next_conflict")
            .icon(IconName::ChevronsDown)
            .ghost()
            .small()
            .disabled(!has_conflicts)
            .tooltip("Next conflict")
            .accessibility_label("Next conflict")
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(NextConflict), cx);
            });

        let wrap_btn = {
            let state = self.state.clone();
            Button::new("merge_wrap")
                .icon(IconName::TextWrap)
                .ghost()
                .small()
                .toggled(word_wrap)
                .tooltip("Toggle word wrap")
                .accessibility_label("Toggle word wrap")
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.toggle_wrap(cx));
                })
        };
        let ancestor_btn = {
            let state = self.state.clone();
            Button::new("merge_ancestor")
                .icon(if show_ancestor {
                    IconName::Eye
                } else {
                    IconName::EyeClosed
                })
                .ghost()
                .small()
                .toggled(show_ancestor)
                .disabled(!has_ancestor)
                .tooltip(if show_ancestor {
                    "Hide the ancestor pane"
                } else {
                    "Show the ancestor pane"
                })
                .accessibility_label("Toggle ancestor pane")
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.toggle_ancestor(cx));
                })
        };
        let save_btn = {
            let state = self.state.clone();
            Button::new("merge_save")
                .icon(IconName::Save)
                .ghost()
                .small()
                .tooltip("Save the merge output…")
                .accessibility_label("Save merge output")
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.save_output(cx));
                })
        };
        let reload_btn = {
            let state = self.state.clone();
            Button::new("merge_reload")
                .icon(IconName::RefreshCw)
                .ghost()
                .small()
                .tooltip("Reload and rebuild the merge")
                .accessibility_label("Reload merge")
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.load(cx));
                })
        };

        Toolbar::new("merge-view-toolbar")
            .child(
                ToolbarGroup::new("merge-view-toolbar-resolve")
                    .label("Resolve")
                    .child(take_left)
                    .child(take_right)
                    .child(take_ancestor),
            )
            .child(
                ToolbarGroup::new("merge-view-toolbar-conflicts")
                    .label("Conflicts")
                    .child(prev_conflict_btn)
                    .child(next_conflict_btn),
            )
            .child(
                ToolbarGroup::new("merge-view-toolbar-view")
                    .label("View")
                    .child(wrap_btn)
                    .child(ancestor_btn),
            )
            .child(
                ToolbarGroup::new("merge-view-toolbar-output")
                    .label("Output")
                    .child(save_btn),
            )
            .child(
                ToolbarGroup::new("merge-view-toolbar-reload")
                    .label("Reload")
                    .child(reload_btn),
            )
            .into_any_element()
    }
}

// ---------------------------------------------------------------------------
// Row rendering
// ---------------------------------------------------------------------------

/// The resolved bar colors for the section kinds.
#[derive(Clone, Copy)]
struct BarColors {
    left: Hsla,
    right: Hsla,
    conflict: Hsla,
    both: Hsla,
}

impl BarColors {
    fn resolve(theme: &Theme) -> Self {
        Self {
            left: theme.info,
            right: theme.primary,
            conflict: theme.danger,
            both: theme.muted_foreground,
        }
    }

    fn for_kind(&self, kind: MergeKind) -> Hsla {
        match kind {
            MergeKind::Same => gpui::hsla(0., 0., 0., 0.),
            MergeKind::LeftChanged => self.left,
            MergeKind::RightChanged => self.right,
            MergeKind::BothSame => self.both,
            MergeKind::Conflict => self.conflict,
        }
    }
}

/// Render one source-pane row: change bar, line number, and text.
#[allow(clippy::too_many_arguments)]
fn render_merge_row(
    side_key: &'static str,
    ix: usize,
    lines: &[String],
    line_section: &[usize],
    sections: &[MergeSection],
    selected: Option<usize>,
    word_wrap: bool,
    row_h: Pixels,
    bars: &BarColors,
    fg: Hsla,
    muted_fg: Hsla,
    list_active: Hsla,
    list_hover: Hsla,
    mono: &SharedString,
    state: &Entity<MergeSession>,
    has_ancestor: bool,
) -> AnyElement {
    let Some(content) = lines.get(ix) else {
        return div().size_full().into_any_element();
    };
    let si = line_section.get(ix).copied().unwrap_or(usize::MAX);
    let kind = sections.get(si).map(|s| s.kind());
    let bar_color = kind.map(|k| bars.for_kind(k)).unwrap_or(bars.conflict);
    let is_selected = selected.is_some_and(|s| s == si);
    div()
        .id((ElementId::from("merge_row"), format!("{side_key}-{ix}")))
        .w_full()
        .flex()
        .flex_shrink_0()
        .text_color(fg)
        .font_family(mono.clone())
        .text_xs()
        .child(div().w(px(3.)).flex_shrink_0().bg(bar_color))
        .when(is_selected, |el| el.bg(list_active))
        .hover(|el| el.bg(list_hover))
        .when(!word_wrap, |el| {
            el.h(row_h).overflow_hidden().whitespace_nowrap()
        })
        .when(word_wrap, |el| el.py_0p5())
        .child(
            div()
                .w(px(40.))
                .flex_shrink_0()
                .pr_1()
                .text_right()
                .text_color(muted_fg)
                .child((ix + 1).to_string()),
        )
        .child(div().flex_1().min_w_0().child(content.clone()))
        .on_click({
            let state = state.clone();
            move |_, window, cx| {
                let focus = state.read(cx).focus_handle();
                window.focus(&focus, cx);
                state.update(cx, |s, cx| s.select_section(si, cx));
            }
        })
        // Right-click focuses the session so the menu's actions dispatch
        // through the view's `TextMerge` key context.
        .on_mouse_down(MouseButton::Right, {
            let state = state.clone();
            move |_, window, cx| {
                let focus = state.read(cx).focus_handle();
                window.focus(&focus, cx);
            }
        })
        .context_menu(move |menu, _, _| {
            let menu = menu;
            if si == usize::MAX {
                return menu;
            }
            let menu = menu
                .menu(
                    "Take left",
                    Box::new(TakeSection {
                        section: si,
                        choice: MergeChoice::Left,
                    }),
                )
                .menu(
                    "Take right",
                    Box::new(TakeSection {
                        section: si,
                        choice: MergeChoice::Right,
                    }),
                );
            if has_ancestor {
                menu.menu(
                    "Take ancestor",
                    Box::new(TakeSection {
                        section: si,
                        choice: MergeChoice::Ancestor,
                    }),
                )
            } else {
                menu
            }
        })
        .into_any_element()
}

/// Render one source pane: header bar plus the virtual list body.
fn render_source_pane(
    label: &'static str,
    body: AnyElement,
    theme: &Theme,
) -> impl IntoElement {
    v_flex()
        .size_full()
        .min_w_0()
        .min_h_0()
        .bg(theme.background)
        .child(
            h_flex()
                .flex_shrink_0()
                .px_2()
                .py_1()
                .bg(theme.muted)
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_h_0().overflow_hidden().child(body))
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for MergeView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Create the output editor on first render and hand it to the
        // session, which tracks its dirty and stale state.
        if self.state.read(cx).output_editor().is_none() {
            let editor = cx.new(|cx| EditorState::new(window, cx));
            let decs = editor.update(cx, |ed, cx| {
                ed.create_range_decorations_collection(Vec::new(), cx)
            });
            self.state
                .update(cx, |s, cx| s.attach_output_editor(editor, cx));
            self.output_decs = Some(decs);
            self.decoration_rev = usize::MAX;
        }
        // Re-seed the output after a load or take; `set_value` clears undo
        // history and emits no change event, matching a rebuilt buffer.
        if self.state.update(cx, |s, _| s.take_output_reseed()) {
            let editor = self.state.read(cx).output_editor().cloned();
            if let Some(editor) = editor {
                let (text, wrap) = {
                    let session = self.state.read(cx);
                    (
                        join_lines(session.output_lines()),
                        session.is_word_wrap(),
                    )
                };
                editor.update(cx, |ed, cx| {
                    ed.set_value(text, window, cx);
                    ed.set_soft_wrap(wrap, window, cx);
                });
                self.decoration_rev = usize::MAX;
            }
        }

        let session = self.state.read(cx);
        let snap = session.snapshot();
        let loading = session.is_loading();
        let error_text = session.error().map(str::to_string);
        let load_label = session.load_label();
        let conflicts = session.conflict_count();
        let unresolved = session.unresolved_count();
        let section_count = session.section_count();
        let selected = snap.selected;
        let model_rev = session.model_rev();
        let output_editor = session.output_editor().cloned();
        let has_ancestor = snap.has_ancestor;

        let theme = cx.theme().clone();
        let bars = BarColors::resolve(&theme);
        let mono = theme.mono_font_family.clone();
        let fg = theme.foreground;
        let muted_fg = theme.muted_foreground;
        let list_active = theme.list_active;
        let list_hover = theme.list_hover;
        let border = theme.border;
        let danger = theme.danger;

        // Refresh the conflict fills whenever the model changed.
        if model_rev != self.decoration_rev {
            self.decoration_rev = model_rev;
            if let Some(decs) = &self.output_decs {
                let fills: Vec<RangeDecoration> = self
                    .state
                    .read(cx)
                    .conflict_fill_ranges()
                    .into_iter()
                    .map(|(lo, hi)| {
                        let mut color = danger;
                        color.a = 0.18;
                        RangeDecoration::new(lo..hi)
                            .with_style(RangeDecorationStyle::Fill)
                            .with_color(color)
                    })
                    .collect();
                decs.set(fills, cx);
            }
        }

        // Reset the lists only when a line count or the wrap mode changed;
        // a reset per frame would destroy the scroll position.
        let row_h = px(window.rem_size().as_f32() * 1.5);
        let counts = (
            snap.left_lines.len(),
            snap.right_lines.len(),
            snap.ancestor_lines.len(),
        );
        if self.list_counts != counts || self.list_wrap != snap.word_wrap {
            for (list, count) in [
                (&self.left_list, counts.0),
                (&self.right_list, counts.1),
                (&self.ancestor_list, counts.2),
            ] {
                if snap.word_wrap {
                    list.reset(count);
                } else {
                    list.reset_with_uniform_height(count, row_h);
                }
            }
            self.list_counts = counts;
            self.list_wrap = snap.word_wrap;
        }

        let wrap = snap.word_wrap;
        let source_list =
            |lst: ListState,
             side_key: &'static str,
             lines: Arc<Vec<String>>,
             line_section: Arc<Vec<usize>>,
             sections: Arc<Vec<MergeSection>>,
             state: Entity<MergeSession>| {
                let mono = mono.clone();
                list(lst, move |ix, _window, _cx| {
                    render_merge_row(
                        side_key,
                        ix,
                        &lines,
                        &line_section,
                        &sections,
                        selected,
                        wrap,
                        row_h,
                        &bars,
                        fg,
                        muted_fg,
                        list_active,
                        list_hover,
                        &mono,
                        &state,
                        has_ancestor,
                    )
                })
                .size_full()
                .min_h_0()
            };

        let left_el = source_list(
            self.left_list.clone(),
            "left",
            snap.left_lines.clone(),
            snap.left_line_section.clone(),
            snap.sections.clone(),
            self.state.clone(),
        );
        let ancestor_el = snap.show_ancestor.then(|| {
            source_list(
                self.ancestor_list.clone(),
                "ancestor",
                snap.ancestor_lines.clone(),
                snap.ancestor_line_section.clone(),
                snap.sections.clone(),
                self.state.clone(),
            )
        });
        let right_el = source_list(
            self.right_list.clone(),
            "right",
            snap.right_lines.clone(),
            snap.right_line_section.clone(),
            snap.sections.clone(),
            self.state.clone(),
        );

        let output_body: AnyElement = match &output_editor {
            Some(editor) => {
                Editor::new(editor).appearance(false).into_any_element()
            }
            None => div().into_any_element(),
        };

        let mut bar = StatusBar::new();
        bar = bar.left(
            h_flex()
                .items_center()
                .gap_3()
                .child(format!("{conflicts} conflict(s)"))
                .child(format!("{unresolved} unresolved"))
                .child(selected.map_or_else(
                    || "No section selected".to_string(),
                    |s| format!("Section {}/{section_count}", s + 1),
                )),
        );
        let right_status = if loading {
            h_flex()
                .items_center()
                .gap_1p5()
                .child(Spinner::new().xsmall())
                .child("Loading…")
                .into_any_element()
        } else if let Some(err) = &error_text {
            div()
                .text_color(danger)
                .child(err.clone())
                .into_any_element()
        } else if let Some(label) = &load_label {
            div().child(label.clone()).into_any_element()
        } else {
            div().child("Ready").into_any_element()
        };
        let status_bar = bar.right(right_status);

        let focus = self.state.read(cx).focus_handle();
        div()
            .key_context("TextMerge")
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(fg)
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    v_resizable("merge-layout")
                        .child(
                            resizable_panel().child(
                                h_resizable("merge-sources")
                                    .child(resizable_panel().child(
                                        render_source_pane(
                                            "Left",
                                            left_el.into_any_element(),
                                            &theme,
                                        ),
                                    ))
                                    .when_some(ancestor_el, |group, el| {
                                        group.child(resizable_panel().child(
                                            render_source_pane(
                                                "Ancestor",
                                                el.into_any_element(),
                                                &theme,
                                            ),
                                        ))
                                    })
                                    .child(resizable_panel().child(
                                        render_source_pane(
                                            "Right",
                                            right_el.into_any_element(),
                                            &theme,
                                        ),
                                    )),
                            ),
                        )
                        .child(
                            resizable_panel().child(
                                v_flex()
                                    .size_full()
                                    .min_h_0()
                                    .border_t_1()
                                    .border_color(border)
                                    .child(
                                        h_flex()
                                            .flex_shrink_0()
                                            .items_center()
                                            .justify_between()
                                            .px_2()
                                            .py_1()
                                            .bg(theme.muted)
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child("Output")
                                            .child(
                                                "Editing the output disables \
                                                 the take buttons until the \
                                                 next take or reload",
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_hidden()
                                            .child(output_body),
                                    ),
                            ),
                        ),
                ),
            )
            .child(status_bar)
            .on_action(cx.listener(Self::handle_take_left))
            .on_action(cx.listener(Self::handle_take_right))
            .on_action(cx.listener(Self::handle_take_ancestor))
            .on_action(cx.listener(Self::handle_take_section))
            .on_action(cx.listener(Self::handle_next_conflict))
            .on_action(cx.listener(Self::handle_prev_conflict))
            .on_action(cx.listener(Self::handle_next_section))
            .on_action(cx.listener(Self::handle_prev_section))
            .on_action(cx.listener(Self::handle_save_output))
            .on_action(cx.listener(Self::handle_toggle_wrap))
            .on_action(cx.listener(Self::handle_toggle_ancestor))
            .on_action(cx.listener(Self::handle_reload))
    }
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/// Base name of an endpoint path, falling back to its full display form.
fn base_name(endpoint: &Endpoint) -> String {
    endpoint
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| endpoint.display())
}

/// Create a merge view entity for the given endpoints.
pub(crate) fn new_text_merge_view(
    left: Endpoint,
    right: Endpoint,
    ancestor: Option<Endpoint>,
    manager: Option<Entity<GuiSessionManager>>,
    cx: &mut App,
) -> Entity<MergeView> {
    let title = match &ancestor {
        Some(_) => {
            format!("{} vs {} (3-way)", base_name(&left), base_name(&right))
        }
        None => format!("{} vs {}", base_name(&left), base_name(&right)),
    }
    .into();
    let session = cx.new(|cx| {
        MergeSession::new(left, right, ancestor, title, manager, cx)
    });
    cx.new(|cx| MergeView::new(session, cx))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    fn session_from(
        cx: &mut TestAppContext,
        ancestor: Option<&str>,
        left: &str,
        right: &str,
    ) -> Entity<MergeSession> {
        cx.update(|cx| {
            cx.new(|cx| {
                MergeSession::from_texts(ancestor, left, right, "t".into(), cx)
            })
        })
    }

    #[gpui::test]
    fn auto_output_takes_left_for_conflicts(cx: &mut TestAppContext) {
        let session =
            session_from(cx, Some("a\nb\nc\n"), "a\nL\nc\n", "a\nR\nc\n");
        session.read_with(cx, |s, _| {
            assert_eq!(s.conflict_count(), 1);
            assert_eq!(s.unresolved_count(), 1);
            assert_eq!(s.output_text_lines(), vec!["a", "L", "c"]);
        });
    }

    #[gpui::test]
    fn take_overrides_the_auto_resolution(cx: &mut TestAppContext) {
        let session =
            session_from(cx, Some("a\nb\nc\n"), "a\nL\nc\n", "a\nR\nc\n");
        session.update(cx, |s, cx| {
            // Select the conflict section (index 1) and take the right side.
            s.select_section(1, cx);
            s.take(MergeChoice::Right, cx);
            assert_eq!(s.output_text_lines(), vec!["a", "R", "c"]);
            assert_eq!(s.unresolved_count(), 0);
            // Take the ancestor to restore the original line.
            s.take(MergeChoice::Ancestor, cx);
            assert_eq!(s.output_text_lines(), vec!["a", "b", "c"]);
        });
    }

    #[gpui::test]
    fn take_on_non_conflict_overrides(cx: &mut TestAppContext) {
        let session =
            session_from(cx, Some("a\nb\nc\n"), "a\nL\nc\n", "a\nb\nc\n");
        session.update(cx, |s, cx| {
            assert_eq!(s.conflict_count(), 0);
            // The left-only change resolves to left automatically.
            assert_eq!(s.output_text_lines(), vec!["a", "L", "c"]);
            s.select_section(1, cx);
            s.take(MergeChoice::Right, cx);
            assert_eq!(s.output_text_lines(), vec!["a", "b", "c"]);
        });
    }

    #[gpui::test]
    fn two_way_mode_marks_differences_as_conflicts(cx: &mut TestAppContext) {
        let session = session_from(cx, None, "a\nL\nc\n", "a\nR\nc\n");
        session.read_with(cx, |s, _| {
            assert!(!s.has_ancestor());
            assert_eq!(s.conflict_count(), 1);
            assert_eq!(s.output_text_lines(), vec!["a", "L", "c"]);
        });
        session.update(cx, |s, cx| {
            // Taking the ancestor is rejected in two-way mode.
            s.select_section(1, cx);
            s.take(MergeChoice::Ancestor, cx);
            assert_eq!(s.output_text_lines(), vec!["a", "L", "c"]);
        });
    }

    #[gpui::test]
    fn navigation_wraps_and_counts_sections(cx: &mut TestAppContext) {
        let session = session_from(
            cx,
            Some("1\n2\n3\n4\n5\n6\n7\n8\n"),
            "1L\n2\n3\n4\n5L\n6\n7\n8\n",
            "1R\n2\n3\n4\n5R\n6\n7\n8\n",
        );
        session.update(cx, |s, cx| {
            assert_eq!(s.conflict_count(), 2);
            let first = s.conflict_delta(1, cx).expect("a conflict");
            let second = s.conflict_delta(1, cx).expect("a second conflict");
            assert_ne!(first, second);
            // Wrapping returns to the first conflict.
            let wrapped = s.conflict_delta(1, cx).expect("wraps");
            assert_eq!(wrapped, first);
            // Section stepping visits every section and wraps.
            let n = s.section_count();
            s.select_section(0, cx);
            for _ in 0..(n - 1) {
                s.section_delta(1, cx);
            }
            assert_eq!(s.selected_section(), Some(n - 1));
            s.section_delta(1, cx);
            assert_eq!(s.selected_section(), Some(0));
        });
    }

    #[gpui::test]
    fn output_ranges_partition_the_output(cx: &mut TestAppContext) {
        let session = session_from(
            cx,
            Some("a\nb\nc\nd\n"),
            "a\nL\nc\nd\n",
            "a\nb\nR\nd\n",
        );
        session.read_with(cx, |s, _| {
            let total: usize =
                s.section_output_ranges.iter().map(|(lo, hi)| hi - lo).sum();
            assert_eq!(total, s.output_lines().len());
            // Ranges are contiguous and ordered.
            let mut prev = 0;
            for (lo, hi) in s.section_output_ranges.iter() {
                assert_eq!(*lo, prev);
                assert!(hi >= lo);
                prev = *hi;
            }
        });
    }

    #[gpui::test]
    fn manual_edit_disables_take(cx: &mut TestAppContext) {
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        cx.update(gpui::init);
        let session = cx.update(|cx| {
            cx.new(|cx| {
                MergeSession::from_texts(
                    Some("a\nb\n"),
                    "a\nL\n",
                    "a\nR\n",
                    "t".into(),
                    cx,
                )
            })
        });
        let (window, _view) = cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|cx| MergeView::new(session.clone(), cx)),
            )
            .expect("failed to open the test window")
        });
        // Render once so the view creates and seeds the output editor.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
        })
        .expect("first frame failed");
        session.update(cx, |s, cx| {
            s.select_section(1, cx);
            assert!(s.can_take());
        });
        // Simulate a manual edit through the editor's change path; the
        // silent `replace` helper emits no event, so go through the input
        // handler like a real keystroke does.
        use gpui::EntityInputHandler as _;
        let editor = session.read_with(cx, |s, _| {
            s.output_editor().cloned().expect("editor attached")
        });
        cx.update_window(window, |_, window, cx| {
            editor.update(cx, |ed, cx| {
                ed.replace_text_in_range(None, "X", window, cx);
            });
        })
        .expect("edit failed");
        session.read_with(cx, |s, _| {
            assert!(!s.can_take(), "manual edit must disable take");
            // Conflict fills are dropped once the ranges are untrustworthy.
            assert!(s.conflict_fill_ranges().is_empty());
        });
    }

    #[gpui::test]
    fn take_actions_drive_the_view(cx: &mut TestAppContext) {
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        cx.update(gpui::init);
        let session = cx.update(|cx| {
            cx.new(|cx| {
                MergeSession::from_texts(
                    Some("a\nb\nc\n"),
                    "a\nL\nc\n",
                    "a\nR\nc\n",
                    "t".into(),
                    cx,
                )
            })
        });
        let (window, view) = cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|cx| MergeView::new(session.clone(), cx)),
            )
            .expect("failed to open the test window")
        });
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let focus = view.read(cx).focus_handle(cx);
            focus.focus(window, cx);
            // Select the conflict section, then take the right side.
            window.dispatch_action(Box::new(NextConflict), cx);
            window.render_frame(cx);
            window.dispatch_action(Box::new(TakeRight), cx);
            window.render_frame(cx);
        })
        .expect("dispatch failed");
        session.read_with(cx, |s, cx| {
            assert_eq!(s.selected_section(), Some(1));
            assert_eq!(s.unresolved_count(), 0);
            assert_eq!(s.output_text(cx), "a\nR\nc\n");
        });
    }
}
