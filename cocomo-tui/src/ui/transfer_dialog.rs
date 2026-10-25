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
//! The dialog shows the selected line, then the operations that apply to it
//! — copy left → right / right → left, or delete left / right / both — plus
//! `abort`. It is the confirmation for destructive work, so a single
//! deliberate choice stands between the selection and the filesystem.
//!
//! The component lives in the App shell (not in a session view) so one
//! implementation serves both comparison views. Like `AlertModal` it owns an
//! exclusive input layer while open, which is what keeps the table and the
//! shell from reacting behind the dialog. Navigation and `Enter` inside the
//! dialog belong to the framework's `Select`; only the accelerators and
//! `Esc` go through the keymap (`KeyScope::Dialog`).

use cocomo_lib::{DirEntry, EntryInfo, format_date, format_size};
use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        style::{Modifier, Style},
        text::{Line, Span, Text as RataText},
        widgets::{ListItem, Padding},
    },
};

use super::theme::CocomoTheme;
use crate::{
    controller::{confirm_transfer, dispatch_dialog_key},
    state::{
        APP_STATE, SessionContent, SessionMode, SessionView, TransferChoice,
        TransferOp, TransferTarget,
    },
};

/// One row of the dialog's option list.
///
/// Carries the choice alongside its label so `Select` can hand the choice
/// back to the controller without an index lookup that could drift from the
/// option list.
#[derive(Clone)]
struct DialogOption {
    choice: TransferChoice,
    label: String,
}

impl From<DialogOption> for ListItem<'static> {
    fn from(option: DialogOption) -> Self {
        ListItem::new(Line::from(option.label))
    }
}

/// Everything the dialog renders, resolved from the active session.
struct DialogData {
    /// Which operation the dialog offers.
    op: TransferOp,
    /// The selected line, rendered so the user sees what they are acting on.
    summary: Vec<Line<'static>>,
    /// The options that apply to the entry, in display order.
    options: Vec<DialogOption>,
}

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
            let target = s.transfer_target()?;
            Some(DialogData {
                op,
                summary: summary_lines(s, &target, &theme),
                options: options(op, &target),
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

            // `j`/`k`/`Enter` belong to the `Select` in this layer, so an
            // unowned key must stay unclaimed or the list stops moving.
            if dispatch_dialog_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    // Flatten the dialog data into render values; closed means empty.
    let (title, summary, options) = match &dialog {
        Some(data) => (
            Line::from(format!(" {} ", data.op.title()))
                .style(theme.title)
                .centered(),
            data.summary.clone(),
            data.options.clone(),
        ),
        None => (Line::from(""), Vec::new(), Vec::new()),
    };
    // Two border rows plus the padding around the content.
    let height = 4 + summary.len() as u16 + options.len().max(1) as u16;

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
                bottom_title: Line::from(" j/k move  Enter run  Esc abort ")
                    .style(theme.dim)
                    .centered(),
                padding: Padding::uniform(1),
            ) {
                Text(text: RataText::from(summary))
                Select<DialogOption>(
                    items: options,
                    // Highlight the first option immediately, so `Enter`
                    // needs no prior navigation.
                    default_index: Some(0),
                    on_select: Handler::from(move |option: DialogOption| {
                        confirm_transfer(option.choice);
                    }),
                )
            }
        }
    )
}

/// Build the option list for `op` restricted to the sides that hold the
/// entry, each labelled with its accelerator.
fn options(op: TransferOp, target: &TransferTarget) -> Vec<DialogOption> {
    op.choices(target.has_left, target.has_right)
        .into_iter()
        .map(|choice| DialogOption {
            choice,
            label: format!("[{}] {}", choice.key(), choice.label(op)),
        })
        .collect()
}

/// Render the selected line: what the transfer would act on.
///
/// A directory session shows the entry row (name, both sides' metadata, and
/// the status symbol); a text session shows the two compared paths. When no
/// option applies, the reason from [`TransferTarget::note`] is appended.
fn summary_lines(
    session: &SessionView,
    target: &TransferTarget,
    theme: &CocomoTheme,
) -> Vec<Line<'static>> {
    let mut lines = match &session.content {
        SessionContent::Dir(_) => {
            let entries = session.filtered_entries();
            match session
                .ui
                .table_state
                .selected()
                .and_then(|i| entries.get(i))
            {
                Some(entry) => entry_lines(entry, theme),
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
    if let Some(note) = target.note {
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
