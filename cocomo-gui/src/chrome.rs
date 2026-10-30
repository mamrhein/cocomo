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
    AnyElement, App, ElementId, Entity, IntoElement, Window,
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
    log::{self, LogLevel},
    menus::SaveSession,
    session_manager::GuiSessionManager,
    state::{AppState, FilterMode},
};
// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

/// Build the session tab bar element using `gpui-component`'s [`TabBar`].
///
/// Each tab shows the session name with a close button and carries a
/// context menu (move, save, close, close all). A "+" button is appended
/// as a suffix for creating new tabs. Tab activation is handled via the
/// `TabBar`'s `on_click` callback.
pub fn tab_bar_element(
    session_manager: Entity<GuiSessionManager>,
    cx: &App,
) -> AnyElement {
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
            let label_area = div()
                .id(ElementId::Name(format!("tab-ctx-{i}").into()))
                .flex_1()
                .min_w_0()
                .truncate()
                .child(name.clone())
                .context_menu(move |menu, _, _| {
                    let mgr = mgr_menu.clone();
                    let mgr_close_all = mgr.clone();
                    menu.item(
                        PopupMenuItem::new("Move tab to new window")
                            .disabled(true),
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
        .children(tabs)
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
    let saved = session_manager.read(cx).recent_sessions().to_vec();
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

    // Rules opens the session-rules dialog (a later phase).
    let rules_btn = Button::new("toolbar_rules")
        .icon(IconName::Gavel)
        .ghost()
        .small()
        .disabled(true)
        .tooltip("Session rules (coming soon)")
        .accessibility_label("Session rules");

    Toolbar::new("main-toolbar")
        .child(
            ToolbarGroup::new("toolbar-nav")
                .label("Navigation")
                .child(home_btn)
                .content(sessions_btn),
        )
        .child(
            ToolbarGroup::new("toolbar-display")
                .label("Display filters")
                .child(all_btn)
                .child(diffs_btn)
                .child(same_btn)
                .child(minor_btn),
        )
        .child(
            ToolbarGroup::new("toolbar-ops")
                .label("Operations")
                .child(refresh_btn)
                .child(rules_btn),
        )
        .into_any_element()
}
