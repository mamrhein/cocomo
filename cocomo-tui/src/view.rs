// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared behavior for interactive views.

use ratatui::widgets::WidgetRef;

/// Common trait for all views
pub(crate) trait View: WidgetRef {
    /// Returns the title of the view.
    fn title(&self) -> String;
}

/// Trait for views that support cursor-style navigation.
pub(crate) trait NavigableView: View {
    /// Makes the previous logical item the current item.
    fn prev(&mut self);

    /// Makes the next logical item the current item.
    fn next(&mut self);

    /// Makes the first logical item the current item.
    fn home(&mut self);

    /// Makes the last logical item the current item.
    fn end(&mut self);
}
