// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! COCOMO CLI — command-line interface for directory and file comparison,
//! synchronization, and snapshot management.
//!
//! The command logic is generic over an [`EndpointResolver`], which maps a
//! CLI endpoint argument (path or URL) to a filesystem instance. The binary
//! uses [`ProductionResolver`] to resolve endpoints to live [`Provider`]
//! instances; tests inject a mock resolver backed by `cocomo_lib::MockFs`
//! to exercise error handling without touching the real filesystem.
//!
//! # Exit codes
//!
//! - `0` — success, no differences found
//! - `1` — differences found (comparison commands only)
//! - `2` — error occurred

mod cli;
mod dir;
mod error;
mod resolver;
mod snapshot;
mod text;

pub use cli::Cli;
use cli::Commands;
use dir::run_dir;
pub use error::{CliError, DiffResult};
pub use resolver::{EndpointResolver, ProductionResolver};
use snapshot::run_snapshot;
use text::run_text;

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

/// Run the parsed `command` against the filesystems provided by `resolver`.
pub async fn run<R: EndpointResolver>(
    command: &Commands,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    match command {
        Commands::Dir(dir_args) => run_dir(&dir_args.command, resolver).await,
        Commands::Text(text_args) => {
            run_text(&text_args.command, resolver).await
        }
        Commands::Snapshot(snap_args) => {
            run_snapshot(&snap_args.command, resolver).await
        }
    }
}
