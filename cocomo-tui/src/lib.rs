// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! cocomo_tui — Terminal UI for directory comparison.
//!
//! Provides a multi-tab folder comparison view with navigation, status
//! indicators, filtering, and session-based directory exploration. Each tab
//! is an independent comparison session; pressing Enter on a directory opens
//! a new tab comparing the subdirectories.
//!
//! The crate is split by layer: [`state`] owns the state types and the global
//! `APP_STATE` atom, [`controller`] is the single owner of all state
//! mutations and the bridge to `cocomo-lib`, [`keys`] is the single source of
//! truth for key bindings, [`ui`] holds the ratatui-kit components, and
//! [`run`] wires everything together. The binary in `main.rs` only calls
//! [`run`].
//!
//! The pure-logic modules ([`state`], [`keys`], [`diff_model`]) are public so
//! the integration tests under `tests/` can drive the state machine directly,
//! without a terminal or the async runtime. Everything else stays
//! `pub(crate)`: it is wiring (CLI, UI, persistence) or mutates the global
//! atom and must not be reachable from outside the crate.

pub mod diff_model;
pub mod keys;
pub mod state;

pub(crate) mod cli;
pub(crate) mod controller;
pub(crate) mod format;
pub(crate) mod session_store;
pub(crate) mod ui;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use ratatui_kit::prelude::*;

use crate::{
    cli::Cli,
    controller::compare,
    state::{APP_STATE, AppState, SessionView},
    ui::app::App,
};

/// Parse the CLI arguments, seed the global [`APP_STATE`], and run the root
/// [`App`] component on the full screen.
pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    let has_paths = cli.left.is_some() && cli.right.is_some();
    let (left, right) = if has_paths {
        let l = cli.left.unwrap();
        let r = cli.right.unwrap();
        let left = tokio::fs::canonicalize(&l).await?;
        let right = tokio::fs::canonicalize(&r).await?;

        // Validate that both paths exist and are of the same type.
        let left_meta = tokio::fs::metadata(&left).await?;
        let right_meta = tokio::fs::metadata(&right).await?;

        if left_meta.is_file() && right_meta.is_file() {
            return Err(anyhow::anyhow!(
                "Comparing individual files is not yet implemented. Please \
                 provide directories."
            ));
        }

        if !left_meta.is_dir() || !right_meta.is_dir() {
            return Err(anyhow::anyhow!(
                "Both --left and --right must be directories, or both must \
                 be files."
            ));
        }

        (left, right)
    } else {
        (PathBuf::from("."), PathBuf::from("."))
    };

    let mut app = AppState::new();

    // Create the initial comparison session if paths were provided.
    if has_paths {
        let comparison = match compare(&left, &right, cli.content).await {
            Ok(comparison) => Some(comparison),
            Err(e) => {
                app.error_message = Some(format!("Comparison failed: {e}"));
                None
            }
        };
        let mut session =
            SessionView::new_dir_compare(left, right, cli.content);
        session.set_comparison(comparison);
        app.add_session(session);
    }

    APP_STATE.set(app);

    element!(App).fullscreen().await?;

    Ok(())
}
