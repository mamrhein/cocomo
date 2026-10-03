// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! COCOMO CLI binary entry point. All command logic lives in the
//! `cocomo_cli` library; this binary only parses arguments, runs the
//! command against the production endpoint resolver, and maps the outcome
//! to an exit code.

use std::process::ExitCode;

use clap::Parser;
use cocomo_cli::{Cli, DiffResult, ProductionResolver, run};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(&cli.command, &ProductionResolver).await {
        Ok(DiffResult::NoDiffs) => ExitCode::SUCCESS,
        Ok(DiffResult::HasDiffs) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}
