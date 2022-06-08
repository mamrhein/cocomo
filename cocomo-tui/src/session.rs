// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::rc::Rc;

use cocomo_core::{FSItem, ItemType};

pub(crate) struct Session {
    pub(crate) name: String,
    pub(crate) left: Rc<FSItem>,
    pub(crate) right: Rc<FSItem>,
}

impl Session {
    pub(crate) fn new(
        name: Option<String>,
        left: Rc<FSItem>,
        right: Rc<FSItem>,
    ) -> Self {
        assert_eq!(left.item_type, right.item_type);
        Self {
            name: name.unwrap_or("".to_string()),
            left,
            right,
        }
    }

    pub(crate) fn session_type(&self) -> ItemType {
        self.left.item_type
    }
}
