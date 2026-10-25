// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Sync confirmation modal: the approval step of `S` (sync).
//!
//! Once the dry-run plan is ready, the controller puts the session into
//! [`SessionMode::SyncConfirm`]; this component turns that mode into the
//! framework's `ConfirmModal`, so the whole planned batch is approved or
//! rejected with a single key: `y`/`Enter` executes, `n`/`N`/`Esc` rejects
//! (the default button highlight is Cancel, so a bare `Enter` cannot
//! execute by accident). There is no per-item review: the preview table
//! behind the modal lists every planned transfer, and rejecting keeps it on
//! screen for closer inspection.
//!
//! Like `TransferDialog` the component lives in the App shell and reads the
//! active session, so one implementation serves every dir tab.
//! `ConfirmModal` owns an exclusive input layer while open, which is what
//! keeps the table and the shell from reacting behind the modal.

use ratatui_kit::{prelude::*, ratatui::text::Line};

use crate::{
    controller::{confirm_sync, reject_sync},
    state::{APP_STATE, SessionMode, SyncStage},
};

/// Sync confirmation modal rendered above the active session.
#[component]
pub(super) fn SyncConfirmModal(
    mut hooks: Hooks,
) -> impl Into<AnyElement<'static>> {
    let app_handle = hooks.use_atom(&APP_STATE);

    // Resolve the pending plan from the active session; the modal only
    // opens on a planned (not yet executed) sync. The content line names
    // the operation and the number of transfers the approval would run.
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
            Some(format!(
                "{}: {} transfer{} planned.",
                dir.sync_operation.label(),
                count,
                if count == 1 { "" } else { "s" }
            ))
        });
    let open = content.is_some();

    element!(
        ConfirmModal(
            open: open,
            title: Line::from(" Sync "),
            content: content.unwrap_or_default(),
            confirm_text: "Execute".to_string(),
            cancel_text: "Cancel".to_string(),
            on_confirm: move |_: ()| {
                confirm_sync();
            },
            on_cancel: move |_: ()| {
                reject_sync();
            },
        )
    )
}
