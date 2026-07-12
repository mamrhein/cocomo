// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Tab bar component for multi-session support.
//!
//! Renders a horizontal bar of session tabs with close buttons. Each tab
//! displays the session name and a dirty indicator.

use std::sync::Arc;

use gpui::{
    App, Context, Entity, SharedString, Styled, Window, div, prelude::*, px,
    rgb,
};

use crate::session_manager::GuiSessionManager;

/// Callback type for tab activation.
type ActivateCallback = Arc<dyn Fn(usize, &mut App) + Send + Sync>;

/// Callback type for tab close.
type CloseCallback = Arc<dyn Fn(usize, &mut App) + Send + Sync>;

/// Callback type for new tab.
type NewCallback = Arc<dyn Fn(&mut App) + Send + Sync>;

// ---------------------------------------------------------------------------
// Tab Bar
// ---------------------------------------------------------------------------

/// A horizontal tab bar showing open sessions.
pub struct TabBar {
    /// Handle to the session manager.
    session_manager: Entity<GuiSessionManager>,
}

impl TabBar {
    /// Create a new tab bar.
    #[allow(dead_code)]
    pub fn new(session_manager: Entity<GuiSessionManager>) -> Self {
        Self { session_manager }
    }

    /// Render the tab bar with callbacks that use `&App`.
    pub fn render<P: gpui::Render + 'static>(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<P>,
        on_activate: ActivateCallback,
        on_close: CloseCallback,
        on_new: NewCallback,
    ) -> impl gpui::IntoElement + use<P> {
        let mgr = self.session_manager.read(cx);
        let active_index = mgr.active_index();
        let tab_ids = mgr.open_tab_ids();

        div()
            .flex()
            .items_center()
            .bg(rgb(0x11111b))
            .border_b_1()
            .border_color(rgb(0x313244))
            .h(px(32.))
            .overflow_hidden()
            .children(tab_ids.iter().enumerate().map(
                |(i, (_tab_id, name))| {
                    let is_active = i == active_index;
                    let name: SharedString = SharedString::from(name.as_str());

                    // Close button.
                    let on_close = on_close.clone();
                    let close_button = div()
                        .id(("close_tab", i))
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(20.))
                        .rounded_sm()
                        .cursor_pointer()
                        .text_xs()
                        .font_weight(gpui::FontWeight(600.))
                        .text_color(rgb(0x6c7086))
                        .child("x")
                        .hover(|this| {
                            this.bg(rgb(0x313244)).text_color(rgb(0xf38ba8))
                        })
                        .active(|this| {
                            this.bg(rgb(0xf38ba8)).text_color(rgb(0x1e1e2e))
                        })
                        .on_click(move |_event, _window, app: &mut App| {
                            on_close(i, app);
                        });

                    // Tab.
                    let on_activate = on_activate.clone();
                    div()
                        .id(("tab", i))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_3()
                        .py_0p5()
                        .min_w(px(100.))
                        .max_w(px(200.))
                        .h_full()
                        .cursor_pointer()
                        .text_xs()
                        .rounded_tl_md()
                        .rounded_tr_md()
                        .when(is_active, |this| {
                            this.bg(rgb(0x1e1e2e))
                                .text_color(rgb(0xcdd6f4))
                                .border_t_2()
                                .border_color(rgb(0x89b4fa))
                        })
                        .when(!is_active, |this| {
                            this.bg(rgb(0x181825))
                                .text_color(rgb(0x6c7086))
                                .hover(|this| {
                                    this.bg(rgb(0x1e1e2e))
                                        .text_color(rgb(0xa6adc8))
                                })
                        })
                        .on_click(move |_event, _window, app: &mut App| {
                            on_activate(i, app);
                        })
                        .child(div().truncate().child(name))
                        .child(close_button)
                },
            ))
            // New tab button.
            .child(
                div()
                    .id("new_tab_btn")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .ml_1()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .text_color(rgb(0x6c7086))
                    .child("+")
                    .hover(|this| {
                        this.bg(rgb(0x313244)).text_color(rgb(0xa6adc8))
                    })
                    .active(|this| this.bg(rgb(0x45475a)))
                    .on_click(move |_event, _window, app: &mut App| {
                        on_new(app);
                    }),
            )
    }
}
