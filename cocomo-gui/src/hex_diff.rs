// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Side-by-side hex compare view (UI spec section 7, new session types).
//!
//! [`HexDiffSession`] owns one binary comparison: the loaded bytes of both
//! files and the byte-level difference model derived from them.
//! [`HexDiffView`] renders the model as two scroll-synchronized hex panes
//! (offset, hex bytes, ASCII) with byte-level diff coloring and a status
//! bar.
//!
//! The model has two layers: [`hex_segments`] aligns the two buffers into
//! equal and different runs, and [`HexLayout`] turns those runs into the
//! 16-byte rows both panes render. Inside an equal run the panes show the
//! same bytes in the same row; inside a different run every pane continues
//! with its own bytes from the run's start, and the shorter side ends in a
//! blank area.

use std::{
    array,
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::Instant,
};

use cocomo_lib::{
    ColorOverrides, Endpoint, FsError, NodeFileSystem, format_size,
};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Hsla,
    IntoElement, ListAlignment, ListScrollEvent, ListState, Pixels, Render,
    SharedString, Subscription, Window,
    component::{
        ActiveTheme, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        spinner::Spinner,
        status_bar::StatusBar,
        v_flex,
    },
    div, list,
    prelude::*,
    px,
};

use crate::{
    color::{ColorRole, Side},
    log::{self, LogLevel},
    session_manager::GuiSessionManager,
};

// ---------------------------------------------------------------------------
// Diff model
// ---------------------------------------------------------------------------

/// The number of bytes one hex row shows.
pub(crate) const ROW_BYTES: usize = 16;

/// The number of hex digits the offset column shows.
const OFFSET_DIGITS: usize = 8;

/// The length of an equal block that re-anchors the two panes inside a
/// different run. Sixteen bytes are specific enough that a match is a real
/// realignment rather than a coincidence in binary data.
const ANCHOR_BYTES: usize = ROW_BYTES;

/// How far into a different run the realignment search reaches, per side.
/// Every position in that window is hashed, so the window bounds both the
/// run length that can realign and the memory the index needs; a longer
/// run simply keeps the panes apart.
const ANCHOR_SCAN_LIMIT: usize = 128 * 1024;

/// The upper bound on the rows built for one comparison. Both files are
/// fully in memory, so the cap only keeps the row model and the virtual
/// list bounded for a pathological pair of huge files; the panes then show
/// a notice row where the display was cut off.
pub(crate) const MAX_ROWS: usize = 512 * 1024;

/// A run of bytes that is equal or different between the two files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HexSegmentKind {
    /// The runs on both sides match byte-for-byte.
    Equal,
    /// The runs differ; the sides may have different lengths.
    Different,
}

/// One aligned run of bytes in the comparison.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HexSegment {
    /// Whether the runs match.
    pub(crate) kind: HexSegmentKind,
    /// First byte offset on the left side.
    pub(crate) left_start: usize,
    /// Number of left bytes covered.
    pub(crate) left_len: usize,
    /// First byte offset on the right side.
    pub(crate) right_start: usize,
    /// Number of right bytes covered.
    pub(crate) right_len: usize,
}

/// Compute the aligned byte segments of two buffers.
///
/// The alignment keeps the common prefix and suffix intact and treats
/// everything between them as different, except where an equal anchor block
/// (see [`anchor_blocks`]) realigns the two sides: those runs become equal
/// segments again, so the panes converge after an insertion or deletion
/// instead of diverging all the way to the suffix.
pub(crate) fn hex_segments(left: &[u8], right: &[u8]) -> Vec<HexSegment> {
    let prefix = left
        .iter()
        .zip(right.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = common_suffix(&left[prefix..], &right[prefix..]);
    let left_mid = left.len() - suffix;
    let right_mid = right.len() - suffix;

    let mut segments = Vec::new();
    push_segment(&mut segments, HexSegmentKind::Equal, 0, prefix, 0, prefix);
    let mut left_at = prefix;
    let mut right_at = prefix;
    for (anchor_left, anchor_right) in
        anchor_blocks(&left[..left_mid], &right[..right_mid], prefix)
    {
        push_segment(
            &mut segments,
            HexSegmentKind::Different,
            left_at,
            anchor_left,
            right_at,
            anchor_right,
        );
        push_segment(
            &mut segments,
            HexSegmentKind::Equal,
            anchor_left,
            anchor_left + ANCHOR_BYTES,
            anchor_right,
            anchor_right + ANCHOR_BYTES,
        );
        left_at = anchor_left + ANCHOR_BYTES;
        right_at = anchor_right + ANCHOR_BYTES;
    }
    push_segment(
        &mut segments,
        HexSegmentKind::Different,
        left_at,
        left_mid,
        right_at,
        right_mid,
    );
    push_segment(
        &mut segments,
        HexSegmentKind::Equal,
        left_mid,
        left.len(),
        right_mid,
        right.len(),
    );
    segments
}

/// The number of trailing bytes two buffers share.
fn common_suffix(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .rev()
        .zip(right.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
}

/// Append a segment, dropping runs that are empty on both sides and
/// merging a run that continues the previous segment of the same kind.
fn push_segment(
    segments: &mut Vec<HexSegment>,
    kind: HexSegmentKind,
    left_start: usize,
    left_end: usize,
    right_start: usize,
    right_end: usize,
) {
    let left_len = left_end - left_start;
    let right_len = right_end - right_start;
    if left_len == 0 && right_len == 0 {
        return;
    }
    if let Some(last) = segments.last_mut()
        && last.kind == kind
        && last.left_start + last.left_len == left_start
        && last.right_start + last.right_len == right_start
    {
        last.left_len += left_len;
        last.right_len += right_len;
        return;
    }
    segments.push(HexSegment {
        kind,
        left_start,
        left_len,
        right_start,
        right_len,
    });
}

/// Find the equal blocks that realign the two sides inside a different run.
///
/// Every [`ANCHOR_BYTES`]-sized block within [`ANCHOR_SCAN_LIMIT`] bytes of
/// the run start is indexed on the right side by a cheap FNV hash of its
/// bytes. Scanning the left side in order and taking the first still
/// available match yields a monotonically increasing list of
/// `(left offset, right offset)` anchors; the scan jumps over a block it
/// just anchored, so anchors never overlap. Candidates are compared byte
/// for byte before they anchor, so a hash collision can only cost an
/// anchor, never align the panes on unequal bytes.
fn anchor_blocks(
    left: &[u8],
    right: &[u8],
    from: usize,
) -> Vec<(usize, usize)> {
    // The search is bounded per side: a run longer than the window keeps
    // the panes apart, which is what the independent continuation is for.
    let left_stop = left.len().min(from + ANCHOR_SCAN_LIMIT);
    let right_stop = right.len().min(from + ANCHOR_SCAN_LIMIT);
    if left_stop - from < ANCHOR_BYTES || right_stop - from < ANCHOR_BYTES {
        return Vec::new();
    }

    let mut index: HashMap<u64, Vec<usize>> =
        HashMap::with_capacity(right_stop - from);
    for start in from..=right_stop - ANCHOR_BYTES {
        let hash = block_hash(&right[start..start + ANCHOR_BYTES]);
        index.entry(hash).or_default().push(start);
    }

    let mut anchors = Vec::new();
    let mut right_at = from;
    let mut start = from;
    while start + ANCHOR_BYTES <= left_stop {
        let block = &left[start..start + ANCHOR_BYTES];
        if let Some(candidates) = index.get(&block_hash(block)) {
            // The candidates are ascending, so the first one at or behind
            // the cursor is the closest realignment.
            let first = candidates.partition_point(|&at| at < right_at);
            let found = candidates[first..]
                .iter()
                .find(|&&at| right[at..at + ANCHOR_BYTES] == *block);
            if let Some(&at) = found {
                anchors.push((start, at));
                right_at = at + ANCHOR_BYTES;
                start += ANCHOR_BYTES;
                continue;
            }
        }
        start += 1;
    }
    anchors
}

/// The FNV-1a hash of a block, used only to narrow the anchor candidates.
fn block_hash(block: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in block {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The number of byte positions at which the two files disagree.
///
/// Inside a different run a position counts once when the sides hold
/// different bytes there, or when only the longer side holds a byte at all.
pub(crate) fn differing_bytes(
    left: &[u8],
    right: &[u8],
    segments: &[HexSegment],
) -> usize {
    segments
        .iter()
        .filter(|segment| segment.kind == HexSegmentKind::Different)
        .map(|segment| {
            let common = segment.left_len.min(segment.right_len);
            let mismatched = left
                [segment.left_start..segment.left_start + common]
                .iter()
                .zip(&right[segment.right_start..segment.right_start + common])
                .filter(|(a, b)| a != b)
                .count();
            mismatched + segment.left_len.max(segment.right_len) - common
        })
        .sum()
}

// ---------------------------------------------------------------------------
// Row model
// ---------------------------------------------------------------------------

/// The bytes one pane shows in a row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HexCell {
    /// Offset of the row's first byte in that side's file.
    pub(crate) start: usize,
    /// Number of bytes shown, at most [`ROW_BYTES`].
    pub(crate) len: usize,
}

/// One row of the hex panes. A side without bytes for the row shows a
/// blank area instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct HexRow {
    /// The left pane's bytes.
    pub(crate) left: Option<HexCell>,
    /// The right pane's bytes.
    pub(crate) right: Option<HexCell>,
    /// Whether the row comes from a different run.
    pub(crate) different: bool,
}

/// The rows both panes render, derived from the byte segments.
#[derive(Clone, Debug)]
pub(crate) struct HexLayout {
    rows: Vec<HexRow>,
    truncated: bool,
}

impl HexLayout {
    /// Build the rows of a comparison.
    ///
    /// An equal run contributes one row per [`ROW_BYTES`] of its
    /// (equal-length) run, showing the same bytes on both sides; a
    /// different run contributes as many rows as its longer side needs,
    /// each pane continuing with its own bytes from the run's start.
    pub(crate) fn from_segments(segments: &[HexSegment]) -> Self {
        let mut rows = Vec::new();
        let mut truncated = false;
        for segment in segments {
            let count = segment
                .left_len
                .div_ceil(ROW_BYTES)
                .max(segment.right_len.div_ceil(ROW_BYTES));
            for index in 0..count {
                if rows.len() >= MAX_ROWS {
                    truncated = true;
                    break;
                }
                rows.push(HexRow {
                    left: row_cell(
                        segment.left_start,
                        segment.left_len,
                        index,
                    ),
                    right: row_cell(
                        segment.right_start,
                        segment.right_len,
                        index,
                    ),
                    different: segment.kind == HexSegmentKind::Different,
                });
            }
            if truncated {
                break;
            }
        }
        Self { rows, truncated }
    }

    /// The byte rows of the comparison.
    pub(crate) fn rows(&self) -> &[HexRow] {
        &self.rows
    }

    /// Whether the rows stop at [`MAX_ROWS`].
    pub(crate) fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// The number of list rows: the byte rows plus the truncation notice
    /// when the display was cut off.
    pub(crate) fn row_count(&self) -> usize {
        self.rows.len() + usize::from(self.truncated)
    }
}

/// The cell of `index` in a run of `len` bytes starting at `start`, or
/// `None` once the run is exhausted.
fn row_cell(start: usize, len: usize, index: usize) -> Option<HexCell> {
    let offset = index * ROW_BYTES;
    // `then` rather than `then_some`: the cell arithmetic must not run for
    // an exhausted run, where `len - offset` would underflow.
    (offset < len).then(|| HexCell {
        start: start + offset,
        len: (len - offset).min(ROW_BYTES),
    })
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// One binary comparison: the loaded bytes of both files and the derived
/// difference model.
pub struct HexDiffSession {
    /// Focus handle shared with the tab entity.
    focus_handle: FocusHandle,
    /// Left file endpoint.
    left: Endpoint,
    /// Right file endpoint.
    right: Endpoint,
    /// Tab title.
    title: SharedString,
    /// The owning window's session manager, for follow-up tabs.
    manager: Option<Entity<GuiSessionManager>>,
    /// Left file bytes once loaded.
    left_bytes: Option<Arc<Vec<u8>>>,
    /// Right file bytes once loaded.
    right_bytes: Option<Arc<Vec<u8>>>,
    /// Aligned byte segments once computed.
    segments: Arc<Vec<HexSegment>>,
    /// Pane rows derived from [`Self::segments`].
    layout: Arc<HexLayout>,
    /// Byte positions at which the files disagree.
    differing: usize,
    /// Whether a load is in flight.
    loading: bool,
    /// Load failure, shown in the view instead of the panes.
    error: Option<String>,
    /// Counter that invalidates superseded loads.
    load_revision: u64,
}

impl HexDiffSession {
    /// Create a session for the given file pair; nothing is read until
    /// [`Self::ensure_loaded`].
    pub fn new(
        left: Endpoint,
        right: Endpoint,
        title: SharedString,
        manager: Option<Entity<GuiSessionManager>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            left,
            right,
            title,
            manager,
            left_bytes: None,
            right_bytes: None,
            segments: Arc::new(Vec::new()),
            layout: Arc::new(HexLayout::from_segments(&[])),
            differing: 0,
            loading: false,
            error: None,
            load_revision: 0,
        }
    }

    /// The session's focus handle.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// The tab title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// The compared endpoints, for "Compare files using ▸".
    pub fn pair_endpoints(&self) -> (Endpoint, Endpoint) {
        (self.left.clone(), self.right.clone())
    }

    /// Re-point the session at a new owning manager (tab moved between
    /// windows).
    pub fn set_manager(&mut self, manager: Entity<GuiSessionManager>) {
        self.manager = Some(manager);
    }

    /// The loaded left bytes, if any.
    pub fn left_bytes(&self) -> Option<&Arc<Vec<u8>>> {
        self.left_bytes.as_ref()
    }

    /// The loaded right bytes, if any.
    pub fn right_bytes(&self) -> Option<&Arc<Vec<u8>>> {
        self.right_bytes.as_ref()
    }

    /// The aligned byte segments.
    pub fn segments(&self) -> &Arc<Vec<HexSegment>> {
        &self.segments
    }

    /// The rows both panes render.
    pub fn layout(&self) -> &Arc<HexLayout> {
        &self.layout
    }

    /// The number of byte positions at which the files disagree.
    pub fn differing_bytes(&self) -> usize {
        self.differing
    }

    /// Whether a load is in flight.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The load failure, if the last load failed.
    pub fn error(&self) -> Option<&String> {
        self.error.as_ref()
    }

    /// Start the initial load unless bytes are already loaded or in
    /// flight.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.left_bytes.is_none() && !self.loading && self.error.is_none() {
            self.load(cx);
        }
    }

    /// Re-read both files and rebuild the diff model.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.load_revision += 1;
        let revision = self.load_revision;
        self.error = None;
        cx.notify();

        let left = self.left.clone();
        let right = self.right.clone();
        let weak = cx.entity().downgrade();

        // File I/O runs on the tokio runtime because the providers use
        // tokio-based IO internally. A missing file yields empty content:
        // one side of a comparison can legitimately not exist.
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let started = Instant::now();
            let read_side = |endpoint: Endpoint| async move {
                match endpoint.fs().read_path(endpoint.path(), None).await {
                    Ok(bytes) => Ok::<_, anyhow::Error>(bytes.to_vec()),
                    Err(FsError::NotFound { .. }) => Ok(Vec::new()),
                    Err(e) => Err(anyhow::anyhow!("{e}")),
                }
            };
            let left = read_side(left).await?;
            let right = read_side(right).await?;
            Ok::<_, anyhow::Error>((left, right, started.elapsed()))
        });

        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    let Some(entity) = weak.upgrade() else { return };
                    entity.update(cx, |session, cx| {
                        if revision != session.load_revision {
                            return;
                        }
                        match result {
                            Ok((left, right, elapsed)) => {
                                session.set_bytes(left, right, cx);
                                log::push(
                                    LogLevel::Info,
                                    format!(
                                        "Hex comparison completed in {} ms",
                                        elapsed.as_millis()
                                    ),
                                    cx,
                                );
                            }
                            Err(e) => {
                                session.error =
                                    Some(format!("Comparison failed: {e}"));
                                log::push(
                                    LogLevel::Error,
                                    format!("Hex comparison failed: {e}"),
                                    cx,
                                );
                            }
                        }
                        session.loading = false;
                        cx.notify();
                    });
                });
            }
        })
        .detach();
    }

    /// Replace the compared bytes and rebuild the diff model from them.
    pub(crate) fn set_bytes(
        &mut self,
        left: Vec<u8>,
        right: Vec<u8>,
        _cx: &mut Context<Self>,
    ) {
        let segments = Arc::new(hex_segments(&left, &right));
        self.differing = differing_bytes(&left, &right, &segments);
        self.layout = Arc::new(HexLayout::from_segments(&segments));
        self.left_bytes = Some(Arc::new(left));
        self.right_bytes = Some(Arc::new(right));
        self.segments = segments;
    }
}

impl Focusable for HexDiffSession {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// The side-by-side hex compare view.
pub struct HexDiffView {
    /// The compare session this view renders.
    state: Entity<HexDiffSession>,
    /// Scroll state of the left hex pane.
    left_list: ListState,
    /// Scroll state of the right hex pane.
    right_list: ListState,
    /// Row count the lists were last reset to.
    list_count: usize,
    /// Row height the lists were last seeded with.
    list_row_h: Pixels,
    /// Re-render whenever the session notifies.
    _state_sub: Subscription,
}

impl HexDiffView {
    /// Create a view over an existing session; starts the initial load.
    pub fn new(state: Entity<HexDiffSession>, cx: &mut Context<Self>) -> Self {
        let left_list = ListState::new(0, ListAlignment::Top, px(100.));
        let right_list = ListState::new(0, ListAlignment::Top, px(100.));
        // Wheel-scrolling one pane scrolls the other to the same logical
        // offset. The handler runs while the source list still holds a
        // mutable borrow of its own state, so reading the offset and
        // scrolling the target must be deferred to the next cycle.
        // `scroll_to` never fires the handler, so this cannot loop.
        left_list.set_scroll_handler({
            let (src, dst) = (left_list.clone(), right_list.clone());
            let weak = cx.entity().downgrade();
            move |_: &ListScrollEvent, _window: &mut Window, cx: &mut App| {
                let (src, dst, weak) =
                    (src.clone(), dst.clone(), weak.clone());
                cx.defer(move |cx| {
                    let top = src.logical_scroll_top();
                    dst.scroll_to(top);
                    let _ = weak.update(cx, |_, cx| cx.notify());
                });
            }
        });
        right_list.set_scroll_handler({
            let (src, dst) = (right_list.clone(), left_list.clone());
            let weak = cx.entity().downgrade();
            move |_: &ListScrollEvent, _window: &mut Window, cx: &mut App| {
                let (src, dst, weak) =
                    (src.clone(), dst.clone(), weak.clone());
                cx.defer(move |cx| {
                    let top = src.logical_scroll_top();
                    dst.scroll_to(top);
                    let _ = weak.update(cx, |_, cx| cx.notify());
                });
            }
        });

        let _state_sub = cx.observe(&state, |_, _, cx| cx.notify());
        state.update(cx, |session, cx| session.ensure_loaded(cx));
        Self {
            state,
            left_list,
            right_list,
            list_count: 0,
            list_row_h: px(0.),
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

    /// The compared endpoints, for "Compare files using ▸".
    pub fn pair_endpoints(&self, cx: &App) -> (Endpoint, Endpoint) {
        self.state.read(cx).pair_endpoints()
    }

    /// Re-point the underlying session at a new owning manager.
    pub fn set_manager(
        &self,
        manager: Entity<GuiSessionManager>,
        cx: &mut App,
    ) {
        self.state
            .update(cx, |session, _| session.set_manager(manager));
    }

    /// Render the view's toolbar row (row 2) for the window root.
    pub fn render_view_toolbar(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        let weak = self.state.clone();
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .child(
                Button::new("hex_reload")
                    .icon(gpui::assets::IconName::RefreshCw)
                    .label("Reload")
                    .ghost()
                    .small()
                    .on_click(move |_, _, cx| {
                        weak.update(cx, |session, cx| session.load(cx));
                    }),
            )
            .into_any_element()
    }

    /// The two scroll-synchronized hex panes of a loaded comparison.
    fn render_panes(
        &mut self,
        window: &mut Window,
        layout: Arc<HexLayout>,
        left_bytes: Arc<Vec<u8>>,
        right_bytes: Arc<Vec<u8>>,
        style: &HexStyle,
    ) -> AnyElement {
        // Seed the lists with the uniform row height so the scrollbar
        // thumb is right from the first frame; reset only when the row
        // count or the rem-derived height changed, since a reset per frame
        // would destroy the scroll position.
        let row_h = px(window.rem_size().as_f32() * 1.5);
        let count = layout.row_count();
        if self.list_count != count || self.list_row_h != row_h {
            self.left_list.reset_with_uniform_height(count, row_h);
            self.right_list.reset_with_uniform_height(count, row_h);
            self.list_count = count;
            self.list_row_h = row_h;
        }

        let left_el = list(self.left_list.clone(), {
            let layout = layout.clone();
            let left_bytes = left_bytes.clone();
            let right_bytes = right_bytes.clone();
            let style = style.clone();
            move |d, _window, _cx| {
                render_hex_row(
                    Side::Left,
                    d,
                    &layout,
                    &left_bytes,
                    &right_bytes,
                    &style,
                )
            }
        })
        .size_full()
        .min_h_0();
        let right_el = list(self.right_list.clone(), {
            let layout = layout.clone();
            let left_bytes = left_bytes.clone();
            let right_bytes = right_bytes.clone();
            let style = style.clone();
            move |d, _window, _cx| {
                render_hex_row(
                    Side::Right,
                    d,
                    &layout,
                    &left_bytes,
                    &right_bytes,
                    &style,
                )
            }
        })
        .size_full()
        .min_h_0();

        h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(left_el),
            )
            .child(div().w_px().flex_shrink_0().bg(style.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(right_el),
            )
            .into_any_element()
    }
}

impl Render for HexDiffView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        // A hex session has no own color overrides, so the user's global
        // choices (Options dialog) decide what an important difference
        // looks like.
        let overrides =
            crate::color::merge_overrides(&ColorOverrides::default(), cx);
        let style = HexStyle {
            foreground: theme.foreground,
            offset: theme.muted_foreground,
            // Difference cells tint with the severity color at low alpha,
            // the same treatment the text view gives its difference rows.
            different: {
                let mut color =
                    ColorRole::Important.resolve(&theme, &overrides);
                color.a = 0.35;
                color
            },
            blank: theme.muted,
            border: theme.border,
            mono: theme.mono_font_family.clone(),
        };

        let (
            focus_handle,
            left_name,
            right_name,
            left_len,
            right_len,
            regions,
            differing,
            loading,
            error,
            layout,
            left_bytes,
            right_bytes,
        ) = {
            let session = self.state.read(cx);
            (
                session.focus_handle(),
                base_name(&session.left),
                base_name(&session.right),
                session.left_bytes().map_or(0, |b| b.len()),
                session.right_bytes().map_or(0, |b| b.len()),
                session
                    .segments()
                    .iter()
                    .filter(|s| s.kind == HexSegmentKind::Different)
                    .count(),
                session.differing_bytes(),
                session.is_loading(),
                session.error().cloned(),
                session.layout().clone(),
                session.left_bytes().cloned().unwrap_or_default(),
                session.right_bytes().cloned().unwrap_or_default(),
            )
        };

        let body = if let Some(err) = &error {
            v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child("The comparison could not be loaded.")
                .child(div().text_xs().child(err.clone()))
                .into_any_element()
        } else if loading {
            h_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme.muted_foreground)
                .child(Spinner::new())
                .child("Reading files…")
                .into_any_element()
        } else {
            self.render_panes(
                window,
                layout.clone(),
                left_bytes,
                right_bytes,
                &style,
            )
        };

        let right_status = if loading {
            h_flex()
                .items_center()
                .gap_1p5()
                .child(Spinner::new().xsmall())
                .child("Reading files…")
                .into_any_element()
        } else if let Some(err) = &error {
            div()
                .text_color(theme.danger)
                .child(err.clone())
                .into_any_element()
        } else {
            div()
                .child(format!("{} rows", layout.row_count()))
                .into_any_element()
        };

        h_flex()
            .key_context("HexCompare")
            .track_focus(&focus_handle)
            .size_full()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .text_sm()
            .child(
                // The header names each file above the pane that shows it.
                h_flex()
                    .w_full()
                    .flex_shrink_0()
                    .items_stretch()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .px_3()
                            .py_1p5()
                            .child(left_name),
                    )
                    .child(div().w_px().flex_shrink_0().bg(theme.border))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .px_3()
                            .py_1p5()
                            .child(right_name),
                    ),
            )
            .child(body)
            .child(
                StatusBar::new()
                    .left(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child("Hex Compare")
                            .child(format!(
                                "Left {}",
                                format_size(left_len as u64)
                            ))
                            .child(format!(
                                "Right {}",
                                format_size(right_len as u64)
                            ))
                            .child(format!("{regions} difference region(s)"))
                            .child(format!("{differing} different bytes"))
                            .when(layout.is_truncated(), |bar| {
                                bar.child("display truncated")
                            }),
                    )
                    .right(right_status),
            )
    }
}

// ---------------------------------------------------------------------------
// Hex row rendering
// ---------------------------------------------------------------------------

/// The style a hex row is painted with.
#[derive(Clone, Debug)]
struct HexStyle {
    /// Text color of matching byte and ASCII cells.
    foreground: Hsla,
    /// Text color of the offset column and of the truncation notice.
    offset: Hsla,
    /// Background tint of a byte without an equal counterpart.
    different: Hsla,
    /// Background of the blank area of a side whose run ended.
    blank: Hsla,
    /// Color of the divider between the two panes.
    border: Hsla,
    /// Monospace font family of every hex row.
    mono: SharedString,
}

/// Filler for a hex cell a side does not have. Non-breaking spaces keep a
/// monospace cell at its full width, so the columns stay aligned.
const BLANK_HEX: &str = "\u{a0}\u{a0}";

/// Filler for an ASCII cell a side does not have.
const BLANK_ASCII: &str = "\u{a0}";

/// Filler for the offset column of a row a side does not have.
const BLANK_OFFSET: &str = "\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}\u{a0}";

/// The two-digit uppercase hex label of every byte value, built once so a
/// scrolled frame does not allocate a string per cell.
fn hex_labels() -> &'static [SharedString; 256] {
    static LABELS: OnceLock<[SharedString; 256]> = OnceLock::new();
    LABELS.get_or_init(|| array::from_fn(|byte| format!("{byte:02X}").into()))
}

/// The printable rendering of a byte in the ASCII column.
fn ascii_char(byte: u8) -> char {
    if (0x20..=0x7e).contains(&byte) {
        char::from(byte)
    } else {
        '.'
    }
}

/// One cell of a hex row: a monospace box that fills the row's height and
/// carries the difference or blank-area background.
fn hex_cell(content: SharedString, background: Option<Hsla>) -> AnyElement {
    div()
        .flex_shrink_0()
        .justify_center()
        .text_center()
        .when_some(background, |el, bg| el.bg(bg))
        .child(content)
        .into_any_element()
}

/// Render one 16-byte row of one pane: the offset column, the hex cells,
/// and the ASCII column.
fn render_hex_row(
    side: Side,
    d: usize,
    layout: &HexLayout,
    left_bytes: &[u8],
    right_bytes: &[u8],
    style: &HexStyle,
) -> AnyElement {
    // The list has one row more than the model when the display was cut
    // off; that trailing row carries the notice.
    let Some(row) = layout.rows().get(d) else {
        return render_truncated_notice(style);
    };
    let (cell, other_cell, bytes, other_bytes) = match side {
        Side::Left => (row.left, row.right, left_bytes, right_bytes),
        Side::Right => (row.right, row.left, right_bytes, left_bytes),
    };
    let offset: SharedString = match cell {
        Some(cell) => {
            format!("{:0width$X}", cell.start, width = OFFSET_DIGITS).into()
        }
        None => BLANK_OFFSET.into(),
    };

    let mut hex_cells = Vec::with_capacity(ROW_BYTES);
    let mut ascii_cells = Vec::with_capacity(ROW_BYTES);
    for column in 0..ROW_BYTES {
        let byte = cell
            .filter(|cell| column < cell.len)
            .and_then(|cell| bytes.get(cell.start + column).copied());
        let counterpart = other_cell
            .filter(|cell| column < cell.len)
            .and_then(|cell| other_bytes.get(cell.start + column).copied());
        // A byte is highlighted only when its counterpart is missing or
        // holds a different value; a different run can still contain long
        // stretches that match byte for byte.
        let differs = byte.is_some() && counterpart != byte;
        let background = match byte {
            None => Some(style.blank),
            Some(_) if differs => Some(style.different),
            Some(_) => None,
        };
        let label = match byte {
            Some(byte) => hex_labels()[usize::from(byte)].clone(),
            None => BLANK_HEX.into(),
        };
        hex_cells.push(hex_cell(label, background));
        let ascii = match byte {
            Some(byte) => ascii_char(byte).to_string().into(),
            None => BLANK_ASCII.into(),
        };
        ascii_cells.push(hex_cell(ascii, background));
    }

    h_flex()
        .w_full()
        .flex_shrink_0()
        .items_stretch()
        .gap_2()
        .px_2()
        .overflow_hidden()
        .whitespace_nowrap()
        .font_family(style.mono.clone())
        .text_xs()
        .text_color(style.foreground)
        .child(
            div()
                .flex_shrink_0()
                .justify_center()
                .text_color(style.offset)
                .child(offset),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .items_stretch()
                .gap_0p5()
                .children(hex_cells),
        )
        .child(
            h_flex()
                .flex_shrink_0()
                .items_stretch()
                .children(ascii_cells),
        )
        .into_any_element()
}

/// The row that closes a comparison whose display was cut off at
/// [`MAX_ROWS`].
fn render_truncated_notice(style: &HexStyle) -> AnyElement {
    div()
        .w_full()
        .flex_shrink_0()
        .px_2()
        .py_1()
        .font_family(style.mono.clone())
        .text_xs()
        .text_color(style.offset)
        .child(format!(
            "… display truncated after {} per side.",
            format_size((MAX_ROWS * ROW_BYTES) as u64)
        ))
        .into_any_element()
}

/// File name of an endpoint, for the tab title.
fn base_name(endpoint: &Endpoint) -> String {
    endpoint
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| endpoint.display())
}

// ---------------------------------------------------------------------------
// Tab factory
// ---------------------------------------------------------------------------

/// Create a hex compare tab view for the given file pair.
pub(crate) fn new_hex_diff_view(
    left: Endpoint,
    right: Endpoint,
    manager: Option<Entity<GuiSessionManager>>,
    cx: &mut App,
) -> Entity<HexDiffView> {
    let title = left.pair_title(&right);
    let session = cx
        .new(|cx| HexDiffSession::new(left, right, title.into(), manager, cx));
    cx.new(|cx| HexDiffView::new(session, cx))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `len` copies of `byte`.
    fn run(byte: u8, len: usize) -> Vec<u8> {
        vec![byte; len]
    }

    #[test]
    fn identical_files_yield_one_equal_segment() {
        let left = vec![1u8, 2, 3, 4];
        let right = vec![1u8, 2, 3, 4];
        let segments = hex_segments(&left, &right);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].kind, HexSegmentKind::Equal);
        assert_eq!(segments[0].left_len, 4);
    }

    #[test]
    fn common_prefix_and_suffix_are_equal() {
        let left = vec![1u8, 2, 9, 3, 4];
        let right = vec![1u8, 2, 8, 3, 4];
        let segments = hex_segments(&left, &right);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].kind, HexSegmentKind::Equal);
        assert_eq!(segments[0].left_len, 2);
        assert_eq!(segments[1].kind, HexSegmentKind::Different);
        assert_eq!(segments[1].left_start, 2);
        assert_eq!(segments[2].kind, HexSegmentKind::Equal);
        assert_eq!(segments[2].left_start, 3);
    }

    #[test]
    fn disjoint_files_yield_one_different_segment() {
        let left = vec![1u8, 2];
        let right = vec![3u8, 4, 5];
        let segments = hex_segments(&left, &right);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].kind, HexSegmentKind::Different);
        assert_eq!(segments[0].left_len, 2);
        assert_eq!(segments[0].right_len, 3);
    }

    #[test]
    fn an_equal_block_reattaches_the_panes_after_an_insertion() {
        // The left side inserts four bytes; the shared block behind them
        // is followed by more differences on both sides, so it is neither
        // the common prefix nor the common suffix.
        let mut left = run(0x11, 16);
        left.extend(run(0xee, 4));
        left.extend(run(0x22, 16));
        left.extend(run(0x33, 16));
        let mut right = run(0x11, 16);
        right.extend(run(0x22, 16));
        right.extend(run(0x44, 16));

        let segments = hex_segments(&left, &right);
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[0].kind, HexSegmentKind::Equal);
        assert_eq!(segments[0].left_len, 16);
        // The insertion is a left-only run: the right side has no bytes
        // for it.
        assert_eq!(segments[1].kind, HexSegmentKind::Different);
        assert_eq!(segments[1].left_start, 16);
        assert_eq!(segments[1].left_len, 4);
        assert_eq!(segments[1].right_len, 0);
        // The shared block realigns both sides, even though the insertion
        // moved it off the row grid on the left.
        assert_eq!(segments[2].kind, HexSegmentKind::Equal);
        assert_eq!(segments[2].left_start, 20);
        assert_eq!(segments[2].right_start, 16);
        assert_eq!(segments[2].left_len, 16);
        assert_eq!(segments[3].kind, HexSegmentKind::Different);
        assert_eq!(segments[3].left_start, 36);
        assert_eq!(segments[3].right_start, 32);
    }

    #[test]
    fn adjacent_anchors_merge_into_one_equal_run() {
        // A whole row inserted in front of a long shared block: every row
        // of that block is an anchor of its own, and the panes must see
        // one equal run, not a segment per row.
        let mut left = run(0x11, 16);
        left.extend(run(0xee, 16));
        left.extend(run(0x22, 64));
        left.extend(run(0x33, 16));
        let mut right = run(0x11, 16);
        right.extend(run(0x22, 64));
        right.extend(run(0x44, 16));

        let segments = hex_segments(&left, &right);
        assert_eq!(segments.len(), 4);
        assert_eq!(segments[1].kind, HexSegmentKind::Different);
        assert_eq!(segments[1].left_len, 16);
        assert_eq!(segments[1].right_len, 0);
        assert_eq!(segments[2].kind, HexSegmentKind::Equal);
        assert_eq!(segments[2].left_start, 32);
        assert_eq!(segments[2].right_start, 16);
        assert_eq!(segments[2].left_len, 64);
        assert_eq!(segments[2].right_len, 64);
        assert_eq!(segments[3].kind, HexSegmentKind::Different);
    }

    #[test]
    fn anchors_need_a_full_block_of_equal_bytes() {
        // Only fifteen shared bytes: too short to be sure the panes are
        // really aligned again, so the run stays one different segment.
        let mut left = run(0x11, 16);
        left.extend(run(0xee, 4));
        left.extend(run(0x22, 15));
        left.extend(run(0x33, 16));
        let mut right = run(0x11, 16);
        right.extend(run(0x22, 15));
        right.extend(run(0x44, 16));

        let segments = hex_segments(&left, &right);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].kind, HexSegmentKind::Different);
    }

    #[test]
    fn equal_runs_pair_both_sides_row_by_row() {
        let segments = vec![HexSegment {
            kind: HexSegmentKind::Equal,
            left_start: 0,
            left_len: 17,
            right_start: 0,
            right_len: 17,
        }];
        let layout = HexLayout::from_segments(&segments);
        assert_eq!(layout.rows().len(), 2);
        assert!(!layout.is_truncated());
        assert_eq!(layout.row_count(), 2);
        assert_eq!(
            layout.rows()[1],
            HexRow {
                left: Some(HexCell { start: 16, len: 1 }),
                right: Some(HexCell { start: 16, len: 1 }),
                different: false,
            }
        );
    }

    #[test]
    fn different_runs_continue_independently_and_blank_the_shorter_side() {
        let segments = vec![HexSegment {
            kind: HexSegmentKind::Different,
            left_start: 32,
            left_len: 4,
            right_start: 16,
            right_len: 20,
        }];
        let layout = HexLayout::from_segments(&segments);
        // The longer side decides the row count.
        assert_eq!(layout.rows().len(), 2);
        assert_eq!(
            layout.rows()[0],
            HexRow {
                left: Some(HexCell { start: 32, len: 4 }),
                right: Some(HexCell { start: 16, len: 16 }),
                different: true,
            }
        );
        assert_eq!(
            layout.rows()[1],
            HexRow {
                left: None,
                right: Some(HexCell { start: 32, len: 4 }),
                different: true,
            }
        );
    }

    #[test]
    fn the_layout_caps_the_rows_and_flags_the_display() {
        let segments = vec![HexSegment {
            kind: HexSegmentKind::Different,
            left_start: 0,
            left_len: (MAX_ROWS + 3) * ROW_BYTES,
            right_start: 0,
            right_len: ROW_BYTES,
        }];
        let layout = HexLayout::from_segments(&segments);
        assert_eq!(layout.rows().len(), MAX_ROWS);
        assert!(layout.is_truncated());
        // The extra list row is the truncation notice.
        assert_eq!(layout.row_count(), MAX_ROWS + 1);
    }

    #[test]
    fn differing_bytes_counts_mismatched_positions() {
        let left = vec![1u8, 2, 3, 4, 5];
        let right = vec![1u8, 9, 3, 4];
        let segments = hex_segments(&left, &right);
        // Byte 2 differs and byte 5 has no counterpart.
        assert_eq!(differing_bytes(&left, &right, &segments), 2);
        // Equal files have nothing to count.
        assert_eq!(
            differing_bytes(&left, &left, &hex_segments(&left, &left)),
            0
        );
    }

    #[test]
    fn cell_labels_cover_every_byte_value() {
        assert_eq!(hex_labels()[0].to_string(), "00");
        assert_eq!(hex_labels()[0xaf].to_string(), "AF");
        assert_eq!(hex_labels()[255].to_string(), "FF");
        assert_eq!(ascii_char(b'A'), 'A');
        assert_eq!(ascii_char(b' '), ' ');
        assert_eq!(ascii_char(0x07), '.');
        assert_eq!(ascii_char(0xff), '.');
    }

    #[gpui::test]
    fn session_derives_its_layout_from_the_bytes(cx: &mut App) {
        let session = cx.new(|cx| {
            HexDiffSession::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "left vs right".into(),
                None,
                cx,
            )
        });
        // Nothing is loaded before the view asks for it.
        session.read_with(cx, |session, _| {
            assert!(!session.is_loading());
            assert!(session.error().is_none());
            assert!(session.left_bytes().is_none());
            assert_eq!(session.layout().row_count(), 0);
        });

        session.update(cx, |session, cx| {
            session.set_bytes(run(0x11, 20), run(0x11, 16), cx);
        });
        session.read_with(cx, |session, cx| {
            assert_eq!(session.title(), "left vs right");
            assert_eq!(session.left_bytes().map(|b| b.len()), Some(20));
            assert_eq!(session.differing_bytes(), 4);
            // Four unmatched tail bytes need one row of their own.
            assert_eq!(session.layout().rows().len(), 2);
            assert_eq!(session.segments().len(), 2);
            let (left, right) = session.pair_endpoints();
            assert_eq!(left.path(), std::path::Path::new("/left"));
            assert_eq!(right.path(), std::path::Path::new("/right"));
            let _ = cx;
        });
    }
}
