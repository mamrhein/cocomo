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
mod folder_panes;
mod fsops;
mod home;
mod log;
mod menus;
mod runtime;
mod session_manager;
mod state;
mod tabview;
mod text_diff;
mod ui;

use anyhow::Result;
use clap::Parser;
use gpui::{
    App, AppContext, Bounds, QuitMode, WindowBounds, WindowOptions,
    application as create_application, px, size,
};

use crate::{
    cli::{Cli, Startup},
    menus::{menu_bindings, register_menu_handlers, set_app_menus},
    session_manager::create_default_manager,
    tabview::WindowRoot,
    ui::folder_compare_bindings,
};

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

        // Quit the application when the last window is closed. Without
        // this, the default quit mode on macOS is `Explicit`, keeping the
        // process alive (with no window) after the user closes the window.
        cx.set_quit_mode(QuitMode::LastWindowClosed);

        // The log pane starts open; the menu bar reads both globals, so
        // they must exist before `set_app_menus`.
        cx.set_global(log::LogBuffer::default());
        cx.set_global(log::LogPaneOpen(true));

        // Center the window on screen. The initial window size is a
        // physical platform default (not layout geometry), so it is a
        // deliberate `px` exception; all in-window layout uses rem helpers.
        let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);

        // Create the session manager.
        let session_manager = create_default_manager(cx);

        // Set up the application menu bar.
        set_app_menus(cx);

        // Register global menu action handlers.
        register_menu_handlers(session_manager.clone(), cx);

        // Register global menu key bindings.
        cx.bind_keys(menu_bindings());

        // Register folder compare key bindings.
        cx.bind_keys(folder_compare_bindings());

        // Register Home view key bindings.
        cx.bind_keys(home::home_bindings());

        // Seed the initial tab from the startup decision: without CLI
        // paths the Home tab (the start screen for new and saved
        // sessions); with paths a folder-compare tab whose comparison
        // loads asynchronously (failures surface in the UI).
        session_manager.update(cx, |mgr, cx| match startup {
            Startup::Welcome => mgr.open_home(cx),
            Startup::Session {
                left,
                right,
                content,
            } => {
                mgr.add_folder_tab(left, right, content, cx);
            }
        });

        gpui::open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    WindowRoot::new(session_manager.clone(), window, cx)
                })
            },
        )
        .unwrap();

        cx.activate(true);
    });

    Ok(())
}
