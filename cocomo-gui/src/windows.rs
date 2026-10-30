// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Multi-window support and workspace orchestration.
//!
//! Every cocomo window owns a [`GuiSessionManager`] holding its tabs. The
//! [`WindowRegistry`] global maps the live windows to their managers, which
//! is what lets a menu command act on the *active* window instead of a
//! manager captured at startup, and what makes "move tab to new window" a
//! transfer between two managers.
//!
//! The same registry produces the [`Workspace`] snapshot that
//! [`crate::workspace`] persists, and consumes it at launch.

use gpui::{
    AnyWindowHandle, App, AppContext as _, BorrowAppContext as _, Bounds,
    Entity, Global, WindowId, WindowOptions, px, size,
};

use crate::{
    log::{self, LogLevel},
    options::AppOptions,
    session_manager::{GuiSessionManager, create_window_manager},
    tabview::WindowRoot,
    workspace::{self, WindowWorkspace, Workspace},
};

/// One live window and the manager that owns its tabs.
pub struct WindowEntry {
    /// The window handle, used to close or refresh it.
    pub handle: AnyWindowHandle,
    /// The manager owning the window's tabs.
    pub manager: Entity<GuiSessionManager>,
}

/// The live windows of this application run.
#[derive(Default)]
pub struct WindowRegistry {
    entries: Vec<WindowEntry>,
}

impl Global for WindowRegistry {}

impl WindowRegistry {
    /// Install the empty registry global.
    pub fn init_global(cx: &mut App) {
        cx.set_global(Self::default());
    }

    /// Record a newly opened window.
    pub fn register(
        &mut self,
        handle: AnyWindowHandle,
        manager: Entity<GuiSessionManager>,
    ) {
        self.entries.push(WindowEntry { handle, manager });
    }

    /// Forget a closed window, returning its manager.
    pub fn unregister(
        &mut self,
        id: WindowId,
    ) -> Option<Entity<GuiSessionManager>> {
        let index = self
            .entries
            .iter()
            .position(|e| e.handle.window_id() == id)?;
        Some(self.entries.remove(index).manager)
    }

    /// The managers of all live windows, in creation order.
    pub fn managers(
        &self,
    ) -> impl Iterator<Item = &Entity<GuiSessionManager>> {
        self.entries.iter().map(|e| &e.manager)
    }

    /// The handles of all live windows.
    fn handles(&self) -> Vec<AnyWindowHandle> {
        self.entries.iter().map(|e| e.handle).collect()
    }

    /// How many windows are live.
    pub fn window_count(&self) -> usize {
        self.entries.len()
    }

    /// The manager of the window that currently has focus, falling back to
    /// the first live window when no window is active (a menu dispatch
    /// from a detached panel, for instance).
    pub fn active_manager(cx: &App) -> Option<Entity<GuiSessionManager>> {
        let registry = cx.try_global::<Self>()?;
        if let Some(active) = cx.active_window()
            && let Some(entry) = registry
                .entries
                .iter()
                .find(|e| e.handle.window_id() == active.window_id())
        {
            return Some(entry.manager.clone());
        }
        registry.entries.first().map(|e| e.manager.clone())
    }

    /// Whether any window holds a session with unsaved changes.
    pub fn has_dirty(cx: &App) -> bool {
        cx.try_global::<Self>().is_some_and(|registry| {
            registry
                .managers()
                .any(|manager| manager.read(cx).has_dirty())
        })
    }

    /// The configs of every dirty session, for crash-recovery snapshots.
    fn dirty_configs(cx: &App) -> Vec<cocomo_lib::SessionConfig> {
        let Some(registry) = cx.try_global::<Self>() else {
            return Vec::new();
        };
        registry
            .entries
            .iter()
            .flat_map(|entry| entry.manager.read(cx).dirty_configs())
            .collect()
    }
}

/// Open a new, empty cocomo window and return its manager.
pub fn open_new_window(cx: &mut App) -> Option<Entity<GuiSessionManager>> {
    let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
    let manager = create_window_manager(cx);
    let handle = gpui::open_window(
        WindowOptions {
            focus: true,
            window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        cx,
        |window, cx| cx.new(|cx| WindowRoot::new(manager.clone(), window, cx)),
    )
    .ok()?
    .0;
    cx.update_global::<WindowRegistry, _>(|registry, _| {
        registry.register(handle, manager.clone());
    });
    Some(manager)
}

/// Register a window that was opened outside [`open_new_window`].
pub fn register_window(
    handle: AnyWindowHandle,
    manager: Entity<GuiSessionManager>,
    cx: &mut App,
) {
    cx.update_global::<WindowRegistry, _>(|registry, _| {
        registry.register(handle, manager);
    });
}

/// The manager of the focused window, or the first live one.
///
/// Menu handlers resolve this per dispatch so every window's menus act on
/// that window's tabs.
pub fn active_manager(cx: &App) -> Option<Entity<GuiSessionManager>> {
    WindowRegistry::active_manager(cx)
}

/// Move the tab at `index` out of `manager` and into a new window.
///
/// The source window is left with a Home tab when its last tab moves out,
/// so no window is ever an empty shell. Dragging a tab between windows is
/// not offered because GPUI delivers an internal drag's mouse-up to the
/// window that started it; the context menu is the reliable path.
pub fn move_tab_to_new_window(
    manager: &Entity<GuiSessionManager>,
    index: usize,
    cx: &mut App,
) {
    let Some(session) = manager.update(cx, |m, _| m.take_session(index))
    else {
        return;
    };
    if manager.update(cx, |m, _| m.is_empty()) {
        manager.update(cx, |m, cx| m.open_home(cx));
    }
    let Some(target) = open_new_window(cx) else {
        // Opening failed: put the tab back rather than lose it.
        manager.update(cx, |m, cx| m.insert_session(session, Some(index), cx));
        return;
    };
    target.update(cx, |m, cx| m.insert_session(session, None, cx));
    log::push(LogLevel::Info, "Moved tab to a new window".to_string(), cx);
}

/// Re-render every live window, so a global change (theme, options) shows
/// up without waiting for the next input event.
pub fn refresh_all_windows(cx: &mut App) {
    let Some(registry) = cx.try_global::<WindowRegistry>() else {
        return;
    };
    let handles = registry.handles();
    for handle in handles {
        let _ = handle.update(cx, |_, window, _| window.refresh());
    }
}

/// Close every live window.
pub fn close_all_windows(cx: &mut App) {
    let Some(registry) = cx.try_global::<WindowRegistry>() else {
        return;
    };
    let handles = registry.handles();
    for handle in handles {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// Collect the current windows and tabs into a workspace snapshot.
pub fn collect_workspace(cx: &App) -> Workspace {
    let mut workspace = Workspace::new();
    let Some(registry) = cx.try_global::<WindowRegistry>() else {
        return workspace;
    };
    for entry in &registry.entries {
        let snapshot = entry.manager.read(cx).workspace_snapshot();
        if snapshot.tabs.is_empty() {
            continue;
        }
        workspace.windows.push(snapshot);
    }
    workspace
}

/// Persist the current workspace, logging rather than surfacing a failure:
/// auto-save runs unobtrusively and must not interrupt the user.
pub fn save_workspace_now(cx: &mut App) {
    let snapshot = collect_workspace(cx);
    if let Err(e) = workspace::save_workspace(&snapshot) {
        log::push(
            LogLevel::Warning,
            format!("Couldn't save the workspace. {e}"),
            cx,
        );
    }
}

/// Write the workspace file and, when unsaved work is open, the session
/// snapshots that back the crash-recovery prompt.
pub fn auto_save(cx: &mut App) {
    let opts = cx.global::<AppOptions>();
    let (restore, snapshot_sessions) =
        (opts.restore_workspace_on_launch, opts.auto_save_sessions);

    if restore {
        save_workspace_now(cx);
    }
    if snapshot_sessions && WindowRegistry::has_dirty(cx) {
        let configs = WindowRegistry::dirty_configs(cx);
        if let Err(e) = workspace::snapshot_sessions(&configs) {
            log::push(
                LogLevel::Warning,
                format!("Couldn't snapshot the sessions. {e}"),
                cx,
            );
        }
    }
}

/// Open one window per workspace entry, restoring its tabs.
///
/// Returns how many tabs opened; a workspace whose endpoints all fail to
/// resolve restores nothing, and the caller falls back to the Home tab.
pub fn restore_workspace(workspace: &Workspace, cx: &mut App) -> usize {
    let mut restored = 0;
    for snapshot in &workspace.windows {
        if snapshot.tabs.is_empty() {
            continue;
        }
        let Some(manager) = open_new_window(cx) else {
            continue;
        };
        restored +=
            manager.update(cx, |m, cx| m.restore_workspace_tabs(snapshot, cx));
    }
    if restored > 0 {
        log::push(
            LogLevel::Info,
            format!("Restored workspace: {restored} sessions"),
            cx,
        );
    }
    restored
}

/// Restore the persisted workspace, if one exists.
pub fn restore_saved_workspace(cx: &mut App) -> usize {
    match workspace::load_workspace() {
        Some(snapshot) => restore_workspace(&snapshot, cx),
        None => 0,
    }
}

/// Restore the auto-saved session snapshots after a crash.
///
/// The snapshots are plain session configs, so they open in a single
/// window; that is enough to get the user's work back in front of them.
pub fn restore_crash_snapshot(cx: &mut App) -> usize {
    let configs = workspace::load_snapshot_sessions();
    if configs.is_empty() {
        return 0;
    }
    let snapshot = WindowWorkspace::new(configs, 0);
    let Some(manager) = open_new_window(cx) else {
        return 0;
    };
    let restored =
        manager.update(cx, |m, cx| m.restore_workspace_tabs(&snapshot, cx));
    if restored == 0 {
        return 0;
    }
    log::push(
        LogLevel::Info,
        format!("Recovered {restored} sessions from the last run"),
        cx,
    );
    restored
}

/// Bookkeeping for a window that just closed.
///
/// Auto-save runs here as well as on the timer and on Quit, because
/// [`gpui::QuitMode::LastWindowClosed`] ends the process without running
/// the Quit action.
pub fn handle_window_closed(id: WindowId, cx: &mut App) {
    if cx.try_global::<WindowRegistry>().is_some() {
        cx.update_global::<WindowRegistry, _>(|registry, _| {
            registry.unregister(id);
        });
    }
    auto_save(cx);
    let empty = cx
        .try_global::<WindowRegistry>()
        .is_none_or(|registry| registry.window_count() == 0);
    if empty {
        // No window left means the run is over, crashed or not.
        workspace::clear_running_marker();
    }
}

/// Quit cleanly: persist, drop the running marker, then close everything.
///
/// With [`gpui::QuitMode::LastWindowClosed`] the last window closing ends
/// the process, so this runs before the windows go away.
pub fn quit_application(cx: &mut App) {
    auto_save(cx);
    workspace::clear_running_marker();
    close_all_windows(cx);
    cx.quit();
}
