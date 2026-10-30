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
//! Provides the styled session tab bar (with per-tab context menus) and
//! the shared toolbar row 1 that integrate natively with the Kit's
//! presentation layer and theme system. Row 2 is view-specific and
//! rendered by the active tab (see `crate::tabview::TabEntity::view_toolbar`).

use gpui::{
    AnyElement, App, Context, ElementId, Entity, IntoElement, Render,
    SharedString, Window,
    assets::IconName,
    base::{Disableable as _, Selectable as _},
    component::{
        ActiveTheme, Sizable as _, Theme,
        button::{Button, ButtonVariants as _, DropdownButton},
        menu::{ContextMenuExt as _, PopupMenuItem},
        tab::{Tab, TabBar},
        toolbar::{Toolbar, ToolbarGroup},
    },
    div,
    prelude::*,
};

use crate::{
    commands, log,
    log::LogLevel,
    menus::{SaveSession, SessionSettings},
    session_manager::GuiSessionManager,
    state::{AppState, FilterMode},
    tabview::TabEntity,
    windows,
};

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

/// Payload of an in-progress tab drag, carrying the dragged tab's id.
#[derive(Clone)]
struct TabDrag {
    tab_id: usize,
}

/// The floating chip shown next to the cursor while a tab is dragged.
struct TabDragLabel {
    label: SharedString,
}

impl Render for TabDragLabel {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .text_color(theme.foreground)
            .text_sm()
            .child(self.label.clone())
    }
}

/// Build the session tab bar element using `gpui-component`'s [`TabBar`].
///
/// Each tab shows the session name (with a dot while the session is
/// dirty), a close button, and a context menu (move to new window, save,
/// close, close all). Dragging a tab onto another tab reorders the list.
/// A "+" button is appended as a suffix for creating new tabs. Tab
/// activation is handled via the `TabBar`'s `on_click` callback.
pub fn tab_bar_element(
    session_manager: Entity<GuiSessionManager>,
    cx: &App,
) -> AnyElement {
    let (active_index, tabs) = {
        let mgr = session_manager.read(cx);
        (mgr.active_index(), mgr.open_tab_ids())
    };
    // Home is a launcher, not a session; moving it to its own window would
    // only produce a duplicate start screen.
    let home_index = {
        let mgr = session_manager.read(cx);
        mgr.tab_entities().iter().position(TabEntity::is_home)
    };

    let mgr_activate = session_manager.clone();
    let drop_primary = cx.theme().primary;
    let on_click = move |index: &usize, _window: &mut Window, cx: &mut App| {
        mgr_activate.update(cx, |m, cx| {
            m.activate_session(*index, cx);
        });
    };

    let tab_elements: Vec<Tab> = tabs
        .iter()
        .enumerate()
        .map(|(i, (tab_id, name, dirty))| {
            let mgr_close = session_manager.clone();
            // Icon-only control: needs an accessible name and a tooltip
            // that clarifies the scope (closes the tab, not the window).
            let close_btn = Button::new(("close_tab", i))
                .icon(IconName::Close)
                .ghost()
                .small()
                .tooltip("Close tab")
                .accessibility_label("Close tab")
                .on_click(move |_, window, cx| {
                    // Route through the manager so a dirty session gets
                    // its save prompt before closing.
                    mgr_close.update(cx, |m, cx| {
                        m.request_close_session(i, window, cx);
                    });
                });

            // The context menu covers the tab's label area; the close
            // button stays outside it so either affordance works.
            let mgr_menu = session_manager.clone();
            let mgr_move = session_manager.clone();
            let mgr_drop = session_manager.clone();
            let drag_label: SharedString = name.clone().into();
            let dragged = TabDrag { tab_id: *tab_id };
            let is_home = home_index == Some(i);
            let label_text = if *dirty {
                format!("{name} •")
            } else {
                name.clone()
            };
            let label_area = div()
                .id(ElementId::Name(format!("tab-ctx-{i}").into()))
                .flex_1()
                .min_w_0()
                .truncate()
                .child(label_text)
                // Dragging a tab onto another tab reorders this window's
                // tabs. Moving a tab to a *different* window goes through
                // the context menu: GPUI delivers an internal drag's
                // mouse-up to the window that started it, so a cross-window
                // drop would never arrive.
                .on_drag(dragged, move |drag, _, window, cx| {
                    let _ = (drag, window);
                    cx.new(|_| TabDragLabel {
                        label: drag_label.clone(),
                    })
                })
                .drag_over::<TabDrag>(move |style, _, _, _| {
                    style.text_color(drop_primary)
                })
                .on_drop::<TabDrag>(move |drag, _, cx| {
                    mgr_drop.update(cx, |m, cx| {
                        let Some(from) = m.session_index(drag.tab_id) else {
                            return;
                        };
                        m.move_session(from, i, cx);
                    });
                })
                .context_menu(move |menu, _, _cx| {
                    let mgr = mgr_menu.clone();
                    let mgr_close_all = mgr.clone();
                    let mgr_move_item = mgr_move.clone();
                    let is_home_here = is_home;
                    menu.item(
                        PopupMenuItem::new("Move tab to new window")
                            .disabled(is_home_here)
                            .on_click(move |_, _, cx| {
                                windows::move_tab_to_new_window(
                                    &mgr_move_item,
                                    i,
                                    cx,
                                );
                            }),
                    )
                    .item(
                        // The save command acts on the active session;
                        // offering it on an inactive tab would lie about
                        // its scope.
                        PopupMenuItem::new("Save session")
                            .disabled(i != active_index)
                            .on_click(|_, _, cx| {
                                cx.dispatch_action(&SaveSession);
                            }),
                    )
                    .separator()
                    .item(PopupMenuItem::new("Close").on_click(
                        move |_, window, cx| {
                            mgr.update(cx, |m, cx| {
                                m.request_close_session(i, window, cx);
                            });
                        },
                    ))
                    .item(
                        PopupMenuItem::new("Close all").on_click(
                            move |_, _, cx| {
                                mgr_close_all.update(cx, |m, cx| {
                                    while !m.is_empty() {
                                        m.close_session(0, cx);
                                    }
                                });
                            },
                        ),
                    )
                });

            Tab::new()
                .child(label_area)
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
        .children(tab_elements)
        .suffix(new_btn)
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Toolbar row 1 (shared)
// ---------------------------------------------------------------------------

/// One button of the exclusive display-filter toggle group.
///
/// The group is disabled while no folder tab is active; the "Diffs"
/// button additionally takes the theme's danger color while selected, as
/// the spec asks for a red difference state. Selection stays visible on
/// its own, so color is never the only cue.
#[allow(clippy::too_many_arguments)]
fn filter_toggle(
    id: &'static str,
    icon: IconName,
    tooltip: &'static str,
    mode: FilterMode,
    current: Option<FilterMode>,
    danger_when_selected: bool,
    theme: &Theme,
    state: Option<Entity<AppState>>,
) -> Button {
    let button = Button::new(id)
        .icon(icon)
        .ghost()
        .small()
        .selected(current == Some(mode))
        .disabled(state.is_none())
        .tooltip(tooltip)
        .accessibility_label(tooltip);
    let button = if current == Some(mode) && danger_when_selected {
        button.text_color(theme.danger)
    } else {
        button
    };
    match state {
        Some(state) => button.on_click(move |_, _, cx| {
            state.update(cx, |state, cx| {
                state.set_filter_mode(mode, cx);
            });
        }),
        None => button,
    }
}

/// Build toolbar row 1 using `gpui-component`'s [`Toolbar`].
///
/// Row 1 is shared across all views: Home, the Sessions dropdown, the
/// exclusive display-filter toggle group, Refresh, and Rules. Controls
/// whose operations arrive in later phases render disabled with a
/// tooltip that names the wait. `active_state` is the active folder
/// tab's state, or `None` for text-diff tabs and an empty window.
pub fn toolbar_element(
    active_state: Option<Entity<AppState>>,
    session_manager: Entity<GuiSessionManager>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let current_mode = active_state
        .as_ref()
        .map(|state| state.read(cx).filter_mode());
    let no_folder = active_state.is_none();

    // Home: activate the Home tab, opening one when none exists yet.
    let mgr_home = session_manager.clone();
    let home_btn = Button::new("toolbar_home")
        .icon(IconName::House)
        .ghost()
        .small()
        .tooltip("Home")
        .accessibility_label("Home")
        .on_click(move |_, _, cx| {
            mgr_home.update(cx, |m, cx| {
                m.open_home(cx);
            });
        });

    // Sessions: pick a saved session to open it in a new tab. The
    // dedicated dropdown button keeps the trigger visibly pressed while
    // the popup is open.
    let saved = session_manager.read(cx).recent_sessions(cx).to_vec();
    let mgr_sessions = session_manager.clone();
    let sessions_btn = DropdownButton::new("toolbar_sessions")
        .button(
            Button::new("toolbar_sessions_btn")
                .label("Sessions")
                .ghost()
                .small(),
        )
        .menu_accessibility_label("Saved sessions")
        .dropdown_menu(move |menu, _, _| {
            if saved.is_empty() {
                return menu.label("No saved sessions");
            }
            let mut menu = menu;
            for entry in saved.iter() {
                let mgr = mgr_sessions.clone();
                let config = entry.config().clone();
                menu = menu.item(
                    PopupMenuItem::new(config.name.clone()).on_click(
                        move |_, _, cx| {
                            mgr.update(cx, |m, cx| {
                                if let Err(e) =
                                    m.open_saved(config.clone(), cx)
                                {
                                    log::push(
                                        LogLevel::Error,
                                        format!("Open session failed: {e}"),
                                        cx,
                                    );
                                }
                            });
                        },
                    ),
                );
            }
            menu
        });

    let all_btn = filter_toggle(
        "toolbar_filter_all",
        IconName::Eye,
        "Show all entries",
        FilterMode::All,
        current_mode,
        false,
        theme,
        active_state.clone(),
    );
    let diffs_btn = filter_toggle(
        "toolbar_filter_diffs",
        IconName::Diff,
        "Show differences only",
        FilterMode::Diffs,
        current_mode,
        true,
        theme,
        active_state.clone(),
    );
    let same_btn = filter_toggle(
        "toolbar_filter_same",
        IconName::Equal,
        "Show identical entries only",
        FilterMode::Same,
        current_mode,
        false,
        theme,
        active_state.clone(),
    );
    // Minor differences (e.g. whitespace-only) are a text-comparison
    // concept; the button is a placeholder until that lands.
    let minor_btn = Button::new("toolbar_filter_minor")
        .icon(IconName::Gauge)
        .ghost()
        .small()
        .disabled(true)
        .tooltip("Minor differences (coming soon)")
        .accessibility_label("Minor differences");

    // Refresh re-reads and re-compares the active session.
    let refresh_btn = Button::new("toolbar_refresh")
        .icon(IconName::RefreshCw)
        .ghost()
        .small()
        .disabled(no_folder)
        .tooltip("Reload comparison")
        .accessibility_label("Reload comparison");
    let refresh_btn = match active_state.clone() {
        Some(state) => refresh_btn.on_click(move |_, _, cx| {
            state.update(cx, |state, cx| {
                state.reload(cx);
            });
        }),
        None => refresh_btn,
    };

    // Rules opens the session-rules dialog: folder rules for a folder
    // tab, text-compare rules for a text tab. The global handler
    // branches on the active tab.
    let is_text_active =
        session_manager.read(cx).active_text_session(cx).is_some();
    let rules_btn = Button::new("toolbar_rules")
        .icon(IconName::Gavel)
        .ghost()
        .small()
        .disabled(no_folder && !is_text_active)
        .tooltip("Session rules")
        .accessibility_label("Session rules")
        .on_click(|_, window, cx| {
            window.dispatch_action(Box::new(SessionSettings), cx);
        });

    // Toolbar personalization (spec section 7.2): the command registry
    // decides which of these controls exist at all.
    let mut nav_group = ToolbarGroup::new("toolbar-nav").label("Navigation");
    if commands::is_visible("toolbar.home", cx) {
        nav_group = nav_group.child(home_btn);
    }
    if commands::is_visible("toolbar.sessions", cx) {
        nav_group = nav_group.content(sessions_btn);
    }

    let mut display_group =
        ToolbarGroup::new("toolbar-display").label("Display filters");
    if commands::is_visible("toolbar.filters", cx) {
        display_group = display_group
            .child(all_btn)
            .child(diffs_btn)
            .child(same_btn)
            .child(minor_btn);
    }

    let mut ops_group = ToolbarGroup::new("toolbar-ops").label("Operations");
    if commands::is_visible("toolbar.refresh", cx) {
        ops_group = ops_group.child(refresh_btn);
    }
    if commands::is_visible("toolbar.rules", cx) {
        ops_group = ops_group.child(rules_btn);
    }

    Toolbar::new("main-toolbar")
        .child(nav_group)
        .child(display_group)
        .child(ops_group)
        .into_any_element()
}
