// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! cocomo-tui — thin binary entry point.
//!
//! All logic lives in the `cocomo_tui` library crate (see `src/lib.rs`); this
//! binary only starts the async runtime and runs [`cocomo_tui::run`].

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    cocomo_tui::run().await
}
