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
//! window chrome (tab bar, toolbar rows, log pane, window title) and
//! delegates content rendering to the active tab's entity via a
//! [`gpui::base::NavStack`], which handles showing only the current tab
//! view without manual flexbox hiding.

use std::any::Any;

use gpui::{
    AnyElement, AnyView, App, Context, ElementId, Entity, FocusHandle, Render,
    SharedString, Subscription, Window,
    assets::IconName,
    base::{NavMotion, NavStack, NavStackState},
    component::{
        ActiveTheme, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex, v_flex,
    },
    div,
    prelude::*,
};

use crate::{
    home::HomeView,
    log::{LogBuffer, LogLevel, LogPaneOpen},
    menus::{self, MenuState},
    session_manager::GuiSessionManager,
    state::AppState,
    text_diff::{TextDiffSession, TextDiffView},
    text_merge::{MergeSession, MergeView},
    ui::FolderCompareView,
};

// ---------------------------------------------------------------------------
// Type-erased tab entity
// ---------------------------------------------------------------------------

/// A type-erased entity that can render itself.
///
/// Stores a [`FolderCompareView`], [`TextDiffView`], or [`HomeView`] entity
/// behind a trait object, exposing only the methods needed for rendering,
/// focus, the window title, and the view-specific toolbar row.
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

    /// Create from a text merge view.
    pub fn from_text_merge(view: Entity<MergeView>) -> Self {
        Self {
            inner: Box::new(MergeTabEntity(view)),
        }
    }

    /// Create from a home view.
    pub fn from_home(view: Entity<HomeView>) -> Self {
        Self {
            inner: Box::new(HomeTabEntity(view)),
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
    /// The window root uses this to sync the toolbar toggle group and the
    /// menu state to the active tab; text-diff tabs return `None`.
    pub fn folder_state(&self, cx: &App) -> Option<Entity<AppState>> {
        self.inner.folder_state(cx)
    }

    /// Return the text-compare session, if this tab is a text compare.
    ///
    /// The window root uses this to sync the view menus (word wrap,
    /// thumbnails) to the active tab; folder tabs return `None`.
    pub fn text_state(&self, cx: &App) -> Option<Entity<TextDiffSession>> {
        self.inner.text_state(cx)
    }

    /// Return the text-merge session, if this tab is a text merge.
    ///
    /// The window root uses this to sync the merge menu state to the
    /// active tab; other tabs return `None`.
    pub fn merge_state(&self, cx: &App) -> Option<Entity<MergeSession>> {
        self.inner.merge_state(cx)
    }

    /// Render this tab's toolbar row 2, or `None` when the view has no
    /// view-specific controls.
    pub fn view_toolbar(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        self.inner.view_toolbar(window, cx)
    }

    /// The window title for this tab, in the spec's
    /// `Cocomo — <View Type> — <name(s)>` format.
    pub fn window_title(&self, cx: &App) -> SharedString {
        self.inner.window_title(cx)
    }

    /// Whether this tab is the Home start screen.
    ///
    /// The window root uses this to hide the view-specific menus while
    /// Home is active.
    pub fn is_home(&self) -> bool {
        self.inner.is_home()
    }
}

/// Inner trait for type-erased tab entities.
trait TabEntityInner: Any + Send + Sync {
    fn focus_handle(&self, cx: &App) -> FocusHandle;
    fn any_view(&self) -> AnyView;
    fn folder_state(&self, cx: &App) -> Option<Entity<AppState>>;
    fn text_state(&self, _cx: &App) -> Option<Entity<TextDiffSession>> {
        None
    }
    fn merge_state(&self, _cx: &App) -> Option<Entity<MergeSession>> {
        None
    }
    fn view_toolbar(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement>;
    fn window_title(&self, cx: &App) -> SharedString;
    fn is_home(&self) -> bool {
        false
    }
    fn clone_inner(&self) -> Box<dyn TabEntityInner>;
}

struct FolderTabEntity(Entity<FolderCompareView>);

/// Base name of an endpoint path, falling back to its full display form.
fn base_name(endpoint: &cocomo_lib::Endpoint) -> String {
    endpoint
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| endpoint.display())
}

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

    fn view_toolbar(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        // `update` (not `read`) because the row lazily creates the filter
        // input on first render; the creation is idempotent.
        Some(
            self.0
                .update(cx, |view, cx| view.render_view_toolbar(window, cx))
                .into_any_element(),
        )
    }

    fn window_title(&self, cx: &App) -> SharedString {
        let state = self.0.read(cx).folder_state();
        let state = state.read(cx);
        format!(
            "Cocomo — Folder Compare — {} vs {}",
            base_name(state.left()),
            base_name(state.right())
        )
        .into()
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

    fn text_state(&self, cx: &App) -> Option<Entity<TextDiffSession>> {
        Some(self.0.read(cx).text_state())
    }

    fn view_toolbar(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        Some(
            self.0
                .update(cx, |view, cx| view.render_view_toolbar(window, cx))
                .into_any_element(),
        )
    }

    fn window_title(&self, cx: &App) -> SharedString {
        format!("Cocomo — Text Compare — {}", self.0.read(cx).title(cx)).into()
    }

    fn clone_inner(&self) -> Box<dyn TabEntityInner> {
        Box::new(TextDiffTabEntity(self.0.clone()))
    }
}

struct HomeTabEntity(Entity<HomeView>);

struct MergeTabEntity(Entity<MergeView>);

impl TabEntityInner for MergeTabEntity {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.0.read(cx).focus_handle(cx)
    }

    fn any_view(&self) -> AnyView {
        self.0.clone().into()
    }

    fn folder_state(&self, _: &App) -> Option<Entity<AppState>> {
        None
    }

    fn merge_state(&self, cx: &App) -> Option<Entity<MergeSession>> {
        Some(self.0.read(cx).merge_state())
    }

    fn view_toolbar(
        &self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        Some(
            self.0
                .update(cx, |view, cx| view.render_view_toolbar(window, cx))
                .into_any_element(),
        )
    }

    fn window_title(&self, cx: &App) -> SharedString {
        format!("Cocomo — Text Merge — {}", self.0.read(cx).title(cx)).into()
    }

    fn clone_inner(&self) -> Box<dyn TabEntityInner> {
        Box::new(MergeTabEntity(self.0.clone()))
    }
}

impl TabEntityInner for HomeTabEntity {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.0.read(cx).focus_handle(cx)
    }

    fn any_view(&self) -> AnyView {
        self.0.clone().into()
    }

    fn folder_state(&self, _: &App) -> Option<Entity<AppState>> {
        None
    }

    fn view_toolbar(&self, _: &mut Window, _: &mut App) -> Option<AnyElement> {
        // Home has no view-specific toolbar row.
        None
    }

    fn window_title(&self, _: &App) -> SharedString {
        "Cocomo — Home".into()
    }

    fn is_home(&self) -> bool {
        true
    }

    fn clone_inner(&self) -> Box<dyn TabEntityInner> {
        Box::new(HomeTabEntity(self.0.clone()))
    }
}

// ---------------------------------------------------------------------------
// Window Root
// ---------------------------------------------------------------------------

/// The root entity of the main window.
///
/// Owns the window chrome (tab bar, toolbar rows, log pane, window
/// title) and keyboard focus: the active tab receives focus when the
/// window opens and whenever the active tab changes. Tab views never
/// request focus from their own `render`.
pub struct WindowRoot {
    /// Handle to the session manager.
    session_manager: Entity<GuiSessionManager>,
    /// Navigation stack managing tab content visibility.
    nav_stack_state: Entity<NavStackState>,
    /// Last known active tab index, for diffing against the session manager.
    last_active_index: usize,
    /// Last menu snapshot pushed to the native menu bar, so the bar is
    /// rebuilt only when it actually changes.
    last_menu_state: Option<MenuState>,
    /// Observers of the log globals, so log lines and pane toggles
    /// re-render the log pane.
    _log_subscriptions: Vec<Subscription>,
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

        // The log buffer and the pane's visibility are globals appended to
        // from anywhere in the app; observing them keeps the pane current
        // without threading a handle through every operation.
        let _log_subscriptions = vec![
            cx.observe_global::<LogBuffer>(|_, cx| cx.notify()),
            cx.observe_global::<LogPaneOpen>(|_, cx| cx.notify()),
        ];

        Self {
            session_manager,
            nav_stack_state: cx.new(|_| NavStackState::new()),
            last_active_index: 0,
            last_menu_state: None,
            _log_subscriptions,
        }
    }

    /// Render the collapsible log pane: a header row with the entry count
    /// and a collapse toggle, plus the recorded lines when open.
    fn render_log_pane(&self, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        let open = cx.global::<LogPaneOpen>().0;
        let buffer = cx.global::<LogBuffer>();
        let (label, icon, tooltip) = if open {
            (
                format!("Log — {}", buffer.len()),
                IconName::PanelBottomClose,
                "Collapse log pane",
            )
        } else {
            (
                format!("Log — {}", buffer.len()),
                IconName::PanelBottomOpen,
                "Expand log pane",
            )
        };

        v_flex()
            .flex_shrink_0()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.background)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_0p5()
                    .bg(theme.muted)
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(SharedString::from(label)),
                    )
                    .child(
                        Button::new("log_pane_toggle")
                            .icon(icon)
                            .ghost()
                            .small()
                            .tooltip(tooltip)
                            .accessibility_label(tooltip)
                            .on_click(|_, _, cx| {
                                cx.update_global::<LogPaneOpen, _>(
                                    |open, _| open.0 = !open.0,
                                );
                            }),
                    ),
            )
            .when(open, |pane| {
                pane.child(
                    div()
                        .id(ElementId::Name("log-scroll".into()))
                        .h_32()
                        .overflow_y_scroll()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .font_family(theme.mono_font_family.clone())
                        .children(buffer.entries().map(|entry| {
                            let color = match entry.level {
                                LogLevel::Info => theme.muted_foreground,
                                LogLevel::Warning => theme.warning,
                                LogLevel::Error => theme.danger,
                            };
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .w_12()
                                        .flex_shrink_0()
                                        .text_color(color)
                                        .child(entry.level.label()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .text_color(theme.foreground)
                                        .child(entry.message.clone()),
                                )
                        })),
                )
            })
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
        let active_tab = tabs.get(active_index).cloned();

        // Sync the NavStackState with the active tab. When the active tab
        // changes or the stack is empty, clear and push the new view,
        // move keyboard focus to it, and set the window title. This branch
        // runs once per activation, not on every frame, so focus is
        // requested on activation rather than as an unconditional side
        // effect of `render`.
        // NavMotion::Immediate switches views without animation, which is
        // appropriate for tab switching.
        let mut sync_title: Option<SharedString> = None;
        if (active_index != self.last_active_index
            || self.nav_stack_state.read(cx).is_empty())
            && let Some(tab) = active_tab.as_ref()
        {
            let view = tab.any_view();
            let focus = tab.focus_handle(cx);
            let title = tab.window_title(cx);
            self.nav_stack_state.update(cx, |state, cx| {
                state.clear(cx);
                state.push(view, NavMotion::Immediate, cx);
            });
            self.last_active_index = active_index;
            focus.focus(window, cx);
            sync_title = Some(title);
        }
        // The session-manager read borrow ends here, so the window title
        // can be set (it needs `&mut Window`).
        if let Some(title) = sync_title {
            window.set_window_title(&title);
        }

        // Keep the native menu bar in sync with the active tab: folder-only
        // commands, the copy-to-side enabled state, and the log pane
        // checkmark. Reading the folder state here also registers the
        // dependency, so selection changes re-render this view (and thus
        // re-evaluate the snapshot).
        let folder_state =
            active_tab.as_ref().and_then(|tab| tab.folder_state(cx));
        // Reading the text state registers the dependency, so word-wrap
        // and thumbnail toggles re-evaluate the menu snapshot.
        let text_state =
            active_tab.as_ref().and_then(|tab| tab.text_state(cx));
        // Reading the merge state registers the dependency, so take and
        // wrap toggles re-evaluate the menu snapshot.
        let merge_state =
            active_tab.as_ref().and_then(|tab| tab.merge_state(cx));
        let menu_state = folder_state
            .as_ref()
            .map(|state| {
                let state = state.read(cx);
                use cocomo_lib::TransferAction;
                MenuState {
                    is_folder: true,
                    is_text: false,
                    is_merge: false,
                    word_wrap: false,
                    show_thumbnails: false,
                    is_home: active_tab
                        .as_ref()
                        .is_some_and(TabEntity::is_home),
                    loading: state.is_loading(),
                    can_copy_left: state
                        .can_transfer(TransferAction::CopyLeft),
                    can_copy_right: state
                        .can_transfer(TransferAction::CopyRight),
                    can_move_left: state
                        .can_transfer(TransferAction::MoveLeft),
                    can_move_right: state
                        .can_transfer(TransferAction::MoveRight),
                    can_delete_left: state
                        .can_transfer(TransferAction::DeleteLeft),
                    can_delete_right: state
                        .can_transfer(TransferAction::DeleteRight),
                    can_go_back: state.can_go_back(),
                    can_go_forward: state.can_go_forward(),
                    log_open: cx.global::<LogPaneOpen>().0,
                    is_text_editing: false,
                    can_save_file: false,
                    detail_mode: Default::default(),
                    show_ancestor: false,
                    merge_can_take: false,
                    merge_has_ancestor: false,
                }
            })
            .or_else(|| {
                text_state.as_ref().map(|state| {
                    let state = state.read(cx);
                    MenuState {
                        is_text: true,
                        word_wrap: state.is_word_wrap(),
                        show_thumbnails: state.is_show_thumbnail(),
                        is_text_editing: state.is_editing(),
                        can_save_file: state.can_save(),
                        detail_mode: state.detail_mode(),
                        is_home: active_tab
                            .as_ref()
                            .is_some_and(TabEntity::is_home),
                        log_open: cx.global::<LogPaneOpen>().0,
                        ..MenuState::empty()
                    }
                })
            })
            .or_else(|| {
                merge_state.as_ref().map(|state| {
                    let state = state.read(cx);
                    MenuState {
                        is_merge: true,
                        word_wrap: state.is_word_wrap(),
                        show_ancestor: state.show_ancestor(),
                        merge_can_take: state.can_take(),
                        merge_has_ancestor: state.has_ancestor(),
                        is_home: active_tab
                            .as_ref()
                            .is_some_and(TabEntity::is_home),
                        log_open: cx.global::<LogPaneOpen>().0,
                        ..MenuState::empty()
                    }
                })
            })
            .unwrap_or_else(|| MenuState {
                is_home: active_tab.as_ref().is_some_and(TabEntity::is_home),
                log_open: cx.global::<LogPaneOpen>().0,
                ..MenuState::empty()
            });
        if self.last_menu_state != Some(menu_state) {
            menus::refresh_view_menus(menu_state, cx);
            self.last_menu_state = Some(menu_state);
        }

        let tab_bar_element =
            crate::chrome::tab_bar_element(self.session_manager.clone(), cx);
        let toolbar_element = crate::chrome::toolbar_element(
            folder_state,
            self.session_manager.clone(),
            cx,
        );
        // Row 2 belongs to the active view.
        let view_toolbar = active_tab
            .as_ref()
            .and_then(|tab| tab.view_toolbar(window, cx));

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .text_sm()
            .child(tab_bar_element)
            .child(toolbar_element)
            .when_some(view_toolbar, |root, row2| {
                root.child(row2.into_any_element())
            })
            .child(
                div().flex_1().min_h_0().overflow_hidden().child(
                    NavStack::new(&self.nav_stack_state).size_full().item(
                        |page, _, _| page.size_full().into_any_element(),
                    ),
                ),
            )
            .child(self.render_log_pane(cx))
    }
}
