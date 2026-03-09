// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Command Line Arguments Module (`cmdargs`)
//!
//! This module defines the command line arguments for the Cocomo TUI
//! application and uses `clap` for parsing.

use std::path::PathBuf;

use clap::Parser;

#[derive(Clone, Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    /// Left-side directory / file
    #[clap(short, long)]
    left: Option<PathBuf>,

    /// Right-side directory / file
    #[clap(short, long)]
    right: Option<PathBuf>,
}

/// Command line arguments for the application.
pub(crate) struct CmdLineArgs {
    /// Path to the left side directory or file.
    pub(crate) left: Option<PathBuf>,
    /// Path to the right side directory or file.
    pub(crate) right: Option<PathBuf>,
}

impl CmdLineArgs {
    /// Parses the command line arguments and returns a `CmdLineArgs` instance.
    pub(crate) fn get() -> Self {
        let args = Args::parse();
        Self {
            left: args.left,
            right: args.right,
        }
    }
}
