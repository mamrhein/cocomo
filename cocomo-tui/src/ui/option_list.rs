// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared decision list of every confirmation dialog.
//!
//! [`OptionList`] is the framework's `Select` plus the two things the
//! decision dialogs need and `Select` cannot express: an accelerator key
//! rendered as part of the row (`[l] left → right`), and options the
//! current selection does not activate, which stay listed but dimmed and
//! unselectable. Everything else matches `Select`: `j`/`k`/arrows/`Home`/
//! `End` move through the enabled rows, `Enter` confirms the current one,
//! a click selects and confirms, and the row under the pointer is
//! highlighted while mouse capture is on.
//!
//! The component owns no application state; it reports the chosen
//! [`OptionItem`] through `on_select`. Its handler runs at
//! `EventPriority::Normal` in the input layer it renders in, so the
//! dialog's `High` handler still sees `Esc` and the accelerators first and
//! only the navigation keys reach the list — the same split the `Select`
//! children provided before.

use std::{cell::RefCell, rc::Rc};

use ratatui_kit::{
    crossterm::event::{Event, KeyCode, KeyEventKind, MouseButton},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        style::{Modifier, Style},
        text::{Line, Span},
    },
};

use super::{mouse_enabled, theme::CocomoTheme};

/// One row of an [`OptionList`].
///
/// `key` is the accelerator rendered as `[key]`, `hint` the right-aligned
/// secondary column, and `disabled` marks an option the current state does
/// not activate: it stays visible so the key map does not shift shape per
/// entry, but navigation skips it and confirming it does nothing.
#[derive(Clone, Debug)]
pub(super) struct OptionItem<T> {
    /// The value reported to `on_select` when the row is confirmed.
    pub(super) value: T,
    /// Accelerator key shown in brackets, `None` for a row without one.
    pub(super) key: Option<&'static str>,
    /// Primary label of the option.
    pub(super) label: String,
    /// Right-aligned secondary column (current value, effect, …).
    pub(super) hint: Option<String>,
    /// Whether the current state activates this option.
    pub(super) disabled: bool,
}

impl<T> OptionItem<T> {
    /// New selectable option without a key code or hint.
    pub(super) fn new(value: T, label: impl Into<String>) -> Self {
        Self {
            value,
            key: None,
            label: label.into(),
            hint: None,
            disabled: false,
        }
    }

    /// Set the accelerator key rendered as `[key]`.
    pub(super) fn key(mut self, key: &'static str) -> Self {
        self.key = Some(key);
        self
    }

    /// Set the right-aligned hint column.
    pub(super) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Mark the option as not activated by the current state.
    pub(super) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// Props for [`OptionList`].
pub(super) struct OptionListProps<T>
where
    T: Clone + Send + Sync + 'static,
{
    /// The options to list, in display order.
    pub(super) items: Vec<OptionItem<T>>,
    /// Called with the option when a row is confirmed.
    pub(super) on_select: Handler<'static, T>,
    /// Called with the option the cursor moves to, including the row the
    /// list seeds to on mount. Lets a dialog mirror the highlighted
    /// option's effect below the list; disabled rows are never highlighted.
    pub(super) on_highlight: Option<Handler<'static, T>>,
    /// Row to start at; the first selectable row at or after it wins, so a
    /// disabled default slides to the next option instead of highlighting
    /// nothing.
    pub(super) default_index: Option<usize>,
    /// Message replacing the list while it is empty, e.g. a list that is
    /// still being filled asynchronously.
    pub(super) empty_message: Option<Line<'static>>,
    /// Height of the list. Defaults to filling the parent, which is right
    /// while the list is the only content of its panel; a dialog that
    /// renders a summary above it passes the rows it needs.
    pub(super) height: Constraint,
}

impl<T> Default for OptionListProps<T>
where
    T: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self {
            items: Vec::new(),
            on_select: Handler::default(),
            on_highlight: None,
            default_index: None,
            empty_message: None,
            height: Constraint::Fill(1),
        }
    }
}

impl<T> Props for OptionListProps<T> where T: Clone + Send + Sync + 'static {}

/// Decision list shared by the confirmation dialogs.
///
/// Each row renders as `▸ [key] label … hint`: the selection pointer, the
/// bracketed accelerator in the theme's title style (its key letter also
/// underlined, so it survives a monochrome terminal), the label padded to
/// the widest one, and the hint right-aligned in a shared dim column. The
/// row the keyboard cursor addresses carries the theme's highlight style,
/// the row under the pointer the hover style, and a disabled row is dimmed
/// either way.
#[component]
pub(super) fn OptionList<T>(
    props: &mut OptionListProps<T>,
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>>
where
    T: Clone + Send + Sync + 'static,
{
    let theme = hooks.use_component_theme::<CocomoTheme>();
    let items = props.items.clone();
    let count = items.len();
    let default_index = props.default_index;

    // Cursor into the list. The effect re-seeds it whenever the item count
    // or the default changes, which covers both the first mount and a list
    // that arrives asynchronously while the dialog stays open.
    let mut index = hooks.use_state(|| seed(&items, default_index));

    // The key and the pointer closures confirm and highlight through the
    // same callbacks; `Handler` is not `Clone`, so each is shared behind an
    // `Rc<RefCell>`.
    let on_select = Rc::new(RefCell::new(props.on_select.take()));
    let on_highlight = Rc::new(RefCell::new(props.on_highlight.take()));

    // Cursor into the list. The effect re-seeds it whenever the item count
    // or the default changes, which covers both the first mount and a list
    // that arrives asynchronously while the dialog stays open. The seeded
    // row is reported to `on_highlight` so a dialog mirrors it right away.
    let seed_items = items.clone();
    let seed_highlight = Rc::clone(&on_highlight);
    hooks.use_effect(
        move || {
            let next = seed(&seed_items, default_index);
            index.set_no_update(next);
            fire_highlight(&seed_highlight, &seed_items, next);
        },
        (count, default_index),
    );
    let mut hover = hooks.use_state(|| None::<usize>);

    hooks.use_event_handler(EventScope::Current, EventPriority::Normal, {
        let on_select = Rc::clone(&on_select);
        let on_highlight = Rc::clone(&on_highlight);
        let items = items.clone();
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press || items.is_empty() {
                return EventResult::Ignored;
            }

            // Every cursor move is mirrored to `on_highlight`, so a dialog
            // can track the option under the cursor.
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    let next = step(&items, index.get(), true);
                    index.set(next);
                    fire_highlight(&on_highlight, &items, next);
                    EventResult::Consumed
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    let next = step(&items, index.get(), false);
                    index.set(next);
                    fire_highlight(&on_highlight, &items, next);
                    EventResult::Consumed
                }
                KeyCode::Home => {
                    let next = seed(&items, Some(0));
                    index.set(next);
                    fire_highlight(&on_highlight, &items, next);
                    EventResult::Consumed
                }
                KeyCode::End => {
                    let next = seed_back(&items, Some(items.len() - 1));
                    index.set(next);
                    fire_highlight(&on_highlight, &items, next);
                    EventResult::Consumed
                }
                KeyCode::Enter => {
                    confirm(&items, index.get(), &mut on_select.borrow_mut());
                    EventResult::Consumed
                }
                _ => EventResult::Ignored,
            }
        }
    });

    // Pointer: hover highlights the row under the cursor, a click selects
    // and confirms it. Registered only when the entry enabled mouse
    // capture, exactly like the framework's built-ins; the flag never
    // changes mid-process, so the hook order stays stable across frames.
    if mouse_enabled() {
        hooks.use_pointer(
            PointerOptions {
                track_move: true,
                ..Default::default()
            },
            {
                let on_select = Rc::clone(&on_select);
                let on_highlight = Rc::clone(&on_highlight);
                let items = items.clone();
                move |ev| {
                    // `position` shares the draw-area coordinates of
                    // `area`, so the y becomes list-local before the row
                    // mapping.
                    let row = ev
                        .position
                        .y
                        .checked_sub(ev.area.y)
                        .map(usize::from)
                        .filter(|&row| row < items.len());
                    match ev.kind {
                        PointerKind::Enter | PointerKind::Move => {
                            if hover.get() != row {
                                hover.set(row);
                                EventResult::Consumed
                            } else {
                                EventResult::Ignored
                            }
                        }
                        PointerKind::Leave => {
                            if hover.get().is_some() {
                                hover.set(None);
                            }
                            EventResult::Ignored
                        }
                        PointerKind::Click {
                            button: MouseButton::Left,
                            ..
                        } => {
                            let enabled =
                                row.filter(|&row| !items[row].disabled);
                            hover.set(None);
                            index.set(enabled);
                            fire_highlight(&on_highlight, &items, enabled);
                            confirm(
                                &items,
                                enabled,
                                &mut on_select.borrow_mut(),
                            );
                            EventResult::Consumed
                        }
                        _ => EventResult::Ignored,
                    }
                }
            },
        );
    }

    let current = index
        .get()
        .filter(|&row| items.get(row).is_some_and(|i| !i.disabled));
    let hovered = hover
        .get()
        .filter(|&row| row < count && Some(row) != current && count > 0);
    let key_width = content_width(items.iter().filter_map(|i| i.key));
    let label_width = content_width(items.iter().map(|i| i.label.as_str()));
    let hint_width =
        content_width(items.iter().map(|i| i.hint.as_deref().unwrap_or("")));

    // An empty list renders the placeholder (if the caller supplied one)
    // instead of a list of rows.
    if count == 0 {
        return element!(
            View(
                flex_direction: Direction::Vertical,
                height: props.height,
            ) {
                if let Some(message) = props.empty_message.clone() {
                    View(height: Constraint::Length(1)) {
                        Text(text: message.style(theme.dim))
                    }
                }
            }
        )
        .into_any();
    }

    element!(
        View(
            flex_direction: Direction::Vertical,
            height: props.height,
        ) {
            for (row, item) in items.iter().enumerate() {
                // One row per option: `Text` fills whatever space the parent
                // offers, so the row gets its own single-height box instead
                // of being spaced out over the list.
                View(key: row, height: Constraint::Length(1)) {
                    Text(text: option_row(
                        item,
                        Some(row) == current,
                        Some(row) == hovered,
                        key_width,
                        label_width,
                        hint_width,
                        &theme,
                    ))
                }
            }
        }
    )
    .into_any()
}

/// Width of the widest of `values`, i.e. the column width they need.
fn content_width<'a>(values: impl Iterator<Item = &'a str>) -> u16 {
    values
        .map(|value| value.chars().count() as u16)
        .max()
        .unwrap_or(0)
}

/// Index of the first selectable row at or after `default`, wrapping to the
/// start when the tail is disabled; `None` for an empty list or one without
/// any selectable row.
fn seed<T>(items: &[OptionItem<T>], default: Option<usize>) -> Option<usize> {
    if items.is_empty() {
        return None;
    }
    let start = default.unwrap_or(0).min(items.len() - 1);
    (start..items.len())
        .chain(0..start)
        .find(|&row| !items[row].disabled)
}

/// [`seed`] walking backward from `default`, i.e. the `End` counterpart.
fn seed_back<T>(
    items: &[OptionItem<T>],
    default: Option<usize>,
) -> Option<usize> {
    if items.is_empty() {
        return None;
    }
    let start = default.unwrap_or(items.len() - 1).min(items.len() - 1);
    (0..=start)
        .rev()
        .chain((start + 1..items.len()).rev())
        .find(|&row| !items[row].disabled)
}

/// Index of the next selectable row from `current` in the `forward`
/// direction, wrapping around the list; `None` when no row is selectable.
fn step<T>(
    items: &[OptionItem<T>],
    current: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let len = items.len();
    if len == 0 {
        return None;
    }
    let last = len - 1;
    // Without a cursor yet, the first step enters the list at its top
    // (`forward`) or bottom, so `j` on a fresh list starts at row 0.
    let mut row = current.unwrap_or(if forward { last } else { 0 }).min(last);
    for _ in 0..len {
        row = if forward {
            (row + 1) % len
        } else if row == 0 {
            last
        } else {
            row - 1
        };
        if !items[row].disabled {
            return Some(row);
        }
    }
    None
}

/// Report the selectable row under the cursor to `on_highlight` (when the
/// caller supplied a handler).
fn fire_highlight<T>(
    on_highlight: &RefCell<Option<Handler<'static, T>>>,
    items: &[OptionItem<T>],
    current: Option<usize>,
) where
    T: Clone,
{
    let Some(item) = current.and_then(|row| items.get(row)) else {
        return;
    };
    if let Some(handler) = on_highlight.borrow_mut().as_mut() {
        handler(item.value.clone());
    }
}

/// Report the selectable row under the cursor to `on_select`.
fn confirm<T>(
    items: &[OptionItem<T>],
    current: Option<usize>,
    on_select: &mut Handler<'static, T>,
) where
    T: Clone,
{
    if let Some(item) = current
        .and_then(|row| items.get(row))
        .filter(|item| !item.disabled)
    {
        on_select(item.value.clone());
    }
}

/// Render one option row: selection pointer, bracketed key code, label
/// padded to the common width, and the hint right-aligned in its own
/// column.
fn option_row<T>(
    item: &OptionItem<T>,
    current: bool,
    hovered: bool,
    key_width: u16,
    label_width: u16,
    hint_width: u16,
    theme: &CocomoTheme,
) -> Line<'static> {
    let mut spans = vec![
        Span::raw(if current { "\u{25b8}" } else { " " }),
        Span::raw(" "),
    ];
    // The key column keeps its width even for an option without a key, so
    // the labels of a mixed list stay aligned.
    match item.key {
        Some(key) => {
            // The key letter is underlined on top of the title color, so
            // the accelerator also reads on terminals without color.
            let pad = gap(key_width, key.chars().count());
            spans.push(Span::styled("[", theme.title));
            spans.push(Span::styled(
                (*key).to_string(),
                theme.title.add_modifier(Modifier::UNDERLINED),
            ));
            spans.push(Span::styled(format!("]{pad}"), theme.title));
        }
        None => spans.push(Span::raw(" ".repeat(usize::from(key_width) + 2))),
    }
    spans.push(Span::raw(" "));
    let label_style = if item.disabled {
        theme.dim
    } else {
        Style::default()
    };
    let label_pad = gap(label_width, item.label.chars().count());
    spans.push(Span::styled(
        format!("{}{label_pad}", item.label),
        label_style,
    ));
    if hint_width > 0 {
        let hint = item.hint.clone().unwrap_or_default();
        let hint_pad = gap(hint_width, hint.chars().count());
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            format!("{hint_pad}{hint}"),
            theme.dim.add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(" "));
    let line = Line::from(spans);
    if current {
        line.style(theme.highlight)
    } else if hovered {
        line.style(theme.hover)
    } else {
        line
    }
}

/// Padding that pads a column of `width` cells holding `len` cells.
fn gap(width: u16, len: usize) -> String {
    " ".repeat(usize::from(width).saturating_sub(len))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// List of options, the rows flagged `true` in `selectable` being
    /// selectable and the others disabled.
    fn items(selectable: &[bool]) -> Vec<OptionItem<usize>> {
        selectable
            .iter()
            .enumerate()
            .map(|(row, &on)| {
                OptionItem::new(row, format!("opt{row}")).disabled(!on)
            })
            .collect()
    }

    /// Flatten a rendered row to its text.
    fn row_text(line: &Line<'static>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn seed_slides_to_the_next_selectable_row() {
        let list = items(&[false, true, false]);
        // A disabled default slides forward instead of highlighting
        // nothing, and wraps to the head when the tail is disabled.
        assert_eq!(seed(&list, Some(0)), Some(1));
        assert_eq!(seed(&list, Some(2)), Some(1));
        assert_eq!(seed(&list, None), Some(1));
        // An empty or fully disabled list has no cursor at all.
        let empty: Vec<OptionItem<usize>> = Vec::new();
        assert_eq!(seed(&empty, None), None);
        assert_eq!(seed(&items(&[false, false]), Some(0)), None);
    }

    #[test]
    fn seed_back_finds_the_last_selectable_row() {
        let list = items(&[false, true, false]);
        assert_eq!(seed_back(&list, Some(2)), Some(1));
        assert_eq!(seed_back(&list, Some(0)), Some(1));
        assert_eq!(seed_back(&items(&[false, false]), None), None);
    }

    #[test]
    fn step_skips_disabled_rows_and_wraps() {
        let list = items(&[true, false, true]);
        assert_eq!(step(&list, Some(0), true), Some(2));
        assert_eq!(step(&list, Some(2), true), Some(0));
        assert_eq!(step(&list, Some(2), false), Some(0));
        assert_eq!(step(&list, Some(0), false), Some(2));
    }

    #[test]
    fn step_without_a_cursor_enters_at_the_edge() {
        let list = items(&[true, true, true]);
        assert_eq!(step(&list, None, true), Some(0));
        assert_eq!(step(&list, None, false), Some(2));
    }

    #[test]
    fn step_moves_nowhere_without_a_selectable_row() {
        assert_eq!(step(&items(&[false, false]), Some(0), true), None);
        let empty: Vec<OptionItem<usize>> = Vec::new();
        assert_eq!(step(&empty, None, true), None);
    }

    #[test]
    fn confirm_reports_only_a_selectable_row() {
        let list = items(&[false, true]);
        let picked = Arc::new(Mutex::new(Vec::new()));
        let mut on_select = {
            let picked = Arc::clone(&picked);
            Handler::from(move |value: usize| {
                picked.lock().unwrap().push(value);
            })
        };
        confirm(&list, Some(0), &mut on_select);
        confirm(&list, Some(1), &mut on_select);
        // A missing cursor is a no-op, too.
        confirm(&list, None, &mut on_select);
        assert_eq!(*picked.lock().unwrap(), vec![1]);
    }

    #[test]
    fn option_row_leads_with_the_bracketed_key_code() {
        let item = OptionItem::new(0, "left").key("l");
        let theme = CocomoTheme::default();
        let row = option_row(&item, false, false, 3, 6, 0, &theme);
        // Pointer cell, `[key]` padded to the widest key, then the label
        // padded to the widest label.
        assert_eq!(row_text(&row), "  [l]   left   ");
        assert!(
            row.spans
                .iter()
                .any(|s| &*s.content == "[" && s.style == theme.title)
                && row.spans.iter().any(|s| {
                    &*s.content == "l"
                        && s.style
                            == theme.title.add_modifier(Modifier::UNDERLINED)
                })
                && row
                    .spans
                    .iter()
                    .any(|s| &*s.content == "]  " && s.style == theme.title),
            "key code not rendered in the title style: {row:?}"
        );
    }

    #[test]
    fn option_row_underlines_the_key_letter() {
        // The underline is the color-free cue for the accelerator, so it
        // must sit on the key letter itself, not on the brackets.
        let item = OptionItem::new(0, "left").key("l");
        let theme = CocomoTheme::default();
        let row = option_row(&item, false, false, 3, 6, 0, &theme);
        let key_span = row
            .spans
            .iter()
            .find(|s| s.content.starts_with('l'))
            .unwrap_or_else(|| panic!("no key span in {row:?}"));
        assert!(
            key_span.style.add_modifier.contains(Modifier::UNDERLINED),
            "key letter not underlined: {row:?}"
        );
        let bracket = row
            .spans
            .iter()
            .find(|s| s.content.starts_with('['))
            .expect("no opening bracket");
        assert!(
            !bracket.style.add_modifier.contains(Modifier::UNDERLINED),
            "bracket must not be underlined: {row:?}"
        );
    }

    #[test]
    fn option_row_keeps_the_layout_without_a_key() {
        let item = OptionItem::new(0, "left");
        let theme = CocomoTheme::default();
        let row = option_row(&item, false, false, 3, 6, 0, &theme);
        // The key column stays blank, so labels of mixed rows align.
        assert_eq!(row_text(&row), "        left   ");
    }

    #[test]
    fn option_row_right_aligns_the_hint() {
        let on = OptionItem::new(0, "Compare content").hint("on");
        let off = OptionItem::new(1, "Hide identical").hint("off");
        let theme = CocomoTheme::default();
        let on_row = option_row(&on, false, false, 0, 15, 3, &theme);
        let off_row = option_row(&off, false, false, 0, 15, 3, &theme);
        assert!(row_text(&on_row).ends_with(" on "), "{}", row_text(&on_row));
        assert!(
            row_text(&off_row).ends_with("off "),
            "{}",
            row_text(&off_row)
        );
    }

    #[test]
    fn option_row_styles_current_hovered_and_disabled_rows() {
        let theme = CocomoTheme::default();
        let enabled = OptionItem::new(0, "Execute").key("y");
        let current = option_row(&enabled, true, false, 1, 7, 0, &theme);
        assert_eq!(current.style, theme.highlight);
        assert!(row_text(&current).starts_with('\u{25b8}'));
        let hovered = option_row(&enabled, false, true, 1, 7, 0, &theme);
        assert_eq!(hovered.style, theme.hover);
        assert!(row_text(&hovered).starts_with(' '));
        let plain = option_row(&enabled, false, false, 1, 7, 0, &theme);
        assert_eq!(plain.style, Style::default());

        // A disabled row is dimmed, also while it is the current one.
        let disabled = OptionItem::new(1, "Both").key("b").disabled(true);
        let row = option_row(&disabled, true, false, 1, 7, 0, &theme);
        assert!(
            row.spans
                .iter()
                .any(|s| s.content.starts_with("Both")
                    && s.style == theme.dim),
            "disabled label not dimmed: {row:?}"
        );
    }
}
