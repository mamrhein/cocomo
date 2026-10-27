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
    component::ActiveTheme,
    div,
    prelude::*,
};

use crate::{
    menus,
    session_manager::GuiSessionManager,
    state::{AppState, DisplayFilters},
    text_diff::TextDiffView,
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

    /// Return the folder-compare state, if this tab is a folder compare.
    ///
    /// The window root uses this to sync the View menu's filter checkmarks
    /// to the active tab; text-diff tabs return `None`.
    pub fn folder_state(&self, cx: &App) -> Option<Entity<AppState>> {
        self.inner.folder_state(cx)
    }
}

/// Inner trait for type-erased tab entities.
trait TabEntityInner: Any + Send + Sync {
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn any_view(&self) -> AnyView;
    fn folder_state(&self, cx: &App) -> Option<Entity<AppState>>;
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

    fn folder_state(&self, cx: &App) -> Option<Entity<AppState>> {
        Some(self.0.read(cx).folder_state())
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

    fn folder_state(&self, _: &App) -> Option<Entity<AppState>> {
        None
    }

    fn clone_inner(&self) -> Box<dyn TabEntityInner> {
        Box::new(TextDiffTabEntity(self.0.clone()))
    }
}

// ---------------------------------------------------------------------------
// Window Root
// ---------------------------------------------------------------------------

/// The root entity of the main window.
///
/// Owns the window chrome (tab bar, toolbar) and keyboard focus: the
/// active tab receives focus when the window opens and whenever the
/// active tab changes. Tab views never request focus from their own
/// `render`.
pub struct WindowRoot {
    /// Handle to the session manager.
    session_manager: Entity<GuiSessionManager>,
    /// Navigation stack managing tab content visibility.
    nav_stack_state: Entity<NavStackState>,
    /// Last known active tab index, for diffing against the session manager.
    last_active_index: usize,
}

impl WindowRoot {
    /// Create a new window root, giving the initially active tab keyboard
    /// focus.
    ///
    /// The tab's element is not in the tree yet; GPUI resolves the focus
    /// path when the view first renders and `track_focus`es its handle.
    pub fn new(
        session_manager: Entity<GuiSessionManager>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial_focus = {
            let mgr = session_manager.read(cx);
            mgr.tab_entities()
                .get(mgr.active_index())
                .map(|tab| tab.focus_handle(cx))
        };
        if let Some(handle) = initial_focus {
            handle.focus(window, cx);
        }

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
        // changes or the stack is empty, clear and push the new view and
        // move keyboard focus to it. This branch runs once per activation,
        // not on every frame, so focus is requested on activation rather
        // than as an unconditional side effect of `render`.
        // NavMotion::Immediate switches views without animation, which is
        // appropriate for tab switching.
        let mut sync_filters: Option<Option<DisplayFilters>> = None;
        if (active_index != self.last_active_index
            || self.nav_stack_state.read(cx).is_empty())
            && let Some(tab) = tabs.get(active_index)
        {
            let view = tab.any_view();
            let focus = tab.focus_handle(cx);
            // Capture the filter state while the session-manager borrow is
            // alive; the menu refresh below needs `&mut App`.
            let filters =
                tab.folder_state(cx).map(|state| state.read(cx).filters());
            self.nav_stack_state.update(cx, |state, cx| {
                state.clear(cx);
                state.push(view, NavMotion::Immediate, cx);
            });
            self.last_active_index = active_index;
            focus.focus(window, cx);
            sync_filters = Some(filters);
        }
        // The session-manager read borrow ends here, so the menu bar can be
        // rebuilt (it needs `&mut App`) to reflect the active tab.
        if let Some(filters) = sync_filters {
            menus::refresh_view_menus(filters, cx);
        }

        let tab_bar_element =
            crate::chrome::tab_bar_element(self.session_manager.clone(), cx);
        let toolbar_element = crate::chrome::toolbar_element();

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .child(tab_bar_element)
            .child(toolbar_element)
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    NavStack::new(&self.nav_stack_state).size_full().item(
                        |page, _, _| page.size_full().into_any_element(),
                    ),
                ),
            )
    }
}
