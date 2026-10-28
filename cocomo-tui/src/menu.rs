// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Nested menu tree: the single source of truth for the menu structure.
//!
//! Mirrors the [`crate::keys`] pattern: the static [`MENUS`] table defines
//! every bar title, Alt accelerator, and dropdown item, and each leaf
//! carries the [`Cmd`] its keyboard counterpart executes, so the menu bar
//! and the keymap cannot drift apart.
//!
//! [`MenuState`] encodes the open menu and its highlighted item; its
//! navigation operations are pure, so the whole dropdown state machine is
//! unit-testable without a terminal. The controller is the only caller
//! that applies them to the global atom.

use cocomo_lib::SyncOperation;

use crate::{keys::Cmd, state::StatusFilter};

/// A leaf entry of a dropdown menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuItemDef {
    /// Label shown in the dropdown.
    pub label: &'static str,
    /// Command run when the item is confirmed. This is the same command
    /// the keyboard counterpart executes, so both entry points share one
    /// dispatch path.
    pub cmd: Cmd,
    /// Whether the item only makes sense on a directory-comparison
    /// session. The controller skips such items when a text session is
    /// active (the dropdown can render them dimmed once it is visible).
    pub requires_dir: bool,
}

/// A top-level menu bar entry with its dropdown items.
#[derive(Clone, Copy, Debug)]
pub struct MenuDef {
    /// Menu bar title.
    pub title: &'static str,
    /// Alt accelerator: `Alt+<accel>` opens this menu directly. The
    /// keymap derives its `Cmd::MenuOpen` bindings from this table.
    pub accel: char,
    /// Dropdown items in display order.
    pub items: &'static [MenuItemDef],
}

/// Every menu, in bar order: Sessions, Compare, Sync, View, Filter, Help.
///
/// The Sync menu lists all eight operations explicitly instead of cycling
/// with `m`/`M`, so the whole strategy set is visible at once; a test
/// keeps the list in sync with [`SyncOperation::ALL`].
pub const MENUS: &[MenuDef] = &[
    MenuDef {
        title: "Sessions",
        accel: 's',
        items: &[
            MenuItemDef {
                label: "open session browser",
                cmd: Cmd::OpenSessionBrowser,
                requires_dir: true,
            },
            MenuItemDef {
                label: "save session",
                cmd: Cmd::StartSaveSession,
                requires_dir: true,
            },
            MenuItemDef {
                label: "close tab",
                cmd: Cmd::CloseTab,
                requires_dir: false,
            },
            MenuItemDef {
                label: "quit",
                cmd: Cmd::Quit,
                requires_dir: false,
            },
        ],
    },
    MenuDef {
        title: "Compare",
        accel: 'c',
        items: &[
            MenuItemDef {
                label: "reload",
                cmd: Cmd::Reload,
                requires_dir: true,
            },
            MenuItemDef {
                label: "compare content on/off",
                cmd: Cmd::ToggleCompareContent,
                requires_dir: true,
            },
            MenuItemDef {
                label: "open entry",
                cmd: Cmd::OpenEntry,
                requires_dir: true,
            },
            MenuItemDef {
                label: "go up",
                cmd: Cmd::GoUp,
                requires_dir: true,
            },
            MenuItemDef {
                label: "next difference",
                cmd: Cmd::NextDiff,
                requires_dir: false,
            },
            MenuItemDef {
                label: "previous difference",
                cmd: Cmd::PrevDiff,
                requires_dir: false,
            },
        ],
    },
    MenuDef {
        title: "Sync",
        accel: 'y',
        items: &[
            MenuItemDef {
                label: "run sync",
                cmd: Cmd::Sync,
                requires_dir: true,
            },
            MenuItemDef {
                label: "mirror left \u{2192} right",
                cmd: Cmd::SetSyncOp(SyncOperation::MirrorLeft),
                requires_dir: true,
            },
            MenuItemDef {
                label: "mirror right \u{2192} left",
                cmd: Cmd::SetSyncOp(SyncOperation::MirrorRight),
                requires_dir: true,
            },
            MenuItemDef {
                label: "update newer",
                cmd: Cmd::SetSyncOp(SyncOperation::UpdateNewer),
                requires_dir: true,
            },
            MenuItemDef {
                label: "update both",
                cmd: Cmd::SetSyncOp(SyncOperation::UpdateBoth),
                requires_dir: true,
            },
            MenuItemDef {
                label: "copy left \u{2192} right",
                cmd: Cmd::SetSyncOp(SyncOperation::CopyLeft),
                requires_dir: true,
            },
            MenuItemDef {
                label: "copy right \u{2192} left",
                cmd: Cmd::SetSyncOp(SyncOperation::CopyRight),
                requires_dir: true,
            },
            MenuItemDef {
                label: "copy newer",
                cmd: Cmd::SetSyncOp(SyncOperation::CopyNewer),
                requires_dir: true,
            },
            MenuItemDef {
                label: "delete orphans",
                cmd: Cmd::SetSyncOp(SyncOperation::DeleteOrphans),
                requires_dir: true,
            },
            MenuItemDef {
                label: "cancel preview",
                cmd: Cmd::Cancel,
                requires_dir: true,
            },
        ],
    },
    MenuDef {
        title: "View",
        accel: 'v',
        items: &[
            MenuItemDef {
                label: "hide identical",
                cmd: Cmd::ToggleHideSame,
                requires_dir: true,
            },
            MenuItemDef {
                label: "expand directory",
                cmd: Cmd::ExpandDir,
                requires_dir: true,
            },
            MenuItemDef {
                label: "collapse directory",
                cmd: Cmd::CollapseDir,
                requires_dir: true,
            },
            MenuItemDef {
                label: "settings",
                cmd: Cmd::OpenSettings,
                requires_dir: true,
            },
        ],
    },
    MenuDef {
        title: "Filter",
        accel: 'f',
        items: &[
            MenuItemDef {
                label: "open filter panel",
                cmd: Cmd::OpenFilterPanel,
                requires_dir: true,
            },
            MenuItemDef {
                label: "toggle same entries",
                cmd: Cmd::ToggleStatusFilter(StatusFilter::Same),
                requires_dir: true,
            },
            MenuItemDef {
                label: "toggle different entries",
                cmd: Cmd::ToggleStatusFilter(StatusFilter::Different),
                requires_dir: true,
            },
            MenuItemDef {
                label: "toggle orphan entries",
                cmd: Cmd::ToggleStatusFilter(StatusFilter::Orphans),
                requires_dir: true,
            },
            MenuItemDef {
                label: "clear filter",
                cmd: Cmd::Cancel,
                requires_dir: true,
            },
        ],
    },
    MenuDef {
        title: "Help",
        accel: 'h',
        items: &[
            MenuItemDef {
                label: "toggle help bar",
                cmd: Cmd::ToggleHelp,
                requires_dir: false,
            },
            MenuItemDef {
                label: "keyboard shortcuts",
                cmd: Cmd::ToggleShortcuts,
                requires_dir: false,
            },
        ],
    },
];

/// Number of menus on the bar.
pub fn count() -> usize {
    MENUS.len()
}

/// Return the menu with the given bar index, if it exists.
pub fn menu_at(bar_index: usize) -> Option<&'static MenuDef> {
    MENUS.get(bar_index)
}

/// The open menu of the menu bar.
///
/// `None` in [`crate::state::AppState::menu`] means no dropdown is open;
/// `Some` means the dropdown of `bar_index` is open with item `item`
/// highlighted. The App shell treats an open menu as owning the keyboard,
/// exactly like a modal input layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuState {
    /// Index of the open menu on the bar.
    pub bar_index: usize,
    /// Index of the highlighted item in that menu's dropdown.
    pub item: usize,
}

impl MenuState {
    /// Open the menu at `bar_index` with its first item highlighted;
    /// `None` for an unknown index.
    pub fn open(bar_index: usize) -> Option<Self> {
        menu_at(bar_index).map(|_| Self { bar_index, item: 0 })
    }

    /// The definition of the open menu.
    pub fn menu(&self) -> Option<&'static MenuDef> {
        menu_at(self.bar_index)
    }

    /// The highlighted dropdown item, if the state addresses one.
    pub fn current(&self) -> Option<&'static MenuItemDef> {
        self.menu()?.items.get(self.item)
    }

    /// Move the item highlight by `delta`, wrapping at both ends.
    pub fn move_item(&mut self, delta: isize) {
        let len = self.menu().map_or(0, |m| m.items.len());
        if len > 0 {
            self.item =
                (self.item as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    /// Switch to the next (`delta > 0`) or previous menu, wrapping around
    /// the bar, and highlight its first item.
    pub fn switch_menu(&mut self, delta: isize) {
        self.bar_index = (self.bar_index as isize + delta)
            .rem_euclid(count() as isize) as usize;
        self.item = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_bar_is_wellformed() {
        assert!(!MENUS.is_empty());
        let mut accels: Vec<(char, &str)> = Vec::new();
        for menu in MENUS {
            assert!(!menu.title.is_empty());
            assert!(
                !menu.items.is_empty(),
                "menu {} lists no items",
                menu.title
            );
            for item in menu.items {
                assert!(!item.label.is_empty());
            }
            assert!(
                accels.iter().all(|(a, _)| *a != menu.accel),
                "menus {} and {} share the accel {}",
                accels
                    .iter()
                    .find(|(a, _)| *a == menu.accel)
                    .map(|(_, t)| *t)
                    .unwrap_or_default(),
                menu.title,
                menu.accel
            );
            accels.push((menu.accel, menu.title));
        }
    }

    #[test]
    fn sync_menu_lists_every_operation() {
        let sync = MENUS
            .iter()
            .find(|m| m.title == "Sync")
            .expect("no Sync menu");
        for op in SyncOperation::ALL {
            assert!(
                sync.items.iter().any(|i| i.cmd == Cmd::SetSyncOp(op)),
                "Sync menu misses the operation {op:?}",
            );
        }
    }

    #[test]
    fn help_menu_lists_both_help_entry_points() {
        let help = MENUS
            .iter()
            .find(|m| m.title == "Help")
            .expect("no Help menu");
        for cmd in [Cmd::ToggleHelp, Cmd::ToggleShortcuts] {
            assert!(
                help.items.iter().any(|i| i.cmd == cmd),
                "Help menu misses the {cmd:?} item",
            );
        }
    }

    #[test]
    fn open_rejects_unknown_menus() {
        assert!(MenuState::open(MENUS.len()).is_none());
        assert_eq!(
            MenuState::open(0),
            Some(MenuState {
                bar_index: 0,
                item: 0
            })
        );
    }

    #[test]
    fn item_navigation_wraps_at_both_ends() {
        let sessions =
            MENUS.iter().position(|m| m.title == "Sessions").unwrap();
        let len = menu_at(sessions).unwrap().items.len();
        let mut menu = MenuState::open(sessions).unwrap();
        // One step up from the first item lands on the last one.
        menu.move_item(-1);
        assert_eq!(menu.item, len - 1);
        // And back down wraps to the first.
        menu.move_item(1);
        assert_eq!(menu.item, 0);
        // Large deltas wrap modulo the item count.
        menu.move_item(len as isize);
        assert_eq!(menu.item, 0);
        menu.move_item(len as isize + 2);
        assert_eq!(menu.item, 2 % len);
    }

    #[test]
    fn menu_navigation_wraps_and_resets_the_item() {
        let mut menu = MenuState::open(0).unwrap();
        menu.move_item(2);
        assert_ne!(menu.item, 0);
        menu.switch_menu(1);
        assert_eq!(menu.bar_index, 1);
        assert_eq!(menu.item, 0);
        menu.switch_menu(-2);
        assert_eq!(menu.bar_index, count() - 1);
        assert_eq!(menu.item, 0);
    }

    #[test]
    fn current_addresses_the_highlighted_item() {
        let menu = MenuState::open(1).unwrap();
        assert_eq!(menu.current(), menu_at(1).map(|m| &m.items[0]));
        let mut menu = menu;
        menu.move_item(1);
        assert_eq!(menu.current(), menu_at(1).map(|m| &m.items[1]));
    }
}
