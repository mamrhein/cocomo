// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Transfer dialog: the confirmation step of `c` (copy) and `d` (delete).
//!
//! The dialog is a pure key interface — no list, no cursor. It shows the
//! selected line (or, for a drag-selected range, the list of affected
//! entries), and its bottom help line lists exactly the keys the selection
//! activates: `l` left, `r` right, `b` both (delete only), `Esc` abort.
//! Activation is decided by [`TransferChoice::is_enabled`] over the whole
//! selection: `l` needs at least one entry with a left side, `r` one with a
//! right side, `b` one with both; abort is always active. Keys that do not
//! apply are not shown at all, and a press of one is absorbed (the dialog
//! stays open). The confirmed choice then applies to every entry in the
//! selection that supports it.
//!
//! The dialog is the confirmation for destructive work, so a single
//! deliberate key press stands between the selection and the filesystem.
//!
//! The component lives in the App shell (not in a session view) so one
//! implementation serves both comparison views. Like `AlertModal` it owns an
//! exclusive input layer while open: the dialog's keys go through the keymap
//! (`KeyScope::Dialog`), and every other key is swallowed by the layer,
//! which is what keeps the table and the shell from reacting behind the
//! dialog.

use cocomo_lib::{DirEntry, EntryInfo, format_date, format_size};
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

use super::theme::CocomoTheme;
use crate::{
    controller::dispatch_dialog_key,
    state::{
        APP_STATE, SessionContent, SessionMode, SessionView, TransferOp,
        TransferTarget,
    },
};

/// Everything the dialog renders, resolved from the active session.
struct DialogData {
    /// Which operation the dialog offers.
    op: TransferOp,
    /// What the transfer would act on, rendered for the user.
    summary: Vec<Line<'static>>,
    /// Bottom help line listing the keys the selection activates.
    help: Line<'static>,
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
            // that do not.
            let has_left = targets.iter().any(|t| t.has_left);
            let has_right = targets.iter().any(|t| t.has_right);
            Some(DialogData {
                op,
                summary: summary_lines(s, &targets, &theme),
                help: help_line(op, has_left, has_right)
                    .style(theme.dim)
                    .centered(),
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

            // The dialog owns `l`/`r`/`b`/`Esc` (via the keymap); an
            // unowned key stays unclaimed here, and the layer's
            // `blocks_lower` swallows it so it never reaches the table.
            if dispatch_dialog_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // Flatten the dialog data into render values; closed means empty.
    let (title, summary, help) = match &dialog {
        Some(data) => (
            Line::from(format!(" {} ", data.op.title()))
                .style(theme.title)
                .centered(),
            data.summary.clone(),
            data.help.clone(),
        ),
        None => (Line::from(""), Vec::new(), Line::from("")),
    };
    // Two border rows plus the padding around the content.
    let height = 4 + summary.len().max(1) as u16;

    element!(
        Modal(
            open: open,
            // Reuse this component's layer instead of letting `Modal` mint
            // its own, which would deafen the handler above.
            layer: Some(layer),
            width: Constraint::Percentage(60),
            height: Constraint::Length(height),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: title,
                bottom_title: help,
                padding: Padding::uniform(1),
            ) {
                Text(text: RataText::from(summary))
            }
        }
    )
}

/// Build the bottom help line: the keys the selection activates, in
/// display order, as `key label` pairs. Keys that do not apply are omitted,
/// so the copy line never carries `b`.
fn help_line(
    op: TransferOp,
    has_left: bool,
    has_right: bool,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for choice in op.choices() {
        if !choice.is_enabled(op, has_left, has_right) {
            continue;
        }
        if !spans.is_empty() {
            spans.push(Span::raw("   "));
        }
        spans.push(Span::raw(format!(
            "{} {}",
            choice.key(),
            choice.label(op)
        )));
    }
    Line::from(spans)
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
