// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Command-line argument definitions.

use std::path::PathBuf;

use clap::Parser;

/// cocomo — COmpare, COpy & MOve directories and files.
#[derive(Parser, Debug)]
#[command(name = "cocomo-tui", version, about)]
pub(crate) struct Cli {
    /// Left directory path.
    #[arg(short, long)]
    pub left: Option<PathBuf>,

    /// Right directory path.
    #[arg(short, long)]
    pub right: Option<PathBuf>,

    /// Compare file content (using hashing).
    #[arg(short = 'c', long, default_value_t = false)]
    pub content: bool,
}
