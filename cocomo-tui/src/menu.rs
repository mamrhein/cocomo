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
//! every bar title, Alt accelerator, and dropdown entry, and each leaf
//! carries the [`Cmd`] its keyboard counterpart executes, so the menu bar
//! and the keymap cannot drift apart.
//!
//! A dropdown row is a [`MenuEntry`]: a command leaf, a submenu parent with
//! its children, or a group separator. Every [`MenuDef`] and [`MenuItemDef`]
//! carries a [`MenuViews`] mask of the session views it shows in, so the
//! bar can be composed per tab ([`bar_indices`]) and view-restricted items
//! can be dimmed and skipped elsewhere.
//!
//! [`MenuState`] encodes the open menu, its highlighted entry, and the open
//! submenu flyout; its navigation operations are pure, so the whole dropdown
//! state machine is unit-testable without a terminal. The controller is the
//! only caller that applies them to the global atom.

use cocomo_lib::filter::DisplayFilters;

use crate::{
    keys::Cmd,
    state::{SessionContent, StatusFilter, TransferChoice, TransferOp},
};

/// The session view a menu bar is composed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuView {
    /// Folder comparison tab.
    Dir,
    /// Text diff tab.
    Text,
}

impl MenuView {
    /// Bit of this view in a [`MenuViews`] mask.
    const fn bit(self) -> u8 {
        match self {
            Self::Dir => 1,
            Self::Text => 2,
        }
    }

    /// The menu view the session `content` belongs to.
    pub fn from_content(content: &SessionContent) -> Self {
        match content {
            SessionContent::Dir(_) => Self::Dir,
            SessionContent::Text(_) => Self::Text,
        }
    }
}

/// Bitmask of the [`MenuView`]s a menu or item shows in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuViews(u8);

impl MenuViews {
    /// Folder comparison view only.
    pub const DIR: Self = Self(MenuView::Dir.bit());
    /// Text diff view only.
    pub const TEXT: Self = Self(MenuView::Text.bit());
    /// Every session view; also stays lit on the welcome screen.
    pub const ALL: Self = Self(MenuView::Dir.bit() | MenuView::Text.bit());

    /// Whether `view` is part of this mask.
    pub const fn contains(self, view: MenuView) -> bool {
        self.0 & view.bit() != 0
    }
}

/// A leaf entry of a dropdown menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuItemDef {
    /// Label shown in the dropdown. Toggle items store only their noun
    /// phrase ("equal items"); [`item_label`] prefixes it with the current
    /// `show`/`hide` verb.
    pub label: &'static str,
    /// Command run when the item is confirmed. This is the same command
    /// the keyboard counterpart executes, so both entry points share one
    /// dispatch path.
    pub cmd: Cmd,
    /// Views the item is available in. Outside them the item renders
    /// dimmed and confirming it is a no-op.
    pub views: MenuViews,
}

impl MenuItemDef {
    /// Whether the item can run while `view` is active (`None` on the
    /// welcome screen, where only view-independent items stay lit).
    pub fn available(&self, view: Option<MenuView>) -> bool {
        view.is_some_and(|v| self.views.contains(v))
            || self.views == MenuViews::ALL
    }
}

/// One row of a dropdown: a command leaf, a submenu parent, or a group
/// separator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuEntry {
    /// A command leaf.
    Item(MenuItemDef),
    /// A parent row whose children open in a flyout.
    SubMenu {
        label: &'static str,
        children: &'static [MenuEntry],
    },
    /// A non-selectable group divider.
    Separator,
}

impl MenuEntry {
    /// The command of a leaf row, `None` for parents and separators.
    pub fn cmd(&self) -> Option<Cmd> {
        match self {
            Self::Item(item) => Some(item.cmd),
            _ => None,
        }
    }

    /// The children of a submenu parent, `None` for other rows.
    pub fn children(&self) -> Option<&'static [MenuEntry]> {
        match self {
            Self::SubMenu { children, .. } => Some(children),
            _ => None,
        }
    }

    /// Whether the keyboard highlight can rest on this row. Separators
    /// are skipped by navigation.
    pub fn selectable(&self) -> bool {
        !matches!(self, Self::Separator)
    }

    /// Whether the row can run while `view` is active. A submenu is
    /// available when at least one child is.
    pub fn available(&self, view: Option<MenuView>) -> bool {
        match self {
            Self::Item(item) => item.available(view),
            Self::SubMenu { children, .. } => {
                children.iter().any(|child| child.available(view))
            }
            Self::Separator => true,
        }
    }
}

/// A top-level menu bar entry with its dropdown entries.
#[derive(Clone, Copy, Debug)]
pub struct MenuDef {
    /// Menu bar title.
    pub title: &'static str,
    /// Alt accelerator: `Alt+<accel>` opens this menu directly. The
    /// keymap derives its `Cmd::MenuOpen` bindings from this table.
    pub accel: char,
    /// Views this menu appears on the bar in.
    pub views: MenuViews,
    /// Dropdown entries in display order.
    pub entries: &'static [MenuEntry],
}

impl MenuDef {
    /// Whether the menu shows on the bar of `view` (`None` is the welcome
    /// screen, which shows every menu).
    pub fn shows_in(&self, view: Option<MenuView>) -> bool {
        view.is_none_or(|v| self.views.contains(v))
    }
}

/// Shorthand for a leaf entry in the [`MENUS`] table.
const fn item(label: &'static str, cmd: Cmd, views: MenuViews) -> MenuEntry {
    MenuEntry::Item(MenuItemDef { label, cmd, views })
}

/// Every menu, in bar order: Sessions, Compare, Sync, View, Filter, Help.
///
/// The Sync menu offers the three directional sync commands; each plans a
/// dry-run and shows a confirmation dialog before anything changes on
/// disk. The five per-entry commands then run a transfer on the current
/// selection directly, with the same enablement rules as the accelerators
/// of the copy/delete/move dialogs.
pub const MENUS: &[MenuDef] = &[
    MenuDef {
        title: "Sessions",
        accel: 's',
        views: MenuViews::ALL,
        entries: &[
            item(
                "open session browser",
                Cmd::OpenSessionBrowser,
                MenuViews::DIR,
            ),
            item("save session", Cmd::StartSaveSession, MenuViews::DIR),
            item("close tab", Cmd::CloseTab, MenuViews::ALL),
            item("quit", Cmd::Quit, MenuViews::ALL),
        ],
    },
    MenuDef {
        title: "Compare",
        accel: 'c',
        views: MenuViews::ALL,
        entries: &[
            item("reload", Cmd::Reload, MenuViews::DIR),
            item(
                "compare content on/off",
                Cmd::ToggleCompareContent,
                MenuViews::DIR,
            ),
            item("open entry", Cmd::OpenEntry, MenuViews::DIR),
            item("go up", Cmd::GoUp, MenuViews::DIR),
            item("next difference", Cmd::NextDiff, MenuViews::ALL),
            item("previous difference", Cmd::PrevDiff, MenuViews::ALL),
        ],
    },
    MenuDef {
        title: "Sync",
        accel: 'y',
        views: MenuViews::ALL,
        entries: &[
            item(
                "sync left \u{2192} right",
                Cmd::SyncLeftRight,
                MenuViews::DIR,
            ),
            item(
                "sync right \u{2192} left",
                Cmd::SyncRightLeft,
                MenuViews::DIR,
            ),
            item("sync both", Cmd::SyncBoth, MenuViews::DIR),
            // Per-entry transfers; not dir-only, since a text session can
            // transfer its two files just like `c`/`d`/`v` do.
            item(
                "move left \u{2192} right",
                Cmd::TransferExecute(TransferOp::Move, TransferChoice::Left),
                MenuViews::ALL,
            ),
            item(
                "move right \u{2192} left",
                Cmd::TransferExecute(TransferOp::Move, TransferChoice::Right),
                MenuViews::ALL,
            ),
            item(
                "delete left",
                Cmd::TransferExecute(TransferOp::Delete, TransferChoice::Left),
                MenuViews::ALL,
            ),
            item(
                "delete right",
                Cmd::TransferExecute(
                    TransferOp::Delete,
                    TransferChoice::Right,
                ),
                MenuViews::ALL,
            ),
            item(
                "delete both",
                Cmd::TransferExecute(TransferOp::Delete, TransferChoice::Both),
                MenuViews::ALL,
            ),
            item("cancel preview", Cmd::Cancel, MenuViews::DIR),
        ],
    },
    MenuDef {
        title: "View",
        accel: 'v',
        views: MenuViews::ALL,
        entries: &[
            item("expand directory", Cmd::ExpandDir, MenuViews::DIR),
            item("collapse directory", Cmd::CollapseDir, MenuViews::DIR),
            item("expand all", Cmd::ExpandAllDirs, MenuViews::DIR),
            item("collapse all", Cmd::CollapseAllDirs, MenuViews::DIR),
        ],
    },
    MenuDef {
        title: "Filter",
        accel: 'f',
        views: MenuViews::ALL,
        entries: &[
            item("open filter panel", Cmd::OpenFilterPanel, MenuViews::DIR),
            item("hidden items", Cmd::ToggleHidden, MenuViews::DIR),
            item(
                "equal items",
                Cmd::ToggleStatusFilter(StatusFilter::Equal),
                MenuViews::DIR,
            ),
            item(
                "different items",
                Cmd::ToggleStatusFilter(StatusFilter::Different),
                MenuViews::DIR,
            ),
            item(
                "left newer items",
                Cmd::ToggleStatusFilter(StatusFilter::LeftNewer),
                MenuViews::DIR,
            ),
            item(
                "right newer items",
                Cmd::ToggleStatusFilter(StatusFilter::RightNewer),
                MenuViews::DIR,
            ),
            item(
                "orphan items",
                Cmd::ToggleStatusFilter(StatusFilter::Orphans),
                MenuViews::DIR,
            ),
            item(
                "left-only items",
                Cmd::ToggleStatusFilter(StatusFilter::LeftOnly),
                MenuViews::DIR,
            ),
            item(
                "right-only items",
                Cmd::ToggleStatusFilter(StatusFilter::RightOnly),
                MenuViews::DIR,
            ),
            item("show all", Cmd::ShowAllFilters, MenuViews::DIR),
        ],
    },
    MenuDef {
        title: "Help",
        accel: 'h',
        views: MenuViews::ALL,
        entries: &[
            item("toggle help bar", Cmd::ToggleHelp, MenuViews::ALL),
            item("keyboard shortcuts", Cmd::ToggleShortcuts, MenuViews::ALL),
        ],
    },
];

/// Number of menus on the bar.
pub fn count() -> usize {
    MENUS.len()
}

/// Bar indices (into [`MENUS`]) of the menus shown for `view` (`None` is
/// the welcome screen, which shows the full bar).
pub fn bar_indices(view: Option<MenuView>) -> Vec<usize> {
    bar_indices_in(MENUS, view)
}

fn bar_indices_in(menus: &[MenuDef], view: Option<MenuView>) -> Vec<usize> {
    menus
        .iter()
        .enumerate()
        .filter_map(|(index, def)| def.shows_in(view).then_some(index))
        .collect()
}

/// The dropdown label of `item`, resolved against the active session's
/// display filters.
///
/// Visibility toggle items read `show …` while their group is filtered
/// out and `hide …` while it is shown (a partial umbrella — one sub-bit
/// shown, its sibling hidden — counts as filtered out). Every other item
/// renders its static label. Without a directory session (`display` is
/// `None`) the toggles advertise `show …`, matching their dimmed
/// no-op state.
pub fn item_label(
    item: &MenuItemDef,
    display: Option<&DisplayFilters>,
) -> String {
    let shown = match item.cmd {
        Cmd::ToggleHidden => display.is_some_and(|d| d.show_hidden),
        Cmd::ToggleStatusFilter(filter) => {
            display.is_some_and(|d| filter.is_shown(d.show_statuses))
        }
        _ => return item.label.to_string(),
    };
    format!("{} {}", if shown { "hide" } else { "show" }, item.label)
}

/// Return the menu with the given bar index, if it exists.
pub fn menu_at(bar_index: usize) -> Option<&'static MenuDef> {
    MENUS.get(bar_index)
}

/// Index of the menu whose accelerator is `c` (case-insensitive).
///
/// Used by the shell's `Esc`-prefix fallback (`Esc s` as a stand-in for
/// `Alt+S`) in terminals that cannot report the Alt modifier.
pub fn menu_index_by_accel(c: char) -> Option<usize> {
    MENUS.iter().position(|m| m.accel.eq_ignore_ascii_case(&c))
}

/// The open menu of the menu bar.
///
/// `None` in [`crate::state::AppState::menu`] means no dropdown is open;
/// `Some` means the dropdown of `bar_index` is open with entry `item`
/// highlighted. While `sub` is `Some`, the submenu flyout of that entry is
/// open with child `sub` highlighted. The App shell treats an open menu as
/// owning the keyboard, exactly like a modal input layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MenuState {
    /// Index of the open menu on the bar.
    pub bar_index: usize,
    /// Index of the highlighted entry in that menu's dropdown.
    pub item: usize,
    /// Index of the highlighted child in the open submenu flyout, `None`
    /// while no flyout is open.
    pub sub: Option<usize>,
}

impl MenuState {
    /// Open the menu at `bar_index` with its first selectable entry
    /// highlighted; `None` for an unknown index.
    pub fn open(bar_index: usize) -> Option<Self> {
        open_in(MENUS, bar_index)
    }

    /// The definition of the open menu.
    pub fn menu(&self) -> Option<&'static MenuDef> {
        menu_at(self.bar_index)
    }

    /// The highlighted top-level dropdown entry, regardless of whether a
    /// submenu flyout is open below it.
    pub fn current_entry(&self) -> Option<&'static MenuEntry> {
        self.menu()?.entries.get(self.item)
    }

    /// The children of the highlighted submenu parent, `None` when that
    /// row is not a parent. Callers gate on [`Self::sub`] to know whether
    /// the flyout is actually open.
    pub fn sub_entries(&self) -> Option<&'static [MenuEntry]> {
        self.current_entry()?.children()
    }

    /// The highlighted command leaf, if the state addresses one. Returns
    /// `None` on separators and on a submenu parent whose flyout is not
    /// open yet.
    pub fn current(&self) -> Option<&'static MenuItemDef> {
        current_in(MENUS, self)
    }

    /// Move the item highlight by `delta`, skipping separators and
    /// wrapping at both ends. While a submenu flyout is open, the
    /// highlight moves within its children instead.
    pub fn move_item(&mut self, delta: isize) {
        move_item_in(MENUS, self, delta);
    }

    /// Open the submenu flyout of the highlighted parent row; `false`
    /// when that row has no children or the flyout is already open.
    pub fn enter_submenu(&mut self) -> bool {
        enter_submenu_in(MENUS, self)
    }

    /// Close the open submenu flyout; `false` when none was open.
    pub fn leave_submenu(&mut self) -> bool {
        self.sub.take().is_some()
    }

    /// Switch to the next (`delta > 0`) or previous menu in `bar` (the
    /// bar indices of the current view, from [`bar_indices`]), wrapping
    /// around the bar, and highlight its first selectable entry.
    pub fn switch_menu(&mut self, delta: isize, bar: &[usize]) {
        switch_menu_in(MENUS, self, delta, bar);
    }
}

fn open_in(menus: &'static [MenuDef], bar_index: usize) -> Option<MenuState> {
    let def = menus.get(bar_index)?;
    Some(MenuState {
        bar_index,
        item: first_selectable(def.entries).unwrap_or(0),
        sub: None,
    })
}

fn current_in(
    menus: &'static [MenuDef],
    menu: &MenuState,
) -> Option<&'static MenuItemDef> {
    let def = menus.get(menu.bar_index)?;
    let entry = def.entries.get(menu.item)?;
    let entry = match menu.sub {
        Some(sub) => entry.children()?.get(sub)?,
        None => entry,
    };
    match entry {
        MenuEntry::Item(item) => Some(item),
        _ => None,
    }
}

fn move_item_in(
    menus: &'static [MenuDef],
    menu: &mut MenuState,
    delta: isize,
) {
    let Some(def) = menus.get(menu.bar_index) else {
        return;
    };
    if let Some(sub) = menu.sub {
        if let Some(children) =
            def.entries.get(menu.item).and_then(MenuEntry::children)
        {
            menu.sub = Some(step(children, sub, delta));
            return;
        }
        // The highlighted row no longer has a flyout; drop the stale one
        // and navigate the dropdown itself.
        menu.sub = None;
    }
    menu.item = step(def.entries, menu.item, delta);
}

fn enter_submenu_in(menus: &'static [MenuDef], menu: &mut MenuState) -> bool {
    if menu.sub.is_some() {
        return false;
    }
    let Some(def) = menus.get(menu.bar_index) else {
        return false;
    };
    let Some(children) =
        def.entries.get(menu.item).and_then(MenuEntry::children)
    else {
        return false;
    };
    menu.sub = Some(first_selectable(children).unwrap_or(0));
    true
}

fn switch_menu_in(
    menus: &'static [MenuDef],
    menu: &mut MenuState,
    delta: isize,
    bar: &[usize],
) {
    if bar.is_empty() {
        return;
    }
    menu.bar_index = match bar.iter().position(|&i| i == menu.bar_index) {
        Some(pos) => {
            bar[(pos as isize + delta).rem_euclid(bar.len() as isize) as usize]
        }
        // The bar no longer contains the open menu (its view changed
        // while the dropdown was open); snap back to its first slot.
        None => bar[0],
    };
    menu.item = menus
        .get(menu.bar_index)
        .and_then(|def| first_selectable(def.entries))
        .unwrap_or(0);
    menu.sub = None;
}

fn first_selectable(entries: &[MenuEntry]) -> Option<usize> {
    entries.iter().position(MenuEntry::selectable)
}

/// Advance a highlight by `delta` selectable entries, wrapping around and
/// skipping separators.
fn step(entries: &[MenuEntry], from: usize, delta: isize) -> usize {
    let selectable = entries.iter().filter(|e| e.selectable()).count();
    if selectable == 0 {
        return from;
    }
    // A negative `delta` is equivalent to walking forward its remainder
    // modulo the selectable count, so one forward loop covers both.
    let steps = delta.rem_euclid(selectable as isize);
    let mut index = from % entries.len();
    for _ in 0..steps {
        loop {
            index = (index + 1) % entries.len();
            if entries[index].selectable() {
                break;
            }
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use cocomo_lib::DirEntryStatus;

    use super::*;

    /// Synthetic table exercising separators, submenus, and view masks.
    /// The navigation cores resolve against an injected table so the state
    /// machine is tested independently from the (currently flat) real
    /// [`MENUS`].
    static TEST_MENUS: &[MenuDef] = &[
        MenuDef {
            title: "Alpha",
            accel: 'a',
            views: MenuViews::ALL,
            entries: &[
                item("a one", Cmd::Quit, MenuViews::ALL),
                MenuEntry::Separator,
                MenuEntry::SubMenu {
                    label: "group",
                    children: &[
                        item("g dir", Cmd::CloseTab, MenuViews::DIR),
                        item("g all", Cmd::ToggleHelp, MenuViews::ALL),
                    ],
                },
                MenuEntry::Separator,
                item("a two", Cmd::ToggleShortcuts, MenuViews::ALL),
            ],
        },
        MenuDef {
            title: "Beta",
            accel: 'b',
            views: MenuViews::DIR,
            entries: &[item("b one", Cmd::MenuClose, MenuViews::ALL)],
        },
    ];

    /// The leaf with `cmd`, panicking when `entries` do not list it.
    fn leaf(entries: &[MenuEntry], cmd: Cmd) -> MenuItemDef {
        entries
            .iter()
            .find_map(|entry| match entry {
                MenuEntry::Item(item) if item.cmd == cmd => Some(*item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no item for {cmd:?}"))
    }

    /// Whether any top-level entry is the leaf `cmd`.
    fn lists(entries: &[MenuEntry], cmd: Cmd) -> bool {
        entries.iter().any(|entry| entry.cmd() == Some(cmd))
    }

    fn check_labels(entries: &[MenuEntry]) {
        for entry in entries {
            match entry {
                MenuEntry::Item(item) => assert!(!item.label.is_empty()),
                MenuEntry::SubMenu { label, children } => {
                    assert!(!label.is_empty());
                    assert!(
                        !children.is_empty(),
                        "submenu {label} lists no children"
                    );
                    check_labels(children);
                }
                MenuEntry::Separator => {}
            }
        }
    }

    #[test]
    fn menu_bar_is_wellformed() {
        assert!(!MENUS.is_empty());
        let mut accels: Vec<(char, &str)> = Vec::new();
        for menu in MENUS {
            assert!(!menu.title.is_empty());
            assert!(
                !menu.entries.is_empty(),
                "menu {} lists no entries",
                menu.title
            );
            assert!(
                menu.entries.iter().any(MenuEntry::selectable),
                "menu {} lists no selectable entry",
                menu.title
            );
            check_labels(menu.entries);
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
    fn sync_menu_lists_the_directional_commands() {
        let sync = MENUS
            .iter()
            .find(|m| m.title == "Sync")
            .expect("no Sync menu");
        for cmd in [Cmd::SyncLeftRight, Cmd::SyncRightLeft, Cmd::SyncBoth] {
            assert!(
                lists(sync.entries, cmd),
                "Sync menu misses the command {cmd:?}",
            );
        }
    }

    #[test]
    fn sync_menu_lists_the_per_entry_transfers() {
        let sync = MENUS
            .iter()
            .find(|m| m.title == "Sync")
            .expect("no Sync menu");
        for cmd in [
            Cmd::TransferExecute(TransferOp::Move, TransferChoice::Left),
            Cmd::TransferExecute(TransferOp::Move, TransferChoice::Right),
            Cmd::TransferExecute(TransferOp::Delete, TransferChoice::Left),
            Cmd::TransferExecute(TransferOp::Delete, TransferChoice::Right),
            Cmd::TransferExecute(TransferOp::Delete, TransferChoice::Both),
        ] {
            assert!(
                lists(sync.entries, cmd),
                "Sync menu misses the per-entry transfer {cmd:?}",
            );
        }
    }

    #[test]
    fn view_menu_lists_the_tree_commands() {
        let view = MENUS
            .iter()
            .find(|m| m.title == "View")
            .expect("no View menu");
        for cmd in [
            Cmd::ExpandDir,
            Cmd::CollapseDir,
            Cmd::ExpandAllDirs,
            Cmd::CollapseAllDirs,
        ] {
            assert!(
                lists(view.entries, cmd),
                "View menu misses the {cmd:?} item",
            );
        }
    }

    #[test]
    fn filter_menu_lists_every_visibility_command() {
        let filter = MENUS
            .iter()
            .find(|m| m.title == "Filter")
            .expect("no Filter menu");
        for cmd in [
            Cmd::OpenFilterPanel,
            Cmd::ToggleHidden,
            Cmd::ToggleStatusFilter(StatusFilter::Equal),
            Cmd::ToggleStatusFilter(StatusFilter::Different),
            Cmd::ToggleStatusFilter(StatusFilter::LeftNewer),
            Cmd::ToggleStatusFilter(StatusFilter::RightNewer),
            Cmd::ToggleStatusFilter(StatusFilter::Orphans),
            Cmd::ToggleStatusFilter(StatusFilter::LeftOnly),
            Cmd::ToggleStatusFilter(StatusFilter::RightOnly),
            Cmd::ShowAllFilters,
        ] {
            assert!(
                lists(filter.entries, cmd),
                "Filter menu misses the {cmd:?} item",
            );
        }
    }

    #[test]
    fn filter_items_switch_between_show_and_hide() {
        let filter = MENUS
            .iter()
            .find(|m| m.title == "Filter")
            .expect("no Filter menu");
        let hidden = leaf(filter.entries, Cmd::ToggleHidden);
        let different = leaf(
            filter.entries,
            Cmd::ToggleStatusFilter(StatusFilter::Different),
        );

        // Shown groups offer "hide …", hidden ones "show …".
        let all = DisplayFilters::all();
        assert_eq!(item_label(&hidden, Some(&all)), "hide hidden items");
        assert_eq!(item_label(&different, Some(&all)), "hide different items");
        let off = DisplayFilters {
            show_statuses: DirEntryStatus::all(),
            show_hidden: false,
            suppress_content_filters: false,
        };
        assert_eq!(item_label(&hidden, Some(&off)), "show hidden items");

        // A partial umbrella reads unchecked, so the item offers "show".
        let partial = DisplayFilters {
            show_statuses: DirEntryStatus::all() - DirEntryStatus::RightNewer,
            show_hidden: true,
            suppress_content_filters: false,
        };
        assert_eq!(
            item_label(&different, Some(&partial)),
            "show different items"
        );

        // Static items keep their label; without a session the toggles
        // advertise "show".
        let open = leaf(filter.entries, Cmd::OpenFilterPanel);
        assert_eq!(item_label(&open, None), "open filter panel");
        assert_eq!(item_label(&hidden, None), "show hidden items");
    }

    #[test]
    fn help_menu_lists_both_help_entry_points() {
        let help = MENUS
            .iter()
            .find(|m| m.title == "Help")
            .expect("no Help menu");
        for cmd in [Cmd::ToggleHelp, Cmd::ToggleShortcuts] {
            assert!(
                lists(help.entries, cmd),
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
                item: 0,
                sub: None,
            })
        );
    }

    #[test]
    fn every_accel_resolves_to_its_menu() {
        for (index, menu) in MENUS.iter().enumerate() {
            assert_eq!(menu_index_by_accel(menu.accel), Some(index));
            assert_eq!(
                menu_index_by_accel(menu.accel.to_ascii_uppercase()),
                Some(index)
            );
        }
        assert_eq!(menu_index_by_accel('x'), None);
    }

    #[test]
    fn item_navigation_wraps_at_both_ends() {
        let sessions =
            MENUS.iter().position(|m| m.title == "Sessions").unwrap();
        let len = menu_at(sessions)
            .unwrap()
            .entries
            .iter()
            .filter(|e| e.selectable())
            .count();
        let mut menu = MenuState::open(sessions).unwrap();
        // One step up from the first item lands on the last one.
        menu.move_item(-1);
        assert_eq!(menu.item, len - 1);
        // And back down wraps to the first.
        menu.move_item(1);
        assert_eq!(menu.item, 0);
        // Large deltas wrap modulo the selectable item count.
        menu.move_item(len as isize);
        assert_eq!(menu.item, 0);
        menu.move_item(len as isize + 2);
        assert_eq!(menu.item, 2 % len);
    }

    #[test]
    fn menu_navigation_wraps_and_resets_the_item() {
        let bar = bar_indices(None);
        let mut menu = MenuState::open(0).unwrap();
        menu.move_item(2);
        assert_ne!(menu.item, 0);
        menu.switch_menu(1, &bar);
        assert_eq!(menu.bar_index, 1);
        assert_eq!(menu.item, 0);
        menu.switch_menu(-2, &bar);
        assert_eq!(menu.bar_index, count() - 1);
        assert_eq!(menu.item, 0);
    }

    #[test]
    fn current_addresses_the_highlighted_item() {
        let menu = MenuState::open(1).unwrap();
        let MenuEntry::Item(first) = MENUS[1].entries[0] else {
            panic!("first Compare entry is not a leaf");
        };
        assert_eq!(menu.current().copied(), Some(first));
        let mut menu = menu;
        menu.move_item(1);
        let MenuEntry::Item(second) = MENUS[1].entries[1] else {
            panic!("second Compare entry is not a leaf");
        };
        assert_eq!(menu.current().copied(), Some(second));
    }

    #[test]
    fn separators_are_skipped_by_navigation() {
        let mut menu = open_in(TEST_MENUS, 0).unwrap();
        assert_eq!(menu.item, 0);
        move_item_in(TEST_MENUS, &mut menu, 1);
        assert_eq!(menu.item, 2, "a separator row got highlighted");
        move_item_in(TEST_MENUS, &mut menu, 1);
        assert_eq!(menu.item, 4);
        // Wrapping past the last entry lands on the first selectable one.
        move_item_in(TEST_MENUS, &mut menu, 1);
        assert_eq!(menu.item, 0);
        // Backward stepping skips separators too.
        move_item_in(TEST_MENUS, &mut menu, -1);
        assert_eq!(menu.item, 4);
    }

    #[test]
    fn submenu_navigation_drives_the_flyout() {
        let mut menu = open_in(TEST_MENUS, 0).unwrap();
        menu.item = 2;
        // The parent row highlights but addresses no command.
        assert!(current_in(TEST_MENUS, &menu).is_none());
        assert!(enter_submenu_in(TEST_MENUS, &mut menu));
        assert_eq!(menu.sub, Some(0));
        assert_eq!(
            current_in(TEST_MENUS, &menu).map(|i| i.cmd),
            Some(Cmd::CloseTab)
        );
        // Stepping moves within the children only, wrapping at the ends.
        move_item_in(TEST_MENUS, &mut menu, 1);
        assert_eq!(menu.sub, Some(1));
        move_item_in(TEST_MENUS, &mut menu, 1);
        assert_eq!(menu.sub, Some(0));
        // A second enter is inert while the flyout is open.
        assert!(!enter_submenu_in(TEST_MENUS, &mut menu));
        // Leaving closes the flyout and reports whether one was open; the
        // parent row stays highlighted.
        assert!(menu.leave_submenu());
        assert!(!menu.leave_submenu());
        assert_eq!(menu.item, 2);
    }

    #[test]
    fn bar_composition_follows_the_view_mask() {
        assert_eq!(
            bar_indices_in(TEST_MENUS, Some(MenuView::Dir)),
            vec![0, 1]
        );
        assert_eq!(bar_indices_in(TEST_MENUS, Some(MenuView::Text)), vec![0]);
        // The welcome screen shows the full bar.
        assert_eq!(bar_indices_in(TEST_MENUS, None), vec![0, 1]);
        // The real table currently shows every menu in every view.
        let full: Vec<usize> = (0..MENUS.len()).collect();
        for view in [None, Some(MenuView::Dir), Some(MenuView::Text)] {
            assert_eq!(bar_indices(view), full);
        }
    }

    #[test]
    fn switch_menu_stays_within_the_view_bar() {
        let mut menu = open_in(TEST_MENUS, 0).unwrap();
        // A one-menu bar cannot be left.
        switch_menu_in(TEST_MENUS, &mut menu, 1, &[0]);
        assert_eq!(menu.bar_index, 0);
        let bar = bar_indices_in(TEST_MENUS, Some(MenuView::Dir));
        switch_menu_in(TEST_MENUS, &mut menu, 1, &bar);
        assert_eq!(menu.bar_index, 1);
        switch_menu_in(TEST_MENUS, &mut menu, 1, &bar);
        assert_eq!(menu.bar_index, 0);
        // Switching closes an open submenu and highlights the first
        // selectable entry of the new menu.
        menu.item = 2;
        assert!(enter_submenu_in(TEST_MENUS, &mut menu));
        switch_menu_in(TEST_MENUS, &mut menu, 1, &bar);
        assert_eq!(menu.bar_index, 1);
        assert_eq!(menu.item, 0);
        assert_eq!(menu.sub, None);
    }

    #[test]
    fn items_are_available_only_in_their_views() {
        let MenuEntry::SubMenu { children, .. } = TEST_MENUS[0].entries[2]
        else {
            panic!("entry 2 is not a submenu");
        };
        let dir_only = leaf(children, Cmd::CloseTab);
        assert!(dir_only.available(Some(MenuView::Dir)));
        assert!(!dir_only.available(Some(MenuView::Text)));
        // On the welcome screen only view-independent items stay lit.
        assert!(!dir_only.available(None));
        let any_view = leaf(children, Cmd::ToggleHelp);
        assert!(any_view.available(Some(MenuView::Dir)));
        assert!(any_view.available(Some(MenuView::Text)));
        assert!(any_view.available(None));
        // A submenu is available when at least one child is.
        let group = MenuEntry::SubMenu {
            label: "group",
            children,
        };
        assert!(group.available(Some(MenuView::Dir)));
        assert!(group.available(Some(MenuView::Text)));
    }
}
