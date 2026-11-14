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

use cocomo_lib::TransferAction;
use gpui::{
    AnyElement, App, Context, ElementId, Entity, IntoElement, Pixels, Render,
    SharedString, Window,
    assets::IconName,
    base::{Disableable as _, Selectable as _},
    component::{
        ActiveTheme, Icon, Sizable as _, WindowExt as _,
        button::{
            Button, ButtonCustomVariant, ButtonVariants as _, DropdownButton,
        },
        h_flex,
        menu::{ContextMenuExt as _, PopupMenuItem},
        tab::{Tab, TabBar},
        toolbar::{Toolbar, ToolbarGroup},
        v_flex,
    },
    div, img,
    prelude::*,
    px,
};

use crate::{
    commands, log,
    log::LogLevel,
    menus::{SaveSession, SessionSettings},
    session_manager::GuiSessionManager,
    state::{AppState, FilterPreset},
    tabview::TabEntity,
    ui::run_transfer_action,
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
                // the context menu ("Move tab to new window"): GPUI
                // delivers an internal drag's mouse-up to the window that
                // started it, so a cross-window drop would never arrive.
                // This is a framework limitation, not a missing feature;
                // the context menu is the supported path (gap plan S1).
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
                            move |_, window, cx| {
                                mgr_close_all.update(cx, |m, cx| {
                                    m.request_close_all_sessions(window, cx);
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

/// Preview glyph for one display-filter preset, at a fixed icon size.
///
/// The preset icons encode state in color (red newer, gray older, blue
/// orphan, hollow identical), so they render as stacked `img` layers to
/// keep the palette; the tinted `Icon` path would collapse mirrored
/// presets into the same silhouette. Every layer shares the composite's
/// viewBox, so pinning the layers to the top-left of this fixed-size
/// container at the icon size reproduces the original glyph exactly.
/// [`FilterPreset::All`] has no custom glyph and uses the Lucide eye.
fn preset_icon(preset: FilterPreset, size: Pixels) -> AnyElement {
    let layers = preset.icon_layers();
    if layers.is_empty() {
        return Icon::new(IconName::Eye).with_size(size).into_any_element();
    }
    div()
        .relative()
        .size(size)
        .flex_shrink_0()
        .children(layers.iter().map(|path| {
            // The `img` element falls back to the image's intrinsic
            // 512px size whenever the style leaves width/height auto,
            // so pin every layer to the container's top-left at the
            // explicit icon size. Later children paint above earlier
            // ones, matching the bottom-to-top layer order from
            // `icon_layers`.
            img(SharedString::from(*path))
                .absolute()
                .top_0()
                .left_0()
                .size(size)
        }))
        .into_any_element()
}

/// Build toolbar row 1 using `gpui-component`'s [`Toolbar`].
///
/// Row 1 is shared across all views: Home, the Sessions dropdown, the
/// display-filter preset toggle group, the Structure and Context
/// dropdowns, the Ops controls (copy, expand/collapse, Select, Files),
/// Refresh, and Rules. Folder-only controls disable on text tabs and
/// vice versa. `active_state` is the active folder tab's state, or
/// `None` for text-diff tabs and an empty window.
pub fn toolbar_element(
    active_state: Option<Entity<AppState>>,
    session_manager: Entity<GuiSessionManager>,
    cx: &App,
) -> AnyElement {
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

    // Display filter presets: an exclusive icon toggle group, one
    // icon-only button per preset in `FilterPreset::ALL` order. The
    // active preset carries the selected styling; "Differences only"
    // tints red while active so the narrowed view stays noticeable.
    let current_preset = active_state
        .as_ref()
        .and_then(|state| state.read(cx).filter_preset());
    let theme = cx.theme();
    let preset_buttons: Vec<AnyElement> = FilterPreset::ALL
        .into_iter()
        .map(|preset| {
            let state = active_state.clone();
            let active = current_preset == Some(preset);
            let label: SharedString = preset.label().into();
            let button = Button::new((
                ElementId::from("toolbar_preset"),
                label.clone(),
            ))
            .small()
            .ghost()
            .child(preset_icon(preset, px(14.)))
            .selected(active)
            .toggled(active)
            .tooltip(label.clone())
            .accessibility_label(label)
            .disabled(no_folder);
            let button = if active && preset == FilterPreset::DiffNoOrphans {
                button.custom(
                    ButtonCustomVariant::new(cx)
                        .color(theme.danger.opacity(0.15))
                        .foreground(theme.danger)
                        .hover(theme.danger.opacity(0.25))
                        .active(theme.danger.opacity(0.35)),
                )
            } else {
                button
            };
            button
                .on_click(move |_, _, cx| {
                    if let Some(state) = &state {
                        state.update(cx, |state, cx| {
                            state.set_filter_preset(preset, cx);
                        });
                    }
                })
                .into_any_element()
        })
        .collect();

    // Structure (folder tabs): the sparse-display toggle plus the
    // expand/collapse presets for the mirrored tree.
    let sparse = active_state
        .as_ref()
        .is_some_and(|state| state.read(cx).is_sparse());
    let structure_state = active_state.clone();
    let structure_btn = DropdownButton::new("toolbar_structure")
        .button(
            Button::new("toolbar_structure_btn")
                .label("Structure")
                .ghost()
                .small()
                .disabled(no_folder),
        )
        .menu_accessibility_label("Tree structure")
        .dropdown_menu(move |menu, _, _| {
            let Some(state) = structure_state.clone() else {
                return menu;
            };
            let mut menu = menu.item(
                PopupMenuItem::new("Sparse display")
                    .checked(sparse)
                    .on_click({
                        let state = state.clone();
                        move |_, _, cx| {
                            state.update(cx, |state, cx| {
                                state.toggle_sparse(cx);
                            });
                        }
                    }),
            );
            menu = menu.separator();
            for (label, action) in [
                ("Expand all", 0usize),
                ("Expand 1 level", 1),
                ("Expand 2 levels", 2),
                ("Expand 3 levels", 3),
                ("Collapse all", usize::MAX),
            ] {
                let state = state.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(
                    move |_, _, cx| {
                        state.update(cx, |state, cx| match action {
                            0 => state.expand_all(cx),
                            usize::MAX => state.collapse_all(cx),
                            depth => state.expand_to_depth(depth, cx),
                        });
                    },
                ));
            }
            menu
        });

    // Context (text tabs): how many context lines stay visible around
    // each difference section.
    let text_session = session_manager.read(cx).active_text_session(cx);
    let context_depth = text_session
        .as_ref()
        .and_then(|session| session.read(cx).context_depth());
    let context_state = text_session.clone();
    let context_btn = DropdownButton::new("toolbar_context")
        .button(
            Button::new("toolbar_context_btn")
                .label("Context")
                .ghost()
                .small()
                .disabled(text_session.is_none()),
        )
        .menu_accessibility_label("Section context depth")
        .dropdown_menu(move |menu, _, _| {
            let Some(session) = context_state.clone() else {
                return menu;
            };
            let mut menu = menu;
            for (label, depth) in [
                ("Full context", None),
                ("Differences only", Some(0usize)),
                ("1 line", Some(1)),
                ("2 lines", Some(2)),
                ("3 lines", Some(3)),
            ] {
                let session = session.clone();
                menu = menu.item(
                    PopupMenuItem::new(label)
                        .checked(context_depth == depth)
                        .on_click(move |_, _, cx| {
                            session.update(cx, |session, cx| {
                                session.set_context_depth(depth, cx);
                            });
                        }),
                );
            }
            menu
        });

    // Ops: reconcile the selected entry toward either side. The
    // confirmation flow is shared with the File menu commands.
    let can_copy_right = active_state.as_ref().is_some_and(|state| {
        state.read(cx).can_transfer(TransferAction::CopyRight)
    });
    let can_copy_left = active_state.as_ref().is_some_and(|state| {
        state.read(cx).can_transfer(TransferAction::CopyLeft)
    });
    let copy_right_btn = Button::new("toolbar_copy_right")
        .icon(IconName::ArrowRightFromLine)
        .ghost()
        .small()
        .disabled(!can_copy_right)
        .tooltip("Copy to right")
        .accessibility_label("Copy to right")
        .on_click({
            let state = active_state.clone();
            move |_, window, cx| {
                if let Some(state) = &state {
                    run_transfer_action(
                        state.clone(),
                        TransferAction::CopyRight,
                        window,
                        cx,
                    );
                }
            }
        });
    let copy_left_btn = Button::new("toolbar_copy_left")
        .icon(IconName::ArrowLeftFromLine)
        .ghost()
        .small()
        .disabled(!can_copy_left)
        .tooltip("Copy to left")
        .accessibility_label("Copy to left")
        .on_click({
            let state = active_state.clone();
            move |_, window, cx| {
                if let Some(state) = &state {
                    run_transfer_action(
                        state.clone(),
                        TransferAction::CopyLeft,
                        window,
                        cx,
                    );
                }
            }
        });

    // Expand/collapse every directory of the tree in one step.
    let expand_btn = Button::new("toolbar_expand")
        .icon(IconName::UnfoldVertical)
        .ghost()
        .small()
        .disabled(no_folder)
        .tooltip("Expand all directories")
        .accessibility_label("Expand all directories")
        .on_click({
            let state = active_state.clone();
            move |_, _, cx| {
                if let Some(state) = &state {
                    state.update(cx, |state, cx| state.expand_all(cx));
                }
            }
        });
    let collapse_btn = Button::new("toolbar_collapse")
        .icon(IconName::FoldVertical)
        .ghost()
        .small()
        .disabled(no_folder)
        .tooltip("Collapse all directories")
        .accessibility_label("Collapse all directories")
        .on_click({
            let state = active_state.clone();
            move |_, _, cx| {
                if let Some(state) = &state {
                    state.update(cx, |state, cx| state.collapse_all(cx));
                }
            }
        });

    // Select: whole-table selection commands over the visible rows.
    let select_state = active_state.clone();
    let select_btn = DropdownButton::new("toolbar_select")
        .button(
            Button::new("toolbar_select_btn")
                .label("Select")
                .ghost()
                .small()
                .disabled(no_folder),
        )
        .menu_accessibility_label("Selection commands")
        .dropdown_menu(move |menu, _, _| {
            let Some(state) = select_state.clone() else {
                return menu;
            };
            let mut menu = menu;
            for (label, action) in [
                ("Select all entries", 0usize),
                ("Invert selection", 1),
                ("Select files by filter", 2),
            ] {
                let state = state.clone();
                menu = menu.item(PopupMenuItem::new(label).on_click(
                    move |_, _, cx| {
                        state.update(cx, |state, cx| match action {
                            0 => state.select_all_entries(cx),
                            1 => state.invert_selection(cx),
                            _ => state.select_by_filter(cx),
                        });
                    },
                ));
            }
            menu
        });

    // Files: export the visible filenames or open the comparison
    // report generated by cocomo-lib.
    let files_state = active_state.clone();
    let files_btn = DropdownButton::new("toolbar_files")
        .button(
            Button::new("toolbar_files_btn")
                .label("Files")
                .ghost()
                .small()
                .disabled(no_folder),
        )
        .menu_accessibility_label("File commands")
        .dropdown_menu(move |menu, _, _| {
            let Some(state) = files_state.clone() else {
                return menu;
            };
            let export_state = state.clone();
            menu.item(PopupMenuItem::new("Export filenames…").on_click(
                move |_, _, cx| {
                    let (paths, dir) =
                        export_state.read_with(cx, |state, _| {
                            (
                                state.visible_file_paths(),
                                state.left().path().to_path_buf(),
                            )
                        });
                    if paths.is_empty() {
                        log::push(
                            LogLevel::Info,
                            "No visible files to export",
                            cx,
                        );
                        return;
                    }
                    let mut text = paths.join("\n");
                    text.push('\n');
                    write_export(
                        text,
                        dir,
                        "filenames.txt",
                        "Filename list",
                        cx,
                    );
                },
            ))
            .item(PopupMenuItem::new("Open report…").on_click(
                move |_, window, cx| {
                    open_folder_report_dialog(state.clone(), window, cx);
                },
            ))
        });

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
        display_group = display_group.content(
            h_flex().items_center().gap_0p5().children(preset_buttons),
        );
    }
    if commands::is_visible("toolbar.structure", cx) {
        display_group = display_group.content(structure_btn);
    }
    if commands::is_visible("toolbar.context", cx) {
        display_group = display_group.content(context_btn);
    }

    let mut ops_group = ToolbarGroup::new("toolbar-ops").label("Operations");
    if commands::is_visible("file.copy_right", cx) {
        ops_group = ops_group.child(copy_right_btn);
    }
    if commands::is_visible("file.copy_left", cx) {
        ops_group = ops_group.child(copy_left_btn);
    }
    if commands::is_visible("toolbar.expand", cx) {
        ops_group = ops_group.child(expand_btn);
    }
    if commands::is_visible("toolbar.collapse", cx) {
        ops_group = ops_group.child(collapse_btn);
    }
    if commands::is_visible("toolbar.select", cx) {
        ops_group = ops_group.content(select_btn);
    }
    if commands::is_visible("toolbar.files", cx) {
        ops_group = ops_group.content(files_btn);
    }
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

/// Prompt for a path and write `text` to it; the outcome goes to the
/// log, mirroring the text view's report save.
fn write_export(
    text: String,
    dir: std::path::PathBuf,
    default_name: &'static str,
    what: &'static str,
    cx: &mut App,
) {
    let receiver = cx.prompt_for_new_path(&dir, Some(default_name));
    cx.spawn(async move |cx| {
        let path = match receiver.await {
            Ok(Ok(Some(path))) => path,
            // The user cancelled the save dialog.
            Ok(Ok(None)) => return,
            Ok(Err(e)) => {
                cx.update(|cx| {
                    log::push(
                        LogLevel::Error,
                        format!("{what} export failed: {e}"),
                        cx,
                    );
                });
                return;
            }
            Err(_) => return,
        };
        let for_log = path.display().to_string();
        let result = crate::runtime::spawn_on_tokio(async move {
            fs_err::tokio::write(&path, text.as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await;
        cx.update(|cx| match result {
            Ok(Ok(())) => log::push(
                LogLevel::Info,
                format!("{what} exported: {for_log}"),
                cx,
            ),
            Ok(Err(e)) => log::push(
                LogLevel::Error,
                format!("{what} export failed: {e}"),
                cx,
            ),
            Err(e) => log::push(
                LogLevel::Error,
                format!("{what} export task panicked: {e}"),
                cx,
            ),
        });
    })
    .detach();
}

/// Open the plain-text comparison report of a folder session in a
/// dialog, generated by cocomo-lib's report writer.
///
/// The dialog previews the report and — mirroring the TUI's report
/// flow, which writes the report to a chosen destination — offers
/// saving the text to a picked file.
pub(crate) fn open_folder_report_dialog(
    state: Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(text) = state.read(cx).comparison().map(|comparison| {
        cocomo_lib::report::generate_report(
            comparison,
            cocomo_lib::report::ReportFormat::Text,
            &cocomo_lib::report::ReportConfig::all(),
        )
    }) else {
        log::push(LogLevel::Info, "No comparison to report on", cx);
        return;
    };
    // Suggest the left root as the save location.
    let dir = state.read(cx).left().path().to_path_buf();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Comparison report")
            .child(
                div()
                    .id(ElementId::Name("folder_report_preview".into()))
                    .max_h(px(384.))
                    .overflow_y_scroll()
                    .child(text.clone()),
            )
            .footer(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("folder_report_close")
                            .label("Close")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                            }),
                    )
                    .child(
                        Button::new("folder_report_save")
                            .label("Save to file…")
                            .primary()
                            .on_click({
                                let text = text.clone();
                                let dir = dir.clone();
                                move |_, window, cx| {
                                    window.close_dialog(cx);
                                    write_export(
                                        text.clone(),
                                        dir.clone(),
                                        "cocomo-report.txt",
                                        "Comparison report",
                                        cx,
                                    );
                                }
                            }),
                    ),
            )
    });
}

/// Open the comparison summary of a folder session: the pair paths,
/// the per-status counts over the entries the filters show, and the
/// active display filters.
pub(crate) fn open_folder_info_dialog(
    state: Entity<AppState>,
    window: &mut Window,
    cx: &mut App,
) {
    let rows = state.read_with(cx, |state, _| {
        let mut rows = vec![
            format!("Left:  {}", state.left().display()),
            format!("Right: {}", state.right().display()),
        ];
        if state.has_comparison() {
            let summary = state.visible_summary();
            rows.push(format!(
                "Entries: {} ({} files, {} directories)",
                summary.total, summary.files, summary.dirs
            ));
            rows.push(format!("Identical: {}", summary.same));
            rows.push(format!("Different: {}", summary.different));
            rows.push(format!("Orphans: {}", summary.orphans));
        } else {
            rows.push("Comparison: not loaded yet".to_string());
        }
        let preset = state
            .filter_preset()
            .map_or("Custom", |preset| preset.label());
        rows.push(format!("Filters: {preset}"));
        rows.push(format!(
            "Hidden items: {}",
            if state.show_hidden() {
                "shown"
            } else {
                "hidden"
            }
        ));
        if let Some(pattern) = state.filter_pattern() {
            rows.push(format!("Name filter: {pattern}"));
        }
        rows
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Session info")
            .child(v_flex().gap_1().text_sm().children(rows.clone()))
    });
}

#[cfg(test)]
mod tests {
    use cocomo_lib::Endpoint;
    use gpui::{
        Bounds, TestAppContext, WindowBounds, WindowOptions,
        test::TestWindowExt,
    };

    use super::{toolbar_element, *};
    use crate::{
        session_manager::{GuiSessionManager, SavedSessionStore},
        state::{AppState, FilterPreset},
    };

    /// Minimal host view that renders the shared toolbar row so a
    /// headless window can exercise its controls without the full
    /// window root.
    struct ToolbarHost {
        state: Entity<AppState>,
        manager: Entity<GuiSessionManager>,
    }

    impl Render for ToolbarHost {
        fn render(
            &mut self,
            _window: &mut Window,
            cx: &mut Context<Self>,
        ) -> impl IntoElement {
            let element: AnyElement = toolbar_element(
                Some(self.state.clone()),
                self.manager.clone(),
                cx,
            );
            div().size_full().child(element)
        }
    }

    fn open_toolbar(
        cx: &mut TestAppContext,
        state: Entity<AppState>,
        manager: Entity<GuiSessionManager>,
    ) -> gpui::AnyWindowHandle {
        cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|_| ToolbarHost { state, manager }),
            )
            .expect("failed to open the test window")
            .0
        })
    }

    /// UI integration test: the preset toggle group is a real control.
    /// Renders the production toolbar in a headless window, clicks the
    /// "Differences only" glyph, and verifies the folder state adopts
    /// that display-filter preset.
    #[gpui::test]
    fn toolbar_preset_toggle_applies_the_display_filter(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::init(cx);
            cx.set_global(crate::options::AppOptions::default());
            cx.set_global(crate::log::LogBuffer::default());
        });
        let state = cx.new(|cx| {
            AppState::new(
                Endpoint::local("/left"),
                Endpoint::local("/right"),
                "test".into(),
                cx,
            )
        });
        let store = cx.new(|_| {
            SavedSessionStore::new(std::path::PathBuf::from(
                "/tmp/cocomo-chrome-tests",
            ))
        });
        let manager = cx.new(|_| GuiSessionManager::new(store));
        let window = open_toolbar(cx, state.clone(), manager);

        let preset_btn = (
            ElementId::from("toolbar_preset"),
            "Differences only".to_string(),
        );
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            assert_eq!(
                state.read(cx).filter_preset(),
                Some(FilterPreset::All)
            );
            window.click(preset_btn, cx);
        })
        .expect("failed to click the preset toggle");

        cx.update(|cx| {
            assert_eq!(
                state.read(cx).filter_preset(),
                Some(FilterPreset::DiffNoOrphans)
            );
        });
    }
}
