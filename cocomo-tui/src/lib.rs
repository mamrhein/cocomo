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
//! mutations and the bridge to `cocomo-lib`, [`error`] types the
//! service-layer failures, [`keys`] is the single source of truth for key
//! bindings, [`ui`] holds the ratatui-kit components, [`cli`] turns the
//! command-line arguments into the [`cli::Startup`] decision, and [`run`]
//! wires everything together. The binary in `main.rs` only calls [`run`].
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
pub(crate) mod error;
pub(crate) mod format;
pub(crate) mod session_store;
pub(crate) mod ui;

use anyhow::Result;
use clap::Parser;
use ratatui_kit::prelude::*;

use crate::{
    cli::{Cli, Startup},
    state::{APP_STATE, AppState},
    ui::app::App,
};

/// Parse the CLI arguments, seed the global [`APP_STATE`], and run the root
/// [`App`] component on the full screen.
///
/// All startup policy lives in [`cli::Cli::startup`]; this function only
/// translates the resulting [`Startup`] decision into the initial
/// [`AppState`] and enters the render loop.
pub async fn run() -> Result<()> {
    let cli = Cli::parse();

    let mut app = AppState::new();
    match cli.startup().await? {
        Startup::Welcome => {}
        Startup::Session(session) => app.add_session(*session),
        Startup::ComparisonFailed(err) => {
            app.error_message = Some(format!("Comparison failed: {err}"));
        }
    }
    APP_STATE.set(app);

    element!(App).fullscreen().await?;

    Ok(())
}
