// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Top menu bar and its dropdowns.
//!
//! Titles and items come from [`crate::menu::MENUS`]; each title renders
//! with its Alt accelerator underlined, and [`MenuDropdown`] draws the open
//! menu as a positioned overlay. Both the bar and the dropdown share the
//! geometry helpers here ([`menu_title`], [`dropdown_rect`],
//! [`dropdown_hit_item`]), so the rendered layout and the mouse hit-testing
//! in the App shell cannot drift apart.

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
    menu::{MenuDef, MenuItemDef, MenuState},
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

/// Column widths of the label and key-hint parts of `def`'s dropdown.
fn content_widths(def: &MenuDef) -> (u16, u16) {
    let mut label_width = 0u16;
    let mut hint_width = 0u16;
    for item in def.items {
        label_width = label_width.max(item.label.chars().count() as u16);
        if let Some(hint) = key_hint(item.cmd) {
            hint_width = hint_width.max(hint.chars().count() as u16);
        }
    }
    (label_width, hint_width)
}

/// The screen rectangle the dropdown of `menu` is drawn in.
///
/// `bar_origin` is the screen position of the menu bar's first cell; the
/// dropdown is anchored under its title's text (one cell past the tab's
/// left padding) and opens directly below the bar. An unknown menu yields
/// [`Rect::ZERO`].
pub(super) fn dropdown_rect(
    titles: &[Line<'static>],
    menu: MenuState,
    bar_origin: Position,
) -> Rect {
    let Some(def) = menu.menu() else {
        return Rect::ZERO;
    };
    let (label_width, hint_width) = content_widths(def);
    // The hint column only exists when at least one item has a hint, and
    // is separated from the labels by two cells. The row itself is
    // `▸ label… hint `, the box adds two border columns.
    let hint_columns = if hint_width > 0 { hint_width + 2 } else { 0 };
    Rect {
        x: bar_origin.x
            + tabs_item_x(titles, menu.bar_index, DIVIDER_WIDTH).unwrap_or(0)
            + 1,
        y: bar_origin.y + 1,
        width: label_width + hint_columns + 5,
        height: def.items.len() as u16 + 2,
    }
}

/// The dropdown item under `pos`, `None` outside the list.
///
/// The title border row and the bottom border row are not items, so a click
/// there only closes the dropdown.
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

/// Top menu bar with tab-like navigation items (Sessions, Compare, Sync,
/// View, Filter, Help).
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
    /// Whether the active session is a directory comparison. Items that
    /// require one are rendered dimmed while it is not.
    pub(super) dir_active: bool,
}

/// Dropdown listing the items of the open menu.
///
/// Drawn as a `Positioned` overlay, so it takes no room in the flex layout
/// and floats above the main view. Every row shows the selection pointer,
/// the item label and — when the keymap labels the command — its key hint,
/// right-aligned in a shared column and underlined so it reads without
/// color. The row the keyboard cursor addresses carries the theme's
/// highlight style.
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
    let (label_width, hint_width) = content_widths(def);
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
                for (index, item) in def.items.iter().enumerate() {
                    Text(
                        key: index,
                        text: dropdown_row(
                            item,
                            index == menu.item,
                            label_width,
                            hint_width,
                            props.dir_active,
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
///
/// The hint is underlined on top of its dim bold style: the key code must
/// also read on terminals without color, matching the underlined
/// accelerators of [`menu_title`] and [`super::option_list::OptionList`].
fn dropdown_row(
    item: &MenuItemDef,
    current: bool,
    label_width: u16,
    hint_width: u16,
    dir_active: bool,
    theme: CocomoTheme,
) -> Line<'static> {
    let label_pad = " ".repeat(
        label_width.saturating_sub(item.label.chars().count() as u16) as usize,
    );
    // Directory-only items are dimmed on a text session (or the welcome
    // screen); confirming one stays a no-op, mirroring the controller.
    let label_style = if item.requires_dir && !dir_active {
        theme.dim
    } else {
        Style::default()
    };
    let mut spans = vec![
        Span::raw(if current { "\u{25b8}" } else { " " }),
        Span::raw(" "),
        Span::styled(format!("{}{label_pad}", item.label), label_style),
    ];
    if hint_width > 0 {
        let hint = key_hint(item.cmd).unwrap_or_default();
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
    use crate::{menu::MENUS, ui::theme::cocomo_palette};

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
        // the divider x=9..=13, and "Compare" starts at x=14.
        assert_eq!(buf[(1, 0)].symbol(), "S");
        assert!(underlined(1));
        // The further `s` letters of "Sessions" are not the accelerator.
        assert!(!underlined(2));
        assert!(!underlined(3));
        assert!(!underlined(8));
        // "Compare" marks its `c`, "Sync" its `y` (at x=26..=29).
        assert_eq!(buf[(14, 0)].symbol(), "C");
        assert!(underlined(14));
        assert!(!underlined(15));
        assert_eq!(buf[(26, 0)].symbol(), "S");
        assert!(!underlined(26));
        assert!(underlined(27));
    }

    #[test]
    fn selected_title_carries_the_highlight_style() {
        // Index 2 of `MENUS` is "Sync", which starts at x=26.
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
        assert_eq!(buf[(26, 0)].style().fg, Some(Color::Yellow));
        assert_ne!(buf[(1, 0)].style().fg, Some(Color::Yellow));
        assert_ne!(buf[(35, 0)].style().fg, Some(Color::Yellow));
    }

    #[test]
    fn dropdown_rect_anchors_under_its_title() {
        let items = titles();
        for (index, def) in MENUS.iter().enumerate() {
            let menu = MenuState::open(index).unwrap();
            let rect = dropdown_rect(&items, menu, BAR_ORIGIN);
            let title_x =
                tabs_item_x(&items, index, DIVIDER_WIDTH).unwrap() + 1;
            assert_eq!(rect.x, title_x, "menu {}", def.title);
            assert_eq!(rect.y, 1, "menu {}", def.title);
            assert_eq!(rect.height, def.items.len() as u16 + 2);
            assert!(rect.width > 0);
            // Every list row maps back to the item it displays, while the
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
                Some(def.items.len() - 1)
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
        // An unknown menu has no rectangle at all.
        let unknown = MenuState {
            bar_index: MENUS.len(),
            item: 0,
        };
        assert_eq!(dropdown_rect(&items, unknown, BAR_ORIGIN), Rect::ZERO);
    }

    #[test]
    fn dropdown_marks_the_current_row_and_dims_disabled_items() {
        let items = titles();
        let menu = MenuState {
            bar_index: menu_index("Sessions"),
            item: 1,
        };
        let rect = dropdown_rect(&items, menu, BAR_ORIGIN);
        let buf = render_frames(
            element!(PaletteProvider(palette: cocomo_palette()) {
                MenuDropdown(menu: Some(menu), rect: rect, dir_active: false)
            }),
            100,
            20,
            1,
        );
        let cell = |x: u16, y: u16| buf[(rect.x + x, rect.y + y)].style();
        // The first row is a directory-only item on a non-directory
        // session: dimmed label, no pointer.
        assert_eq!(buf[(rect.x + 3, rect.y + 1)].symbol(), "o");
        assert_eq!(cell(3, 1).fg, Some(cocomo_palette().fg_dim));
        assert!(!cell(1, 1).add_modifier.contains(Modifier::REVERSED));
        // The current row carries the pointer and the highlight style.
        assert_eq!(buf[(rect.x + 1, rect.y + 2)].symbol(), "\u{25b8}");
        assert!(cell(3, 2).add_modifier.contains(Modifier::REVERSED));
        assert_eq!(buf[(rect.x + 3, rect.y + 2)].symbol(), "s");
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
        let item = def
            .items
            .iter()
            .find(|i| key_hint(i.cmd).is_some())
            .expect("item with a key hint");
        let hint = key_hint(item.cmd).unwrap();
        let theme = CocomoTheme::default();
        let row = dropdown_row(
            item,
            false,
            20,
            hint.chars().count() as u16,
            true,
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

    fn menu_index(title: &str) -> usize {
        MENUS.iter().position(|m| m.title == title).unwrap()
    }
}
