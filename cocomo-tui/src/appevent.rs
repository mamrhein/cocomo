// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

/// Application events.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum AppEvent {
    /// Quit the application.
    Quit,
    /// Navigate the current item of the current view to the previous item.
    NavigatePrev,
    /// Navigate the current item of the current view to the next item.
    NavigateNext,
    /// Navigate the current item of the current view to the first item.
    NavigateFirst,
    /// Navigate the current item of the current view to the last item.
    NavigateLast,
    /// Close the current tab.
    CloseTab,
    /// Open a new comparison view.
    OpenView,
    /// Copy the current item to the other side.
    Copy,
    /// Move the current item to the other side.
    Move,
    /// Delete the current item.
    Delete,
    /// Rename the current item.
    Rename,
    /// Refresh the current view.
    Refresh,
}
