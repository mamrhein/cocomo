// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Picture compare view (UI spec section 7, new session types).
//!
//! [`PictureDiffSession`] owns one image comparison: the decoded pixels of
//! both files and the per-pixel difference derived from them.
//! [`PictureDiffView`] renders the two images side by side with a
//! difference overlay and a status bar.
//!
//! The panes display the raw file bytes through GPUI's `img` element
//! ([`gpui::ImageSource::Image`], decoded asynchronously by GPUI's asset
//! loader), so a file whose pixels this app cannot decode may still
//! display. When both sides decode at equal size, the right pane can be
//! replaced by a difference image built as a [`gpui::RenderImage`] from
//! the per-pixel delta, in GPUI's BGRA frame order.

use std::{sync::Arc, time::Instant};

use cocomo_lib::{Endpoint, FsError, NodeFileSystem};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, Hsla, Image,
    ImageFormat, ImageSource, IntoElement, ObjectFit, Render, RenderImage,
    Rgba, SharedString, Subscription, Window,
    base::{Disableable as _, Selectable as _},
    component::{
        ActiveTheme, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        spinner::Spinner,
        status_bar::StatusBar,
        v_flex,
    },
    div, img,
    prelude::*,
};

use crate::{
    log::{self, LogLevel},
    session_manager::GuiSessionManager,
};

// ---------------------------------------------------------------------------
// Diff model
// ---------------------------------------------------------------------------

/// The decoded pixel content of one image side.
#[derive(Clone, Debug)]
pub(crate) struct PictureSide {
    /// Width in pixels.
    pub(crate) width: u32,
    /// Height in pixels.
    pub(crate) height: u32,
    /// RGBA pixels, row-major, 4 bytes per pixel.
    pub(crate) rgba: Arc<Vec<u8>>,
}

/// The per-pixel difference between two sides of equal size.
#[derive(Clone, Debug)]
pub(crate) struct PictureDiff {
    /// One byte per pixel: 0 for equal, otherwise the largest per-channel
    /// delta scaled to 0..=255.
    pub(crate) delta: Arc<Vec<u8>>,
    /// Number of pixels that differ.
    pub(crate) differing_pixels: usize,
}

/// Compute the per-pixel delta of two equal-sized RGBA buffers.
///
/// Buffers of different lengths produce no delta map; the caller reports
/// the dimension mismatch instead.
pub(crate) fn pixel_delta(left: &[u8], right: &[u8]) -> Option<PictureDiff> {
    if left.len() != right.len() || !left.len().is_multiple_of(4) {
        return None;
    }
    let pixels = left.len() / 4;
    let mut delta = vec![0u8; pixels];
    let mut differing = 0usize;
    for (ix, d) in delta.iter_mut().enumerate() {
        let o = ix * 4;
        let mut max = 0i16;
        for c in 0..4 {
            let diff = (left[o + c] as i16 - right[o + c] as i16).abs();
            max = max.max(diff);
        }
        if max > 0 {
            differing += 1;
        }
        *d = max.min(255) as u8;
    }
    Some(PictureDiff {
        delta: Arc::new(delta),
        differing_pixels: differing,
    })
}

/// The color of pixels that do not differ in the difference image.
const EQUAL_PIXEL: [u8; 3] = [0x40, 0x40, 0x40];

/// Build the difference image as a BGRA buffer, GPUI's frame order.
///
/// Pixels with a zero delta become a dim gray; differing pixels take
/// the danger color scaled by the delta intensity, so a small delta
/// reads dark and a full-scale delta reads as pure danger. The buffer
/// is BGRA because every GPUI image decoder swaps R and B before
/// uploading a frame.
pub(crate) fn diff_image_bgra(
    width: u32,
    height: u32,
    delta: &[u8],
    danger: [u8; 3],
) -> Vec<u8> {
    let pixels = width as usize * height as usize;
    debug_assert_eq!(delta.len(), pixels);
    let mut out = Vec::with_capacity(pixels * 4);
    for d in delta.iter().take(pixels) {
        if *d == 0 {
            out.extend_from_slice(&[
                EQUAL_PIXEL[0],
                EQUAL_PIXEL[1],
                EQUAL_PIXEL[2],
                255,
            ]);
        } else {
            // Integer scaling keeps the mapping exact and testable.
            let scale = |c: u8| ((c as u32 * *d as u32) / 255) as u8;
            out.extend_from_slice(&[
                scale(danger[2]),
                scale(danger[1]),
                scale(danger[0]),
                255,
            ]);
        }
    }
    out
}

/// Decode one side's pixels, if the bytes are a supported image.
fn decode_side(bytes: &[u8]) -> Option<PictureSide> {
    let pixels = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = pixels.dimensions();
    Some(PictureSide {
        width,
        height,
        rgba: Arc::new(pixels.into_raw()),
    })
}

/// Wrap the raw bytes in a GPUI display image, if the detected format
/// is one GPUI can decode here.
fn display_image(bytes: &[u8]) -> Option<Image> {
    let format = gpui_image_format(image::guess_format(bytes).ok()?)?;
    Some(Image::from_bytes(format, bytes.to_vec()))
}

/// Map a detected format onto the GPUI image formats this app displays.
fn gpui_image_format(format: image::ImageFormat) -> Option<ImageFormat> {
    match format {
        image::ImageFormat::Png => Some(ImageFormat::Png),
        image::ImageFormat::Jpeg => Some(ImageFormat::Jpeg),
        image::ImageFormat::Gif => Some(ImageFormat::Gif),
        image::ImageFormat::WebP => Some(ImageFormat::Webp),
        image::ImageFormat::Bmp => Some(ImageFormat::Bmp),
        _ => None,
    }
}

/// The dimensions text for one side in the status bar.
fn dimensions_text(side: Option<&PictureSide>) -> String {
    match side {
        Some(side) => format!("{}×{}", side.width, side.height),
        None => "—".to_string(),
    }
}

/// The status bar's right-side summary: decode notices, a dimension
/// mismatch, or the differing pixel count with its percentage.
fn difference_status(
    left: &Option<Arc<PictureSide>>,
    right: &Option<Arc<PictureSide>>,
    diff: Option<&Arc<PictureDiff>>,
) -> String {
    let mut notices = Vec::new();
    if left.is_none() {
        notices.push("left side is not a decodable image");
    }
    if right.is_none() {
        notices.push("right side is not a decodable image");
    }
    if !notices.is_empty() {
        return notices.join(", ");
    }
    match diff {
        Some(diff) => {
            let total = left
                .as_ref()
                .map_or(0, |s| s.width as usize * s.height as usize);
            let pct = if total == 0 {
                0.0
            } else {
                diff.differing_pixels as f64 * 100.0 / total as f64
            };
            format!("{} differing pixels ({pct:.2}%)", diff.differing_pixels)
        }
        None => "sizes differ".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// One image comparison: the decoded pixels of both files and the derived
/// difference.
pub struct PictureDiffSession {
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
    /// Raw left file bytes once loaded (for the `img` panes).
    left_bytes: Option<Arc<Vec<u8>>>,
    /// Raw right file bytes once loaded.
    right_bytes: Option<Arc<Vec<u8>>>,
    /// Decoded left side, if the file decoded.
    left_side: Option<Arc<PictureSide>>,
    /// Decoded right side, if the file decoded.
    right_side: Option<Arc<PictureSide>>,
    /// Per-pixel difference, when both sides decoded at equal size.
    diff: Option<Arc<PictureDiff>>,
    /// Left pane's display image, when the format is displayable.
    left_image: Option<Arc<Image>>,
    /// Right pane's display image, when the format is displayable.
    right_image: Option<Arc<Image>>,
    /// Whether the right pane shows the computed difference.
    difference_view: bool,
    /// Whether a load is in flight.
    loading: bool,
    /// Load failure, shown in the view instead of the panes.
    error: Option<String>,
    /// Counter that invalidates superseded loads.
    load_revision: u64,
}

impl PictureDiffSession {
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
            left_side: None,
            right_side: None,
            diff: None,
            left_image: None,
            right_image: None,
            difference_view: false,
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

    /// The decoded left side, if any.
    pub fn left_side(&self) -> Option<&Arc<PictureSide>> {
        self.left_side.as_ref()
    }

    /// The decoded right side, if any.
    pub fn right_side(&self) -> Option<&Arc<PictureSide>> {
        self.right_side.as_ref()
    }

    /// The per-pixel difference, when both sides decoded at equal size.
    pub fn diff(&self) -> Option<&Arc<PictureDiff>> {
        self.diff.as_ref()
    }

    /// The left pane's display image, if the format is displayable.
    pub fn left_image(&self) -> Option<&Arc<Image>> {
        self.left_image.as_ref()
    }

    /// The right pane's display image, if the format is displayable.
    pub fn right_image(&self) -> Option<&Arc<Image>> {
        self.right_image.as_ref()
    }

    /// Whether the right pane shows the computed difference.
    pub fn is_difference_view(&self) -> bool {
        self.difference_view
    }

    /// Switch the right pane between the right image and the difference.
    pub fn toggle_difference_view(&mut self, cx: &mut Context<Self>) {
        self.difference_view = !self.difference_view;
        cx.notify();
    }

    /// Whether a load is in flight.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The load failure, if the last load failed.
    pub fn error(&self) -> Option<&String> {
        self.error.as_ref()
    }

    /// Start the initial load unless pixels are already loaded or in
    /// flight.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.left_bytes.is_none() && !self.loading && self.error.is_none() {
            self.load(cx);
        }
    }

    /// Re-read both files and rebuild the difference.
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
        // tokio-based IO internally.
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let started = Instant::now();
            let read_side = |endpoint: Endpoint| async move {
                match endpoint.fs().read_path(endpoint.path(), None).await {
                    Ok(bytes) => Ok::<_, anyhow::Error>(bytes.to_vec()),
                    Err(FsError::NotFound { .. }) => Ok(Vec::new()),
                    Err(e) => Err(anyhow::anyhow!("{e}")),
                }
            };
            // Decoding is CPU work, so it runs here rather than on the
            // main thread. A side whose pixels fail to decode keeps its
            // raw bytes for the `img` pane; only the pixel view drops.
            let prepare_side = |bytes: Vec<u8>| {
                let side = decode_side(&bytes).map(Arc::new);
                let display = display_image(&bytes).map(Arc::new);
                (Arc::new(bytes), side, display)
            };
            let left = prepare_side(read_side(left).await?);
            let right = prepare_side(read_side(right).await?);
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
                                let (left_bytes, left_side, left_image) = left;
                                let (right_bytes, right_side, right_image) =
                                    right;
                                session.left_bytes = Some(left_bytes);
                                session.right_bytes = Some(right_bytes);
                                session.left_side = left_side;
                                session.right_side = right_side;
                                session.left_image = left_image;
                                session.right_image = right_image;
                                session.diff = match (
                                    session.left_side.as_deref(),
                                    session.right_side.as_deref(),
                                ) {
                                    (Some(l), Some(r))
                                        if l.width == r.width
                                            && l.height == r.height =>
                                    {
                                        pixel_delta(&l.rgba, &r.rgba)
                                            .map(Arc::new)
                                    }
                                    _ => None,
                                };
                                log::push(
                                    LogLevel::Info,
                                    format!(
                                        "Picture comparison completed in {} \
                                         ms",
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
                                    format!("Picture comparison failed: {e}"),
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
}

impl Focusable for PictureDiffSession {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// The difference image cache: the diff it was built from, its pixel
/// size, the danger color it was tinted with, and the render result.
type DiffImageCache =
    (Arc<PictureDiff>, (u32, u32), [u8; 3], Arc<RenderImage>);

/// The side-by-side picture compare view.
pub struct PictureDiffView {
    /// The compare session this view renders.
    state: Entity<PictureDiffSession>,
    /// The rendered difference image, cached until the diff, its size,
    /// or the theme's danger color changes.
    diff_image: Option<DiffImageCache>,
    /// Re-render whenever the session notifies.
    _state_sub: Subscription,
}

impl PictureDiffView {
    /// Create a view over an existing session; starts the initial load.
    pub fn new(
        state: Entity<PictureDiffSession>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _state_sub = cx.observe(&state, |_, _, cx| cx.notify());
        state.update(cx, |session, cx| session.ensure_loaded(cx));
        Self {
            state,
            diff_image: None,
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
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (has_diff, show_difference) = {
            let session = self.state.read(cx);
            (session.diff().is_some(), session.is_difference_view())
        };
        let reload_state = self.state.clone();
        let toggle_state = self.state.clone();
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .px_2()
            .child(
                Button::new("picture_reload")
                    .icon(gpui::assets::IconName::RefreshCw)
                    .label("Reload")
                    .ghost()
                    .small()
                    .on_click(move |_, _, cx| {
                        reload_state.update(cx, |session, cx| {
                            session.load(cx);
                        });
                    }),
            )
            .child(
                Button::new("picture_difference")
                    .icon(gpui::assets::IconName::Diff)
                    .label("Difference")
                    .ghost()
                    .small()
                    .disabled(!has_diff)
                    .selected(show_difference)
                    .toggled(show_difference)
                    .tooltip("Show the pixel difference in the right pane")
                    .on_click(move |_, _, cx| {
                        toggle_state.update(cx, |session, cx| {
                            session.toggle_difference_view(cx);
                        });
                    }),
            )
            .into_any_element()
    }

    /// The difference image as a GPUI render image, rebuilt only when
    /// the diff, its size, or the danger color changed.
    fn difference_render(
        &mut self,
        diff: &Arc<PictureDiff>,
        width: u32,
        height: u32,
        danger: [u8; 3],
    ) -> Arc<RenderImage> {
        if let Some((cached, cached_size, cached_danger, render)) =
            &self.diff_image
            && Arc::ptr_eq(cached, diff)
            && *cached_size == (width, height)
            && *cached_danger == danger
        {
            return render.clone();
        }
        let buffer = diff_image_bgra(width, height, &diff.delta, danger);
        let pixels = image::ImageBuffer::from_raw(width, height, buffer)
            .expect("the difference buffer holds one pixel per delta entry");
        let render =
            Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]));
        self.diff_image =
            Some((diff.clone(), (width, height), danger, render.clone()));
        render
    }
}

/// The two labeled image panes side by side.
fn render_panes(
    left_name: &str,
    right_name: &str,
    left_image: Option<Arc<Image>>,
    right_image: Option<Arc<Image>>,
    diff_render: Option<Arc<RenderImage>>,
    muted: Hsla,
    border: Hsla,
) -> AnyElement {
    let left_source = left_image.map(ImageSource::Image);
    let right_label: SharedString = if diff_render.is_some() {
        "Difference".into()
    } else {
        right_name.into()
    };
    let right_source = diff_render
        .map(ImageSource::Render)
        .or_else(|| right_image.map(ImageSource::Image));
    h_flex()
        .flex_1()
        .min_h_0()
        // `h_flex` centers on the cross axis; the panes own a scroll
        // region that must resolve against the row's height.
        .items_stretch()
        .w_full()
        .child(image_pane(
            left_name.into(),
            left_source,
            muted,
            border,
            true,
        ))
        .child(image_pane(right_label, right_source, muted, border, false))
        .into_any_element()
}

/// One labeled pane holding an image scaled to fit, or a notice when
/// the file cannot be displayed.
fn image_pane(
    label: SharedString,
    source: Option<ImageSource>,
    muted: Hsla,
    border: Hsla,
    divider: bool,
) -> AnyElement {
    let content = match source {
        Some(source) => img(source)
            .object_fit(ObjectFit::Contain)
            .size_full()
            .with_loading(|| {
                h_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .child(Spinner::new())
                    .into_any_element()
            })
            .with_fallback(move || {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(muted)
                    .child("The image could not be displayed.")
                    .into_any_element()
            })
            .into_any_element(),
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(muted)
            .child("This file is not a displayable image.")
            .into_any_element(),
    };
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .p_2()
        .when(divider, |pane| pane.border_r_1().border_color(border))
        .child(div().text_xs().text_color(muted).truncate().child(label))
        .child(div().flex_1().min_h_0().child(content))
        .into_any_element()
}

impl Render for PictureDiffView {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (theme_bg, theme_fg, muted, border, danger_rgb) = {
            let theme = cx.theme();
            let danger = Rgba::from(theme.danger);
            (
                theme.background,
                theme.foreground,
                theme.muted_foreground,
                theme.border,
                [
                    (danger.r * 255.0) as u8,
                    (danger.g * 255.0) as u8,
                    (danger.b * 255.0) as u8,
                ],
            )
        };
        let (
            focus_handle,
            left_name,
            right_name,
            left_image,
            right_image,
            left_side,
            right_side,
            diff,
            show_difference,
        ) = {
            let session = self.state.read(cx);
            (
                session.focus_handle(),
                base_name(&session.left),
                base_name(&session.right),
                session.left_image().cloned(),
                session.right_image().cloned(),
                session.left_side().cloned(),
                session.right_side().cloned(),
                session.diff().cloned(),
                session.is_difference_view(),
            )
        };
        let error = self.state.read(cx).error().cloned();
        let loading = self.state.read(cx).is_loading();

        let body = if let Some(err) = error.as_ref() {
            v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(muted)
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
                .text_color(muted)
                .child(Spinner::new())
                .child("Reading images…")
                .into_any_element()
        } else {
            let diff_render =
                match (show_difference, diff.as_ref(), left_side.as_ref()) {
                    (true, Some(diff), Some(side)) => {
                        Some(self.difference_render(
                            diff,
                            side.width,
                            side.height,
                            danger_rgb,
                        ))
                    }
                    _ => None,
                };
            render_panes(
                &left_name,
                &right_name,
                left_image,
                right_image,
                diff_render,
                muted,
                border,
            )
        };

        // The status bar stays quiet while the comparison is not
        // resolved yet; the panes already show the waiting state.
        let (left_dims, right_dims, right_status) =
            if error.is_some() || loading {
                ("—".to_string(), "—".to_string(), String::new())
            } else {
                (
                    dimensions_text(left_side.as_deref()),
                    dimensions_text(right_side.as_deref()),
                    difference_status(&left_side, &right_side, diff.as_ref()),
                )
            };

        div()
            .key_context("PictureCompare")
            .track_focus(&focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme_bg)
            .text_color(theme_fg)
            .text_sm()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(border)
                    .child(left_name)
                    .child(right_name),
            )
            .child(body)
            .child(
                StatusBar::new()
                    .left(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child("Picture Compare")
                            .child(format!("left {left_dims}"))
                            .child(format!("right {right_dims}")),
                    )
                    .right(right_status),
            )
    }
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

/// Create a picture compare tab view for the given file pair.
pub(crate) fn new_picture_diff_view(
    left: Endpoint,
    right: Endpoint,
    manager: Option<Entity<GuiSessionManager>>,
    cx: &mut App,
) -> Entity<PictureDiffView> {
    let title = left.pair_title(&right);
    let session = cx.new(|cx| {
        PictureDiffSession::new(left, right, title.into(), manager, cx)
    });
    cx.new(|cx| PictureDiffView::new(session, cx))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn equal_pixels_report_no_difference(cx: &mut App) {
        let px = vec![10u8, 20, 30, 255, 1, 2, 3, 4];
        let diff = pixel_delta(&px, &px).expect("equal lengths");
        assert_eq!(diff.differing_pixels, 0);
        assert!(diff.delta.iter().all(|d| *d == 0));
        let _ = cx;
    }

    #[gpui::test]
    fn channel_delta_is_the_largest_difference(cx: &mut App) {
        let left = vec![10u8, 20, 30, 255];
        let right = vec![10u8, 25, 30, 250];
        let diff = pixel_delta(&left, &right).expect("equal lengths");
        assert_eq!(diff.differing_pixels, 1);
        assert_eq!(diff.delta[0], 5);
        let _ = cx;
    }

    #[gpui::test]
    fn mismatched_sizes_produce_no_delta(cx: &mut App) {
        assert!(pixel_delta(&[0u8; 4], &[0u8; 8]).is_none());
        let _ = cx;
    }

    #[test]
    fn diff_image_marks_equal_pixels_dim_gray() {
        let buf = diff_image_bgra(2, 1, &[0, 0], [255, 0, 0]);
        assert_eq!(buf, vec![0x40, 0x40, 0x40, 255, 0x40, 0x40, 0x40, 255]);
    }

    #[test]
    fn diff_image_scales_danger_by_delta_intensity_in_bgra_order() {
        let buf = diff_image_bgra(3, 1, &[255, 128, 1], [255, 0, 0]);
        // Full intensity: pure danger red, stored blue-first.
        assert_eq!(&buf[0..4], &[0, 0, 255, 255]);
        // Half intensity: half-scaled red.
        assert_eq!(&buf[4..8], &[0, 0, 128, 255]);
        // Minimal intensity stays a nonzero, but dark, blue byte.
        assert_eq!(&buf[8..12], &[0, 0, 1, 255]);
    }

    #[test]
    fn diff_image_has_one_bgra_pixel_per_delta_entry() {
        let buf = diff_image_bgra(3, 2, &[7u8; 6], [10, 20, 30]);
        assert_eq!(buf.len(), 3 * 2 * 4);
        assert!(buf.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }

    #[test]
    fn only_displayable_formats_are_mapped() {
        assert_eq!(
            gpui_image_format(image::ImageFormat::Png),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            gpui_image_format(image::ImageFormat::WebP),
            Some(ImageFormat::Webp)
        );
        assert_eq!(
            gpui_image_format(image::ImageFormat::Jpeg),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(
            gpui_image_format(image::ImageFormat::Gif),
            Some(ImageFormat::Gif)
        );
        assert_eq!(
            gpui_image_format(image::ImageFormat::Bmp),
            Some(ImageFormat::Bmp)
        );
        assert_eq!(gpui_image_format(image::ImageFormat::Tiff), None);
        assert_eq!(gpui_image_format(image::ImageFormat::Qoi), None);
    }

    #[test]
    fn difference_status_reports_count_and_percentage() {
        let side = || {
            Some(Arc::new(PictureSide {
                width: 2,
                height: 2,
                rgba: Arc::new(vec![0u8; 16]),
            }))
        };
        let diff = Arc::new(PictureDiff {
            delta: Arc::new(vec![0u8; 4]),
            differing_pixels: 1,
        });
        assert_eq!(
            difference_status(&side(), &side(), Some(&diff)),
            "1 differing pixels (25.00%)"
        );
    }

    #[test]
    fn difference_status_names_decode_and_size_problems() {
        let side = Some(Arc::new(PictureSide {
            width: 2,
            height: 2,
            rgba: Arc::new(vec![0u8; 16]),
        }));
        let missing: Option<Arc<PictureSide>> = None;
        assert_eq!(
            difference_status(&missing, &side, None),
            "left side is not a decodable image"
        );
        assert_eq!(
            difference_status(&side, &missing, None),
            "right side is not a decodable image"
        );
        assert_eq!(
            difference_status(&missing, &missing, None),
            "left side is not a decodable image, right side is not a \
             decodable image"
        );
        assert_eq!(difference_status(&side, &side, None), "sizes differ");
    }
}
