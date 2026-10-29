// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Sync approval modal: the confirmation step of `S` (sync).
//!
//! Once the dry-run plan is ready, the controller puts the session into
//! [`SessionMode::SyncConfirm`]; this component turns that mode into an
//! [`OptionList`] with the two decisions of the whole planned batch: `[y]`
//! executes, `[n]`/`Esc` rejects. The cursor starts on Cancel, so a bare
//! `Enter` cannot execute by accident. Above the decisions the
//! [`change_summary`] of the planned items lists the change the approval
//! would apply (per-action totals and affected paths, collapsed beyond a
//! cap); the preview table behind the modal still lists every planned
//! transfer in full, and rejecting keeps it on screen for closer
//! inspection.
//!
//! Like `TransferDialog` the component lives in the App shell and reads the
//! active session, so one implementation serves every dir tab. It owns an
//! exclusive input layer while open: its handler runs in that layer with
//! `EventPriority::High` and owns `y`/`n`/`Esc` (the `KeyScope::SyncConfirm`
//! bindings), while `j`/`k`/`Enter` reach the `OptionList` child in the same
//! layer. That exclusivity is what keeps the table and the shell from
//! reacting behind the modal.

use ratatui_kit::{
    crossterm::event::{Event, KeyEventKind},
    prelude::*,
    ratatui::{
        layout::{Constraint, Direction},
        text::{Line, Text as RataText},
        widgets::Padding,
    },
};

use super::{
    frame::hint_line,
    option_list::{OptionItem, OptionList},
    summary::change_summary,
    theme::CocomoTheme,
};
use crate::{
    controller::{confirm_sync, dispatch_sync_confirm_key, reject_sync},
    state::{APP_STATE, SessionMode, SyncStage},
};

/// Sync approval modal rendered above the active session.
#[component]
pub(super) fn SyncConfirmModal(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);
    let theme = hooks.use_component_theme::<CocomoTheme>();

    // Resolve the pending plan from the active session; the modal only
    // opens on a planned (not yet executed) sync. The first content line
    // names the operation and the number of transfers the approval would
    // run; the [`change_summary`] below it spells out the concrete changes.
    let content = app_handle
        .read()
        .active()
        .filter(|s| matches!(s.ui.mode, SessionMode::SyncConfirm))
        .and_then(|s| {
            let dir = s.dir()?;
            let SyncStage::Planned(result) = dir.sync.as_ref()? else {
                return None;
            };
            let count = result.planned_count();
            let mut lines = vec![Line::from(format!(
                "{}: {} transfer{} planned.",
                dir.sync_operation.label(),
                count,
                if count == 1 { "" } else { "s" }
            ))];
            // Name clashes are excluded from the plan; show them here so
            // the approval covers what will *not* happen, too.
            for conflict in &result.conflicts {
                lines.push(Line::from(format!("skipped: {conflict}")));
            }
            lines.extend(change_summary(&result.planned, &theme));
            Some(lines)
        });
    let open = content.is_some();

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

            // The modal owns `y`/`n`/`Esc` (via the keymap); an unowned key
            // stays unclaimed here, so the `OptionList` child in the same
            // layer gets `j`/`k`/`Enter`.
            if dispatch_sync_confirm_key(key) {
                EventResult::Consumed
            } else {
                EventResult::Ignored
            }
        },
    );

    let Some(content) = content else {
        // Closed: a zero-sized `Modal` instead of a `Fragment` whose default
        // `Fill(1)` height would steal the main view's rows. Returning early
        // also unmounts the option list, so every open starts on Cancel.
        return element!(Modal(open: false)).into_any();
    };

    // Two border rows plus the padding around the content: the plan
    // summary and the two decision rows.
    let height = 4 + content.len() as u16 + 2;

    element!(
        Modal(
            open: true,
            // Reuse this component's layer instead of letting `Modal` mint
            // its own, which would deafen the handler above.
            layer: Some(layer),
            // Wide enough for the summary's action label and path columns.
            width: Constraint::Percentage(60),
            height: Constraint::Length(height),
        ) {
            Border(
                flex_direction: Direction::Vertical,
                border_style: theme.border,
                top_title: Line::from(" Sync ")
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
                Text(text: RataText::from(content))
                OptionList<bool>(
                    items: vec![
                        OptionItem::new(true, "Execute").key("y"),
                        OptionItem::new(false, "Cancel").key("n"),
                    ],
                    on_select: Handler::from(move |execute: bool| {
                        if execute {
                            confirm_sync();
                        } else {
                            reject_sync();
                        }
                    }),
                    // Highlighting Cancel keeps an accidental `Enter` from
                    // running the batch.
                    default_index: Some(1),
                    // The two decision rows; the summary above keeps the
                    // remaining row of the modal.
                    height: Constraint::Length(2),
                )
            }
        }
    )
    .into_any()
}
