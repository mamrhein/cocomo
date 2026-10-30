// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! cocomo_gui — Desktop GUI for directory comparison.
//!
//! Provides a folder comparison view with navigation, status
//! indicators, and directory tree browsing. Built on gpui for GPU-accelerated
//! rendering.

mod chrome;
mod cli;
mod color;
mod commands;
mod file_formats;
mod folder_panes;
mod fsops;
mod home;
mod log;
mod menus;
mod options;
mod runtime;
mod session_manager;
mod state;
mod tabview;
mod text_diff;
mod text_merge;
mod ui;
mod windows;
mod workspace;

use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use gpui::{
    App, AppContext as _, AsyncApp, Bounds, Entity, QuitMode, WindowBounds,
    WindowId, WindowOptions, application as create_application,
    component::WindowExt as _, px, size,
};

use crate::{
    cli::{Cli, Startup},
    menus::{register_menu_handlers, set_app_menus},
    options::AppOptions,
    session_manager::{GuiSessionManager, create_window_manager},
    tabview::WindowRoot,
};

/// How often the auto-save timer checks for work while the app is running.
const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    // Start a tokio multi-thread runtime so that tokio::fs operations work.
    // gpui uses its own executor, but cocomo-lib's async fs operations rely
    // on tokio's reactor for non-blocking IO.
    //
    // We call `enter()` to install the runtime handle in thread-local
    // storage, and `set_handle()` to store it globally so background tasks
    // can use `Handle::block_on`. The runtime guard is kept alive for the
    // entire duration of the gpui event loop.
    //
    // The runtime must exist before the startup decision is resolved:
    // remote endpoint arguments connect through tokio-based providers and
    // validate via async metadata.
    let rt = tokio::runtime::Runtime::new()
        .expect("failed to create tokio runtime");
    let _rt_guard = rt.enter();
    runtime::set_handle();

    // Parse the CLI arguments and resolve them into the startup decision.
    // Argument-level problems (unresolvable endpoints, wrong types) exit
    // with a message before any window is opened.
    let cli = Cli::parse();
    let startup = rt.block_on(cli.startup())?;

    create_application().run(move |cx: &mut App| {
        gpui::init(cx);

        // Snapshot the component layer's bindings before this application
        // adds any, so a keymap rebuild can restore them.
        commands::capture_framework_bindings(cx);

        // Quit the application when the last window is closed. Without
        // this, the default quit mode on macOS is `Explicit`, keeping the
        // process alive (with no window) after the user closes the window.
        cx.set_quit_mode(QuitMode::LastWindowClosed);

        // The log pane starts open; the menu bar reads both globals, so
        // they must exist before `set_app_menus`.
        cx.set_global(log::LogBuffer::default());
        cx.set_global(log::LogPaneOpen(true));
        file_formats::FileFormatRegistry::init_global(cx);
        options::init_global(cx);
        windows::WindowRegistry::init_global(cx);

        // Record that a run is live and learn whether the last one ended
        // cleanly. The check happens inside `mark_running`, before the
        // marker is written, so a fresh launch never mistakes its own
        // marker for a crash.
        let crashed = workspace::mark_running();

        // Register global menu action handlers, then build the keymap from
        // the registry so the user's binding overrides apply from the
        // first frame.
        register_menu_handlers(cx);
        commands::rebuild_keymap(cx);

        // Set up the application menu bar.
        set_app_menus(cx);

        // Track window lifetimes: a closed window leaves the registry,
        // triggers an auto-save, and ends the run's marker when it was the
        // last window.
        cx.on_window_closed(|cx, id| on_window_closed(id, cx))
            .detach();

        // Periodic auto-save of the workspace and of dirty sessions.
        cx.spawn(|cx: &mut AsyncApp| {
            // Clone before the async block: an `async move` over the
            // borrowed `&mut AsyncApp` would capture its lifetime.
            let cx = cx.clone();
            async move {
                let executor = cx.background_executor().clone();
                loop {
                    executor.timer(AUTO_SAVE_INTERVAL).await;
                    cx.update(windows::auto_save);
                }
            }
        })
        .detach();

        open_initial_windows(startup, crashed, cx);
        cx.activate(true);
    });

    Ok(())
}

/// Bookkeeping for a closed window.
fn on_window_closed(id: WindowId, cx: &mut App) {
    windows::handle_window_closed(id, cx);
}

/// Open the windows this run starts with.
///
/// `crashed` reports whether the previous run left its running marker
/// behind. Explicit CLI paths always win. Otherwise the saved workspace is
/// restored when the user asked for it, and a run that did not quit
/// cleanly is offered back the sessions it had open when it died.
fn open_initial_windows(startup: Startup, crashed: bool, cx: &mut App) {
    match startup {
        Startup::Session {
            left,
            right,
            content,
        } => {
            let manager = create_window_manager(cx);
            open_window(manager.clone(), cx);
            manager.update(cx, |m, cx| {
                m.add_folder_tab(left, right, content, cx);
            });
        }
        Startup::Welcome => {
            let restore =
                cx.global::<AppOptions>().restore_workspace_on_launch;
            let mut restored = 0;
            if !crashed && restore {
                restored = windows::restore_saved_workspace(cx);
            }
            if restored == 0 {
                let manager = create_window_manager(cx);
                manager.update(cx, |m, cx| m.open_home(cx));
                open_window(manager, cx);
            }
            if crashed {
                offer_crash_recovery(cx);
            }
        }
    }
}

/// Open a cocomo window rooted at `manager` and register it.
fn open_window(manager: Entity<GuiSessionManager>, cx: &mut App) {
    // Center the window on screen. The initial window size is a physical
    // platform default (not layout geometry), so it is a deliberate `px`
    // exception; all in-window layout uses rem helpers.
    let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
    let handle = gpui::open_window(
        WindowOptions {
            focus: true,
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        },
        cx,
        |window, cx| cx.new(|cx| WindowRoot::new(manager.clone(), window, cx)),
    )
    .expect("failed to open window")
    .0;
    windows::register_window(handle, manager, cx);
}

/// Ask whether to bring back the sessions snapshotted during the run that
/// did not quit cleanly.
///
/// An alert dialog because the alternative — silently reopening or silently
/// dropping the user's open tabs — both guess at intent.
fn offer_crash_recovery(cx: &mut App) {
    let Some(window) = cx.active_window() else {
        return;
    };
    let _ = window.update(cx, |_, window, cx| {
        window.open_alert_dialog(cx, |alert, _, _| {
            alert
                .confirm()
                .title("Recover the last session?")
                .description(
                    "cocomo did not quit cleanly. The sessions that were \
                     open can be reopened now.",
                )
                .ok_text("Reopen them")
                .on_ok(|_, _, cx| {
                    windows::restore_crash_snapshot(cx);
                    true
                })
        });
    });
}
