// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Transfer dialog: the confirmation step of `c` (copy), `d` (delete) and
//! `v` (move).
//!
//! The dialog lists the four options of the operation as an
//! [`OptionList`], each row labelled with its accelerator (`[l]` left, `[r]`
//! right, `[b]` both (delete only), `[Esc]` abort), and shows what the
//! transfer would act on above that list. Each option also carries the
//! effect it would have over the whole selection (`(3 files, 1 dir)`), and
//! below the list the [`change_summary`] of the option under the cursor
//! spells out the concrete changes (per-action totals and affected paths),
//! so the dialog answers *what will change*, not just what is selected.
//! Options the current selection does not activate stay listed but dimmed
//! and unselectable (an empty effect); activation is decided by
//! [`TransferChoice::is_enabled`] over the whole selection: `l`
//! needs at least one entry with a left side, `r` one with a right side, `b`
//! one with both; abort is always active. Pressing the accelerator of a
//! dimmed option is absorbed (the dialog stays open), and the confirmed
//! choice applies to every entry in the selection that supports it.
//!
//! The dialog is the confirmation for destructive work, so a single
//! deliberate key press stands between the selection and the filesystem.
//!
//! The component lives in the App shell (not in a session view) so one
//! implementation serves both comparison views. Like `AlertModal` it owns an
//! exclusive input layer while open: the dialog's handler runs in that layer
//! with `EventPriority::High` and owns the accelerator keys (the
//! `KeyScope::Dialog` bindings), while `j`/`k`/`Enter` reach the `OptionList`
//! child in the same layer. Every other key is swallowed by the layer,
//! which is what keeps the table and the shell from reacting behind the
//! dialog.

use cocomo_lib::{
    DirEntry, EntryInfo, TransferItem, format_date, format_size,
};
use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        style::{Modifier, Style},
        text::{Line, Span, Text as RataText},
        widgets::Padding,
    },
};

use super::{
    frame::hint_line,
    option_list::{OptionItem, OptionList},
    summary::{change_summary, count_totals},
    theme::CocomoTheme,
};
use crate::{
    controller::{dispatch_dialog_key, pick_transfer_choice},
    state::{
        APP_STATE, SessionContent, SessionMode, SessionView, TransferChoice,
        TransferOp, TransferTarget,
    },
};

/// Everything the dialog renders, resolved from the active session.
struct DialogData {
    /// Which operation the dialog offers.
    op: TransferOp,
    /// What the transfer would act on, rendered for the user.
    summary: Vec<Line<'static>>,
    /// The entries the transfer would act on, for the option previews.
    targets: Vec<TransferTarget>,
    /// The four options, with the ones the selection does not activate
    /// marked disabled.
    items: Vec<OptionItem<TransferChoice>>,
}

/// Maximum number of entries a multi-target summary lists before it
/// collapses the rest into a `+N more` line.
const MAX_SUMMARY_TARGETS: usize = 5;

/// Transfer dialog rendered above the active session.
#[component]
pub(super) fn TransferDialog(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // The option the list's cursor is on, mirrored by its `on_highlight`
    // callback; it drives the change summary below the list. Stored with
    // the op it belongs to so a highlight from a previous open cannot leak
    // into a dialog for a different operation.
    let mut highlighted =
        hooks.use_state(|| None::<(TransferOp, TransferChoice)>);

    // Resolve the dialog from the active session. While the dialog is open
    // its input layer blocks every lower handler, so the selection cannot
    // move underneath it.
    let dialog: Option<DialogData> =
        app_handle.read().active().and_then(|s| {
            let SessionMode::Transfer { op } = s.ui.mode else {
                return None;
            };
            let targets = s.transfer_targets();
            if targets.is_empty() {
                return None;
            }
            // An option activates when any target in the range supports
            // it; the items are then built per target, skipping the ones
            // that do not. Name clashes support nothing.
            let has_left =
                targets.iter().any(|t| t.has_left && !t.name_conflict);
            let has_right =
                targets.iter().any(|t| t.has_right && !t.name_conflict);
            Some(DialogData {
                op,
                summary: summary_lines(s, &targets, &theme),
                targets: targets.clone(),
                items: op
                    .choices()
                    .into_iter()
                    .map(|choice| {
                        let item = OptionItem::new(choice, choice.label(op))
                            .key(choice.key());
                        let item =
                            match effect_hint(&choice.preview(op, &targets)) {
                                Some(hint) => item.hint(hint),
                                None => item,
                            };
                        item.disabled(
                            !choice.is_enabled(op, has_left, has_right),
                        )
                    })
                    .collect(),
            })
        });
    let open = dialog.is_some();

    // All hooks must run on every frame, open or not, so the layer is
    // minted unconditionally and simply inactive while `open` is false.
    let layer = hooks.use_input_layer(open, true);
    hooks.use_event_handler(
        EventScope::Layer(layer),
        EventPriority::High,
        move |event| {
            let Event::Key(key) = event else {
                return EventResult::Ignored;
            };
            if key.kind != KeyEventKind::Press {
                return EventResult::Ignored;
            }

            // The dialog owns `l`/`r`/`b`/`a`/`Esc` (via the keymap); an
            // unowned key stays unclaimed here, so the `OptionList` child
            // in the same layer gets `j`/`k`/`Enter`, and the layer's
            // `blocks_lower` swallows everything else before it can reach
            // the table.
            if dispatch_dialog_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let Some(data) = dialog else {
        // A childless `Fragment` would fall back to the default `Fill(1)`
        // height and steal the main view's rows; a closed `Modal` is
        // zero-sized instead. Returning early also unmounts the option
        // list, so every open starts with a fresh cursor.
        return element!(Modal(open: false)).into_any();
    };

    // Two border rows plus the padding around the content: the summary,
    // then one row per option, then the change summary of the highlighted
    // option.
    //
    // The change summary describes the option under the cursor; until the
    // list reports its seeded cursor it falls back to the first enabled
    // option, the same row the list seeds to.
    let changes = match highlighted.get() {
        Some((op, choice)) if op == data.op => {
            change_summary(&choice.preview(data.op, &data.targets), &theme)
        }
        _ => data
            .items
            .iter()
            .find(|item| !item.disabled)
            .map(|item| {
                change_summary(
                    &item.value.preview(data.op, &data.targets),
                    &theme,
                )
            })
            .unwrap_or_default(),
    };
    let summary_rows = data.summary.len().max(1) as u16;
    let option_rows = data.items.len() as u16;
    let change_rows = changes.len() as u16;
    let height = 4 + summary_rows + option_rows + change_rows;

    element!(
        Modal(
            open: true,
            // Reuse this component's layer instead of letting `Modal` mint
            // its own, which would deafen the handler above.
            layer: Some(layer),
            width: Constraint::Percentage(60),
            height: Constraint::Length(height),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(format!(" {} ", data.op.title()))
                    .style(theme.title)
                    .centered(),
                bottom_title: hint_line(&[
                    ("j/k", "select"),
                    ("Enter", "confirm"),
                    ("Esc", "cancel"),
                ])
                .style(theme.dim)
                .centered(),
                padding: Padding::uniform(1),
            ) {
                // Both children state their row count, so the summary keeps
                // its lines and the list below gets exactly one row per
                // option instead of sharing the remaining space.
                View(height: Constraint::Length(summary_rows)) {
                    Text(text: RataText::from(data.summary))
                }
                OptionList<TransferChoice>(
                    items: data.items,
                    on_select: Handler::from(move |choice| {
                        pick_transfer_choice(choice);
                    }),
                    on_highlight: Handler::from(move |choice| {
                        highlighted.set(Some((data.op, choice)));
                    }),
                    default_index: Some(0),
                    height: Constraint::Length(option_rows),
                )
                if change_rows > 0 {
                    View(height: Constraint::Length(change_rows)) {
                        Text(text: RataText::from(changes))
                    }
                }
            }
        }
    )
    .into_any()
}

/// Render the effect of a choice's preview as its hint column, e.g.
/// `(3 files, 1 dir)`.
///
/// Entries present on both sides under a two-sided choice (delete `b`) are
/// counted once, so the hint names the entries the change touches rather
/// than the number of transfer items it produces.
fn effect_hint(items: &[TransferItem]) -> Option<String> {
    if items.is_empty() {
        return None;
    }
    let mut files = 0;
    let mut dirs = 0;
    let mut seen: Vec<&str> = Vec::new();
    for item in items {
        if seen.contains(&item.rel_path.as_str()) {
            continue;
        }
        seen.push(&item.rel_path);
        if item.is_dir {
            dirs += 1;
        } else {
            files += 1;
        }
    }
    Some(format!("({})", count_totals(files, dirs)))
}

/// Render what the transfer would act on.
///
/// A single target keeps the detailed rendering: a directory session shows
/// the entry row (name, both sides' metadata, and the status symbol), a
/// text session shows the two compared paths, and the reason from
/// [`TransferTarget::note`] is appended when no key applies. A drag range
/// lists the affected entries by root-relative path, collapsing the tail
/// beyond [`MAX_SUMMARY_TARGETS`] into a `+N more` line.
fn summary_lines(
    session: &SessionView,
    targets: &[TransferTarget],
    theme: &CocomoTheme,
) -> Vec<Line<'static>> {
    if targets.len() > 1 {
        let mut lines: Vec<Line<'static>> = targets
            .iter()
            .take(MAX_SUMMARY_TARGETS)
            .map(|target| Line::from(target.rel_path.clone()))
            .collect();
        if let Some(more) = targets.len().checked_sub(MAX_SUMMARY_TARGETS) {
            lines.push(Line::from(Span::styled(
                format!("+{more} more"),
                theme.dim,
            )));
        }
        return lines;
    }

    let mut lines = match &session.content {
        SessionContent::Dir(_) => {
            let rows = session.visible_rows();
            match session.ui.table_state.selected().and_then(|i| rows.get(i)) {
                Some(row) => entry_lines(row.entry, theme),
                None => Vec::new(),
            }
        }
        SessionContent::Text(_) => vec![
            Line::from(Span::styled(
                session.core.left.display().to_string(),
                theme.left_name,
            )),
            Line::from(Span::styled(
                session.core.right.display().to_string(),
                theme.right_name,
            )),
        ],
    };
    if let Some(note) = targets.first().and_then(|t| t.note) {
        lines.push(Line::from(Span::styled(format!("({note})"), theme.dim)));
    }
    lines
}

/// Render one directory entry as name plus the two sides' metadata.
fn entry_lines(entry: &DirEntry, theme: &CocomoTheme) -> Vec<Line<'static>> {
    let meta = |info: Option<&EntryInfo>, style: Style| match info {
        Some(info) => Span::styled(
            format!(
                "{:>6}  {}",
                format_size(info.size),
                format_date(&info.modified)
            ),
            style,
        ),
        None => Span::styled("       -  ".to_string(), theme.dim),
    };
    vec![
        Line::from(Span::styled(
            entry.name.clone(),
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            meta(entry.left.as_ref(), theme.left_name),
            Span::styled(
                format!(" {} ", entry.status.symbol()),
                theme.status_style(entry.status),
            ),
            meta(entry.right.as_ref(), theme.right_name),
        ]),
    ]
}
