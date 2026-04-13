// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$
// ---------------------------------------------------------------------------

use core::fmt::Debug;
use std::sync;

use futures::executor::block_on;
use ratatui::{
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent},
    layout::Rect,
    style::Stylize,
    text::{Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Padding, Widget, WidgetRef},
};

use crate::{
    app::send_event,
    appevent::AppEvent,
    keymap::{KeyMap, KeyMapItem},
};

/// A trait for dialog widgets that handle key events and have a keymap.
pub(crate) trait Dialog: Debug + WidgetRef {
    /// Returns a reference to the dialog's keymap.
    fn keymap(&self) -> &KeyMap;
    /// Handles a key event by mapping it to an `AppEvent` and sending that to
    /// the app.
    fn handle_key_event(&self, key_event: KeyEvent) -> color_eyre::Result<()> {
        if let Some(event) = self.keymap().map_key_code(&key_event.code) {
            block_on(send_event(event));
        }
        Ok(())
    }
}

/// A simple confirmation dialog widget that displays a title and message and
/// waits for the user to confirm or cancel.
#[derive(Debug)]
pub(crate) struct SimpleConfirm {
    pub title: String,
    pub message: String,
}

impl SimpleConfirm {
    /// Creates a new `SimpleConfirm` dialog with the given title and message.
    #[inline(always)]
    pub fn new(title: &str, message: &str) -> Self {
        Self {
            title: title.to_owned(),
            message: message.to_owned(),
        }
    }
}

/// Pre-built key map items for the `SimpleConfirm` dialog.
const SIMPLE_CONFIRM_KEYMAP_ITEMS: [KeyMapItem; 2] = [
    KeyMapItem::new(
        KeyCode::Enter,
        Some(KeyCode::Char('y')),
        "Yes",
        true,
        AppEvent::Confirmed,
    ),
    KeyMapItem::new(
        KeyCode::Esc,
        Some(KeyCode::Char('n')),
        "No",
        true,
        AppEvent::NotConfirmed,
    ),
];

/// Pre-built key map instance for the `SimpleConfirm` dialog.
static SIMPLE_CONFIRM_KEYMAP: sync::LazyLock<KeyMap> =
    sync::LazyLock::new(|| {
        KeyMap::from(SIMPLE_CONFIRM_KEYMAP_ITEMS.as_slice())
    });

impl Dialog for SimpleConfirm {
    #[inline(always)]
    fn keymap(&self) -> &KeyMap {
        &SIMPLE_CONFIRM_KEYMAP
    }
}

impl WidgetRef for SimpleConfirm {
    #[allow(clippy::cast_possible_truncation)]
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        // Build content
        let mut txt = Text::default();
        if !self.message.is_empty() {
            txt.push_span(Span::from(self.message.clone()).bold());
            txt.push_line("");
        };
        txt.push_line(format!("{}", &*SIMPLE_CONFIRM_KEYMAP));
        let padding = Padding {
            left: 2,
            right: 2,
            top: 1,
            bottom: 1,
        };
        // Needed width = text width + padding.left + padding.right + border
        let width = txt.width() as u16 + 6;
        // Needed height = text height + padding.top + padding.bottom + border
        let height = txt.height() as u16 + 4;
        let area = centered_rect(width, height, area);
        Clear.render(area, buf);
        let block = Block::default()
            .title(self.title.clone())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .padding(padding);
        let inner = block.inner(area);
        block.render(area, buf);
        txt.centered().render(inner, buf);
    }
}

/// Helper function to create a Rect sized `width`x`height` centered within
/// `rect`
#[allow(clippy::integer_division)]
#[inline(always)]
const fn centered_rect(width: u16, height: u16, rect: Rect) -> Rect {
    Rect {
        x: rect.x + (rect.width - width) / 2,
        y: rect.y + (rect.height - height) / 2,
        width,
        height,
    }
}
