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
    App, AppContext, Bounds, WindowBounds, WindowOptions,
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
        // Center the window on screen.
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

        // Seed the initial tab from the startup decision: without CLI
        // paths an empty untitled tab (the GUI equivalent of the TUI's
        // welcome screen); with paths a folder-compare tab whose
        // comparison loads asynchronously (failures surface in the UI).
        session_manager.update(cx, |mgr, cx| match startup {
            Startup::Welcome => mgr.add_new_folder_tab(cx),
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
            |_, cx| cx.new(|cx| WindowRoot::new(session_manager.clone(), cx)),
        )
        .unwrap();

        cx.activate(true);
    });

    Ok(())
}
