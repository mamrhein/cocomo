// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$
// ---------------------------------------------------------------------------

use std::sync;

use ratatui::{
    buffer::Buffer,
    crossterm::event::KeyCode,
    layout::{Constraint, Layout, Rect},
    widgets::{
        Block, BorderType, Borders, Clear, Padding, Paragraph, Widget,
        WidgetRef,
    },
};

use crate::{
    appevent::AppEvent,
    keymap::{KeyMap, KeyMapItem},
};

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

impl WidgetRef for SimpleConfirm {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        let area = centered_rect(40, 10, area);
        Clear.render(area, buf);
        let block = Block::default()
            .title(self.title.clone())
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .padding(Padding {
                left: 2,
                right: 2,
                top: 1,
                bottom: 1,
            });
        let inner = block.inner(area);
        block.render(area, buf);
        // Create layout
        let vert_constraints = [
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ];
        let [msg_area, _, key_bar] =
            Layout::vertical(vert_constraints).areas(inner);
        Paragraph::new(self.message.clone())
            .centered()
            .render(msg_area, buf);
        Paragraph::new(format!("{}", &*SIMPLE_CONFIRM_KEYMAP))
            .centered()
            .render(key_bar, buf);
    }
}

/// helper function to create a centered rect using up certain % of the
/// available rect `r`
#[allow(clippy::integer_division)]
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(r);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
