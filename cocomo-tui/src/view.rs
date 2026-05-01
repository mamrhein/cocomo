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
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    widgets::WidgetRef,
};

use crate::{
    event::{Event, NavEvent, OpEvent},
    keymap::{AggregatedKeyMap, KeyMapItem, KeyMapper},
};

/// Common trait for all views
pub(crate) trait View: Debug + WidgetRef {
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

    /// Returns the aggregated key map for this view.
    fn keymap<'a>(&'a self) -> AggregatedKeyMap<'a>;

    /// Handles a key event by mapping it to an app event and then handling
    /// that.
    fn handle_key_event(
        &mut self,
        key_event: KeyEvent,
    ) -> color_eyre::Result<()> {
        if let Some(event) = self.keymap().map_key_code(key_event.code) {
            return match event {
                Event::Nav(nav_event) => self.handle_nav_event(nav_event),
                Event::Op(op_event) => self.handle_op_event(op_event),
                _ => unreachable!(), // should not happen!
            };
        }
        Ok(())
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
