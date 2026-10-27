// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared bottom scaffolding: the one-line `StatusBar` and the multi-line
//! `HelpBar` used by both comparison views.
//!
//! Both bars are rendered borderless so they cost only the rows they
//! actually need; the panel above them already draws the separating
//! border, so a full box would only steal vertical space from the main
//! view area.
//!
//! The module also hosts [`tabs_hit_item`] and [`tabs_item_x`], the click
//! mapping and title anchor shared by the menu and tab bars, which both
//! render through ratatui's `Tabs`, [`hint_line`], the shared bottom hint
//! of every modal, and the busy indicator ([`use_busy_tick`] plus
//! [`busy_indicator`]) the two comparison views prepend to their status
//! bar while background tasks run.

use std::time::Duration;

use ratatui_kit::{
    prelude::*,
    ratatui::{
        layout::Constraint,
        style::{Modifier, Style},
        text::{Line, Span, Text as RataText},
    },
};

use crate::state::APP_STATE;

/// Frames of the busy-indicator spinner (rotating braille dots).
pub(super) const SPINNER_FRAMES: [char; 10] =
    ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Interval between spinner animation steps.
const BUSY_TICK_INTERVAL: Duration = Duration::from_millis(120);

/// Return the spinner glyph for `tick`, wrapping through the frames.
pub(super) fn spinner_glyph(tick: u8) -> char {
    SPINNER_FRAMES[usize::from(tick) % SPINNER_FRAMES.len()]
}

/// Advance a local frame counter while the app has pending background
/// work, and return the current value.
///
/// The future runs once for the lifetime of the calling component and
/// sleeps between steps. It writes the tick only while
/// [`crate::state::AppState::is_busy`], so an idle app spends no frames on
/// the animation; the writes that do happen wake just this component, which
/// re-reads the busy label from the [`APP_STATE`] atom it subscribes to
/// anyway. Offscreen render tests drive the tree without a tokio runtime,
/// so the loop exits early there and the indicator simply stays on its
/// first frame.
///
/// Call unconditionally at the top of a view component (stable hook order)
/// and pass the returned tick to [`busy_indicator`].
pub(super) fn use_busy_tick(hooks: &mut Hooks) -> u8 {
    let mut tick = hooks.use_state(|| 0_u8);
    hooks.use_future(async move {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        loop {
            tokio::time::sleep(BUSY_TICK_INTERVAL).await;
            if APP_STATE.state().read().is_busy() {
                tick.set(tick.get().wrapping_add(1));
            }
        }
    });
    tick.get()
}

/// Build the leading `(spinner, label)` spans of a busy status bar.
///
/// The spinner and label carry `style` (the caller's status highlight);
/// the trailing separator is unstyled bar chrome, matching the `|`
/// separators the comparison views already use between their status
/// sections.
pub(super) fn busy_indicator(
    tick: u8,
    label: &'static str,
    style: Style,
) -> Vec<Span<'static>> {
    vec![
        Span::styled(
            format!("{} {label}\u{2026}", spinner_glyph(tick)),
            style,
        ),
        Span::raw("  |  "),
    ]
}

/// Build the bottom hint line of a modal from `(key, action)` pairs.
///
/// Every dialog renders its key map with the same wording, `·` separator,
/// and surrounding padding, so the only degrees of freedom are the pairs
/// themselves. The key spans are bold to stand out from the action text;
/// the caller applies the theme's dim style (and centering), which keeps
/// the color decision with the other border chrome.
pub(super) fn hint_line(hints: &[(&str, &str)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, (key, action)) in hints.iter().enumerate() {
        spans.push(Span::raw(if index == 0 { " " } else { " \u{b7} " }));
        spans.push(Span::styled(
            (*key).to_string(),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(format!(" {action}")));
    }
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// Map an x coordinate local to a [`ratatui::widgets::Tabs`] row to the
/// index of the item it hits, `None` on a divider or past the last item.
///
/// Mirrors the widget's layout: every item renders as
/// `[pad-left][title][pad-right]` with `divider_width` cells between
/// items, so the clickable region of an item spans its title plus the two
/// padding cells.
pub(super) fn tabs_hit_item(
    x: u16,
    titles: &[Line<'static>],
    divider_width: u16,
) -> Option<usize> {
    let mut cursor = 0u16;
    for (i, title) in titles.iter().enumerate() {
        let end = cursor
            .saturating_add(title.width() as u16)
            .saturating_add(2);
        if x >= cursor && x < end {
            return Some(i);
        }
        cursor = end.saturating_add(divider_width);
    }
    None
}

/// Return the x offset at which the item with `index` starts, `None` when
/// the index is past the last item.
///
/// The inverse of [`tabs_hit_item`]: it walks the same
/// `[pad-left][title][pad-right][divider]` layout from the left edge, so a
/// dropdown can be anchored under its own title.
pub(super) fn tabs_item_x(
    titles: &[Line<'static>],
    index: usize,
    divider_width: u16,
) -> Option<u16> {
    let mut cursor = 0u16;
    for (i, title) in titles.iter().enumerate() {
        if i == index {
            return Some(cursor);
        }
        cursor = cursor
            .saturating_add(title.width() as u16)
            .saturating_add(2)
            .saturating_add(divider_width);
    }
    None
}

/// Placeholder text of the command-mode (`:`) input, shared by both
/// comparison views.
pub(super) const COMMAND_PLACEHOLDER: &str =
    "Command (reload, sync, filter <pattern>, help)...";

/// Props for [`StatusBar`].
#[derive(Default, Props)]
pub(super) struct BarProps {
    /// The single content line of the bar.
    pub(super) line: Line<'static>,
}

/// One-line status bar at the bottom of a comparison view.
#[component]
pub(super) fn StatusBar(props: &BarProps) -> impl Into<AnyElement<'static>> {
    element!(
        View(height: Constraint::Length(1)) {
            Text(text: props.line.clone())
        }
    )
}

/// Props for [`HelpBar`].
#[derive(Default, Props)]
pub(super) struct HelpBarProps {
    /// The content lines of the bar; the bar grows to show them all.
    pub(super) lines: Vec<Line<'static>>,
}

/// Help bar at the very bottom of a comparison view.
///
/// Takes as many rows as its (pre-wrapped) content lines, so no help item
/// is cut off on narrow terminals. Only rendered when the user toggles it
/// on (`?`); when hidden the main view area gains the rows.
#[component]
pub(super) fn HelpBar(props: &HelpBarProps) -> impl Into<AnyElement<'static>> {
    element!(
        View(height: Constraint::Length(props.lines.len().max(1) as u16)) {
            Text(text: RataText::from(props.lines.clone()))
        }
    )
}

#[cfg(test)]
mod tests {
    use ratatui_kit::ratatui::style::Color;

    use super::*;

    fn titles(names: &[&str]) -> Vec<Line<'static>> {
        names.iter().map(|n| Line::from(n.to_string())).collect()
    }

    #[test]
    fn spinner_glyph_wraps_through_the_frames() {
        assert_eq!(spinner_glyph(0), SPINNER_FRAMES[0]);
        assert_eq!(spinner_glyph(3), SPINNER_FRAMES[3]);
        assert_eq!(
            spinner_glyph(SPINNER_FRAMES.len() as u8),
            SPINNER_FRAMES[0]
        );
    }

    #[test]
    fn busy_indicator_prefixes_label_with_spinner_and_separator() {
        let spans =
            busy_indicator(0, "Comparing", Style::new().fg(Color::Yellow));
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            text,
            format!("{} Comparing\u{2026}  |  ", SPINNER_FRAMES[0])
        );
        // Only the spinner and label take the caller's style; the
        // separator stays plain bar chrome.
        assert_eq!(spans[0].style.fg, Some(Color::Yellow));
        assert_eq!(spans[1].style, Style::new());
    }

    #[test]
    fn hint_line_bold_formats_each_key_action_pair() {
        let line = hint_line(&[
            ("j/k", "select"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ]);
        // One bold span per key, plain spans around it, `·` between pairs.
        let text: String =
            line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, " j/k select · Enter confirm · Esc cancel ");
        let bold: Vec<&str> = line
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(bold, vec!["j/k", "Enter", "Esc"]);
    }

    #[test]
    fn hint_line_is_empty_without_hints() {
        assert!(hint_line(&[]).spans.is_empty());
    }

    #[test]
    fn tabs_hit_item_includes_the_padding_but_not_the_divider() {
        // Layout: `[x][Sessions][x] | [x][Help][x]` → item 0 spans 0..10,
        // the divider 10..13, item 1 spans 13..19.
        let items = titles(&["Sessions", "Help"]);
        assert_eq!(tabs_hit_item(0, &items, 3), Some(0));
        assert_eq!(tabs_hit_item(9, &items, 3), Some(0));
        assert_eq!(tabs_hit_item(10, &items, 3), None);
        assert_eq!(tabs_hit_item(12, &items, 3), None);
        assert_eq!(tabs_hit_item(13, &items, 3), Some(1));
        assert_eq!(tabs_hit_item(18, &items, 3), Some(1));
        assert_eq!(tabs_hit_item(19, &items, 3), None);
    }

    #[test]
    fn tabs_hit_item_is_empty_without_titles() {
        assert_eq!(tabs_hit_item(0, &[], 3), None);
    }

    #[test]
    fn tabs_item_x_anchors_at_the_start_of_an_item() {
        let items = titles(&["Sessions", "Help"]);
        assert_eq!(tabs_item_x(&items, 0, 3), Some(0));
        assert_eq!(tabs_item_x(&items, 1, 3), Some(13));
        assert_eq!(tabs_item_x(&items, 2, 3), None);
        // Every anchor is a cell the hit mapping attributes to its item.
        for index in 0..2 {
            let x = tabs_item_x(&items, index, 3).unwrap();
            assert_eq!(tabs_hit_item(x, &items, 3), Some(index));
        }
    }
}
