// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Toolbar component with action buttons.
//!
//! Renders a horizontal row of toolbar buttons below the menu bar.
//! Each button dispatches a gpui action when clicked.

use gpui::{
    App, Context, SharedString, Styled, Window, div, prelude::*, px, rgb,
};

use crate::menus::{CloseSession, NewCompare, ReloadCompare, SaveSession};

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

/// A horizontal toolbar with action buttons.
pub struct Toolbar;

impl Toolbar {
    /// Create a new toolbar.
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self
    }

    /// Render the toolbar.
    pub fn render<P: gpui::Render + 'static>(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<P>,
    ) -> impl gpui::IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .py_0p5()
            .h(px(32.))
            .bg(rgb(0x181825))
            .border_b_1()
            .border_color(rgb(0x313244))
            // Left side — action buttons.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    // New Compare button.
                    .child(
                        div()
                            .id("toolbar_new")
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_2()
                            .py_0p5()
                            .min_w(px(32.))
                            .h(px(26.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .bg(rgb(0x1e1e2e))
                            .child(SharedString::from("+"))
                            .hover(|this| {
                                this.bg(rgb(0x313244))
                                    .text_color(rgb(0xcdd6f4))
                            })
                            .active(|this| this.bg(rgb(0x45475a)))
                            .on_click(move |_event, _window, cx: &mut App| {
                                cx.dispatch_action(&NewCompare);
                            }),
                    )
                    // Save button.
                    .child(
                        div()
                            .id("toolbar_save")
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_2()
                            .py_0p5()
                            .min_w(px(32.))
                            .h(px(26.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .bg(rgb(0x1e1e2e))
                            .child(SharedString::from("💾"))
                            .hover(|this| {
                                this.bg(rgb(0x313244))
                                    .text_color(rgb(0xcdd6f4))
                            })
                            .active(|this| this.bg(rgb(0x45475a)))
                            .on_click(move |_event, _window, cx: &mut App| {
                                cx.dispatch_action(&SaveSession);
                            }),
                    )
                    // Reload button.
                    .child(
                        div()
                            .id("toolbar_reload")
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_2()
                            .py_0p5()
                            .min_w(px(32.))
                            .h(px(26.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .bg(rgb(0x1e1e2e))
                            .child(SharedString::from("↻"))
                            .hover(|this| {
                                this.bg(rgb(0x313244))
                                    .text_color(rgb(0xcdd6f4))
                            })
                            .active(|this| this.bg(rgb(0x45475a)))
                            .on_click(move |_event, _window, cx: &mut App| {
                                cx.dispatch_action(&ReloadCompare);
                            }),
                    )
                    // Close button.
                    .child(
                        div()
                            .id("toolbar_close")
                            .flex()
                            .items_center()
                            .justify_center()
                            .px_2()
                            .py_0p5()
                            .min_w(px(32.))
                            .h(px(26.))
                            .rounded_md()
                            .cursor_pointer()
                            .text_sm()
                            .text_color(rgb(0xa6adc8))
                            .bg(rgb(0x1e1e2e))
                            .child(SharedString::from("✕"))
                            .hover(|this| {
                                this.bg(rgb(0x313244))
                                    .text_color(rgb(0xcdd6f4))
                            })
                            .active(|this| this.bg(rgb(0x45475a)))
                            .on_click(move |_event, _window, cx: &mut App| {
                                cx.dispatch_action(&CloseSession);
                            }),
                    ),
            )
            // Right side — reserved for future status indicators.
            .child(div().flex().items_center().gap_2())
    }
}
