// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Key Mapping Module (`keymap`)
//!
//! This module provides key mapping functionality for translating terminal
//! keyboard events into application-level actions. It defines structures for
//! managing key bindings and converting between different key representations.
//!
//! ## Overview
//!
//! The module consists of two main components:
//!
//! - **`KeyMapItem`**: Represents a single key binding, associating a
//!   `KeyCode` with an `AppEvent`, name, and enabled state.
//! - **`KeyMap`**: A collection of `KeyMapItem`s that provides mapping
//!   functionality to look up events based on keyboard input.
//!
//! ## Key Representation
//!
//! Special keys are represented with Unicode symbols for display:
//!
//! | Key | Symbol |
//! |-----|--------|
//! | Enter | ↵ |
//! | Left Arrow | ← |
//! | Right Arrow | → |
//! | Up Arrow | ↑ |
//! | Down Arrow | ↓ |
//! | Home | ⤒ |
//! | End | ⤓ |
//! | Page Up | ⇞ |
//! | Page Down | ⇟ |
//! | Tab | ⇥ |
//! | Back Tab | ⇤ |
//! | Escape | Esc |

use std::fmt;

use ratatui::{
    crossterm::event::KeyCode,
    style::Style,
    text::{Line, Span, Text},
};

use crate::event::Event;

/// Represents a single key mapping binding a keyboard key to an application
/// event.
///
/// A `KeyMapItem` associates:
/// - A specific `KeyCode` (the physical key pressed)
/// - A display name for the action
/// - An enabled state (whether the binding is currently enabled)
/// - An `AppEvent` that should be triggered when the key is pressed
#[derive(Debug, Clone)]
pub(crate) struct KeyMapItem {
    /// The keyboard key that triggers this mapping.
    key_code: KeyCode,
    /// An optional alternate key code for this mapping.
    alt_key_code: Option<KeyCode>,
    /// Human-readable name of the action for display purposes.
    name: &'static str,
    /// Whether this key mapping is currently enabled.
    enabled: bool,
    /// The event to emit when this key is pressed.
    event: Event,
}

impl KeyMapItem {
    /// Creates a new `KeyMapItem` with the specified parameters.
    ///
    /// # Arguments
    ///
    /// - `key_code`: The keyboard key to bind
    /// - `alt_key_code`: An optional alternate key code for this mapping
    /// - `name`: A descriptive name for display purposes
    /// - `enabled`: Whether the mapping should be enabled initially
    /// - `event`: The event that will be triggered when this key is pressed
    #[inline(always)]
    pub(crate) const fn new(
        key_code: KeyCode,
        alt_key_code: Option<KeyCode>,
        name: &'static str,
        enabled: bool,
        event: Event,
    ) -> Self {
        Self {
            key_code,
            alt_key_code,
            name,
            enabled,
            event,
        }
    }

    /// Returns the mapped keyboard key.
    #[inline(always)]
    pub(crate) const fn key_code(&self) -> KeyCode {
        self.key_code
    }

    /// Returns the alternate key code, if set. This allows checking
    /// for secondary key bindings associated with this mapping.
    #[inline(always)]
    pub(crate) const fn alt_key_code(&self) -> Option<KeyCode> {
        self.alt_key_code
    }

    /// Returns the display name of this key mapping.
    #[inline(always)]
    pub(crate) const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the event that will be triggered when this key is pressed.
    #[inline(always)]
    pub(crate) fn event(&self) -> Event {
        self.event.clone()
    }

    /// Returns whether this key mapping is currently enabled.
    #[inline(always)]
    pub(crate) const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Sets whether this key mapping should be enabled.
    ///
    /// Disabled mappings are ignored when looking up events via `KeyMap`.
    #[inline(always)]
    pub(crate) const fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}

/// Returns a display-friendly string representation of a key code.
///
/// This function converts special keys to their symbolic representations:
///
/// | Input Key  | Output   |
/// |------------|----------|
/// | Enter      | ↵        |
/// | Left       | ←        |
/// | Right      | →        |
/// | Up         | ↑        |
/// | Down       | ↓        |
/// | Home       | ⤒        |
/// | End        | ⤓        |
/// | PageUp     | ⇞        |
/// | PageDown   | ⇟        |
/// | Tab        | ⇥        |
/// | BackTab    | ⇤        |
/// | Escape     | Esc      |
///
/// Regular character keys are returned as their string representation.
fn repr_key_code(key_code: &KeyCode) -> String {
    match key_code {
        KeyCode::Enter => "↵".to_owned(),
        KeyCode::Left => "←".to_owned(),
        KeyCode::Right => "→".to_owned(),
        KeyCode::Up => "↑".to_owned(),
        KeyCode::Down => "↓".to_owned(),
        KeyCode::Home => "⤒".to_owned(),
        KeyCode::End => "⤓".to_owned(),
        KeyCode::PageUp => "⇞".to_owned(),
        KeyCode::PageDown => "⇟".to_owned(),
        KeyCode::Tab => "⇥".to_owned(),
        KeyCode::BackTab => "⇤".to_owned(),
        KeyCode::Esc => "Esc".to_owned(),
        _ => key_code.to_string(),
    }
}

/// Displays the key mapping in format "Name: Key".
impl fmt::Display for KeyMapItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(alt) = self.alt_key_code {
            write!(
                f,
                "{}: [{}]/[{}]",
                self.name,
                repr_key_code(&self.key_code),
                repr_key_code(&alt)
            )
        } else {
            write!(f, "{}: [{}]", self.name, repr_key_code(&self.key_code))
        }
    }
}

/// A convenience implementation for converting a `KeyMapItem` into a `Span`.
impl<'a> From<&'a KeyMapItem> for Span<'a> {
    fn from(item: &'a KeyMapItem) -> Self {
        Span::styled(
            format!("{}", item),
            if item.enabled {
                Style::default()
            } else {
                Style::default().dim()
            },
        )
    }
}

/// A collection of key mappings that can act as a `KeyMapper`.
///
/// When looking up keys, only **enabled** mappings are considered.
#[derive(Debug, Clone)]
pub(crate) struct KeyMap(Vec<KeyMapItem>);

/// Creates a `KeyMap` from a slice of key map items.
///
/// The provided slice is cloned into the internal vector.
impl From<&[KeyMapItem]> for KeyMap {
    fn from(key_maps: &[KeyMapItem]) -> Self {
        debug_assert!(!key_maps.is_empty());
        Self(key_maps.to_vec())
    }
}

/// Formats all key mappings as a display string.
///
/// The output format is: `| Key1: val1 | Key2: val2 | ... |`
/// Only enabled mappings are included in the output.
impl fmt::Display for KeyMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let delim = " ";
        write!(
            f,
            "{}",
            self.0
                .iter()
                .filter(|km| km.is_enabled())
                .map(|key_map| format!("{}", key_map))
                .collect::<Vec<_>>()
                .join(delim)
        )
    }
}

/// Converts a `KeyMap` into a `Line` for display purposes.
#[allow(clippy::fallible_impl_from)]
impl<'a> From<&'a KeyMap> for Line<'a> {
    fn from(key_map: &'a KeyMap) -> Self {
        let mut spans = vec![Span::from(key_map.0.first().unwrap())];
        for item in key_map.0.iter().skip(1) {
            spans.push(Span::raw(" "));
            spans.push(Span::from(item));
        }
        Line::from_iter(spans)
    }
}

/// Converts a `KeyMap` into a `Text` for display purposes.
#[allow(clippy::fallible_impl_from)]
impl<'a> From<&'a KeyMap> for Text<'a> {
    #[inline(always)]
    fn from(key_map: &'a KeyMap) -> Self {
        Text::from(Line::from(key_map))
    }
}

/// A fixed-size array of `KeyMap`s that can act as a `KeyMapper`.
///
/// When looking up keys, only **enabled** mappings are considered.
#[derive(Debug)]
pub(crate) struct KeyMapArray<const N: usize>([KeyMap; N]);

impl<const N: usize> KeyMapArray<N> {
    #[inline(always)]
    pub(crate) const fn new(items: [KeyMap; N]) -> Self {
        Self(items)
    }

    #[inline(always)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &KeyMap> + '_ {
        self.0.iter()
    }
}

/// Converts a `KeyMapArray` into a `Text` for display purposes.
#[allow(clippy::fallible_impl_from)]
impl<'a, const N: usize> From<&'a KeyMapArray<N>> for Text<'a> {
    fn from(key_maps: &'a KeyMapArray<N>) -> Self {
        let lines: Vec<Line<'a>> =
            key_maps.0.iter().map(Line::from).collect::<Vec<_>>();
        Text::from(lines)
    }
}

pub(crate) trait KeyMapper {
    /// Returns a reference to the underlying `KeyMap`.
    fn keymap(&self) -> &dyn KeyMapper;

    /// Maps a `KeyCode` to an `Event`, if one exists.
    ///
    /// When looking up keys, only **enabled** mappings are considered.
    fn map_key_code(&self, key_code: KeyCode) -> Option<Event> {
        self.keymap().map_key_code(key_code)
    }
}

impl KeyMapper for KeyMap {
    #[inline(always)]
    fn keymap(&self) -> &dyn KeyMapper {
        self
    }

    fn map_key_code(&self, key_code: KeyCode) -> Option<Event> {
        self.0
            .iter()
            .filter(|key_map| key_map.is_enabled())
            .find(|key_map| {
                key_map.key_code() == key_code
                    || key_map
                        .alt_key_code()
                        .is_some_and(|alt| alt == key_code)
            })
            .map(|key_map| key_map.event())
    }
}

impl<const N: usize> KeyMapper for KeyMapArray<N> {
    fn keymap(&self) -> &dyn KeyMapper {
        self
    }

    fn map_key_code(&self, key_code: KeyCode) -> Option<Event> {
        self.0.iter().find_map(|map| map.map_key_code(key_code))
    }
}

pub(crate) trait KeyHint {
    fn key_hint(&self) -> Text<'_>;
}

impl KeyHint for KeyMap {
    #[inline(always)]
    fn key_hint(&self) -> Text<'_> {
        Text::from(self)
    }
}

impl<const N: usize> KeyHint for KeyMapArray<N> {
    #[inline(always)]
    fn key_hint(&self) -> Text<'_> {
        Text::from(self)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use crate::event::{AppEvent, NavEvent, OpEvent};

    use super::*;

    /// Default set of key map items used across multiple tests.
    ///
    /// Includes:
    /// - Enter: OpenView (enabled, alt: o)
    /// - q: Quit (enabled, alt: None)
    /// - c: Copy (enabled, alt: None)
    /// - m: Move (enabled, alt: None)
    /// - d: Delete (enabled, alt: None)
    /// - r: Rename (disabled, alt: None - for testing disabled filtering)
    const KEYMAP_ITEMS: [KeyMapItem; 6] = [
        KeyMapItem::new(
            KeyCode::Enter,
            Some(KeyCode::Char('o')),
            "Open",
            true,
            Event::App(AppEvent::OpenView),
        ),
        KeyMapItem::new(
            KeyCode::Char('q'),
            None,
            "Quit",
            true,
            Event::App(AppEvent::Quit),
        ),
        KeyMapItem::new(
            KeyCode::Char('c'),
            None,
            "Copy",
            true,
            Event::Op(OpEvent::Copy),
        ),
        KeyMapItem::new(
            KeyCode::Char('m'),
            None,
            "Move",
            true,
            Event::Op(OpEvent::Move),
        ),
        KeyMapItem::new(
            KeyCode::Char('d'),
            None,
            "Delete",
            true,
            Event::Op(OpEvent::Delete),
        ),
        KeyMapItem::new(
            KeyCode::Char('r'),
            None,
            "Rename",
            false,
            Event::Op(OpEvent::Rename),
        ),
    ];

    /// Pre-built key mapper instance for testing.
    static KEY_MAPPER: LazyLock<KeyMap> =
        LazyLock::new(|| KeyMap::from(KEYMAP_ITEMS.as_slice()));

    /// Tests `KeyMapItem::new()` constructor and all getter methods.
    #[test]
    fn test_keymap_new_and_getters() {
        let key_code = KeyCode::Char('c');
        let alt_key_code = None;
        let name = "Copy";
        let enabled = true;
        let event = Event::Op(OpEvent::Copy);

        let key_map = KeyMapItem::new(
            key_code,
            alt_key_code,
            name,
            enabled,
            event.clone(),
        );

        assert_eq!(key_map.key_code(), key_code);
        assert!(key_map.alt_key_code().is_none());
        assert_eq!(key_map.name(), name);
        assert_eq!(key_map.event(), event);
        assert!(key_map.is_enabled());
    }

    /// Tests creating a key map with disabled state.
    #[test]
    fn test_keymap_disabled() {
        let key_code = KeyCode::Char('x');
        let alt_key_code = None;
        let key_map = KeyMapItem::new(
            key_code,
            alt_key_code,
            "Test",
            false,
            Event::Op(OpEvent::Copy),
        );

        assert!(!key_map.is_enabled());
    }

    /// Tests toggling the enabled state of a key map.
    #[test]
    fn test_set_enabled() {
        let mut key_map = KeyMapItem::new(
            KeyCode::Char('a'),
            None,
            "Test",
            false,
            Event::Op(OpEvent::Copy),
        );
        assert!(!key_map.is_enabled());
        key_map.set_enabled(true);
        assert!(key_map.is_enabled());
        key_map.set_enabled(false);
        assert!(!key_map.is_enabled());
    }

    /// Tests the `repr_key_code` function for various key types.
    ///
    /// Covers explicit mappings (arrows, navigation keys) and regular
    /// characters.
    #[test]
    fn test_repr_key_code() {
        // Explicitely mapped key codes
        assert_eq!(repr_key_code(&KeyCode::Enter), "↵");
        assert_eq!(repr_key_code(&KeyCode::Left), "←");
        assert_eq!(repr_key_code(&KeyCode::Right), "→");
        assert_eq!(repr_key_code(&KeyCode::Up), "↑");
        assert_eq!(repr_key_code(&KeyCode::Down), "↓");
        assert_eq!(repr_key_code(&KeyCode::Home), "⤒");
        assert_eq!(repr_key_code(&KeyCode::End), "⤓");
        assert_eq!(repr_key_code(&KeyCode::PageUp), "⇞");
        assert_eq!(repr_key_code(&KeyCode::PageDown), "⇟");
        assert_eq!(repr_key_code(&KeyCode::Tab), "⇥");
        assert_eq!(repr_key_code(&KeyCode::BackTab), "⇤");
        // Regular character keys
        assert_eq!(repr_key_code(&KeyCode::Char('a')), "a");
        assert_eq!(repr_key_code(&KeyCode::Char('Z')), "Z");
        assert_eq!(repr_key_code(&KeyCode::Char('1')), "1");
        // Some special keys
        assert_eq!(repr_key_code(&KeyCode::Esc), "Esc");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(repr_key_code(&KeyCode::Backspace), "Backspace");
        #[cfg(target_os = "macos")]
        assert_eq!(repr_key_code(&KeyCode::Backspace), "Delete");
        assert_eq!(repr_key_code(&KeyCode::CapsLock), "Caps Lock");
        assert_eq!(repr_key_code(&KeyCode::Pause), "Pause");
        assert_eq!(repr_key_code(&KeyCode::Menu), "Menu");
    }

    /// Tests the `Display` implementation for a single `KeyMapItem`.
    #[test]
    fn test_keymap_display_format() {
        let key_map = KeyMapItem::new(
            KeyCode::Char('c'),
            None,
            "Copy",
            true,
            Event::Op(OpEvent::Copy),
        );

        assert_eq!(format!("{}", key_map), "Copy: [c]");
    }

    /// Tests display formatting with special keys (arrows, etc.).
    #[test]
    fn test_keymap_display_with_special_key() {
        let key_map = KeyMapItem::new(
            KeyCode::Enter,
            Some(KeyCode::F(10)),
            "Open",
            true,
            Event::App(AppEvent::OpenView),
        );

        assert_eq!(format!("{}", key_map), "Open: [↵]/[F10]");
    }

    /// Tests successful key mapping lookups for enabled keys.
    #[test]
    fn test_keymapper_map_key_code_found() {
        // Test primary key code
        assert_eq!(
            KEY_MAPPER.map_key_code(KeyCode::Char('c')),
            Some(Event::Op(OpEvent::Copy))
        );
        assert_eq!(
            KEY_MAPPER.map_key_code(KeyCode::Char('m')),
            Some(Event::Op(OpEvent::Move))
        );
        assert_eq!(
            KEY_MAPPER.map_key_code(KeyCode::Char('d')),
            Some(Event::Op(OpEvent::Delete))
        );
    }

    /// Tests key mapping lookups that should return `None`.
    #[test]
    fn test_keymapper_map_key_code_not_found() {
        // Not in map
        assert_eq!(KEY_MAPPER.map_key_code(KeyCode::Char('y')), None);
        // Disabled (key 'r' exists but is marked as disabled)
        assert_eq!(KEY_MAPPER.map_key_code(KeyCode::Char('r')), None);
    }

    /// Tests mapping using alt_key_code.
    #[test]
    fn test_alt_key_code_mapping() {
        // Test primary key code
        assert_eq!(
            KEY_MAPPER.map_key_code(KeyCode::Enter),
            Some(Event::App(AppEvent::OpenView))
        );
        // Test alternate key code
        assert_eq!(
            KEY_MAPPER.map_key_code(KeyCode::Char('o')),
            Some(Event::App(AppEvent::OpenView))
        );
    }

    /// Tests that alt_key_code respects enabled state.
    #[test]
    fn test_alt_key_code_respects_enabled() {
        let key_maps = vec![KeyMapItem::new(
            KeyCode::Char('c'),
            Some(KeyCode::Char('C')),
            "Copy",
            false, // disabled
            Event::Op(OpEvent::Copy),
        )];

        let key_mapper = KeyMap(key_maps);

        // Both primary and alt should not map when disabled
        assert_eq!(key_mapper.map_key_code(KeyCode::Char('c')), None);
        assert_eq!(key_mapper.map_key_code(KeyCode::Char('C')), None);
    }

    /// Tests the `Display` implementation for `KeyMap` with multiple keys.
    #[test]
    fn test_keymapper_display_multiple_keys() {
        let display = format!("{}", &*KEY_MAPPER);
        // Check that all keys are in the display
        assert!(display.contains("Open: [↵]/[o]"));
        assert!(display.contains("Quit: [q]"));
        assert!(display.contains("Copy: [c]"));
        assert!(display.contains("Move: [m]"));
        assert!(display.contains("Delete: [d]"));
    }

    /// Tests that key matching is case-sensitive.
    #[test]
    fn test_keymapper_map_key_code_case_sensitivity() {
        assert!(KEY_MAPPER.map_key_code(KeyCode::Char('c')).is_some());
        // Uppercase should not match lowercase
        assert_eq!(KEY_MAPPER.map_key_code(KeyCode::Char('C')), None);
    }

    /// Tests mapping when multiple keys exist for the same code but only one
    /// is enabled.
    #[test]
    fn test_duplicate_key() {
        let key_maps = vec![
            KeyMapItem::new(
                KeyCode::Enter,
                None,
                "Rename",
                false,
                Event::Op(OpEvent::Rename),
            ),
            KeyMapItem::new(
                KeyCode::Enter,
                None,
                "Open in Dir",
                true,
                Event::App(AppEvent::OpenView),
            ),
            KeyMapItem::new(
                KeyCode::Enter,
                None,
                "Select in File",
                true,
                Event::Nav(NavEvent::Next),
            ),
        ];

        let key_mapper = KeyMap(key_maps);

        // Only first enabled key should be found
        assert_eq!(
            key_mapper.map_key_code(KeyCode::Enter),
            Some(Event::App(AppEvent::OpenView))
        );
    }

    /// Tests that changing a key's enabled state affects lookup results.
    ///
    /// Verifies that a key can be dynamically enabled and then successfully
    /// mapped to its event.
    #[test]
    fn test_set_enabled_then_map() {
        let key_map = KeyMapItem::new(
            KeyCode::Char('x'),
            None,
            "Test",
            false,
            Event::Op(OpEvent::Copy),
        );

        let key_mapper = KeyMap(vec![key_map]);

        // Initially disabled
        assert_eq!(key_mapper.map_key_code(KeyCode::Char('x')), None);

        // Enable and try again
        let mut key_map = key_mapper.0.into_iter().next().unwrap();
        key_map.set_enabled(true);

        let key_mapper = KeyMap(vec![key_map]);
        assert_eq!(
            key_mapper.map_key_code(KeyCode::Char('x')),
            Some(Event::Op(OpEvent::Copy))
        );
    }

    /// Tests the `Span` conversion from a single `KeyMapItem`.
    #[test]
    fn test_span_from_keymapitem() {
        let key_map_item = &KEYMAP_ITEMS[0];
        assert!(key_map_item.is_enabled());
        let span: Span<'_> = Span::from(key_map_item);
        assert_eq!(span.content.to_string(), format!("{}", key_map_item));
        // Enabled items should not be dimmed
        assert_eq!(span.style, Style::default());

        let key_map_item = &KEYMAP_ITEMS[5];
        assert!(!key_map_item.is_enabled());
        let span: Span<'_> = Span::from(key_map_item);
        assert_eq!(span.content.to_string(), format!("{}", key_map_item));
        // Disabled items should be dimmed
        assert_eq!(span.style, Style::default().dim());
    }

    /// Tests the `Text` conversion from a `KeyMap`.
    #[test]
    fn test_text_from_keymap() {
        let key_map = &*KEY_MAPPER;
        let text: Text<'_> = Text::from(key_map);
        assert_eq!(text.lines.len(), 1);
        // Check that all items are in the text
        let line = text.lines.first().unwrap();
        let spans = line.spans.iter().filter(|span| span.to_string() != " ");
        for (span, item) in spans.zip(key_map.0.iter()) {
            assert_eq!(span.content.to_string(), format!("{}", item));
        }
    }

    #[test]
    fn test_keymap_array() {
        let key_map_1 = KeyMap::from(&KEYMAP_ITEMS[..3]);
        let key_map_2 = KeyMap::from(&KEYMAP_ITEMS[3..]);
        let key_maps = KeyMapArray([key_map_1, key_map_2]);
        assert_eq!(
            key_maps.map_key_code(KeyCode::Char('q')),
            Some(Event::App(AppEvent::Quit))
        );
        assert_eq!(
            key_maps.map_key_code(KeyCode::Char('c')),
            Some(Event::Op(OpEvent::Copy))
        );
        assert!(key_maps.map_key_code(KeyCode::Char('r')).is_none());
    }
}
