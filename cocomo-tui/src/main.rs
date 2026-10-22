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
//! This module is the thin entry point only: it parses the CLI, seeds the
//! global [`APP_STATE`], and runs the root [`App`] component. The actual UI
//! lives in [`ui`], async work is dispatched by [`controller`], and state
//! types are defined in [`state`].

mod cli;
mod controller;
mod diff_model;
mod format;
mod keys;
mod session_store;
mod state;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use ratatui_kit::prelude::*;

use crate::{
    cli::Cli,
    controller::run_comparison,
    state::{APP_STATE, AppState, SessionView},
    ui::app::App,
};

#[tokio::main]
async fn main() -> Result<()> {
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
        let mut session =
            SessionView::new_dir_compare(left, right, cli.content);
        if let Err(e) = run_comparison(&mut session).await {
            app.error_message = Some(format!("Comparison failed: {e}"));
        }
        app.add_session(session);
    }

    APP_STATE.set(app);

    element!(App).fullscreen().await?;

    Ok(())
}
