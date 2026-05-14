// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use crate::{
    event::{Event, NavEvent},
    keymap::{HasNavKeyMap, KeyMap},
    view::TableViewState,
};

pub(crate) trait KeyState {
    fn update_key_state(&mut self);
}

impl<T: HasNavKeyMap + TableViewState> KeyState for T {
    fn update_key_state(&mut self) {
        let selected = self.selected();
        let first = self.first();
        let last = self.last();
        let keymap = self.keymap_mut();
        match selected {
            Some(index) => {
                if index == first.unwrap() {
                    keymap.disable_key(&Event::Nav(NavEvent::First));
                    keymap.disable_key(&Event::Nav(NavEvent::Prev));
                } else {
                    keymap.enable_key(&Event::Nav(NavEvent::First));
                    keymap.enable_key(&Event::Nav(NavEvent::Prev));
                }
                if index == last.unwrap() {
                    keymap.disable_key(&Event::Nav(NavEvent::Last));
                    keymap.disable_key(&Event::Nav(NavEvent::Next));
                } else {
                    keymap.enable_key(&Event::Nav(NavEvent::Last));
                    keymap.enable_key(&Event::Nav(NavEvent::Next));
                }
            }
            None => {
                keymap.disable_key(&Event::Nav(NavEvent::First));
                keymap.disable_key(&Event::Nav(NavEvent::Last));
                keymap.disable_key(&Event::Nav(NavEvent::Prev));
                keymap.disable_key(&Event::Nav(NavEvent::Next));
            }
        }
    }
}
