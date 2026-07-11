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
mod session_manager;
mod state;
mod tab_bar;
mod toolbar;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use gpui::{
    App, AppContext, Bounds, SharedString, WindowBounds, WindowOptions, px,
    size,
};
use gpui_platform::application as create_application;

use crate::{
    menus::{menu_bindings, register_menu_handlers, set_app_menus},
    session_manager::create_default_manager,
    state::AppState,
    ui::{FolderCompareView, folder_compare_bindings},
};

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    // Parse CLI arguments for left and right paths.
    let args: Vec<String> = std::env::args().collect();
    let (left, right) = parse_args(&args)?;

    create_application().run(|cx: &mut App| {
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

        // Add an initial session with the CLI-provided paths.
        session_manager.update(cx, |mgr, cx| {
            mgr.add_new_session(cx);
        });

        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                // Create the app state.
                let app_state = cx.new(|cx| {
                    AppState::new(
                        left,
                        right,
                        SharedString::from("cocomo — compare"),
                        cx,
                    )
                });

                // Create the folder compare view as the window root.
                cx.new(|cx| {
                    FolderCompareView::new(
                        app_state,
                        session_manager.clone(),
                        cx,
                    )
                })
            },
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
