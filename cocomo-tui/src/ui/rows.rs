// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Pre-computed row data for the comparison tables.

use ratatui_kit::ratatui::style::Style;

/// Pre-computed rendering data for a single row in the text diff table.
///
/// Styles and formatted strings are computed in the component body (before
/// `element!`) so that the `render_row` callback has everything it needs
/// without capturing external data.  This mirrors how the legacy
/// `render_text_main` computed per-line styles using grammar classification
/// and diff highlighting.
#[derive(Clone)]
pub(super) struct TextDiffRow {
    /// Left line number, right-aligned in a 6-char field.
    pub left_num: String,
    /// Diff marker for the left side ("\u{25B6} " if different, "  "
    /// otherwise).
    pub left_marker: &'static str,
    /// Left line content (empty when the file has no line at this index).
    pub left_content: String,
    /// Style for the left marker and content (red bold for diffs, grammar
    /// style otherwise).
    pub left_style: Style,
    /// Right line number, right-aligned in a 6-char field.
    pub right_num: String,
    /// Diff marker for the right side ("\u{25B6} " if different, "  "
    /// otherwise).
    pub right_marker: &'static str,
    /// Right line content (empty when the file has no line at this index).
    pub right_content: String,
    /// Style for the right marker and content (green bold for diffs, grammar
    /// style otherwise).
    pub right_style: Style,
}
