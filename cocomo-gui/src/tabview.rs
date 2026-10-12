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
//! window chrome (tab bar, toolbar) and delegates content rendering to the
//! active tab's entity via a [`gpui::base::NavStack`], which handles
//! showing only the current tab view without manual flexbox hiding.

use std::any::Any;

use gpui::{
    AnyView, App, Context, Entity, FocusHandle, Render, Window,
    base::{NavMotion, NavStack, NavStackState},
    div,
    prelude::*,
    rgb,
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

    /// Return the tab's view as a type-erased [`AnyView`].
    ///
    /// Used by [`WindowRoot`] to push the view into its [`NavStackState`].
    pub fn any_view(&self) -> AnyView {
        self.inner.any_view()
    }
}

/// Inner trait for type-erased tab entities.
trait TabEntityInner: Any + Send + Sync {
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn any_view(&self) -> AnyView;
    fn clone_inner(&self) -> Box<dyn TabEntityInner>;
}

struct FolderTabEntity(Entity<FolderCompareView>);

impl TabEntityInner for FolderTabEntity {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.0.read(cx).focus_handle(cx)
    }

    fn any_view(&self) -> AnyView {
        self.0.clone().into()
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

    fn any_view(&self) -> AnyView {
        self.0.clone().into()
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
    /// Navigation stack managing tab content visibility.
    nav_stack_state: Entity<NavStackState>,
    /// Last known active tab index, for diffing against the session manager.
    last_active_index: usize,
}

impl WindowRoot {
    /// Create a new window root.
    pub fn new(
        session_manager: Entity<GuiSessionManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            session_manager,
            nav_stack_state: cx.new(|_| NavStackState::new()),
            last_active_index: 0,
        }
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
        let tabs = mgr.tab_entities();

        // Sync the NavStackState with the active tab. When the active tab
        // changes or the stack is empty, clear and push the new view.
        // NavMotion::Immediate switches views without animation, which is
        // appropriate for tab switching.
        if (active_index != self.last_active_index
            || self.nav_stack_state.read(cx).is_empty())
            && let Some(tab) = tabs.get(active_index)
        {
            let view = tab.any_view();
            self.nav_stack_state.update(cx, |state, cx| {
                state.clear(cx);
                state.push(view, NavMotion::Immediate, cx);
            });
            self.last_active_index = active_index;
        }

        // Focus the active tab.
        if let Some(tab) = tabs.get(active_index) {
            let focus = tab.focus_handle(cx);
            window.focus(&focus, cx);
        }

        let tab_bar_element =
            crate::chrome::tab_bar_element(self.session_manager.clone(), cx);
        let toolbar_element = crate::chrome::toolbar_element();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .text_sm()
            .child(tab_bar_element)
            .child(toolbar_element)
            .child(
                div().flex_1().overflow_hidden().child(
                    NavStack::new(&self.nav_stack_state).size_full().item(
                        |page, _, _| page.size_full().into_any_element(),
                    ),
                ),
            )
    }
}
