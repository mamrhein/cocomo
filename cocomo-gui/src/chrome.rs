// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Window chrome elements built on `gpui-component`.
//!
//! Provides styled tab bar and toolbar elements that integrate natively
//! with the Kit's presentation layer and theme system.

use gpui::{
    App, Entity, IntoElement, Window,
    assets::IconName,
    base::Selectable,
    component::{
        Sizable as _,
        button::{Button, ButtonVariants as _},
        tab::{Tab, TabBar},
        toolbar::{Toolbar, ToolbarGroup},
    },
};

use crate::{
    menus::{CloseSession, NewCompare, ReloadCompare, SaveSession},
    session_manager::GuiSessionManager,
};

/// Build the session tab bar element using `gpui-component`'s [`TabBar`].
///
/// Each tab shows the session name with a close button. A "+" button is
/// appended as a suffix for creating new tabs. Tab activation is handled
/// via the `TabBar`'s `on_click` callback.
pub fn tab_bar_element(
    session_manager: Entity<GuiSessionManager>,
    cx: &App,
) -> impl IntoElement {
    let (active_index, tab_ids) = {
        let mgr = session_manager.read(cx);
        (mgr.active_index(), mgr.open_tab_ids())
    };

    let mgr_activate = session_manager.clone();
    let on_click = move |index: &usize, _window: &mut Window, cx: &mut App| {
        mgr_activate.update(cx, |m, cx| {
            m.activate_session(*index, cx);
        });
    };

    let tabs: Vec<Tab> = tab_ids
        .iter()
        .enumerate()
        .map(|(i, (_id, name))| {
            let mgr_close = session_manager.clone();
            // Icon-only control: needs an accessible name and a tooltip
            // that clarifies the scope (closes the tab, not the window).
            let close_btn = Button::new(("close_tab", i))
                .icon(IconName::Close)
                .ghost()
                .small()
                .tooltip("Close tab")
                .accessibility_label("Close tab")
                .on_click(move |_, _, cx| {
                    mgr_close.update(cx, |m, cx| {
                        m.close_session(i, cx);
                    });
                });
            Tab::new()
                .label(name.as_str())
                .selected(i == active_index)
                .suffix(close_btn)
        })
        .collect();

    let mgr_new = session_manager.clone();
    let new_btn = Button::new("new_tab_btn")
        .icon(IconName::Plus)
        .ghost()
        .small()
        .tooltip("New compare")
        .accessibility_label("New compare")
        .on_click(move |_, _, cx| {
            mgr_new.update(cx, |m, cx| {
                m.add_new_folder_tab(cx);
            });
        });

    TabBar::new("session-tabs")
        .selected_index(active_index)
        .on_click(on_click)
        .children(tabs)
        .suffix(new_btn)
}

/// Build the toolbar element using `gpui-component`'s [`Toolbar`].
///
/// Contains action buttons for New Compare, Save, Reload, and Close,
/// each dispatching the corresponding application action. The toolbar
/// group renders its buttons with the quiet ghost treatment; every
/// icon-only button carries a tooltip and an accessible name.
pub fn toolbar_element() -> impl IntoElement {
    Toolbar::new("main-toolbar").child(
        ToolbarGroup::new("toolbar-actions")
            .label("Session commands")
            .child(
                Button::new("toolbar_new")
                    .icon(IconName::Plus)
                    .tooltip("New compare")
                    .accessibility_label("New compare")
                    .on_click(|_, _, cx| {
                        cx.dispatch_action(&NewCompare);
                    }),
            )
            .child(
                Button::new("toolbar_save")
                    .icon(IconName::Save)
                    .tooltip("Save session")
                    .accessibility_label("Save session")
                    .on_click(|_, _, cx| {
                        cx.dispatch_action(&SaveSession);
                    }),
            )
            .child(
                Button::new("toolbar_reload")
                    .icon(IconName::RefreshCw)
                    .tooltip("Reload comparison")
                    .accessibility_label("Reload comparison")
                    .on_click(|_, _, cx| {
                        cx.dispatch_action(&ReloadCompare);
                    }),
            )
            .child(
                Button::new("toolbar_close")
                    .icon(IconName::Close)
                    .tooltip("Close session")
                    .accessibility_label("Close session")
                    .on_click(|_, _, cx| {
                        cx.dispatch_action(&CloseSession);
                    }),
            ),
    )
}
