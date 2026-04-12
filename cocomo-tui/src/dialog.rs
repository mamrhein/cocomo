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

pub(crate) trait Dialog: Debug + WidgetRef {
    fn keymap(&self) -> &KeyMap;
    async fn handle_key_event(
        &mut self,
        key_event: KeyEvent,
    ) -> color_eyre::Result<()> {
        if let Some(event) = self.keymap().map_key_code(&key_event.code) {
            send_event(event).await;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct SimpleConfirm {
    pub title: String,
    pub message: String,
}

impl SimpleConfirm {
    pub fn new(title: &str, message: &str) -> Self {
        Self {
            title: title.to_owned(),
            message: message.to_owned(),
        }
    }
}

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

static SIMPLE_CONFIRM_KEYMAP: sync::LazyLock<KeyMap> =
    sync::LazyLock::new(|| {
        KeyMap::from(SIMPLE_CONFIRM_KEYMAP_ITEMS.as_slice())
    });

impl Dialog for SimpleConfirm {
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
const fn centered_rect(width: u16, height: u16, rect: Rect) -> Rect {
    Rect {
        x: rect.x + (rect.width - width) / 2,
        y: rect.y + (rect.height - height) / 2,
        width,
        height,
    }
}
