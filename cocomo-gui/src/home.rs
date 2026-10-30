// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Home view: the start screen for launching and reopening sessions.
//!
//! [`HomeView`] renders as a tab (see [`crate::tabview::TabEntity`]). Its
//! left region offers large per-type launcher cards that accept file-system
//! drops (one item for both sides, two items for left and right); its right
//! panel lists saved sessions (searchable, with a context menu) and the
//! auto-saved history of recently opened sessions.

use std::cell::Cell;

use cocomo_lib::{Endpoint, SessionType};
use gpui::{
    App, AsyncApp, ClickEvent, Context, ElementId, Entity, ExternalPaths,
    FocusHandle, Focusable, FontWeight, KeyBinding, PathPromptOptions, Render,
    SharedString, Subscription, Window, actions,
    assets::IconName,
    base::Disableable as _,
    component::{
        ActiveTheme, Sizable as _, WindowExt as _,
        button::{Button, ButtonVariant, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{ContextMenuExt as _, PopupMenuItem},
        v_flex,
    },
    div,
    prelude::*,
};

use crate::{
    log::{self, LogLevel},
    session_manager::{GuiSessionManager, SavedSession},
};

// ---------------------------------------------------------------------------
// Actions and key bindings
// ---------------------------------------------------------------------------

actions!(
    home,
    [OpenSavedSession, SelectSessionNext, SelectSessionPrev]
);

/// Key bindings for the Home view's saved-session list.
///
/// Bound globally but handled only while the Home view is in the focused
/// region, so they never collide with the folder view's own `enter`.
pub fn home_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("down", SelectSessionNext, None),
        KeyBinding::new("up", SelectSessionPrev, None),
        KeyBinding::new("enter", OpenSavedSession, None),
    ]
}

// ---------------------------------------------------------------------------
// Text Compare prompt flow
// ---------------------------------------------------------------------------

/// Ask for two files in sequence, then open a Text Compare tab.
///
/// The second dialog is only shown after the first file is chosen;
/// cancelling either step abandons the flow silently.
fn prompt_text_compare_pair(mgr: Entity<GuiSessionManager>, cx: &mut App) {
    let options = |prompt: &'static str| PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(prompt.into()),
    };
    let left_receiver = cx.prompt_for_paths(options("Choose the left file"));
    cx.spawn(move |cx: &mut AsyncApp| {
        let cx = cx.clone();
        async move {
            let Ok(Ok(Some(paths))) = left_receiver.await else {
                return;
            };
            let Some(left) = paths.into_iter().next() else {
                return;
            };
            let right_receiver = cx.update(|cx| {
                cx.prompt_for_paths(options("Choose the right file"))
            });
            let Ok(Ok(Some(paths))) = right_receiver.await else {
                return;
            };
            let Some(right) = paths.into_iter().next() else {
                return;
            };
            cx.update(|cx| {
                mgr.update(cx, |m, cx| {
                    m.add_text_diff_tab(
                        Endpoint::local(left),
                        Endpoint::local(right),
                        None,
                        cx,
                    );
                });
            });
        }
    })
    .detach();
}

// ---------------------------------------------------------------------------
// Launcher cards
// ---------------------------------------------------------------------------

/// One large launcher card in the new-session grid.
///
/// The card div owns the drop target (a [`Button`] cannot carry drop
/// listeners); the button inside owns the click. A drop needs no preceding
/// mouse-down, and a click has no active drag, so the two never collide.
fn session_card(
    id: &'static str,
    icon: IconName,
    title: &'static str,
    drop_kind: Option<SessionType>,
    coming_soon: bool,
    session_manager: Entity<GuiSessionManager>,
    cx: &App,
) -> gpui::AnyElement {
    let theme = cx.theme();
    let primary = theme.primary;

    let button = Button::new(ElementId::Name(format!("{id}-launch").into()))
        .icon(icon)
        .label(title)
        .outline()
        .large();
    let button = if coming_soon {
        button.disabled(true).tooltip("Coming soon")
    } else if drop_kind == Some(SessionType::TextCompare) {
        let mgr = session_manager.clone();
        button.on_click(move |_, _, cx| {
            prompt_text_compare_pair(mgr.clone(), cx);
        })
    } else {
        let mgr = session_manager.clone();
        button.on_click(move |_, _, cx| {
            mgr.update(cx, |m, cx| m.add_new_folder_tab(cx));
        })
    };

    let card = div()
        .id(ElementId::Name(format!("{id}-card").into()))
        .flex()
        .items_center()
        .justify_center()
        .w_56()
        .p_2()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .child(button);

    match drop_kind {
        Some(kind) => card
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_color(primary)
            })
            .on_drop::<ExternalPaths>(move |paths, _, cx| {
                session_manager.update(cx, |m, cx| {
                    m.start_dropped_session(kind, paths.paths(), cx);
                });
            })
            .into_any_element(),
        None => card.into_any_element(),
    }
}

// ---------------------------------------------------------------------------
// Dialog helpers
// ---------------------------------------------------------------------------

/// Open the rename dialog for a saved session file.
fn open_rename_dialog(
    mgr: Entity<GuiSessionManager>,
    path: std::path::PathBuf,
    current_name: String,
    window: &mut Window,
    cx: &mut App,
) {
    // Created before opening: the dialog builder re-runs on every layer
    // rebuild, so persistent entities must live outside it.
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder("Session name")
            .default_value(current_name)
    });
    let initial_focus = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if initial_focus.replace(false) {
            input.focus_handle(cx).focus(window, cx);
        }
        let rename_mgr = mgr.clone();
        let rename_path = path.clone();
        let rename_input = input.clone();
        dialog
            .title("Rename session")
            .child(Input::new(&input))
            .footer(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("rename_cancel").label("Cancel").on_click(
                            |_, window, cx| {
                                window.close_dialog(cx);
                            },
                        ),
                    )
                    .child(
                        Button::new("rename_ok")
                            .label("Rename")
                            .primary()
                            .on_click(move |_, window, cx| {
                                let name = rename_input
                                    .read(cx)
                                    .value()
                                    .trim()
                                    .to_string();
                                if !name.is_empty() {
                                    rename_mgr.update(cx, |m, cx| {
                                        m.rename_saved_session(
                                            rename_path.clone(),
                                            name,
                                            cx,
                                        );
                                    });
                                }
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}

/// Confirm and delete a saved session file. Deletion cannot be undone, so
/// the confirm uses the danger variant.
fn open_delete_confirm(
    mgr: Entity<GuiSessionManager>,
    path: std::path::PathBuf,
    name: String,
    window: &mut Window,
    cx: &mut App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        // Clone per builder call: the builder is an `Fn` closure that may
        // run on every layer rebuild.
        let delete_mgr = mgr.clone();
        let delete_path = path.clone();
        alert
            .confirm()
            .title(format!("Delete \"{name}\"?"))
            .description(
                "The saved session file is removed. This cannot be undone.",
            )
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, _, cx| {
                delete_mgr.update(cx, |m, cx| {
                    m.delete_saved_session(delete_path.clone(), cx);
                });
                true
            })
    });
}

// ---------------------------------------------------------------------------
// Home View
// ---------------------------------------------------------------------------

/// The Home start screen: new-session launchers and saved sessions.
///
/// Keyboard focus lives on the view's root; the up/down/enter actions walk
/// and open the saved-session list. The view never requests focus from its
/// own `render` (the window root focuses it on tab activation).
pub struct HomeView {
    focus_handle: FocusHandle,
    session_manager: Entity<GuiSessionManager>,
    /// Search field filtering the saved-session list. Created lazily in
    /// `render` because `InputState` needs a window handle.
    search_input: Option<Entity<InputState>>,
    /// Subscription that re-renders on search changes and opens on Enter.
    _search_subscription: Option<Subscription>,
    /// Observer of the session manager, keeping the lists current.
    _manager_subscription: Subscription,
    /// Selected row in the filtered saved-session list.
    selected_index: Option<usize>,
}

impl HomeView {
    pub fn new(
        session_manager: Entity<GuiSessionManager>,
        cx: &mut Context<Self>,
    ) -> Self {
        let _manager_subscription =
            cx.observe(&session_manager, |_, _, cx| cx.notify());
        Self {
            focus_handle: cx.focus_handle(),
            session_manager,
            search_input: None,
            _search_subscription: None,
            _manager_subscription,
            selected_index: None,
        }
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// The saved sessions matching the current search text, in list order.
    fn filtered_saved(&self, cx: &App) -> Vec<SavedSession> {
        let query = self
            .search_input
            .as_ref()
            .map(|input| input.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let mgr = self.session_manager.read(cx);
        mgr.recent_sessions()
            .iter()
            .filter(|saved| {
                query.is_empty()
                    || saved.config().name.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    /// Open the selected saved session, or the first one when nothing is
    /// selected yet.
    fn open_active_saved(&mut self, cx: &mut Context<Self>) {
        let saved = self.filtered_saved(cx);
        let Some(index) = self
            .selected_index
            .or_else(|| (!saved.is_empty()).then_some(0))
        else {
            return;
        };
        let Some(entry) = saved.get(index) else {
            return;
        };
        let config = entry.config().clone();
        self.session_manager.update(cx, |m, cx| {
            if let Err(e) = m.open_saved(config, cx) {
                log::push(
                    LogLevel::Error,
                    format!("Open session failed: {e}"),
                    cx,
                );
            }
        });
    }

    fn select_next(
        &mut self,
        _: &SelectSessionNext,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.filtered_saved(cx).len();
        if count > 0 {
            self.selected_index = Some(
                self.selected_index.map_or(0, |ix| (ix + 1).min(count - 1)),
            );
            cx.notify();
        }
    }

    fn select_prev(
        &mut self,
        _: &SelectSessionPrev,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.filtered_saved(cx).is_empty() {
            return;
        }
        self.selected_index =
            Some(self.selected_index.map_or(0, |ix| ix.saturating_sub(1)));
        cx.notify();
    }

    fn open_selected(
        &mut self,
        _: &OpenSavedSession,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_active_saved(cx);
    }

    /// One row of the saved-session list: click selects, double-click
    /// opens, and the context menu carries rename, delete, and default.
    fn saved_row(
        &self,
        index: usize,
        entry: &SavedSession,
        is_default: bool,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = cx.theme();
        let name: SharedString = entry.config().name.clone().into();
        let path = entry.path().to_path_buf();

        let open_mgr = self.session_manager.clone();
        let open_config = entry.config().clone();
        let rename_mgr = self.session_manager.clone();
        let rename_path = path.clone();
        let rename_name = entry.config().name.clone();
        let delete_mgr = self.session_manager.clone();
        let delete_path = path.clone();
        let delete_name = entry.config().name.clone();
        let default_mgr = self.session_manager.clone();
        let default_path = path.clone();

        div()
            .id(ElementId::Name(path.to_string_lossy().into_owned().into()))
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded_md()
            .cursor_pointer()
            .hover(|style| style.bg(theme.muted))
            .when(selected, |row| row.bg(theme.accent))
            .child(div().flex_1().min_w_0().truncate().child(name))
            .when(is_default, |row| {
                row.child(
                    div()
                        .text_xs()
                        .flex_shrink_0()
                        .text_color(theme.muted_foreground)
                        .child("Default"),
                )
            })
            .on_click(cx.listener(
                move |this, event: &ClickEvent, window: &mut Window, cx| {
                    this.selected_index = Some(index);
                    if event.click_count() >= 2 {
                        this.open_active_saved(cx);
                    } else {
                        // Keep keyboard focus on the view so up/down/enter
                        // keep working after a click.
                        this.focus_handle.focus(window, cx);
                    }
                    cx.notify();
                },
            ))
            .context_menu(move |menu, _, _| {
                // Clone per builder call: the builder is an `Fn` closure
                // that may run on every menu open.
                let open_mgr = open_mgr.clone();
                let open_config = open_config.clone();
                let rename_mgr = rename_mgr.clone();
                let rename_path = rename_path.clone();
                let rename_name = rename_name.clone();
                let delete_mgr = delete_mgr.clone();
                let delete_path = delete_path.clone();
                let delete_name = delete_name.clone();
                let default_mgr = default_mgr.clone();
                let default_path = default_path.clone();
                menu.item(PopupMenuItem::new("Open").on_click(
                    move |_, _, cx| {
                        open_mgr.update(cx, |m, cx| {
                            if let Err(e) =
                                m.open_saved(open_config.clone(), cx)
                            {
                                log::push(
                                    LogLevel::Error,
                                    format!("Open session failed: {e}"),
                                    cx,
                                );
                            }
                        });
                    },
                ))
                .item(PopupMenuItem::new("Rename…").on_click(
                    move |_, window, cx| {
                        open_rename_dialog(
                            rename_mgr.clone(),
                            rename_path.clone(),
                            rename_name.clone(),
                            window,
                            cx,
                        );
                    },
                ))
                .item(PopupMenuItem::new("Delete…").on_click(
                    move |_, window, cx| {
                        open_delete_confirm(
                            delete_mgr.clone(),
                            delete_path.clone(),
                            delete_name.clone(),
                            window,
                            cx,
                        );
                    },
                ))
                .item(
                    PopupMenuItem::new(if is_default {
                        "Unset as default"
                    } else {
                        "Set as default"
                    })
                    .on_click(move |_, _, cx| {
                        default_mgr.update(cx, |m, cx| {
                            m.set_default_session(default_path.clone(), cx);
                        });
                    }),
                )
            })
            .into_any_element()
    }

    /// One row of the auto-saved history list. History is transient, so a
    /// single click opens it and there is no context menu.
    fn history_row(
        entry: &SavedSession,
        session_manager: Entity<GuiSessionManager>,
    ) -> gpui::AnyElement {
        let config = entry.config().clone();
        let tooltip: SharedString = format!(
            "{} vs {}",
            config.left.path.display(),
            config.right.path.display()
        )
        .into();
        Button::new(ElementId::Name(
            format!("history-{}", entry.path().display()).into(),
        ))
        .label(config.name.clone())
        .ghost()
        .small()
        .w_full()
        .justify_start()
        .truncate()
        .tooltip(tooltip)
        .on_click(move |_, _, cx| {
            session_manager.update(cx, |m, cx| {
                if let Err(e) = m.open_saved(config.clone(), cx) {
                    log::push(
                        LogLevel::Error,
                        format!("Open session failed: {e}"),
                        cx,
                    );
                }
            });
        })
        .into_any_element()
    }
}

impl Focusable for HomeView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HomeView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Create the search input on first render: re-render on every
        // change so the list filters live, and open on Enter.
        if self.search_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Search sessions")
            });
            self._search_subscription = Some(cx.subscribe(
                &input,
                |this, _input, event, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => {
                        this.open_active_saved(cx);
                    }
                    _ => {}
                },
            ));
            self.search_input = Some(input);
        }

        // Copy the theme colors up front: `cx.theme()` borrows the app,
        // and building rows needs mutable app access.
        let (bg, fg, border, muted_foreground) = {
            let theme = cx.theme();
            (
                theme.background,
                theme.foreground,
                theme.border,
                theme.muted_foreground,
            )
        };
        let focus_handle = self.focus_handle(cx);
        let query_empty = self
            .search_input
            .as_ref()
            .is_none_or(|input| input.read(cx).value().trim().is_empty());

        let saved = self.filtered_saved(cx);
        // Drop a selection that the current filter no longer contains.
        if let Some(ix) = self.selected_index
            && ix >= saved.len()
        {
            self.selected_index = None;
        }

        let (history, default_name) = {
            let mgr = self.session_manager.read(cx);
            (
                if query_empty {
                    mgr.history().to_vec()
                } else {
                    Vec::new()
                },
                mgr.default_session_name().map(str::to_owned),
            )
        };

        let mgr = self.session_manager.clone();
        let cards = h_flex()
            .flex_wrap()
            .gap_3()
            .items_start()
            .child(session_card(
                "folder-compare",
                IconName::Folder,
                "Folder Compare",
                Some(SessionType::DirCompare),
                false,
                mgr.clone(),
                cx,
            ))
            .child(session_card(
                "text-compare",
                IconName::FileDiff,
                "Text Compare",
                Some(SessionType::TextCompare),
                false,
                mgr.clone(),
                cx,
            ))
            .child(session_card(
                "text-merge",
                IconName::Merge,
                "Text Merge",
                None,
                true,
                mgr.clone(),
                cx,
            ))
            .child(session_card(
                "folder-sync",
                IconName::FolderSync,
                "Folder Sync",
                None,
                true,
                mgr.clone(),
                cx,
            ))
            .child(session_card(
                "table-compare",
                IconName::Table,
                "Table Compare",
                None,
                true,
                mgr.clone(),
                cx,
            ))
            .child(session_card(
                "hex-compare",
                IconName::Binary,
                "Hex Compare",
                None,
                true,
                mgr.clone(),
                cx,
            ))
            .child(session_card(
                "picture-compare",
                IconName::Image,
                "Picture Compare",
                None,
                true,
                mgr.clone(),
                cx,
            ))
            .child(session_card(
                "registry-compare",
                IconName::Database,
                "Registry Compare",
                None,
                true,
                mgr.clone(),
                cx,
            ));

        let left = v_flex()
            .id(ElementId::Name("home-new-scroll".into()))
            .flex_1()
            .min_w_0()
            .overflow_y_scroll()
            .p_6()
            .gap_4()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("New session"),
                    )
                    .child(
                        div().text_sm().text_color(muted_foreground).child(
                            "Pick a session type, or drop one or two items \
                             onto a card to start it.",
                        ),
                    ),
            )
            .child(cards);

        let mut rows = Vec::new();
        for (index, entry) in saved.iter().enumerate() {
            let is_default = default_name.as_deref().is_some_and(|name| {
                entry.path().file_name().is_some_and(|file| file == name)
            });
            let selected = self.selected_index == Some(index);
            rows.push(self.saved_row(index, entry, is_default, selected, cx));
        }
        let history_rows: Vec<gpui::AnyElement> = history
            .iter()
            .map(|entry| Self::history_row(entry, mgr.clone()))
            .collect();

        let right = v_flex()
            .w_80()
            .flex_shrink_0()
            .border_l_1()
            .border_color(border)
            .p_4()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Saved sessions"),
            )
            .child(Input::new(
                self.search_input
                    .as_ref()
                    .expect("search input created above"),
            ))
            .child(
                v_flex()
                    .id(ElementId::Name("saved-scroll".into()))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_0p5()
                    .when(saved.is_empty(), |list| {
                        list.child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(if query_empty {
                                    "No saved sessions yet."
                                } else {
                                    "No sessions match the search."
                                }),
                        )
                    })
                    .children(rows)
                    .when(query_empty, |list| {
                        list.child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .pt_3()
                                .child("History"),
                        )
                        .when(history.is_empty(), |list| {
                            list.child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child("No recently opened sessions."),
                            )
                        })
                        .children(history_rows)
                    }),
            );

        div()
            .key_context("Home")
            .track_focus(&focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::open_selected))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(left)
                    .child(right),
            )
    }
}
