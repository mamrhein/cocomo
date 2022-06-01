// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use clap::Parser;

#[derive(Parser, Debug)]
#[clap(version, about, long_about = None)]
struct Args {
    /// Left-side directory / file
    #[clap(short, long)]
    left: Option<String>,

    /// Right-side directory / file
    #[clap(short, long)]
    right: Option<String>,
}

pub(crate) struct CmdLineArgs {
    pub(crate) left: Option<String>,
    pub(crate) right: Option<String>,
}

impl CmdLineArgs {
    pub(crate) fn get() -> CmdLineArgs {
        let args = Args::parse();
        CmdLineArgs {
            left: args.left,
            right: args.right,
        }
    }
}
