// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared behavior for interactive views.

/// Trait for views that support cursor-style navigation.
pub(crate) trait NavigableView {
    /// Moves the selection up by one logical item.
    fn move_up(&mut self);

    /// Moves the selection down by one logical item.
    fn move_down(&mut self);

    /// Moves the selection to the first logical item.
    fn move_home(&mut self);

    /// Moves the selection to the last logical item.
    fn move_end(&mut self);
}
