// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Key mappings for the terminal events.

use std::fmt;

use crossterm::event::KeyCode;

use crate::appevent::AppEvent;

/// Key mappings for the terminal events.
#[derive(Debug, Clone)]
pub(crate) struct KeyMap {
    key_code: KeyCode,
    section: u32,
    name: &'static str,
    active: bool,
    event: AppEvent,
}

impl KeyMap {
    pub(crate) const fn new(
        key_code: KeyCode,
        section: u32,
        name: &'static str,
        active: bool,
        event: AppEvent,
    ) -> Self {
        Self {
            key_code,
            section,
            name,
            active,
            event,
        }
    }

    pub(crate) const fn key_code(&self) -> &KeyCode {
        &self.key_code
    }

    pub(crate) const fn section(&self) -> u32 {
        self.section
    }

    pub(crate) const fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) const fn event(&self) -> AppEvent {
        self.event
    }

    pub(crate) const fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) const fn set_active(&mut self, active: bool) {
        self.active = active;
    }
}

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

impl fmt::Display for KeyMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.name, repr_key_code(&self.key_code))
    }
}

pub(crate) struct KeyMapper(Vec<KeyMap>);

impl KeyMapper {
    pub(crate) fn map_key_code(&self, key_code: &KeyCode) -> Option<AppEvent> {
        self.0
            .iter()
            .filter(|key_map| key_map.is_active())
            .find(|key_map| key_map.key_code() == key_code)
            .map(|key_map| key_map.event())
    }
}

impl From<&[KeyMap]> for KeyMapper {
    fn from(key_maps: &[KeyMap]) -> Self {
        Self(key_maps.to_vec())
    }
}

impl fmt::Display for KeyMapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let delim = " | ";
        write!(
            f,
            "| {} |",
            self.0
                .iter()
                .map(|key_map| format!("{}", key_map))
                .collect::<Vec<_>>()
                .join(delim)
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync;

    use super::*;

    const KEYMAPS: [KeyMap; 6] = [
        KeyMap::new(KeyCode::Enter, 1, "Open", true, AppEvent::OpenView),
        KeyMap::new(KeyCode::Char('q'), 1, "Quit", true, AppEvent::Quit),
        KeyMap::new(KeyCode::Char('c'), 2, "Copy", true, AppEvent::Copy),
        KeyMap::new(KeyCode::Char('m'), 2, "Move", true, AppEvent::Move),
        KeyMap::new(KeyCode::Char('d'), 2, "Delete", true, AppEvent::Delete),
        KeyMap::new(KeyCode::Char('r'), 2, "Rename", false, AppEvent::Rename),
    ];

    static KEY_MAPPER: sync::LazyLock<KeyMapper> =
        sync::LazyLock::new(|| KeyMapper::from(KEYMAPS.as_slice()));

    #[test]
    fn test_keymap_new_and_getters() {
        let key_code = KeyCode::Char('c');
        let section = 1;
        let name = "Copy";
        let active = true;
        let event = AppEvent::Copy;

        let key_map = KeyMap::new(key_code, section, name, active, event);

        assert_eq!(*key_map.key_code(), key_code);
        assert_eq!(key_map.section(), section);
        assert_eq!(key_map.name(), name);
        assert_eq!(key_map.event(), event);
        assert!(key_map.is_active());
    }

    #[test]
    fn test_keymap_inactive() {
        let key_code = KeyCode::Char('x');
        let key_map = KeyMap::new(key_code, 1, "Test", false, AppEvent::Copy);

        assert!(!key_map.is_active());
    }

    #[test]
    fn test_set_active() {
        let mut key_map =
            KeyMap::new(KeyCode::Char('a'), 1, "Test", false, AppEvent::Copy);
        assert!(!key_map.is_active());
        key_map.set_active(true);
        assert!(key_map.is_active());
        key_map.set_active(false);
        assert!(!key_map.is_active());
    }

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

    #[test]
    fn test_keymap_display_format() {
        let key_map =
            KeyMap::new(KeyCode::Char('c'), 1, "Copy", true, AppEvent::Copy);

        assert_eq!(format!("{}", key_map), "Copy: c");
    }

    #[test]
    fn test_keymap_display_with_special_key() {
        let key_map =
            KeyMap::new(KeyCode::Enter, 1, "Open", true, AppEvent::OpenView);

        assert_eq!(format!("{}", key_map), "Open: ↵");
    }

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

    #[test]
    fn test_keymapper_map_key_code_not_found() {
        // Not in map
        assert_eq!(KEY_MAPPER.map_key_code(&KeyCode::Char('y')), None);
        // Not active
        assert_eq!(KEY_MAPPER.map_key_code(&KeyCode::Char('x')), None);
    }

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

    #[test]
    fn test_keymapper_map_key_code_case_sensitivity() {
        assert!(KEY_MAPPER.map_key_code(&KeyCode::Char('c')).is_some());
        // Uppercase should not match lowercase
        assert_eq!(KEY_MAPPER.map_key_code(&KeyCode::Char('C')), None);
    }

    #[test]
    fn test_multiple_keymaps_same_key_different_sections() {
        let key_maps = vec![
            KeyMap::new(
                KeyCode::Enter,
                1,
                "Open in Dir",
                true,
                AppEvent::OpenView,
            ),
            KeyMap::new(
                KeyCode::Enter,
                2,
                "Select in File",
                false,
                AppEvent::NavigateNext,
            ),
        ];

        let key_mapper = KeyMapper(key_maps);

        // Only first active key should be found
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Enter),
            Some(AppEvent::OpenView)
        );
    }

    #[test]
    fn test_keymapper_map_key_code_multiple_sections() {
        let key_maps = vec![
            KeyMap::new(KeyCode::Enter, 1, "Open", true, AppEvent::OpenView),
            KeyMap::new(
                KeyCode::Enter,
                2,
                "Select",
                false,
                AppEvent::NavigateNext,
            ),
        ];

        let key_mapper = KeyMapper(key_maps);

        // Only active key should be found
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Enter),
            Some(AppEvent::OpenView)
        );
    }

    #[test]
    fn test_multiple_active_keys_same_section() {
        let key_maps = vec![
            KeyMap::new(
                KeyCode::Enter,
                1,
                "Open in Dir",
                true,
                AppEvent::OpenView,
            ),
            KeyMap::new(
                KeyCode::Enter,
                2,
                "Select in File",
                true,
                AppEvent::NavigateNext,
            ),
        ];

        let key_mapper = KeyMapper(key_maps);

        // First active key should be found (order matters)
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Enter),
            Some(AppEvent::OpenView)
        );
    }

    #[test]
    fn test_set_active_then_map() {
        let key_map =
            KeyMap::new(KeyCode::Char('x'), 1, "Test", false, AppEvent::Copy);

        let key_mapper = KeyMapper(vec![key_map]);

        // Initially inactive
        assert_eq!(key_mapper.map_key_code(&KeyCode::Char('x')), None);

        // Activate and try again
        let mut key_map = key_mapper.0.into_iter().next().unwrap();
        key_map.set_active(true);

        let key_mapper = KeyMapper(vec![key_map]);
        assert_eq!(
            key_mapper.map_key_code(&KeyCode::Char('x')),
            Some(AppEvent::Copy)
        );
    }
}
