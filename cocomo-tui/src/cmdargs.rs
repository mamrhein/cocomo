// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

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

pub(crate) struct CmdLineArgs {
    pub(crate) left: Option<PathBuf>,
    pub(crate) right: Option<PathBuf>,
}

impl CmdLineArgs {
    pub(crate) fn get() -> Self {
        let args = Args::parse();
        Self {
            left: args.left,
            right: args.right,
        }
    }
}
