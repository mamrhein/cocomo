// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! ratatui-kit UI components: the root `App` shell, the menu and tab bars,
//! the welcome screen, the two comparison views, and the shared
//! scaffolding (frame, table, diff list) and theme they build on.
//!
//! The module also mirrors the entry point's mouse-capture decision in
//! [`set_mouse_enabled`]/[`mouse_enabled`] so components can gate
//! pointer-hook registration; see those functions for why the app cannot
//! read the decision from the framework.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the app entry enabled terminal mouse capture.
///
/// ratatui-kit keeps `SystemContext::mouse_enabled` crate-private, so the
/// app mirrors the `RunOptions { mouse: true }` decision here: [`run`]
/// (in the crate root) calls [`set_mouse_enabled`] before entering the
/// render loop, and components gate `use_pointer` registration on
/// [`mouse_enabled`]. The flag never changes mid-process, so the
/// conditional hook registration keeps a stable hook order across frames
/// — the same contract the framework's built-in components follow with
/// their internal `pointer_enabled` check. Offscreen tests
/// (`render_frames`) never run the entry point, so they keep the default
/// `false` and register no pointer hooks, exactly like the built-ins.
///
/// [`run`]: crate::run
static MOUSE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Record that the entry point enabled mouse capture.
pub(crate) fn set_mouse_enabled() {
    MOUSE_ENABLED.store(true, Ordering::Relaxed);
}

/// Whether the entry point enabled mouse capture.
pub(crate) fn mouse_enabled() -> bool {
    MOUSE_ENABLED.load(Ordering::Relaxed)
}

pub(crate) mod app;
mod diff_list;
mod dir_compare;
mod filter_panel;
mod frame;
mod menu_bar;
mod option_list;
mod report_modal;
mod rows;
mod session_browser;
mod session_table;
mod settings_modal;
mod summary;
mod sync_confirm;
mod tab_bar;
mod text_diff;
pub(crate) mod theme;
mod transfer_dialog;
mod welcome;
