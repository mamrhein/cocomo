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

mod menus;
mod runtime;
mod session_manager;
mod state;
mod tab_bar;
mod tabview;
mod text_diff;
mod toolbar;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_platform::application as create_application;

use crate::{
    menus::{menu_bindings, register_menu_handlers, set_app_menus},
    session_manager::create_default_manager,
    tabview::WindowRoot,
    ui::folder_compare_bindings,
};

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    // Parse CLI arguments for left and right paths.
    let args: Vec<String> = std::env::args().collect();
    let (left, right) = parse_args(&args)?;

    // Clone paths for the closure.
    let left_path = left.clone();
    let right_path = right.clone();

    // Start a tokio multi-thread runtime so that tokio::fs operations work.
    // gpui uses its own executor, but cocomo-lib's async fs operations rely
    // on tokio's reactor for non-blocking IO.
    //
    // We call `enter()` to install the runtime handle in thread-local
    // storage, and `set_handle()` to store it globally so background tasks
    // can use `Handle::block_on`. The runtime guard is kept alive for the
    // entire duration of the gpui event loop.
    let rt = tokio::runtime::Runtime::new()
        .expect("failed to create tokio runtime");
    let _rt_guard = rt.enter();
    runtime::set_handle();

    create_application().run(move |cx: &mut App| {
        let left = left_path;
        let right = right_path;
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

        // Add an initial folder-compare tab with the CLI-provided paths.
        session_manager.update(cx, |mgr, cx| {
            mgr.add_new_folder_tab(cx);
            // Override the default paths with CLI args.
            mgr.set_active_paths(left.clone(), right.clone());
            // Re-create the tab entity with correct paths.
            let title = gpui::SharedString::from("cocomo — compare");
            let config = mgr.active_config().unwrap().clone();
            let state = cx.new(|cx| {
                crate::state::AppState::from_config(&config, title, cx)
            });
            let mgr_clone = cx.entity().clone();
            let view = cx.new(|cx| {
                crate::ui::FolderCompareView::new(state, mgr_clone, cx)
            });
            mgr.replace_active_tab(crate::tabview::TabEntity::from_folder(
                view,
            ));
        });

        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| WindowRoot::new(session_manager.clone(), cx)),
        )
        .unwrap();

        cx.activate(true);
    });

    Ok(())
}

/// Parse command-line arguments for left and right directory paths.
fn parse_args(args: &[String]) -> Result<(PathBuf, PathBuf)> {
    let mut left: Option<PathBuf> = None;
    let mut right: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--left" | "-l" => {
                i += 1;
                if i < args.len() {
                    left = Some(PathBuf::from(&args[i]));
                }
            }
            "--right" | "-r" => {
                i += 1;
                if i < args.len() {
                    right = Some(PathBuf::from(&args[i]));
                }
            }
            "--" => {
                // Positional args after --.
                i += 1;
                if i < args.len() && left.is_none() {
                    left = Some(PathBuf::from(&args[i]));
                }
                i += 1;
                if i < args.len() && right.is_none() {
                    right = Some(PathBuf::from(&args[i]));
                }
            }
            _ => {
                // Positional args.
                if left.is_none() {
                    left = Some(PathBuf::from(&args[i]));
                } else if right.is_none() {
                    right = Some(PathBuf::from(&args[i]));
                }
            }
        }
        i += 1;
    }

    // Validate and canonicalize.
    let left = left.ok_or_else(|| anyhow::anyhow!("--left is required"))?;
    let right = right.ok_or_else(|| anyhow::anyhow!("--right is required"))?;

    if !left.is_dir() {
        return Err(anyhow::anyhow!("{} is not a directory", left.display()));
    }
    if !right.is_dir() {
        return Err(anyhow::anyhow!("{} is not a directory", right.display()));
    }

    Ok((left.canonicalize()?, right.canonicalize()?))
}
