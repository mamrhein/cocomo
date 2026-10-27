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
//! bindings, [`menu`] is the single source of truth for the nested menu
//! tree, [`ui`] holds the ratatui-kit components, [`cli`] turns the
//! command-line arguments into the [`cli::Startup`] decision, and [`run`]
//! wires everything together. The binary in `main.rs` only calls [`run`].
//!
//! The pure-logic modules ([`state`], [`keys`], [`menu`], [`diff_model`]) are
//! public so
//! the integration tests under `tests/` can drive the state machine directly,
//! without a terminal or the async runtime. Everything else stays
//! `pub(crate)`: it is wiring (CLI, UI, persistence) or mutates the global
//! atom and must not be reachable from outside the crate.

pub mod diff_model;
pub mod keys;
pub mod menu;
pub mod state;

pub(crate) mod cli;
pub(crate) mod controller;
pub(crate) mod error;
pub(crate) mod session_store;
pub(crate) mod ui;

use anyhow::Result;
use clap::Parser;
use ratatui_kit::{
    crossterm::{
        event::{DisableBracketedPaste, EnableBracketedPaste},
        execute,
    },
    prelude::*,
};

use crate::{
    cli::{Cli, Startup},
    state::{APP_STATE, AppState},
    ui::{app::App, set_mouse_enabled, theme::cocomo_palette},
};

/// Disables bracketed paste mode again when it goes out of scope.
///
/// Disabling is best-effort: the mode was only ever enabled by [`run`],
/// and terminals that never entered it ignore the reset sequence.
struct BracketedPasteGuard;

impl Drop for BracketedPasteGuard {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    }
}

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

    // Enable mouse capture (click/drag selection, menu activation) and
    // bracketed paste (pastes arrive as a single `Event::Paste` instead of
    // a burst of key presses that would trigger commands). Bracketed paste
    // is best-effort: terminals without support keep delivering pastes as
    // keys, which the paste handler simply does not see.
    set_mouse_enabled();
    let _paste_guard = BracketedPasteGuard;
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);

    // Inject the app palette at the root so every component (framework
    // built-ins and `CocomoTheme` readers alike) resolves its styles from
    // the central scheme.
    element!(PaletteProvider(palette: cocomo_palette()) {
        App()
    })
    .fullscreen_with(RunOptions { mouse: true })
    .await?;

    Ok(())
}
