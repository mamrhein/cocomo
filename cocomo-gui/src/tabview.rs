// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Tab content types and the window root.
//!
//! [`WindowRoot`] is the root entity of the main window. It manages the
//! window chrome (tab bar, toolbar) and delegates content rendering to
//! the active tab's entity.

use std::any::Any;

use gpui::{
    AnyElement, App, AppContext, Context, Entity, FocusHandle, Focusable,
    Render, SharedString, Window, div, prelude::*, rgb,
};

use crate::{
    session_manager::GuiSessionManager, text_diff::TextDiffView,
    ui::FolderCompareView,
};

// ---------------------------------------------------------------------------
// Type-erased tab entity
// ---------------------------------------------------------------------------

/// A type-erased entity that can render itself.
///
/// Stores either a [`FolderCompareView`] or [`TextDiffView`] entity behind
/// a trait object, exposing only the methods needed for rendering and focus.
pub struct TabEntity {
    inner: Box<dyn TabEntityInner>,
}

impl Clone for TabEntity {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone_inner(),
        }
    }
}

impl TabEntity {
    /// Create from a folder compare view.
    pub fn from_folder(view: Entity<FolderCompareView>) -> Self {
        Self {
            inner: Box::new(FolderTabEntity(view)),
        }
    }

    /// Create from a text diff view.
    pub fn from_text_diff(view: Entity<TextDiffView>) -> Self {
        Self {
            inner: Box::new(TextDiffTabEntity(view)),
        }
    }

    /// Get the focus handle.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.inner.focus_handle(cx)
    }

    /// Get the tab title.
    pub fn title(&self, cx: &App) -> SharedString {
        self.inner.title(cx)
    }

    /// Get the type label.
    pub fn type_label(&self) -> &'static str {
        self.inner.type_label()
    }

    /// Render this tab's content as an element.
    pub fn render(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.inner.render(window, cx)
    }
}

/// Inner trait for type-erased tab entities.
trait TabEntityInner: Any + Send + Sync {
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn title(&self, cx: &App) -> SharedString;
    fn type_label(&self) -> &'static str;
    fn render(&self, window: &mut Window, cx: &mut App) -> AnyElement;
    fn clone_inner(&self) -> Box<dyn TabEntityInner>;
}

struct FolderTabEntity(Entity<FolderCompareView>);

impl TabEntityInner for FolderTabEntity {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.0.read(cx).focus_handle(cx)
    }

    fn title(&self, cx: &App) -> SharedString {
        self.0.read(cx).title(cx)
    }

    fn type_label(&self) -> &'static str {
        "dir compare"
    }

    fn render(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        use gpui::IntoElement;
        let entity = self.0.clone();
        entity
            .update(cx, |view, cx| view.render(window, cx).into_any_element())
    }

    fn clone_inner(&self) -> Box<dyn TabEntityInner> {
        Box::new(FolderTabEntity(self.0.clone()))
    }
}

struct TextDiffTabEntity(Entity<TextDiffView>);

impl TabEntityInner for TextDiffTabEntity {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.0.read(cx).focus_handle(cx)
    }

    fn title(&self, cx: &App) -> SharedString {
        self.0.read(cx).title(cx)
    }

    fn type_label(&self) -> &'static str {
        "text diff"
    }

    fn render(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        use gpui::IntoElement;
        let entity = self.0.clone();
        entity
            .update(cx, |view, cx| view.render(window, cx).into_any_element())
    }

    fn clone_inner(&self) -> Box<dyn TabEntityInner> {
        Box::new(TextDiffTabEntity(self.0.clone()))
    }
}

// ---------------------------------------------------------------------------
// Window Root
// ---------------------------------------------------------------------------

/// The root entity of the main window.
pub struct WindowRoot {
    /// Handle to the session manager.
    session_manager: Entity<GuiSessionManager>,
}

impl WindowRoot {
    /// Create a new window root.
    pub fn new(
        session_manager: Entity<GuiSessionManager>,
        _cx: &mut Context<Self>,
    ) -> Self {
        let _ = _cx;
        Self { session_manager }
    }
}

impl Render for WindowRoot {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mgr = self.session_manager.read(cx);
        let active_index = mgr.active_index();

        // Collect tab entities from the session manager.
        let tabs: Vec<TabEntity> = mgr.tab_entities();

        // Focus the active tab.
        if let Some(tab) = tabs.get(active_index) {
            let focus = tab.focus_handle(cx);
            window.focus(&focus, cx);
        }

        // Render tab content first to avoid borrow conflicts.
        use gpui::IntoElement;
        let tab_contents: Vec<AnyElement> = tabs
            .iter()
            .enumerate()
            .map(|(i, tab)| {
                let element = tab.render(window, cx);
                if i == active_index {
                    element
                } else {
                    div()
                        .absolute()
                        .size_0()
                        .overflow_hidden()
                        .child(element)
                        .into_any_element()
                }
            })
            .collect();

        // Build tab bar callbacks.
        let mgr_activate = self.session_manager.clone();
        let on_activate: std::sync::Arc<
            dyn Fn(usize, &mut App) + Send + Sync,
        > = std::sync::Arc::new(move |index: usize, app: &mut App| {
            mgr_activate.update(app, |m, cx| {
                m.activate_session(index, cx);
            });
        });

        let mgr_close = self.session_manager.clone();
        let on_close: std::sync::Arc<dyn Fn(usize, &mut App) + Send + Sync> =
            std::sync::Arc::new(move |index: usize, app: &mut App| {
                mgr_close.update(app, |m, cx| {
                    m.close_session(index, cx);
                });
            });

        let mgr_new = self.session_manager.clone();
        let on_new: std::sync::Arc<dyn Fn(&mut App) + Send + Sync> =
            std::sync::Arc::new(move |app: &mut App| {
                mgr_new.update(app, |m, cx| {
                    m.add_new_folder_tab(cx);
                });
            });

        let mut tab_bar =
            crate::tab_bar::TabBar::new(self.session_manager.clone());
        let tab_bar_element =
            tab_bar.render(window, cx, on_activate, on_close, on_new);

        let mut toolbar = crate::toolbar::Toolbar::new();
        let toolbar_element = toolbar.render(window, cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .text_sm()
            .child(tab_bar_element)
            .child(toolbar_element)
            .child(div().flex_1().overflow_hidden().children(tab_contents))
    }
}
