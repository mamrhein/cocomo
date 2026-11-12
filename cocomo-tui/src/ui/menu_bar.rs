// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Top menu bar, its dropdowns, and submenu flyouts.
//!
//! Titles and entries come from [`crate::menu::MENUS`]; each title renders
//! with its Alt accelerator underlined, and [`MenuDropdown`] draws the open
//! menu as a positioned overlay ([`MenuFlyout`] draws its open submenu).
//! The bar, the dropdown and the flyout share the geometry helpers here
//! ([`menu_title`], [`dropdown_rect`], [`flyout_rect`],
//! [`dropdown_hit_item`]), so the rendered layout and the mouse hit-testing
//! in the App shell cannot drift apart.

use cocomo_lib::filter::DisplayFilters;
use ratatui_kit::{
    crossterm::event::MouseButton,
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction, Position, Rect},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Tabs,
    },
};

use super::{
    frame::{tabs_hit_item, tabs_item_x},
    mouse_enabled,
    theme::CocomoTheme,
};
use crate::{
    keys::key_hint,
    menu::{MenuDef, MenuEntry, MenuState, MenuView, item_label},
};

/// Width of the `" | "` divider between menu items.
pub(super) const DIVIDER_WIDTH: u16 = 3;

/// Render a menu bar title with its accelerator letter underlined.
///
/// The underline marks the letter of <kbd>Alt</kbd>+<kbd>letter</kbd>; the
/// title itself gets the theme's highlight style while its dropdown is
/// open.
pub(super) fn menu_title(menu: &MenuDef) -> Line<'static> {
    let mut accented = false;
    Line::from(
        menu.title
            .chars()
            .map(|c| {
                // Only the first occurrence is the accelerator: underlining
                // every match would streak titles such as "Sessions".
                let mark = !accented && c.eq_ignore_ascii_case(&menu.accel);
                accented |= mark;
                let style = if mark {
                    Style::new().add_modifier(Modifier::UNDERLINED)
                } else {
                    Style::default()
                };
                Span::styled(c.to_string(), style)
            })
            .collect::<Vec<Span<'static>>>(),
    )
}

/// The dropdown label of `entry`, `None` for separators. Submenu parents
/// carry a right-pointing pointer to advertise their flyout.
fn entry_label(
    entry: &MenuEntry,
    display: Option<&DisplayFilters>,
) -> Option<String> {
    match entry {
        MenuEntry::Item(item) => Some(item_label(item, display)),
        MenuEntry::SubMenu { label, .. } => Some(format!("{label} \u{25b8}")),
        MenuEntry::Separator => None,
    }
}

/// Column widths of the label and key-hint parts of a dropdown listing
/// `entries` (separators do not contribute).
///
/// Labels are measured through [`entry_label`] because visibility toggles
/// render a state-dependent `show`/`hide` prefix; `display` is the active
/// session's filter state (`None` without a directory session).
fn content_widths(
    entries: &[MenuEntry],
    display: Option<&DisplayFilters>,
) -> (u16, u16) {
    let mut label_width = 0u16;
    let mut hint_width = 0u16;
    for entry in entries {
        if let Some(label) = entry_label(entry, display) {
            label_width = label_width.max(label.chars().count() as u16);
        }
        if let Some(hint) = entry.cmd().and_then(key_hint) {
            hint_width = hint_width.max(hint.chars().count() as u16);
        }
    }
    (label_width, hint_width)
}

/// Width of the hint column (including its two-cell gap), `0` when no
/// entry has a hint.
fn hint_columns(hint_width: u16) -> u16 {
    if hint_width > 0 { hint_width + 2 } else { 0 }
}

/// The screen rectangle the dropdown of `def` is drawn in.
///
/// `titles` are the rendered bar titles, `bar_pos` the index of `def`
/// among them (the visible bar may be a subset of [`crate::menu::MENUS`]);
/// `bar_origin` is the screen position of the menu bar's first cell. The
/// dropdown is anchored under its title's text (one cell past the tab's
/// left padding) and opens directly below the bar. `display` is the active
/// session's filter state, needed to size the state-dependent Filter
/// labels.
pub(super) fn dropdown_rect(
    titles: &[Line<'static>],
    def: &MenuDef,
    bar_pos: usize,
    bar_origin: Position,
    display: Option<&DisplayFilters>,
) -> Rect {
    let (label_width, hint_width) = content_widths(def.entries, display);
    // The row itself is `▸ label… hint `, the box adds two border columns.
    Rect {
        x: bar_origin.x
            + tabs_item_x(titles, bar_pos, DIVIDER_WIDTH).unwrap_or(0)
            + 1,
        y: bar_origin.y + 1,
        width: label_width + hint_columns(hint_width) + 5,
        height: def.entries.len() as u16 + 2,
    }
}

/// The screen rectangle the flyout of `children` is drawn in.
///
/// `parent_row` is the dropdown row of the highlighted submenu parent (as
/// [`MenuState::item`], counting from the dropdown's top border row); the
/// flyout opens to the right of the dropdown, aligned with its parent row.
pub(super) fn flyout_rect(
    children: &[MenuEntry],
    parent_row: usize,
    dropdown: Rect,
    display: Option<&DisplayFilters>,
) -> Rect {
    let (label_width, hint_width) = content_widths(children, display);
    Rect {
        x: dropdown.x + dropdown.width - 1,
        y: dropdown.y + parent_row as u16,
        width: label_width + hint_columns(hint_width) + 5,
        height: children.len() as u16 + 2,
    }
}

/// The dropdown item under `pos`, `None` outside the list.
///
/// The title border row and the bottom border row are not items, so a click
/// there only closes the dropdown. Works for both dropdown and flyout
/// boxes.
pub(super) fn dropdown_hit_item(rect: Rect, pos: Position) -> Option<usize> {
    if !rect.contains(pos) {
        return None;
    }
    let row = pos.y.checked_sub(rect.y)?;
    if row == 0 || row.saturating_add(1) >= rect.height {
        return None;
    }
    Some(row as usize - 1)
}

/// Props for [`MenuBar`].
#[derive(Default, Props)]
pub(super) struct MenuBarProps {
    /// Titles of the menu items (from [`menu_title`]).
    pub(super) menu_titles: Vec<Line<'static>>,
    /// Index of the menu whose dropdown is open, `None` while the bar owns
    /// no dropdown.
    pub(super) selected: Option<usize>,
    /// Called with the item index when a menu item is clicked.
    pub(super) on_select: Handler<'static, usize>,
}

/// Top menu bar with tab-like navigation items (Sessions, File / Folder,
/// Search, View, Options, Help).
///
/// Rendered borderless on a single row to keep the chrome compact; the
/// title of the open dropdown is highlighted with the theme's title style.
/// Clicking an item toggles its dropdown (the keyboard counterparts are the
/// Alt accelerators and the menu scope, so no focus model is needed here).
/// While a dropdown is open the menu input layer handles the clicks, so the
/// pointer hook only runs for the closed bar.
#[component]
pub(super) fn MenuBar(
    props: &mut MenuBarProps,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();

    if mouse_enabled() {
        let mut on_select = props.on_select.take();
        let titles = props.menu_titles.clone();
        hooks.use_pointer(PointerOptions::default(), move |ev| {
            let PointerKind::Click {
                button: MouseButton::Left,
                ..
            } = ev.kind
            else {
                return EventResult::Ignored;
            };
            // `position` shares the draw-area coordinates of `area`, so
            // the x is made bar-local before the item mapping.
            let item = ev
                .position
                .x
                .checked_sub(ev.area.x)
                .and_then(|x| tabs_hit_item(x, &titles, DIVIDER_WIDTH));
            match item {
                Some(index) => {
                    on_select(index);
                    EventResult::Consumed
                }
                None => EventResult::Ignored,
            }
        });
    }

    let menu_tabs = Tabs::new(props.menu_titles.clone())
        .select(props.selected)
        .highlight_style(theme.title)
        .divider(Span::raw(" | "));

    element!(
        View(height: Constraint::Length(1)) {
            widget(menu_tabs)
        }
    )
}

/// Props for [`MenuDropdown`].
#[derive(Default, Props)]
pub(super) struct MenuDropdownProps {
    /// The open menu; nothing is rendered while it is `None`.
    pub(super) menu: Option<MenuState>,
    /// Screen rectangle from [`dropdown_rect`].
    pub(super) rect: Rect,
    /// The active session's view (`None` on the welcome screen). Entries
    /// that are not available there are rendered dimmed.
    pub(super) view: Option<MenuView>,
    /// The active session's display filters, used to resolve the
    /// state-dependent `show`/`hide` labels (`None` without a directory
    /// session).
    pub(super) display: Option<DisplayFilters>,
}

/// Dropdown listing the entries of the open menu.
///
/// Drawn as a `Positioned` overlay, so it takes no room in the flex layout
/// and floats above the main view. Every row shows the selection pointer,
/// the entry label and — when the keymap labels the command — its key hint,
/// right-aligned in a shared column and underlined so it reads without
/// color. Separator rows render as a full-width rule. The row the keyboard
/// cursor addresses carries the theme's highlight style.
#[component]
pub(super) fn MenuDropdown(
    props: &MenuDropdownProps,
    hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let Some(menu) = props.menu else {
        return element!(Fragment).into_any();
    };
    let Some(def) = menu.menu() else {
        return element!(Fragment).into_any();
    };
    let (label_width, hint_width) =
        content_widths(def.entries, props.display.as_ref());
    let rect = props.rect;
    element!(
        Positioned(
            clear: true,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
            ) {
                for (index, entry) in def.entries.iter().enumerate() {
                    Text(
                        key: index,
                        text: dropdown_row(
                            entry,
                            index == menu.item,
                            label_width,
                            hint_width,
                            props.view,
                            props.display.as_ref(),
                            theme,
                        ),
                    )
                }
            }
        }
    )
    .into_any()
}

/// Props for [`MenuFlyout`].
#[derive(Default, Props)]
pub(super) struct MenuFlyoutProps {
    /// The open menu; nothing is rendered unless its submenu flyout is
    /// open (`MenuState::sub`).
    pub(super) menu: Option<MenuState>,
    /// Screen rectangle from [`flyout_rect`].
    pub(super) rect: Rect,
    /// The active session's view (`None` on the welcome screen).
    pub(super) view: Option<MenuView>,
    /// The active session's display filters.
    pub(super) display: Option<DisplayFilters>,
}

/// Flyout listing the children of the highlighted submenu parent.
///
/// A second `Positioned` overlay owned by the same input layer as its
/// parent dropdown; it renders only while [`MenuState::sub`] is `Some`.
#[component]
pub(super) fn MenuFlyout(
    props: &MenuFlyoutProps,
    hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let Some(menu) = props.menu else {
        return element!(Fragment).into_any();
    };
    let Some(sub) = menu.sub else {
        return element!(Fragment).into_any();
    };
    let Some(children) = menu.sub_entries() else {
        return element!(Fragment).into_any();
    };
    let (label_width, hint_width) =
        content_widths(children, props.display.as_ref());
    let rect = props.rect;
    element!(
        Positioned(
            clear: true,
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
            ) {
                for (index, entry) in children.iter().enumerate() {
                    Text(
                        key: index,
                        text: dropdown_row(
                            entry,
                            index == sub,
                            label_width,
                            hint_width,
                            props.view,
                            props.display.as_ref(),
                            theme,
                        ),
                    )
                }
            }
        }
    )
    .into_any()
}

/// Render one dropdown row: selection pointer, label padded to the common
/// width, and the command's key hint right-aligned in its own column.
/// Separator rows render as a rule across the box's inner width.
///
/// Labels are resolved through [`entry_label`], so toggle rows show their
/// state-dependent `show`/`hide` prefix and submenu parents carry their
/// flyout pointer.
///
/// The hint is underlined on top of its dim bold style: the key code must
/// also read on terminals without color, matching the underlined
/// accelerators of [`menu_title`] and [`super::option_list::OptionList`].
fn dropdown_row(
    entry: &MenuEntry,
    current: bool,
    label_width: u16,
    hint_width: u16,
    view: Option<MenuView>,
    display: Option<&DisplayFilters>,
    theme: CocomoTheme,
) -> Line<'static> {
    let hint_columns = hint_columns(hint_width);
    if let MenuEntry::Separator = entry {
        // The rule spans the box's inner width (the outer width minus the
        // two border columns).
        let width = label_width + hint_columns + 3;
        return Line::from("\u{2500}".repeat(width as usize))
            .style(theme.border);
    }
    let label = entry_label(entry, display).unwrap_or_default();
    let label_pad = " ".repeat(
        label_width.saturating_sub(label.chars().count() as u16) as usize,
    );
    // Entries unavailable in the active view (or on the welcome screen)
    // are dimmed; confirming one stays a no-op, mirroring the controller.
    let label_style = if entry.available(view) {
        Style::default()
    } else {
        theme.dim
    };
    let mut spans = vec![
        Span::raw(if current { "\u{25b8}" } else { " " }),
        Span::raw(" "),
        Span::styled(format!("{label}{label_pad}"), label_style),
    ];
    if hint_width > 0 {
        let hint = entry.cmd().and_then(key_hint).unwrap_or_default();
        let hint_pad = " "
            .repeat(hint_width.saturating_sub(hint.chars().count() as u16)
                as usize);
        spans.push(Span::raw("  "));
        spans.push(Span::raw(hint_pad));
        spans.push(Span::styled(
            hint.to_string(),
            theme
                .dim
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        ));
    }
    spans.push(Span::raw(" "));
    if current {
        Line::from(spans).style(theme.highlight)
    } else {
        Line::from(spans)
    }
}

#[cfg(test)]
mod tests {
    use ratatui_kit::{ratatui::style::Color, test_util::render_frames};

    use super::*;
    use crate::{
        keys::Cmd,
        menu::{MENUS, MenuItemDef, MenuViews},
        ui::theme::cocomo_palette,
    };

    /// The bar titles the App shell renders.
    fn titles() -> Vec<Line<'static>> {
        MENUS.iter().map(menu_title).collect()
    }

    /// Screen position of the menu bar's first cell.
    const BAR_ORIGIN: Position = Position { x: 0, y: 0 };

    #[test]
    fn menu_titles_underline_their_accelerator_once() {
        let buf = render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                MenuBar(
                    menu_titles: titles(),
                    selected: None,
                    on_select: Handler::from(|_: usize| {}),
                )
            }),
            100,
            2,
            1,
        );
        let underlined = |x: u16| {
            buf[(x, 0)]
                .style()
                .add_modifier
                .contains(Modifier::UNDERLINED)
        };
        // Tabs pad each title by one cell, so "Sessions" spans x=1..=8,
        // the divider x=10..=12, and "File / Folder" starts at x=14.
        assert_eq!(buf[(1, 0)].symbol(), "S");
        assert!(underlined(1));
        // The further `s` letters of "Sessions" are not the accelerator.
        assert!(!underlined(2));
        assert!(!underlined(3));
        assert!(!underlined(8));
        // "File / Folder" marks its leading `F` (at x=14..=26).
        assert_eq!(buf[(14, 0)].symbol(), "F");
        assert!(underlined(14));
        assert!(!underlined(15));
        // "Search" is addressed by `Alt+/`, which is not a letter of the
        // title, so nothing in it is underlined (title spans x=32..=37).
        assert_eq!(buf[(32, 0)].symbol(), "S");
        assert!(!underlined(32));
        assert!(!underlined(37));
    }

    #[test]
    fn selected_title_carries_the_highlight_style() {
        // Index 2 of `MENUS` is "Search", which starts at x=32.
        let buf = render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                MenuBar(
                    menu_titles: titles(),
                    selected: Some(2),
                    on_select: Handler::from(|_: usize| {}),
                )
            }),
            100,
            2,
            1,
        );
        // The open title is highlighted, its neighbours are not.
        assert_eq!(buf[(32, 0)].style().fg, Some(Color::Yellow));
        assert_ne!(buf[(1, 0)].style().fg, Some(Color::Yellow));
        assert_ne!(buf[(43, 0)].style().fg, Some(Color::Yellow));
    }

    #[test]
    fn dropdown_rect_anchors_under_its_title() {
        let items = titles();
        for (index, def) in MENUS.iter().enumerate() {
            let rect = dropdown_rect(&items, def, index, BAR_ORIGIN, None);
            let title_x =
                tabs_item_x(&items, index, DIVIDER_WIDTH).unwrap() + 1;
            assert_eq!(rect.x, title_x, "menu {}", def.title);
            assert_eq!(rect.y, 1, "menu {}", def.title);
            assert_eq!(rect.height, def.entries.len() as u16 + 2);
            assert!(rect.width > 0);
            // Every list row maps back to the entry it displays, while the
            // border rows and the cells left of the box do not.
            assert_eq!(
                dropdown_hit_item(
                    rect,
                    Position {
                        x: rect.x + 1,
                        y: rect.y + 1
                    }
                ),
                Some(0)
            );
            assert_eq!(
                dropdown_hit_item(
                    rect,
                    Position {
                        x: rect.x + 1,
                        y: rect.y + rect.height - 2
                    }
                ),
                Some(def.entries.len() - 1)
            );
            assert_eq!(
                dropdown_hit_item(
                    rect,
                    Position {
                        x: rect.x + 1,
                        y: rect.y
                    }
                ),
                None
            );
            assert_eq!(
                dropdown_hit_item(
                    rect,
                    Position {
                        x: rect.x - 1,
                        y: rect.y + 1
                    }
                ),
                None
            );
        }
    }

    #[test]
    fn dropdown_marks_the_current_row_and_dims_disabled_items() {
        let items = titles();
        let menu = MenuState {
            bar_index: menu_index("Sessions"),
            item: 1,
            sub: None,
        };
        let def = menu.menu().unwrap();
        let rect =
            dropdown_rect(&items, def, menu.bar_index, BAR_ORIGIN, None);
        let buf = render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                MenuDropdown(
                    menu: Some(menu),
                    rect: rect,
                    view: None,
                    display: None,
                )
            }),
            100,
            20,
            1,
        );
        let cell = |x: u16, y: u16| buf[(rect.x + x, rect.y + y)].style();
        // The first row is available on the welcome screen, so it is lit
        // and carries no pointer.
        assert_eq!(buf[(rect.x + 3, rect.y + 1)].symbol(), "O");
        assert_ne!(cell(3, 1).fg, Some(cocomo_palette().fg_dim));
        assert_ne!(cell(1, 1).bg, Some(cocomo_palette().selection));
        // The current row carries the pointer and the highlight style.
        assert_eq!(buf[(rect.x + 1, rect.y + 2)].symbol(), "\u{25b8}");
        assert_eq!(cell(3, 2).bg, Some(cocomo_palette().selection));
        assert_eq!(buf[(rect.x + 3, rect.y + 2)].symbol(), "S");
        // The "Report…" row (fifth entry) is directory-only and dimmed on
        // the welcome screen.
        assert_eq!(buf[(rect.x + 3, rect.y + 5)].symbol(), "R");
        assert_eq!(cell(3, 5).fg, Some(cocomo_palette().fg_dim));
        // The box is drawn closed on the right, i.e. it is wide enough for
        // its longest label.
        assert_eq!(
            buf[(rect.x + rect.width - 1, rect.y + 2)].symbol(),
            "\u{2502}"
        );
    }

    #[test]
    fn dropdown_rows_underline_their_key_hint() {
        let def = MENUS
            .iter()
            .find(|m| m.title == "Sessions")
            .expect("Sessions menu");
        // The first item is keyboard-bound (`O`), so its hint must carry
        // the color-free underline cue like every other key code.
        let entry = def
            .entries
            .iter()
            .find(|e| e.cmd().and_then(key_hint).is_some())
            .expect("entry with a key hint");
        let hint = key_hint(entry.cmd().unwrap()).unwrap();
        let theme = CocomoTheme::default();
        let row = dropdown_row(
            entry,
            false,
            20,
            hint.chars().count() as u16,
            None,
            None,
            theme,
        );
        let hint_span =
            row.spans.iter().find(|s| s.content == hint).unwrap_or_else(
                || panic!("hint {hint:?} missing from {row:?}"),
            );
        assert!(
            hint_span.style.add_modifier.contains(Modifier::UNDERLINED),
            "key hint not underlined: {row:?}"
        );
    }

    #[test]
    fn dropdown_rows_render_separators_and_submenu_markers() {
        let theme = CocomoTheme::default();
        // A separator renders as a rule across the box's inner width
        // (label column, hint column with its gap, plus the paddings).
        let sep = dropdown_row(
            &MenuEntry::Separator,
            false,
            10,
            4,
            None,
            None,
            theme,
        );
        assert_eq!(sep.spans.len(), 1);
        assert_eq!(sep.spans[0].content.as_ref(), "─".repeat(19).as_str());
        assert_eq!(sep.style, theme.border);
        // A submenu parent shows its label with a right-pointing pointer.
        static CHILDREN: &[MenuEntry] = &[MenuEntry::Item(MenuItemDef {
            label: "child",
            cmd: Cmd::Quit,
            views: MenuViews::ALL,
        })];
        let parent = dropdown_row(
            &MenuEntry::SubMenu {
                label: "group",
                children: CHILDREN,
            },
            false,
            10,
            0,
            None,
            None,
            theme,
        );
        assert!(parent.spans[2].content.starts_with("group"), "{parent:?}");
        assert!(parent.spans[2].content.contains('\u{25b8}'), "{parent:?}");
    }

    #[test]
    fn dropdown_rows_dim_entries_of_other_views() {
        let theme = CocomoTheme::default();
        let entry = MenuEntry::Item(MenuItemDef {
            label: "dir only",
            cmd: Cmd::Quit,
            views: MenuViews::DIR,
        });
        let row = |view| {
            let row = dropdown_row(&entry, false, 12, 0, view, None, theme);
            row.spans[2].style
        };
        // Dimmed on the welcome screen and on a text session, lit on a
        // directory session.
        assert_eq!(row(None), theme.dim);
        assert_eq!(row(Some(MenuView::Text)), theme.dim);
        assert_ne!(row(Some(MenuView::Dir)), theme.dim);
    }

    #[test]
    fn flyout_rect_anchors_at_the_parent_row() {
        let children = [
            MenuEntry::Item(MenuItemDef {
                label: "first child",
                cmd: Cmd::Quit,
                views: MenuViews::ALL,
            }),
            MenuEntry::Item(MenuItemDef {
                label: "second",
                cmd: Cmd::CloseTab,
                views: MenuViews::ALL,
            }),
        ];
        let dropdown = Rect {
            x: 5,
            y: 2,
            width: 20,
            height: 8,
        };
        let rect = flyout_rect(&children, 3, dropdown, None);
        // The flyout shares the dropdown's right border column and aligns
        // its top border with the parent row.
        assert_eq!(rect.x, dropdown.x + dropdown.width - 1);
        assert_eq!(rect.y, dropdown.y + 3);
        assert_eq!(rect.height, children.len() as u16 + 2);
        assert!(rect.width > 0);
    }

    fn menu_index(title: &str) -> usize {
        MENUS.iter().position(|m| m.title == title).unwrap()
    }
}
