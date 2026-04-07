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
//!   `KeyCode` with an `AppEvent`, name, and active state.
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

use crossterm::event::KeyCode;

use crate::appevent::AppEvent;

/// Represents a single key mapping binding a keyboard key to an application
/// event.
///
/// A `KeyMapItem` associates:
/// - A specific `KeyCode` (the physical key pressed)
/// - A display name for the action
/// - An active state (whether the binding is currently enabled)
/// - An `AppEvent` that should be triggered when the key is pressed
#[derive(Debug, Clone)]
pub(crate) struct KeyMapItem {
    /// The keyboard key that triggers this mapping.
    key_code: KeyCode,
    /// Human-readable name of the action for display purposes.
    name: &'static str,
    /// Whether this key mapping is currently active/enabled.
    active: bool,
    /// The application event to emit when this key is pressed.
    event: AppEvent,
}

impl KeyMapItem {
    /// Creates a new `KeyMapItem` with the specified parameters.
    ///
    /// # Arguments
    ///
    /// - `key_code`: The keyboard key to bind
    /// - `name`: A descriptive name for display purposes
    /// - `active`: Whether the mapping should be active initially
    /// - `event`: The event that will be triggered when this key is pressed
    pub(crate) const fn new(
        key_code: KeyCode,
        name: &'static str,
        active: bool,
        event: AppEvent,
    ) -> Self {
        Self {
            key_code,
            name,
            active,
            event,
        }
    }

    /// Returns a reference to the mapped keyboard key.
    pub(crate) const fn key_code(&self) -> &KeyCode {
        &self.key_code
    }

    /// Returns the display name of this key mapping.
    pub(crate) const fn name(&self) -> &'static str {
        self.name
    }

    /// Returns the event that will be triggered when this key is pressed.
    ///
    /// Note: This consumes and returns the event value, as events are not
    /// cloned.
    pub(crate) const fn event(&self) -> AppEvent {
        self.event
    }

    /// Returns whether this key mapping is currently active.
    pub(crate) const fn is_active(&self) -> bool {
        self.active
    }

    /// Sets whether this key mapping should be active.
    ///
    /// Inactive mappings are ignored when looking up events via `KeyMap`.
    pub(crate) const fn set_active(&mut self, active: bool) {
        self.active = active;
    }
}

/// Returns a display-friendly string representation of a key code.
///
/// This function converts special keys to their symbolic representations:
///
/// | Input Key | Output |
/// |-----------|--------|
/// | Enter | ↵ |
/// | Left/Right/Up/Down | ← → ↑ ↓ |
/// | Home/End | ⤒ ⤓ |
/// | PageUp/PageDown | ⇞ ⇟ |
/// | Tab/BackTab | ⇥ ⇤ |
/// | Escape | Esc |
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
///
/// # Example
///
/// ```
/// use crossterm::event::KeyCode;
///
/// use crate::{appevent::AppEvent, keymap::KeyMapItem};
///
/// let map =
///     KeyMapItem::new(KeyCode::Char('c'), "Copy", true, AppEvent::Copy);
/// assert_eq!(format!("{}", map), "Copy: c");
/// ```
impl fmt::Display for KeyMapItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, repr_key_code(&self.key_code))
    }
}

/// A collection of key mappings that can look up events by key code.
///
/// `KeyMap` stores a vector of `KeyMapItem`s and provides functionality to:
/// - Look up which event should be triggered for a given key code
/// - Format all active mappings as a display string
///
/// When looking up keys, only **active** mappings are considered.
pub(crate) struct KeyMap(Vec<KeyMapItem>);

impl KeyMap {
    pub(crate) fn map_key_code(&self, key_code: &KeyCode) -> Option<AppEvent> {
        self.0
            .iter()
            .filter(|key_map| key_map.is_active())
            .find(|key_map| key_map.key_code() == key_code)
            .map(|key_map| key_map.event())
    }
}

/// Creates a `KeyMap` from a slice of key map items.
///
/// The provided slice is cloned into the internal vector.
impl From<&[KeyMapItem]> for KeyMap {
    fn from(key_maps: &[KeyMapItem]) -> Self {
        Self(key_maps.to_vec())
    }
}

/// Formats all key mappings as a display string.
///
/// The output format is: `| Key1: val1 | Key2: val2 | ... |`
/// Only active mappings are included in the output.
impl fmt::Display for KeyMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let delim = " | ";
        write!(
            f,
            "| {} |",
            self.0
                .iter()
                .filter(|km| km.is_active())
                .map(|key_map| format!("{}", key_map))
                .collect::<Vec<_>>()
                .join(delim)
        )
    }
}

/// Test suite for the keymap module.
///
/// Tests cover:
/// - Key map item creation and getter methods
/// - Active/inactive state management
/// - Key code representation (special keys, characters)
/// - Display formatting for both single items and collections
/// - Event mapping lookups (found, not found, inactive filtering)
/// - Case sensitivity of key matching
#[cfg(test)]
mod tests {
    use std::sync;

    use super::*;

    /// Default set of key map items used across multiple tests.
    ///
    /// Includes:
    /// - Enter: OpenView (active)
    /// - q: Quit (active)
    /// - c: Copy (active)
    /// - m: Move (active)
    /// - d: Delete (active)
    /// - r: Rename (inactive - for testing inactive filtering)
    const KEYMAP_ITEMS: [KeyMapItem; 6] = [
        KeyMapItem::new(KeyCode::Enter, "Open", true, AppEvent::OpenView),
        KeyMapItem::new(KeyCode::Char('q'), "Quit", true, AppEvent::Quit),
        KeyMapItem::new(KeyCode::Char('c'), "Copy", true, AppEvent::Copy),
        KeyMapItem::new(KeyCode::Char('m'), "Move", true, AppEvent::Move),
        KeyMapItem::new(KeyCode::Char('d'), "Delete", true, AppEvent::Delete),
        KeyMapItem::new(KeyCode::Char('r'), "Rename", false, AppEvent::Rename),
    ];

    /// Pre-built key mapper instance for testing.
    ///
    /// Uses lazy initialization to share the same instance across tests.
    static KEY_MAPPER: sync::LazyLock<KeyMap> =
        sync::LazyLock::new(|| KeyMap::from(KEYMAP_ITEMS.as_slice()));

    /// Tests `KeyMapItem::new()` constructor and all getter methods.
    #[test]
    fn test_keymap_new_and_getters() {
        let key_code = KeyCode::Char('c');
        let name = "Copy";
        let active = true;
        let event = AppEvent::Copy;

        let key_map = KeyMapItem::new(key_code, name, active, event);

        assert_eq!(*key_map.key_code(), key_code);
        assert_eq!(key_map.name(), name);
        assert_eq!(key_map.event(), event);
        assert!(key_map.is_active());
    }

    /// Tests creating a key map with inactive state.
    #[test]
    fn test_keymap_inactive() {
        let key_code = KeyCode::Char('x');
        let key_map = KeyMapItem::new(key_code, "Test", false, AppEvent::Copy);

        assert!(!key_map.is_active());
    }

    /// Tests toggling the active state of a key map.
    #[test]
    fn test_set_active() {
        let mut key_map =
            KeyMapItem::new(KeyCode::Char('a'), "Test", false, AppEvent::Copy);
        assert!(!key_map.is_active());
        key_map.set_active(true);
        assert!(key_map.is_active());
        key_map.set_active(false);
        assert!(!key_map.is_active());
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
        assert_eq!(repr_key_code(&KeyCode::Backspace), "Backspace");
        assert_eq!(repr_key_code(&KeyCode::CapsLock), "Caps Lock");
        assert_eq!(repr_key_code(&KeyCode::Pause), "Pause");
        assert_eq!(repr_key_code(&KeyCode::Menu), "Menu");
    }

    /// Tests the `Display` implementation for a single `KeyMapItem`.
    #[test]
    fn test_keymap_display_format() {
        let key_map =
            KeyMapItem::new(KeyCode::Char('c'), "Copy", true, AppEvent::Copy);

        assert_eq!(format!("{}", key_map), "Copy: c");
    }

    /// Tests display formatting with special keys (arrows, etc.).
    #[test]
    fn test_keymap_display_with_special_key() {
        let key_map =
            KeyMapItem::new(KeyCode::Enter, "Open", true, AppEvent::OpenView);

        assert_eq!(format!("{}", key_map), "Open: ↵");
    }

    /// Tests successful key mapping lookups for active keys.
    #[test]
    fn test_keymapper_map_key_code_found() {
        assert_eq!(
            KEY_MAPPER.map_key_code(&KeyCode::Char('c')),
            Some(AppEvent::Copy)
        );
        assert_eq!(
            KEY_MAPPER.map_key_code(&KeyCode::Char('m')),
            Some(AppEvent::Move)
        );
        assert_eq!(
            KEY_MAPPER.map_key_code(&KeyCode::Char('d')),
            Some(AppEvent::Delete)
        );
    }

    /// Tests key mapping lookups that should return `None`.
    ///
    /// Covers two cases:
    /// 1. Keys not present in the mapping
    /// 2. Active keys (tests that inactive keys are filtered out)
    #[test]
    fn test_keymapper_map_key_code_not_found() {
        // Not in map
        assert_eq!(KEY_MAPPER.map_key_code(&KeyCode::Char('y')), None);
        // Not active (key 'r' exists but is marked as inactive)
        assert_eq!(KEY_MAPPER.map_key_code(&KeyCode::Char('r')), None);
    }

    /// Tests the `Display` implementation for `KeyMap` with multiple keys.
    #[test]
    fn test_keymapper_display_multiple_keys() {
        let display = format!("{}", &*KEY_MAPPER);
        // Check that all keys are in the display
        assert!(display.contains("Open: ↵"));
        assert!(display.contains("Quit: q"));
        assert!(display.contains("Copy: c"));
        assert!(display.contains("Move: m"));
        assert!(display.contains("Delete: d"));

        // Check delimiter
        assert!(display.contains("|"));
    }

    /// Tests that key matching is case-sensitive.
    #[test]
    fn test_keymapper_map_key_code_case_sensitivity() {
        assert!(KEY_MAPPER.map_key_code(&KeyCode::Char('c')).is_some());
        // Uppercase should not match lowercase
        assert_eq!(KEY_MAPPER.map_key_code(&KeyCode::Char('C')), None);
    }

    /// Tests mapping when multiple keys exist for the same code but only one
    /// is active.
    #[test]
    fn test_multiple_keymaps_same_key_different_sections() {
        let key_maps = vec![
            KeyMapItem::new(
                KeyCode::Enter,
                "Open in Dir",
                true,
                AppEvent::OpenView,
            ),
            KeyMapItem::new(
                KeyCode::Enter,
                "Select in File",
                false,
                AppEvent::NavigateNext,
            ),
        ];

        let key_mapper = KeyMap(key_maps);

        // Only first active key should be found
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Enter),
            Some(AppEvent::OpenView)
        );
    }

    /// Tests that when multiple keys are active for the same code,
    /// the first one is returned (order matters).
    #[test]
    fn test_multiple_active_keys_same_section() {
        let key_maps = vec![
            KeyMapItem::new(
                KeyCode::Enter,
                "Open in Dir",
                true,
                AppEvent::OpenView,
            ),
            KeyMapItem::new(
                KeyCode::Enter,
                "Select in File",
                true,
                AppEvent::NavigateNext,
            ),
        ];

        let key_mapper = KeyMap(key_maps);

        // First active key should be found (order matters)
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Enter),
            Some(AppEvent::OpenView)
        );
    }

    /// Tests that inactive keys are properly filtered during lookup.
    #[test]
    fn test_inactive_keys_filtered() {
        let key_maps = vec![
            KeyMapItem::new(
                KeyCode::Enter,
                "Open in Dir",
                true,
                AppEvent::OpenView,
            ),
            KeyMapItem::new(
                KeyCode::Enter,
                "Select in File",
                false,
                AppEvent::NavigateNext,
            ),
        ];

        let key_mapper = KeyMap(key_maps);

        // Only active key should be found
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Enter),
            Some(AppEvent::OpenView)
        );
    }

    /// Tests that changing a key's active state affects lookup results.
    ///
    /// Verifies that a key can be dynamically activated and then successfully
    /// mapped to its event.
    #[test]
    fn test_set_active_then_map() {
        let key_map =
            KeyMapItem::new(KeyCode::Char('x'), "Test", false, AppEvent::Copy);

        let key_mapper = KeyMap(vec![key_map]);

        // Initially inactive
        assert_eq!(key_mapper.map_key_code(&KeyCode::Char('x')), None);

        // Activate and try again
        let mut key_map = key_mapper.0.into_iter().next().unwrap();
        key_map.set_active(true);

        let key_mapper = KeyMap(vec![key_map]);
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Char('x')),
            Some(AppEvent::Copy)
        );
    }
}
