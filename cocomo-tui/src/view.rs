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
pub(crate) trait TableView: View + TableViewState {
    /// Makes the previous logical item the current item.
    fn prev(&mut self) {
        if let Some(i) = self.selected() {
            self.select(i.saturating_sub(1));
        }
    }

    /// Makes the next logical item the current item.
    fn next(&mut self) {
        if let Some(i) = self.selected() {
            // If selected is Some, last is also Some, so unwrap is safe
            self.select(i.saturating_add(1).min(self.last().unwrap()));
        }
    }

    /// Makes the first logical item the current item.
    fn home(&mut self) {
        if let Some(i) = self.first() {
            self.select(i);
        }
    }

    /// Makes the last logical item the current item.
    fn end(&mut self) {
        if let Some(i) = self.last() {
            self.select(i);
        }
    }

    /// Handles a navigation event.
    fn handle_nav_event(
        &mut self,
        nav_event: crate::event::NavEvent,
    ) -> color_eyre::Result<()> {
        match nav_event {
            NavEvent::Prev => {
                self.prev();
            }
            NavEvent::Next => {
                self.next();
            }
            NavEvent::First => {
                self.home();
            }
            NavEvent::Last => {
                self.end();
            }
        }
        Ok(())
    }
}

/// Trait for managing the state of a table view, including the number of
/// items and the cursor position.
///
/// Implementors typically hold a [`TableState`] from `ratatui` to track the
/// currently selected row.
pub(crate) trait TableViewState {
    /// Returns the total number of items in the table.
    fn n_items(&self) -> usize;

    /// Returns the index of the first item, if the table is non-empty.
    fn first(&self) -> Option<usize> {
        (self.n_items() > 0).then_some(0)
    }

    /// Returns the index of the last item, if the table is non-empty.
    fn last(&self) -> Option<usize> {
        let n_items = self.n_items();
        (n_items > 0).then_some(n_items - 1)
    }

    /// Returns the index of the currently selected item, if any.
    fn selected(&self) -> Option<usize>;

    /// Selects the item at the given index.
    fn select(&mut self, index: usize);
}
