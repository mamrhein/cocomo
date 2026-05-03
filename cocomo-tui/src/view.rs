// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared behavior for interactive views.

use core::fmt::Debug;

use cocomo_core::DiffItem;
use ratatui::{crossterm::event::KeyCode, widgets::WidgetRef};

use crate::{
    event::{Event, NavEvent, OpEvent},
    keymap::{KeyHint, KeyMapItem, KeyMapper},
};

/// Common trait for all views
pub(crate) trait View:
    Debug + KeyHint + KeyMapper + WidgetRef
{
    /// Returns the title of the view.
    fn title(&self) -> String;

    /// Returns `true` if the view is a directory view.
    fn is_dir_view(&self) -> bool {
        // There will only be one directory view but several file views.
        false
    }

    /// Returns `true` if the view is a file view.
    fn is_file_view(&self) -> bool {
        // There will only be one directory view but several file views.
        true
    }

    /// Returns the current diff item, if any.
    fn current_diff_item(&self) -> Option<&DiffItem> {
        None
    }

    /// Handles a navigation event.
    fn handle_nav_event(
        &mut self,
        nav_event: crate::event::NavEvent,
    ) -> color_eyre::Result<()>;

    /// Handles an event triggering an operation.
    fn handle_op_event(
        &mut self,
        op_event: OpEvent,
    ) -> color_eyre::Result<()>;
}

/// Pre-built key map items for navigable views.
#[rustfmt::skip]
pub(crate) const NAV_KEYMAP_ITEMS: [KeyMapItem; 4] = [
    KeyMapItem::new(
        KeyCode::Up,
        None,
        "Up",
        true,
        Event::Nav(NavEvent::Prev),
    ),
    KeyMapItem::new(
        KeyCode::Down,
        None,
        "Down",
        true,
        Event::Nav(NavEvent::Next),
    ),
    KeyMapItem::new(
        KeyCode::Home,
        None,
        "Top",
        true,
        Event::Nav(NavEvent::First),
    ),
    KeyMapItem::new(
        KeyCode::End,
        None,
        "Bottom",
        true,
        Event::Nav(NavEvent::Last),
    ),
];

/// Trait for views that show a table of items and support cursor-style
/// navigation.
pub(crate) trait TableView: View {
    /// Makes the previous logical item the current item.
    fn prev(&mut self);

    /// Makes the next logical item the current item.
    fn next(&mut self);

    /// Makes the first logical item the current item.
    fn home(&mut self);

    /// Makes the last logical item the current item.
    fn end(&mut self);
}
