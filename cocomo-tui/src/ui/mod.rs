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

pub(crate) mod app;
mod diff_list;
mod dir_compare;
mod frame;
mod menu_bar;
mod rows;
mod session_table;
mod sync_confirm;
mod tab_bar;
mod text_diff;
pub(crate) mod theme;
mod transfer_dialog;
mod welcome;
