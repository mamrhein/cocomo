// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Side-by-side text compare view (UI spec section 5).
//!
//! [`TextDiffSession`] owns one text comparison: the loaded bytes of both
//! files, the [`TextDiff`] from `cocomo_lib`, and the aligned row model
//! derived from it. [`TextDiffView`] renders the model as two
//! scroll-synchronized virtual lists with a gutter (arrows and section
//! collapse boxes), per-pane file bars, an optional thumbnail strip,
//! read-only detail panes, and a status bar.
//!
//! The alignment model inserts blank counterpart cells so both panes show
//! the same row at the same height; difference rows group into *sections*
//! that can be collapsed to a "N FILTERED LINES" header in either pane.

use std::{
    cell::Cell, collections::HashSet, rc::Rc, sync::Arc, time::Instant,
};

use chrono::{DateTime, Local, Utc};
use cocomo_lib::{
    ColorOverrides, Endpoint, FsError, Grammar, Importance, LineEnding,
    LineInfo, NodeFileSystem, TextCompareSettings, TextDiff, TextDifference,
    TextEncoding, WhitespaceMode, compare_texts, format_size,
};
use gpui::{
    Action, AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, Div,
    Element, ElementId, Entity, FocusHandle, GlobalElementId, Hitbox, Hsla,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, ListAlignment,
    ListScrollEvent, ListState, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PathBuilder, Pixels, Render, SharedString, Style,
    Subscription, WeakEntity, Window, actions,
    assets::IconName,
    base::Disableable as _,
    component::{
        ActiveTheme, Sizable as _, Theme, WindowExt as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{
            Editor, EditorState, Input, InputEvent, InputState,
            RangeDecoration, RangeDecorationCollection, RangeDecorationStyle,
            RopeExt,
        },
        menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenuItem},
        resizable::{h_resizable, resizable_panel},
        spinner::Spinner,
        status_bar::StatusBar,
        toolbar::{Toolbar, ToolbarGroup},
        v_flex,
    },
    div, fill, list, outline, point,
    prelude::*,
    px, relative, size,
};

use crate::{
    color::{ColorRole, Side},
    log::{self, LogLevel},
    menus::{
        NextSection, PrevSection, ReloadCompare, SessionInfo, SessionReport,
        ShowThumbnails, ToggleWordWrap,
    },
    session_manager::GuiSessionManager,
    state::AppState,
    ui::{NextDifference, PrevDifference, SwapSides},
};

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

actions!(
    text_compare,
    [
        NextLine,
        PrevLine,
        ToggleTextEdit,
        SelectSection,
        DeleteLine,
        InsertLineBefore,
        InsertLineAfter,
        ConvertTrimTrailing,
        ConvertSpacesToTabs,
        ConvertTabsToSpaces,
        ConvertEndingsPc,
        ConvertEndingsMac,
        ConvertEndingsUnix,
        CompareSelectionToClipboard,
        AlignWithAnchor,
        ClearAlignments,
        IsolateSelection,
        CopySectionRight,
        CopySectionLeft,
        CopyFileRightNext,
        CopyFileLeftNext,
    ]
);

/// Mark the clicked row's line on one side as the alignment anchor.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = text_compare, no_json)]
pub(crate) struct SetAlignAnchor {
    /// The side whose line is marked.
    pub(crate) side: Side,
}

/// Select which detail pane pair the view shows below the diff.
#[derive(Action, Clone, Debug, PartialEq)]
#[action(namespace = text_compare, no_json)]
pub(crate) struct SetDetailMode {
    /// The detail mode to switch to.
    pub(crate) mode: DetailMode,
}

/// Which pair of detail panes is shown below the diff.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DetailMode {
    /// The selected line on each side, editable.
    #[default]
    Text,
    /// A read-only hex dump of the selected line's bytes.
    Hex,
    /// The selected lines with a caret under the first differing char.
    Alignment,
}

/// A whole-buffer conversion from the Convert File menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvertOp {
    TrimTrailing,
    SpacesToTabs,
    TabsToSpaces,
    EndingsPc,
    EndingsMac,
    EndingsUnix,
}

/// Split on CR, LF, and CRLF (unlike `str::lines`, a lone CR ends a
/// line), without producing a trailing empty line.
fn split_any_line(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let (mut start, mut i) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'\r' | b'\n' => {
                lines.push(&text[start..i]);
                i += if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Rewrite line endings to one convention, preserving whether the text
/// ends with a newline.
pub(crate) fn normalize_endings(text: &str, ending: LineEnding) -> String {
    let sep = match ending {
        LineEnding::Crlf => "\r\n",
        LineEnding::Cr => "\r",
        LineEnding::Lf | LineEnding::Ignore => "\n",
    };
    let ends_nl = text.ends_with('\n') || text.ends_with('\r');
    let mut out = split_any_line(text).join(sep);
    if ends_nl {
        out.push_str(sep);
    }
    out
}

/// Apply a Convert File operation to a whole buffer.
fn convert_text(text: &str, op: ConvertOp) -> String {
    match op {
        ConvertOp::TrimTrailing => {
            let ends_nl = text.ends_with('\n') || text.ends_with('\r');
            let mut out = split_any_line(text)
                .into_iter()
                .map(str::trim_end)
                .collect::<Vec<_>>()
                .join("\n");
            if ends_nl {
                out.push('\n');
            }
            out
        }
        ConvertOp::SpacesToTabs => {
            let ends_nl = text.ends_with('\n') || text.ends_with('\r');
            let mut out = String::new();
            for line in split_any_line(text) {
                // Only leading indentation is converted, in groups of
                // four, mirroring the editor's tab size.
                let indent = line.len() - line.trim_start_matches(' ').len();
                out.push_str(&"\t".repeat(indent / 4));
                out.push_str(&" ".repeat(indent % 4));
                out.push_str(&line[indent..]);
                out.push('\n');
            }
            if !ends_nl && out.ends_with('\n') {
                out.pop();
            }
            out
        }
        ConvertOp::TabsToSpaces => text.replace('\t', "    "),
        ConvertOp::EndingsPc => normalize_endings(text, LineEnding::Crlf),
        ConvertOp::EndingsMac => normalize_endings(text, LineEnding::Cr),
        ConvertOp::EndingsUnix => normalize_endings(text, LineEnding::Lf),
    }
}

/// Encode edited text for writing back to disk.
pub(crate) fn encode_text(
    text: &str,
    ending: LineEnding,
    encoding: TextEncoding,
) -> Vec<u8> {
    let text = normalize_endings(text, ending);
    match encoding {
        // Latin-1 is the byte-for-byte inverse of `decode_bytes`.
        TextEncoding::Latin1 => text.chars().map(|c| c as u8).collect(),
        _ => text.into_bytes(),
    }
}

/// A read-only hex dump of one line's bytes, 16 per row.
fn hex_dump(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return "—".to_string();
    }
    let mut out = String::new();
    for (row, chunk) in bytes.chunks(16).enumerate() {
        let hex = chunk
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        let ascii: String = chunk
            .iter()
            .map(|&b| {
                if (0x20..0x7F).contains(&b) {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();
        out.push_str(&format!("{row:04X}  {hex:<47}  |{ascii}|\n"));
    }
    out.truncate(out.len() - 1);
    out
}

/// The column of the first differing character on an alignment row.
fn first_difference_caret(left: &str, right: &str) -> String {
    let pos = left
        .chars()
        .zip(right.chars())
        .take_while(|(l, r)| l == r)
        .count();
    format!("{}^", " ".repeat(pos))
}

// ---------------------------------------------------------------------------
// Diff alignment model
// ---------------------------------------------------------------------------

/// Status of one aligned row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowStatus {
    /// Both sides carry the same line.
    Same,
    /// Both sides carry a line, but the contents differ.
    Changed,
    /// Only the right side carries the line.
    Inserted,
    /// Only the left side carries the line.
    Deleted,
}

impl RowStatus {
    /// Whether this row participates in a difference.
    pub(crate) fn is_difference(self) -> bool {
        self != RowStatus::Same
    }

    /// Gutter arrow for a difference row, per spec section 5.2.
    pub(crate) fn gutter_arrow(self) -> Option<&'static str> {
        match self {
            Self::Same => None,
            Self::Inserted => Some("→"),
            Self::Deleted => Some("←"),
            Self::Changed => Some("↔"),
        }
    }

    /// Short label for the status bar's state field.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Same => "Same",
            Self::Changed => "Change",
            Self::Inserted => "Insert",
            Self::Deleted => "Delete",
        }
    }
}

/// One row of the side-by-side alignment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DiffRow {
    /// 1-based left line number, `None` for an inserted row.
    pub(crate) left: Option<usize>,
    /// 1-based right line number, `None` for a deleted row.
    pub(crate) right: Option<usize>,
    /// Alignment status of the row.
    pub(crate) status: RowStatus,
    /// Whether every token on the row is comment-or-lower importance.
    pub(crate) minor: bool,
}

/// Whether all tokens of the given lines are of minor importance.
///
/// Minor means comment-or-lower; lines without tokens (no grammar was
/// applied) are *not* minor, so an unclassified diff stays important.
fn lines_are_minor<'a>(lines: impl IntoIterator<Item = &'a LineInfo>) -> bool {
    let mut saw_token = false;
    for line in lines {
        for token in &line.tokens {
            saw_token = true;
            if token.importance > Importance::Comment {
                return false;
            }
        }
    }
    saw_token
}

/// Push aligned context rows until both cursors reach their targets, then
/// emit any leftover left rows as deleted and right rows as inserted.
///
/// Cursors are 0-based line indices; targets are exclusive 0-based bounds.
fn push_context(
    rows: &mut Vec<DiffRow>,
    l: &mut usize,
    r: &mut usize,
    left_target: usize,
    right_target: usize,
) {
    while *l < left_target && *r < right_target {
        rows.push(DiffRow {
            left: Some(*l + 1),
            right: Some(*r + 1),
            status: RowStatus::Same,
            minor: false,
        });
        *l += 1;
        *r += 1;
    }
    while *l < left_target {
        rows.push(DiffRow {
            left: Some(*l + 1),
            right: None,
            status: RowStatus::Deleted,
            minor: false,
        });
        *l += 1;
    }
    while *r < right_target {
        rows.push(DiffRow {
            left: None,
            right: Some(*r + 1),
            status: RowStatus::Inserted,
            minor: false,
        });
        *r += 1;
    }
}

/// Force the given `(left, right)` line pairs onto shared rows.
///
/// Applied after [`align_rows`]; each override either merges a deleted
/// row with an inserted row into one changed row, or swaps partners
/// between two changed rows. Pairs that cannot be honored (already
/// paired, or shapes the model cannot express) are left alone.
fn apply_align_overrides(
    rows: &mut Vec<DiffRow>,
    overrides: &[(usize, usize)],
) {
    for &(l, r) in overrides {
        let Some(li) = rows.iter().position(|row| row.left == Some(l)) else {
            continue;
        };
        let Some(ri) = rows.iter().position(|row| row.right == Some(r)) else {
            continue;
        };
        if li == ri {
            continue;
        }
        let (lrow, rrow) = (rows[li], rows[ri]);
        if lrow.status == RowStatus::Changed
            && rrow.status == RowStatus::Changed
        {
            // Re-pair: the anchor pair wins, the displaced right line
            // keeps the other row's left line.
            rows[li].right = Some(r);
            rows[ri].right = lrow.right;
        } else if lrow.status == RowStatus::Deleted
            && rrow.status == RowStatus::Inserted
        {
            let (first, second) = (li.min(ri), li.max(ri));
            rows[first] = DiffRow {
                left: Some(l),
                right: Some(r),
                status: RowStatus::Changed,
                minor: false,
            };
            rows.remove(second);
        }
    }
}

/// Align a [`TextDiff`] into side-by-side rows.
///
/// Context between differences is aligned pairwise; added and removed
/// blocks get a blank counterpart cell on the other side.
pub(crate) fn align_rows(diff: &TextDiff) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let (mut l, mut r) = (0usize, 0usize);
    for difference in &diff.differences {
        match difference {
            TextDifference::LineDifferent(left, right) => {
                push_context(
                    &mut rows,
                    &mut l,
                    &mut r,
                    left.number - 1,
                    right.number - 1,
                );
                let minor = lines_are_minor([left, right]);
                rows.push(DiffRow {
                    left: Some(left.number),
                    right: Some(right.number),
                    status: RowStatus::Changed,
                    minor,
                });
                l = left.number;
                r = right.number;
            }
            TextDifference::LinesAdded(start, lines) => {
                let right_target = start - 1;
                let left_target = l + right_target.saturating_sub(r);
                push_context(
                    &mut rows,
                    &mut l,
                    &mut r,
                    left_target,
                    right_target,
                );
                for line in lines {
                    let minor = lines_are_minor([line]);
                    rows.push(DiffRow {
                        left: None,
                        right: Some(line.number),
                        status: RowStatus::Inserted,
                        minor,
                    });
                }
                r = right_target + lines.len();
            }
            TextDifference::LinesRemoved(start, lines) => {
                let left_target = start - 1;
                let right_target = r + left_target.saturating_sub(l);
                push_context(
                    &mut rows,
                    &mut l,
                    &mut r,
                    left_target,
                    right_target,
                );
                for line in lines {
                    let minor = lines_are_minor([line]);
                    rows.push(DiffRow {
                        left: Some(line.number),
                        right: None,
                        status: RowStatus::Deleted,
                        minor,
                    });
                }
                l = left_target + lines.len();
            }
            TextDifference::LinesChanged(start, removed, added) => {
                let left_target = start - 1;
                let right_target = r + left_target.saturating_sub(l);
                push_context(
                    &mut rows,
                    &mut l,
                    &mut r,
                    left_target,
                    right_target,
                );
                let paired = removed.len().min(added.len());
                for i in 0..paired {
                    let minor = lines_are_minor([&removed[i], &added[i]]);
                    rows.push(DiffRow {
                        left: Some(removed[i].number),
                        right: Some(added[i].number),
                        status: RowStatus::Changed,
                        minor,
                    });
                }
                for line in &removed[paired..] {
                    let minor = lines_are_minor([line]);
                    rows.push(DiffRow {
                        left: Some(line.number),
                        right: None,
                        status: RowStatus::Deleted,
                        minor,
                    });
                }
                for line in &added[paired..] {
                    let minor = lines_are_minor([line]);
                    rows.push(DiffRow {
                        left: None,
                        right: Some(line.number),
                        status: RowStatus::Inserted,
                        minor,
                    });
                }
                l = left_target + removed.len();
                r = right_target + added.len();
            }
            // A blank-line classification difference carries no line
            // numbers to align on; ignore it.
            TextDifference::BlankLine => {}
        }
    }
    push_context(
        &mut rows,
        &mut l,
        &mut r,
        diff.left_line_count,
        diff.right_line_count,
    );
    rows
}

/// Kind of a difference section, for the report and gutter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SectionKind {
    /// The section only inserts lines on the right.
    Insert,
    /// The section only deletes lines from the left.
    Delete,
    /// The section changes lines on both sides.
    Change,
}

/// A maximal run of consecutive difference rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DiffSection {
    /// Aggregated kind of the section's rows.
    pub(crate) kind: SectionKind,
    /// Whether every row of the section is minor.
    pub(crate) minor: bool,
    /// First row index of the section.
    pub(crate) start_row: usize,
    /// One past the last row index of the section.
    pub(crate) end_row: usize,
}

/// Group consecutive difference rows into sections.
pub(crate) fn diff_sections(rows: &[DiffRow]) -> Vec<DiffSection> {
    let mut sections = Vec::new();
    let mut start: Option<usize> = None;
    for (ix, row) in rows.iter().enumerate() {
        if row.status.is_difference() {
            if start.is_none() {
                start = Some(ix);
            }
        } else if let Some(s) = start.take() {
            sections.push(make_section(rows, s, ix));
        }
    }
    if let Some(s) = start {
        sections.push(make_section(rows, s, rows.len()));
    }
    sections
}

fn make_section(rows: &[DiffRow], start: usize, end: usize) -> DiffSection {
    let (mut has_insert, mut has_delete, mut has_change) =
        (false, false, false);
    let mut minor = true;
    for row in &rows[start..end] {
        match row.status {
            RowStatus::Inserted => has_insert = true,
            RowStatus::Deleted => has_delete = true,
            RowStatus::Changed => has_change = true,
            RowStatus::Same => {}
        }
        minor &= row.minor;
    }
    let kind = if has_change || (has_insert && has_delete) {
        SectionKind::Change
    } else if has_insert {
        SectionKind::Insert
    } else {
        SectionKind::Delete
    };
    DiffSection {
        kind,
        minor,
        start_row: start,
        end_row: end,
    }
}

/// One row of the *displayed* list: either a diff row or a collapsed
/// section's filtered-lines header.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum DisplayRow {
    /// Display index maps to this row index.
    Line(usize),
    /// Collapsed section header for `count` hidden rows.
    Filtered { section: usize, count: usize },
}

/// Build the display list from the rows and the collapsed sections.
///
/// Returns the display rows, the row-index → display-index map, and the
/// section-index → display-index map (`usize::MAX` for absent entries).
pub(crate) fn display_rows(
    rows: &[DiffRow],
    sections: &[DiffSection],
    collapsed: &HashSet<usize>,
) -> (Vec<DisplayRow>, Vec<usize>, Vec<usize>) {
    let mut section_of_row = vec![usize::MAX; rows.len()];
    for (si, section) in sections.iter().enumerate() {
        for slot in &mut section_of_row[section.start_row..section.end_row] {
            *slot = si;
        }
    }
    let mut display = Vec::with_capacity(rows.len());
    let mut line_display = vec![usize::MAX; rows.len()];
    let mut section_display = vec![usize::MAX; sections.len()];
    let mut i = 0;
    while i < rows.len() {
        let si = section_of_row[i];
        if si != usize::MAX && collapsed.contains(&si) {
            let section = &sections[si];
            section_display[si] = display.len();
            display.push(DisplayRow::Filtered {
                section: si,
                count: section.end_row - section.start_row,
            });
            i = section.end_row;
        } else {
            line_display[i] = display.len();
            display.push(DisplayRow::Line(i));
            i += 1;
        }
    }
    (display, line_display, section_display)
}

/// Next display index at or after `from` (wrapping) that shows a
/// difference row.
fn next_difference_index(
    display: &[DisplayRow],
    rows: &[DiffRow],
    from: usize,
) -> Option<usize> {
    if display.is_empty() {
        return None;
    }
    let n = display.len();
    for k in 0..n {
        let d = (from + k) % n;
        if let DisplayRow::Line(r) = display[d]
            && rows[r].status.is_difference()
        {
            return Some(d);
        }
    }
    None
}

/// Previous display index before `from` (wrapping) that shows a
/// difference row.
fn prev_difference_index(
    display: &[DisplayRow],
    rows: &[DiffRow],
    from: usize,
) -> Option<usize> {
    if display.is_empty() {
        return None;
    }
    let n = display.len();
    for k in 1..=n {
        let d = (from + n - k) % n;
        if let DisplayRow::Line(r) = display[d]
            && rows[r].status.is_difference()
        {
            return Some(d);
        }
    }
    None
}

/// Next section header/first-row display index after `selected`,
/// wrapping to the first section.
fn next_section_index(
    section_display: &[usize],
    selected: Option<usize>,
) -> Option<usize> {
    let present: Vec<usize> = section_display
        .iter()
        .copied()
        .filter(|&d| d != usize::MAX)
        .collect();
    match selected {
        None => present.first().copied(),
        Some(cur) => present
            .iter()
            .copied()
            .find(|&d| d > cur)
            .or_else(|| present.first().copied()),
    }
}

/// Previous section display index before `selected`, wrapping to the last.
fn prev_section_index(
    section_display: &[usize],
    selected: Option<usize>,
) -> Option<usize> {
    let present: Vec<usize> = section_display
        .iter()
        .copied()
        .filter(|&d| d != usize::MAX)
        .collect();
    match selected {
        None => present.last().copied(),
        Some(cur) => present
            .iter()
            .copied()
            .rev()
            .find(|&d| d < cur)
            .or_else(|| present.last().copied()),
    }
}

/// Runs of the thumbnail strip: (start fraction, end fraction, role).
///
/// Consecutive difference rows of the same severity merge into one run so
/// the strip paints one quad per run instead of one per line.
pub(crate) fn thumbnail_runs(
    display: &[DisplayRow],
    rows: &[DiffRow],
) -> Vec<(f32, f32, ColorRole)> {
    let total = display.len();
    if total == 0 {
        return Vec::new();
    }
    let mut runs: Vec<(f32, f32, ColorRole)> = Vec::new();
    for (d, item) in display.iter().enumerate() {
        let DisplayRow::Line(r) = item else { continue };
        let row = &rows[*r];
        if !row.status.is_difference() {
            continue;
        }
        let role = if row.minor {
            ColorRole::Minor
        } else {
            ColorRole::Important
        };
        let start = d as f32 / total as f32;
        let end = (d + 1) as f32 / total as f32;
        match runs.last_mut() {
            Some(last)
                if last.2 == role && (last.1 - start).abs() < f32::EPSILON =>
            {
                last.1 = end;
            }
            _ => runs.push((start, end, role)),
        }
    }
    runs
}

// ---------------------------------------------------------------------------
// Text decoding helpers
// ---------------------------------------------------------------------------

/// Decode raw file bytes under the selected encoding.
///
/// UTF-8 is lossy so undecodable bytes never abort a comparison; ANSI is
/// treated as Latin-1, which maps bytes 1:1 onto the first Unicode plane.
pub(crate) fn decode_bytes(bytes: &[u8], encoding: TextEncoding) -> String {
    match encoding {
        TextEncoding::Latin1 => bytes.iter().map(|&b| char::from(b)).collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Split decoded text into 1-based [`LineInfo`] rows, treating CR, LF, and
/// CRLF all as line endings. A trailing newline does not add an empty line.
fn split_lines(text: &str) -> Vec<LineInfo> {
    let mut lines = Vec::new();
    let bytes = text.as_bytes();
    let (mut start, mut i, mut number) = (0usize, 0usize, 1usize);
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                lines.push(LineInfo::new(number, text[start..i].to_string()));
                number += 1;
                i += if bytes.get(i + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = i;
            }
            b'\n' => {
                lines.push(LineInfo::new(number, text[start..i].to_string()));
                number += 1;
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < text.len() {
        lines.push(LineInfo::new(number, text[start..].to_string()));
    }
    lines
}

/// Detect the dominant line-ending convention from raw bytes.
fn detect_line_ending(bytes: &[u8]) -> LineEnding {
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                return if bytes.get(i + 1) == Some(&b'\n') {
                    LineEnding::Crlf
                } else {
                    LineEnding::Cr
                };
            }
            b'\n' => return LineEnding::Lf,
            _ => i += 1,
        }
    }
    LineEnding::Lf
}

/// Display label for an encoding selector entry.
fn encoding_label(encoding: TextEncoding) -> &'static str {
    match encoding {
        TextEncoding::Latin1 => "ANSI",
        _ => "UTF-8",
    }
}

/// Display label for a line-ending selector entry.
fn line_ending_label(ending: LineEnding) -> &'static str {
    match ending {
        LineEnding::Crlf => "PC",
        LineEnding::Cr => "Mac",
        LineEnding::Lf => "Unix",
        LineEnding::Ignore => "—",
    }
}

// ---------------------------------------------------------------------------
// Session state
// ---------------------------------------------------------------------------

/// Per-side file metadata shown in the file bar.
#[derive(Clone, Debug)]
struct FileMeta {
    /// Modification time, formatted for display.
    modified: String,
    /// Size in bytes.
    size: u64,
}

impl FileMeta {
    fn from_parts(size: u64, modified: DateTime<Utc>) -> Self {
        Self {
            modified: modified
                .with_timezone(&Local)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
            size,
        }
    }
}

/// Statistics for the Info dialog.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InfoStats {
    /// Lines in the left file.
    pub(crate) left_lines: usize,
    /// Lines in the right file.
    pub(crate) right_lines: usize,
    /// Number of difference sections.
    pub(crate) sections: usize,
    /// Lines inserted on the right.
    pub(crate) insertions: usize,
    /// Lines deleted from the left.
    pub(crate) deletions: usize,
    /// Lines changed on both sides.
    pub(crate) changes: usize,
}

/// State of one text compare session.
///
/// Owns the endpoints, the raw bytes and decoded lines of both sides, the
/// diff model, and the view options (wrap, thumbnail, collapsed sections,
/// selection). The view observes this entity and re-renders on notify.
pub struct TextDiffSession {
    /// Focus handle shared with the view.
    focus_handle: FocusHandle,
    /// Tab title.
    title: SharedString,
    /// Left file endpoint.
    left: Endpoint,
    /// Right file endpoint.
    right: Endpoint,
    /// Raw left bytes from the last load.
    left_raw: Option<Vec<u8>>,
    /// Raw right bytes from the last load.
    right_raw: Option<Vec<u8>>,
    /// Decoded left lines.
    left_lines: Arc<Vec<LineInfo>>,
    /// Decoded right lines.
    right_lines: Arc<Vec<LineInfo>>,
    /// Left file metadata, if the file exists.
    left_meta: Option<FileMeta>,
    /// Right file metadata, if the file exists.
    right_meta: Option<FileMeta>,
    /// Left read encoding.
    left_encoding: TextEncoding,
    /// Right read encoding.
    right_encoding: TextEncoding,
    /// Left line-ending convention (detected, selectable).
    left_ending: LineEnding,
    /// Right line-ending convention (detected, selectable).
    right_ending: LineEnding,
    /// Previously loaded left paths, oldest first.
    left_history: Vec<String>,
    /// Previously loaded right paths, oldest first.
    right_history: Vec<String>,
    /// Raw diff result.
    diff: Option<TextDiff>,
    /// Aligned rows derived from the diff.
    rows: Arc<Vec<DiffRow>>,
    /// Sections derived from the rows.
    sections: Arc<Vec<DiffSection>>,
    /// Display list (rows plus filtered headers).
    display: Arc<Vec<DisplayRow>>,
    /// Section index → display index.
    section_display: Arc<Vec<usize>>,
    /// Row index → section index (`usize::MAX` when none).
    row_section: Arc<Vec<usize>>,
    /// Row index → is the first row of its section.
    is_section_start: Arc<Vec<bool>>,
    /// Collapsed section indices.
    collapsed: HashSet<usize>,
    /// Selected display index.
    selected: Option<usize>,
    /// Whether a load is in flight.
    loading: bool,
    /// Last load error, if any.
    error: Option<String>,
    /// Duration of the last successful load, in milliseconds.
    load_ms: Option<u128>,
    /// Word-wrap rows instead of clipping them.
    word_wrap: bool,
    /// Show the thumbnail strip beside the panes.
    show_thumbnail: bool,
    /// Per-session color overrides.
    overrides: ColorOverrides,
    /// Left text editor, created when edit mode is first entered.
    left_editor: Option<Entity<EditorState>>,
    /// Right text editor, created when edit mode is first entered.
    right_editor: Option<Entity<EditorState>>,
    /// Subscriptions tracking the editors' change and focus events.
    _editor_subs: Vec<Subscription>,
    /// The left buffer has unsaved edits.
    left_dirty: bool,
    /// The right buffer has unsaved edits.
    right_dirty: bool,
    /// Whether the panes show editable editors instead of the diff list.
    editing: bool,
    /// The side whose editor last had focus (target of edit commands).
    focus_side: Side,
    /// Whether the editors still need re-seeding from the last load.
    editors_stale: bool,
    /// Rules the current diff was computed under.
    compare_settings: TextCompareSettings,
    /// Forced alignment pairs `(left line, right line)`, 1-based.
    align_overrides: Vec<(usize, usize)>,
    /// Line marked as the pending alignment anchor.
    align_anchor: Option<(Side, usize)>,
    /// Which detail pane pair is shown below the diff.
    detail_mode: DetailMode,
    /// Bumps whenever the row model changes, so the view can refresh
    /// editor decorations.
    model_rev: usize,
    /// The parent folder session this diff was opened from, if any.
    parent: Option<WeakEntity<AppState>>,
    /// The session manager, for opening follow-up tabs.
    manager: Option<Entity<GuiSessionManager>>,
}

impl TextDiffSession {
    /// Create a session for the given endpoints; loads on demand.
    ///
    /// `parent` and `manager` wire the session back to the folder view it
    /// was opened from, enabling "copy file and open next difference".
    pub fn new(
        left: Endpoint,
        right: Endpoint,
        title: SharedString,
        parent: Option<WeakEntity<AppState>>,
        manager: Option<Entity<GuiSessionManager>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            title,
            left,
            right,
            left_raw: None,
            right_raw: None,
            left_lines: Arc::new(Vec::new()),
            right_lines: Arc::new(Vec::new()),
            left_meta: None,
            right_meta: None,
            left_encoding: TextEncoding::Utf8,
            right_encoding: TextEncoding::Utf8,
            left_ending: LineEnding::Lf,
            right_ending: LineEnding::Lf,
            left_history: Vec::new(),
            right_history: Vec::new(),
            diff: None,
            rows: Arc::new(Vec::new()),
            sections: Arc::new(Vec::new()),
            display: Arc::new(Vec::new()),
            section_display: Arc::new(Vec::new()),
            row_section: Arc::new(Vec::new()),
            is_section_start: Arc::new(Vec::new()),
            collapsed: HashSet::new(),
            selected: None,
            loading: false,
            error: None,
            load_ms: None,
            word_wrap: false,
            show_thumbnail: false,
            overrides: ColorOverrides::default(),
            left_editor: None,
            right_editor: None,
            _editor_subs: Vec::new(),
            left_dirty: false,
            right_dirty: false,
            editing: false,
            focus_side: Side::Left,
            editors_stale: false,
            compare_settings: TextCompareSettings::new(),
            align_overrides: Vec::new(),
            align_anchor: None,
            detail_mode: DetailMode::default(),
            model_rev: 0,
            parent,
            manager,
        }
    }

    /// Build a session from in-memory texts, without any file I/O. Used by
    /// tests and by "compare selection to clipboard".
    pub(crate) fn from_texts(
        left_text: &str,
        right_text: &str,
        title: SharedString,
        manager: Option<Entity<GuiSessionManager>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut session = Self::new(
            Endpoint::local("selection"),
            Endpoint::local("clipboard"),
            title,
            None,
            manager,
            cx,
        );
        session.left_raw = Some(left_text.as_bytes().to_vec());
        session.right_raw = Some(right_text.as_bytes().to_vec());
        session.load_ms = Some(0);
        session.redecode_and_compare();
        session
    }

    /// Build a session from in-memory texts, without any file I/O. Used by
    /// tests that exercise the model and view synchronously.
    #[cfg(test)]
    fn for_test(
        left_text: &str,
        right_text: &str,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut session = Self::new(
            Endpoint::local("/left.txt"),
            Endpoint::local("/right.txt"),
            "test".into(),
            None,
            None,
            cx,
        );
        session.left_raw = Some(left_text.as_bytes().to_vec());
        session.right_raw = Some(right_text.as_bytes().to_vec());
        session.load_ms = Some(1);
        session.redecode_and_compare();
        session
    }

    /// Return the focus handle.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Return the tab title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// The left file endpoint.
    pub fn left_endpoint(&self) -> Endpoint {
        self.left.clone()
    }

    /// The right file endpoint.
    pub fn right_endpoint(&self) -> Endpoint {
        self.right.clone()
    }

    /// Whether a load is in flight.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Return the last error message, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether word wrap is on.
    pub fn is_word_wrap(&self) -> bool {
        self.word_wrap
    }

    /// Whether the thumbnail strip is shown.
    pub fn is_show_thumbnail(&self) -> bool {
        self.show_thumbnail
    }

    /// Number of difference sections.
    pub fn section_count(&self) -> usize {
        self.sections.len()
    }

    /// The selected aligned row, if any.
    fn selected_row(&self) -> Option<&DiffRow> {
        let d = self.selected?;
        match self.display.get(d)? {
            DisplayRow::Line(r) => self.rows.get(*r),
            DisplayRow::Filtered { .. } => None,
        }
    }

    /// Alignment label `left:right` for the status bar.
    pub fn alignment_label(&self) -> String {
        match self.selected_row() {
            Some(row) => format!(
                "{}:{}",
                row.left
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".into()),
                row.right
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "—".into()),
            ),
            None => "—:—".to_string(),
        }
    }

    /// State label (Same/Change/Insert/Delete) for the status bar.
    pub fn state_label(&self) -> &'static str {
        self.selected_row()
            .map(|row| row.status.label())
            .unwrap_or("—")
    }

    /// Load-duration label for the status bar.
    pub fn load_label(&self) -> Option<String> {
        self.load_ms.map(|ms| format!("Loaded in {ms} ms"))
    }

    /// Statistics for the Info dialog.
    pub fn info_stats(&self) -> InfoStats {
        let (mut insertions, mut deletions, mut changes) = (0, 0, 0);
        for row in self.rows.iter() {
            match row.status {
                RowStatus::Inserted => insertions += 1,
                RowStatus::Deleted => deletions += 1,
                RowStatus::Changed => changes += 1,
                RowStatus::Same => {}
            }
        }
        InfoStats {
            left_lines: self.left_lines.len(),
            right_lines: self.right_lines.len(),
            sections: self.sections.len(),
            insertions,
            deletions,
            changes,
        }
    }

    /// Plain-text report of the comparison for the Report dialog and file.
    pub fn report_text(&self) -> String {
        let stats = self.info_stats();
        let mut out = String::new();
        out.push_str("Text compare report\n");
        out.push_str("===================\n\n");
        out.push_str(&format!("Left:  {}\n", self.left.display()));
        out.push_str(&format!("Right: {}\n\n", self.right.display()));
        out.push_str(&format!("Left lines:  {}\n", stats.left_lines));
        out.push_str(&format!("Right lines: {}\n", stats.right_lines));
        out.push_str(&format!("Difference sections: {}\n", stats.sections));
        out.push_str(&format!("Inserted lines: {}\n", stats.insertions));
        out.push_str(&format!("Deleted lines: {}\n", stats.deletions));
        out.push_str(&format!("Changed lines: {}\n", stats.changes));
        if let Some(ms) = self.load_ms {
            out.push_str(&format!("Loaded in {ms} ms\n"));
        }
        if !self.sections.is_empty() {
            out.push_str("\nSections:\n");
            for (ix, section) in self.sections.iter().enumerate() {
                let first = &self.rows[section.start_row];
                let last = &self.rows[section.end_row - 1];
                let side_range = |lo: Option<usize>, hi: Option<usize>| {
                    format!(
                        "{}-{}",
                        lo.map(|n| n.to_string())
                            .unwrap_or_else(|| "—".into()),
                        hi.map(|n| n.to_string())
                            .unwrap_or_else(|| "—".into()),
                    )
                };
                out.push_str(&format!(
                    "  #{} {:<6}{} left {} right {}\n",
                    ix + 1,
                    match section.kind {
                        SectionKind::Insert => "Insert",
                        SectionKind::Delete => "Delete",
                        SectionKind::Change => "Change",
                    },
                    if section.minor { " (minor)" } else { "" },
                    side_range(first.left, last.left),
                    side_range(first.right, last.right),
                ));
            }
        }
        out
    }

    /// (Re)read both files and recompute the diff.
    pub fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        cx.notify();

        let left = self.left.clone();
        let right = self.right.clone();
        let weak = cx.entity().downgrade();

        // File I/O runs on the tokio runtime because the providers use
        // tokio-based IO internally. A missing file yields empty content:
        // one side of a comparison can legitimately not exist.
        let join_handle = crate::runtime::spawn_on_tokio(async move {
            let started = Instant::now();
            let read_side = |endpoint: Endpoint| async move {
                let bytes =
                    match endpoint.fs().read_path(endpoint.path(), None).await
                    {
                        Ok(bytes) => bytes.to_vec(),
                        Err(FsError::NotFound { .. }) => Vec::new(),
                        Err(e) => return Err(anyhow::anyhow!("{e}")),
                    };
                let meta = endpoint
                    .fs()
                    .metadata_path(endpoint.path())
                    .await
                    .ok()
                    .map(|m| (m.size, m.modified));
                Ok::<_, anyhow::Error>((bytes, meta))
            };
            let left = read_side(left).await?;
            let right = read_side(right).await?;
            Ok::<_, anyhow::Error>((
                left,
                right,
                started.elapsed().as_millis(),
            ))
        });

        cx.spawn(|_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    if let Some(entity) = weak.upgrade() {
                        entity.update(cx, |session, cx| {
                            session.loading = false;
                            match result {
                                Ok(((lb, lm), (rb, rm), ms)) => {
                                    session.left_raw = Some(lb);
                                    session.right_raw = Some(rb);
                                    session.left_meta = lm.map(|(s, m)| {
                                        FileMeta::from_parts(s, m)
                                    });
                                    session.right_meta = rm.map(|(s, m)| {
                                        FileMeta::from_parts(s, m)
                                    });
                                    session.load_ms = Some(ms);
                                    session.redecode_and_compare();
                                    // A reload replaces the buffers, so the
                                    // editors must re-seed and lose their
                                    // dirty state; the view re-seeds them
                                    // because `set_value` needs a window.
                                    session.editors_stale = true;
                                    session.left_dirty = false;
                                    session.right_dirty = false;
                                }
                                Err(e) => {
                                    session.error =
                                        Some(format!("Compare failed: {e}"));
                                }
                            }
                            cx.notify();
                        });
                    }
                });
            }
        })
        .detach();
    }

    /// Start the initial load unless the model is already populated.
    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.diff.is_none()
            && !self.loading
            && self.error.is_none()
            && self.left_raw.is_none()
        {
            self.load(cx);
        }
    }

    /// Re-decode both sides from the raw bytes, re-split, re-compare, and
    /// rebuild the model. Also refreshes the detected line endings.
    fn redecode_and_compare(&mut self) {
        let left_text = decode_bytes(
            self.left_raw.as_deref().unwrap_or(&[]),
            self.left_encoding,
        );
        let right_text = decode_bytes(
            self.right_raw.as_deref().unwrap_or(&[]),
            self.right_encoding,
        );
        self.left_ending =
            detect_line_ending(self.left_raw.as_deref().unwrap_or(&[]));
        self.right_ending =
            detect_line_ending(self.right_raw.as_deref().unwrap_or(&[]));
        self.left_lines = Arc::new(split_lines(&left_text));
        self.right_lines = Arc::new(split_lines(&right_text));
        self.diff = Some(compare_texts(
            &left_text,
            &right_text,
            &self.compare_settings,
        ));
        self.rebuild_model();
    }

    /// Rebuild rows, sections, and the display list from the diff.
    fn rebuild_model(&mut self) {
        let mut rows = self.diff.as_ref().map(align_rows).unwrap_or_default();
        apply_align_overrides(&mut rows, &self.align_overrides);
        let sections = diff_sections(&rows);
        let mut row_section = vec![usize::MAX; rows.len()];
        let mut is_section_start = vec![false; rows.len()];
        for (si, section) in sections.iter().enumerate() {
            for slot in &mut row_section[section.start_row..section.end_row] {
                *slot = si;
            }
            if section.start_row < rows.len() {
                is_section_start[section.start_row] = true;
            }
        }
        self.rows = Arc::new(rows);
        self.sections = Arc::new(sections);
        self.row_section = Arc::new(row_section);
        self.is_section_start = Arc::new(is_section_start);
        self.model_rev += 1;
        self.rebuild_display();
    }

    /// Rebuild the display list from rows and collapsed sections, keeping
    /// the selection within bounds.
    fn rebuild_display(&mut self) {
        let (display, _line_display, section_display) =
            display_rows(&self.rows, &self.sections, &self.collapsed);
        self.display = Arc::new(display);
        self.section_display = Arc::new(section_display);
        if let Some(sel) = self.selected
            && sel >= self.display.len()
        {
            self.selected = self.display.len().checked_sub(1);
        }
    }

    /// Point one side at a new path and reload; the previous path goes to
    /// that side's history.
    pub fn set_side_path(
        &mut self,
        side: Side,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let (endpoint, history) = match side {
            Side::Left => (&self.left, &mut self.left_history),
            Side::Right => (&self.right, &mut self.right_history),
        };
        let current = endpoint.display();
        if current != path {
            if !history.contains(&current) {
                history.push(current);
                if history.len() > 20 {
                    history.remove(0);
                }
            }
            let updated = endpoint.with_path(path);
            match side {
                Side::Left => self.left = updated,
                Side::Right => self.right = updated,
            }
            self.load(cx);
        }
    }

    /// Change one side's read encoding and re-decode from the raw bytes.
    pub fn set_encoding(
        &mut self,
        side: Side,
        encoding: TextEncoding,
        cx: &mut Context<Self>,
    ) {
        match side {
            Side::Left => self.left_encoding = encoding,
            Side::Right => self.right_encoding = encoding,
        }
        if self.left_raw.is_some() || self.right_raw.is_some() {
            self.redecode_and_compare();
            cx.notify();
        }
    }

    /// Change one side's line-ending indicator (display-only on read).
    pub fn set_line_ending(
        &mut self,
        side: Side,
        ending: LineEnding,
        cx: &mut Context<Self>,
    ) {
        match side {
            Side::Left => self.left_ending = ending,
            Side::Right => self.right_ending = ending,
        }
        cx.notify();
    }

    /// Swap all per-side state and recompute.
    pub fn swap_sides(&mut self, cx: &mut Context<Self>) {
        std::mem::swap(&mut self.left, &mut self.right);
        std::mem::swap(&mut self.left_raw, &mut self.right_raw);
        std::mem::swap(&mut self.left_lines, &mut self.right_lines);
        std::mem::swap(&mut self.left_meta, &mut self.right_meta);
        std::mem::swap(&mut self.left_encoding, &mut self.right_encoding);
        std::mem::swap(&mut self.left_ending, &mut self.right_ending);
        std::mem::swap(&mut self.left_history, &mut self.right_history);
        self.rebuild_model();
        cx.notify();
    }

    /// Collapse or expand one section.
    pub fn toggle_section(&mut self, section: usize, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&section) {
            self.collapsed.insert(section);
        }
        self.rebuild_display();
        cx.notify();
    }

    /// Toggle word wrap.
    pub fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.word_wrap = !self.word_wrap;
        cx.notify();
    }

    /// Toggle the thumbnail strip.
    pub fn toggle_thumbnail(&mut self, cx: &mut Context<Self>) {
        self.show_thumbnail = !self.show_thumbnail;
        cx.notify();
    }

    /// Select a display row (from a row click or the thumbnail).
    pub fn select_display(&mut self, d: usize, cx: &mut Context<Self>) {
        if d < self.display.len() {
            self.selected = Some(d);
            cx.notify();
        }
    }

    /// Select the row at a vertical fraction of the strip.
    pub fn select_frac(
        &mut self,
        frac: f32,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let n = self.display.len();
        if n == 0 {
            return None;
        }
        let ix = (frac * n as f32).clamp(0., 1.) as usize;
        self.selected = Some(ix.min(n - 1));
        cx.notify();
        self.selected
    }

    /// Move the selection by one display row, clamped.
    pub fn select_line_delta(
        &mut self,
        delta: isize,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        if self.display.is_empty() {
            return None;
        }
        let cur = self.selected.unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, self.display.len() as isize - 1);
        self.selected = Some(next as usize);
        cx.notify();
        self.selected
    }

    /// Walk to the next difference row, wrapping.
    pub fn next_difference(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let from = self.selected.map(|s| s + 1).unwrap_or(0);
        let from = from % self.display.len().max(1);
        let target = next_difference_index(&self.display, &self.rows, from);
        if target.is_some() {
            self.selected = target;
            cx.notify();
        }
        target
    }

    /// Walk to the previous difference row, wrapping.
    pub fn prev_difference(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let from = self.selected.unwrap_or(0);
        let target = prev_difference_index(&self.display, &self.rows, from);
        if target.is_some() {
            self.selected = target;
            cx.notify();
        }
        target
    }

    /// Walk to the next difference section, wrapping.
    pub fn next_section(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        let target = next_section_index(&self.section_display, self.selected);
        if target.is_some() {
            self.selected = target;
            cx.notify();
        }
        target
    }

    /// Walk to the previous difference section, wrapping.
    pub fn prev_section(&mut self, cx: &mut Context<Self>) -> Option<usize> {
        let target = prev_section_index(&self.section_display, self.selected);
        if target.is_some() {
            self.selected = target;
            cx.notify();
        }
        target
    }

    /// Left endpoint display path.
    pub fn left_path(&self) -> String {
        self.left.display()
    }

    /// Right endpoint display path.
    pub fn right_path(&self) -> String {
        self.right.display()
    }

    /// Left file-bar metadata line.
    pub fn left_meta_label(&self) -> String {
        meta_label(self.left_meta.as_ref(), self.left_ending)
    }

    /// Right file-bar metadata line.
    pub fn right_meta_label(&self) -> String {
        meta_label(self.right_meta.as_ref(), self.right_ending)
    }

    /// Left encoding selector label.
    pub fn left_encoding_label(&self) -> &'static str {
        encoding_label(self.left_encoding)
    }

    /// Right encoding selector label.
    pub fn right_encoding_label(&self) -> &'static str {
        encoding_label(self.right_encoding)
    }

    /// Left line-ending selector label.
    pub fn left_ending_label(&self) -> &'static str {
        line_ending_label(self.left_ending)
    }

    /// Right line-ending selector label.
    pub fn right_ending_label(&self) -> &'static str {
        line_ending_label(self.right_ending)
    }

    /// Left path history, oldest first.
    pub fn left_history(&self) -> &[String] {
        &self.left_history
    }

    /// Right path history, oldest first.
    pub fn right_history(&self) -> &[String] {
        &self.right_history
    }

    // -- Editing, rules, and sync (phase 6) --------------------------------

    /// Whether the panes show editable editors.
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    /// Whether either buffer has unsaved edits.
    pub fn is_dirty(&self) -> bool {
        self.left_dirty || self.right_dirty
    }

    /// Whether a side can be saved (its content is loaded).
    pub fn can_save(&self) -> bool {
        self.left_raw.is_some() || self.right_raw.is_some()
    }

    /// The side whose editor last had focus.
    pub fn focus_side(&self) -> Side {
        self.focus_side
    }

    /// The session manager, when this session was opened from one.
    pub fn manager(&self) -> Option<Entity<GuiSessionManager>> {
        self.manager.clone()
    }

    /// Clear the stale-editor flag and the dirty marks after the view
    /// has re-seeded both editors from the model.
    pub fn mark_editors_seeded(&mut self) {
        self.editors_stale = false;
        self.left_dirty = false;
        self.right_dirty = false;
    }

    /// The current detail mode.
    pub fn detail_mode(&self) -> DetailMode {
        self.detail_mode
    }

    /// Switch the detail panes.
    pub fn set_detail_mode(
        &mut self,
        mode: DetailMode,
        cx: &mut Context<Self>,
    ) {
        self.detail_mode = mode;
        cx.notify();
    }

    /// Enter or leave edit mode.
    pub fn toggle_editing(&mut self, cx: &mut Context<Self>) {
        self.editing = !self.editing;
        cx.notify();
    }

    /// The row-model revision, for refreshing editor decorations.
    pub fn model_rev(&self) -> usize {
        self.model_rev
    }

    /// Whether the editors still need re-seeding from the last load.
    pub fn editors_stale(&self) -> bool {
        self.editors_stale
    }

    /// The left editor, once edit mode has created one.
    pub fn left_editor(&self) -> Option<&Entity<EditorState>> {
        self.left_editor.as_ref()
    }

    /// The right editor, once edit mode has created one.
    pub fn right_editor(&self) -> Option<&Entity<EditorState>> {
        self.right_editor.as_ref()
    }

    /// The editor for one side.
    pub fn editor(&self, side: Side) -> Option<&Entity<EditorState>> {
        match side {
            Side::Left => self.left_editor.as_ref(),
            Side::Right => self.right_editor.as_ref(),
        }
    }

    /// Take ownership of the view-created editors and start tracking
    /// their change and focus events. The editors are flagged stale so
    /// the view seeds their text on the next render.
    pub fn attach_editors(
        &mut self,
        left: Entity<EditorState>,
        right: Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        self._editor_subs.clear();
        for (side, editor) in [(Side::Left, &left), (Side::Right, &right)] {
            let sub =
                cx.subscribe(editor, move |this, _, event, cx| match event {
                    InputEvent::Change => this.mark_side_dirty(side, cx),
                    InputEvent::Focus => this.focus_side = side,
                    _ => {}
                });
            self._editor_subs.push(sub);
        }
        self.left_editor = Some(left);
        self.right_editor = Some(right);
        self.editors_stale = true;
        cx.notify();
    }

    /// Record unsaved edits on one side and flag the tab dirty.
    pub fn mark_side_dirty(&mut self, side: Side, cx: &mut Context<Self>) {
        match side {
            Side::Left => self.left_dirty = true,
            Side::Right => self.right_dirty = true,
        }
        if let Some(manager) = self.manager.clone() {
            manager.update(cx, |m, cx| m.mark_dirty(cx));
        }
        cx.notify();
    }

    /// The current text of one side: the editor buffer once it is
    /// seeded, otherwise the decoded file bytes normalized to LF.
    pub fn side_text(&self, side: Side, cx: &App) -> String {
        if !self.editors_stale
            && let Some(editor) = self.editor(side)
        {
            return editor.read(cx).value().to_string();
        }
        let (raw, encoding) = match side {
            Side::Left => (&self.left_raw, self.left_encoding),
            Side::Right => (&self.right_raw, self.right_encoding),
        };
        normalize_endings(
            &decode_bytes(raw.as_deref().unwrap_or(&[]), encoding),
            LineEnding::Lf,
        )
    }

    /// Re-diff the current buffers without touching disk. Edits are
    /// preserved; this is what "recompare" means once editing landed.
    pub fn recompare(&mut self, cx: &mut Context<Self>) {
        let left_text = self.side_text(Side::Left, cx);
        let right_text = self.side_text(Side::Right, cx);
        self.left_lines = Arc::new(split_lines(&left_text));
        self.right_lines = Arc::new(split_lines(&right_text));
        self.diff = Some(compare_texts(
            &left_text,
            &right_text,
            &self.compare_settings,
        ));
        self.rebuild_model();
        cx.notify();
    }

    /// Replace the session's compare rules and re-diff.
    pub fn set_compare_settings(
        &mut self,
        settings: TextCompareSettings,
        cx: &mut Context<Self>,
    ) {
        self.compare_settings = settings;
        self.recompare(cx);
    }

    /// The current compare rules, for the rules dialog.
    pub fn compare_settings(&self) -> &TextCompareSettings {
        &self.compare_settings
    }

    /// The pending alignment anchor, if a line is marked.
    pub fn align_anchor(&self) -> Option<(Side, usize)> {
        self.align_anchor
    }

    /// Whether any alignment overrides are in effect.
    pub fn has_align_overrides(&self) -> bool {
        !self.align_overrides.is_empty()
    }

    /// Mark the clicked row's line on `side` as the alignment anchor.
    pub fn set_align_anchor(&mut self, side: Side, cx: &mut Context<Self>) {
        let Some(line) = self.selected_row().and_then(|row| match side {
            Side::Left => row.left,
            Side::Right => row.right,
        }) else {
            return;
        };
        self.align_anchor = Some((side, line));
        log::push(
            LogLevel::Info,
            format!(
                "Alignment anchor: {} line {line}",
                if side == Side::Left { "left" } else { "right" }
            ),
            cx,
        );
        cx.notify();
    }

    /// Pair the marked anchor line with the selected row's line on the
    /// opposite side, then re-diff honoring the override.
    pub fn align_with_selected(&mut self, cx: &mut Context<Self>) {
        let Some((anchor_side, anchor_line)) = self.align_anchor else {
            return;
        };
        let Some(row) = self.selected_row().copied() else {
            return;
        };
        let pair = match anchor_side {
            Side::Left => row.right.map(|r| (anchor_line, r)),
            Side::Right => row.left.map(|l| (l, anchor_line)),
        };
        let Some(pair) = pair else {
            log::push(
                LogLevel::Warning,
                "The selected row has no line on the opposite side to align \
                 with."
                    .to_string(),
                cx,
            );
            return;
        };
        if !self.align_overrides.contains(&pair) {
            self.align_overrides.push(pair);
        }
        self.align_anchor = None;
        self.recompare(cx);
    }

    /// Drop all alignment overrides and re-diff.
    pub fn clear_alignments(&mut self, cx: &mut Context<Self>) {
        if self.align_overrides.is_empty() && self.align_anchor.is_none() {
            return;
        }
        self.align_overrides.clear();
        self.align_anchor = None;
        self.recompare(cx);
    }

    /// Collapse every section except the one containing the selection.
    pub fn isolate_selected(&mut self, cx: &mut Context<Self>) {
        let Some(keep) = self.selected_section() else {
            return;
        };
        self.collapsed =
            (0..self.sections.len()).filter(|s| *s != keep).collect();
        self.rebuild_display();
        cx.notify();
    }

    /// The section index of the selected row, if it is in one.
    pub fn selected_section(&self) -> Option<usize> {
        let d = self.selected?;
        let DisplayRow::Line(r) = *self.display.get(d)? else {
            return None;
        };
        let si = self.row_section.get(r).copied().unwrap_or(usize::MAX);
        (si != usize::MAX).then_some(si)
    }

    /// The 1-based inclusive line range one side contributes to a
    /// section, if it contributes any lines.
    pub fn section_line_range(
        &self,
        section: usize,
        side: Side,
    ) -> Option<(usize, usize)> {
        let sec = self.sections.get(section)?;
        let (mut lo, mut hi) = (usize::MAX, 0usize);
        for row in &self.rows[sec.start_row..sec.end_row] {
            if let Some(n) = match side {
                Side::Left => row.left,
                Side::Right => row.right,
            } {
                lo = lo.min(n);
                hi = hi.max(n);
            }
        }
        (hi > 0).then(|| (lo, hi))
    }

    /// The right line just above a section — the insertion point when
    /// the target side has no lines in the section (0 = before all).
    pub fn section_insert_after(&self, section: usize, side: Side) -> usize {
        let Some(sec) = self.sections.get(section) else {
            return 0;
        };
        for row in self.rows[..sec.start_row].iter().rev() {
            if let Some(n) = match side {
                Side::Left => row.left,
                Side::Right => row.right,
            } {
                return n;
            }
        }
        0
    }

    /// The joined text one side contributes to a section, newline
    /// terminated so it can replace target lines directly.
    pub fn section_text(&self, section: usize, side: Side) -> String {
        let Some((lo, hi)) = self.section_line_range(section, side) else {
            return String::new();
        };
        let lines = match side {
            Side::Left => &self.left_lines,
            Side::Right => &self.right_lines,
        };
        let mut out = String::new();
        for line in lines.iter().filter(|l| l.number >= lo && l.number <= hi) {
            out.push_str(&line.content);
            out.push('\n');
        }
        out
    }

    /// The selected row's line text on one side, for the detail panes.
    pub fn selected_line_text(&self, side: Side) -> Option<String> {
        let row = self.selected_row()?;
        let n = match side {
            Side::Left => row.left,
            Side::Right => row.right,
        }?;
        let lines = match side {
            Side::Left => &self.left_lines,
            Side::Right => &self.right_lines,
        };
        lines.get(n - 1).map(|l| l.content.clone())
    }

    /// The selected row's line number on one side.
    pub fn selected_line_number(&self, side: Side) -> Option<usize> {
        let row = self.selected_row()?;
        match side {
            Side::Left => row.left,
            Side::Right => row.right,
        }
    }

    /// Save one side's buffer to its file, or prompt for a new path when
    /// `save_as` is set. The outcome is logged; failures also notify.
    pub fn save_side(
        &mut self,
        side: Side,
        save_as: bool,
        cx: &mut Context<Self>,
    ) {
        let text = self.side_text(side, cx);
        let (ending, encoding, endpoint) = match side {
            Side::Left => (self.left_ending, self.left_encoding, &self.left),
            Side::Right => {
                (self.right_ending, self.right_encoding, &self.right)
            }
        };
        let bytes = encode_text(&text, ending, encoding);
        let path = endpoint.path().to_path_buf();
        let needs_prompt = save_as
            || path.as_os_str().is_empty()
            || path == std::path::Path::new("selection")
            || path == std::path::Path::new("clipboard");
        let entity = cx.entity().clone();
        if needs_prompt {
            let dir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file.txt".to_string());
            let receiver = cx.prompt_for_new_path(&dir, Some(&name));
            cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    let target = match receiver.await {
                        Ok(Ok(Some(p))) => p,
                        // The user cancelled the save dialog.
                        Ok(Ok(None)) => return,
                        Ok(Err(e)) => {
                            cx.update(|cx| {
                                log::push(
                                    LogLevel::Error,
                                    format!("Save failed: {e}"),
                                    cx,
                                );
                            });
                            return;
                        }
                        Err(_) => return,
                    };
                    Self::finish_save(
                        entity,
                        side,
                        bytes,
                        Some(target),
                        &mut cx,
                    )
                    .await;
                }
            })
            .detach();
        } else {
            cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
                let mut cx = cx.clone();
                async move {
                    Self::finish_save(entity, side, bytes, None, &mut cx)
                        .await;
                }
            })
            .detach();
        }
    }

    /// Write the buffer (optionally to a new path) and settle the
    /// session's dirty and metadata state.
    async fn finish_save(
        entity: Entity<TextDiffSession>,
        side: Side,
        bytes: Vec<u8>,
        new_path: Option<std::path::PathBuf>,
        cx: &mut gpui::AsyncApp,
    ) {
        let target = new_path.clone().unwrap_or_else(|| {
            entity.read_with(cx, |s, _| match side {
                Side::Left => s.left.path().to_path_buf(),
                Side::Right => s.right.path().to_path_buf(),
            })
        });
        let for_log = target.display().to_string();
        let size = bytes.len() as u64;
        let result = crate::runtime::spawn_on_tokio(async move {
            fs_err::tokio::write(&target, &bytes)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await;
        let _ = entity.update(cx, |session, cx| {
            match result {
                Ok(Ok(())) => {
                    match side {
                        Side::Left => session.left_dirty = false,
                        Side::Right => session.right_dirty = false,
                    }
                    if let Some(path) = &new_path {
                        let path_str = path.display().to_string();
                        let updated = match side {
                            Side::Left => session.left.with_path(&path_str),
                            Side::Right => session.right.with_path(&path_str),
                        };
                        match side {
                            Side::Left => session.left = updated,
                            Side::Right => session.right = updated,
                        }
                    }
                    let meta = FileMeta::from_parts(size, Utc::now());
                    match side {
                        Side::Left => session.left_meta = Some(meta),
                        Side::Right => session.right_meta = Some(meta),
                    }
                    log::push(LogLevel::Info, format!("Saved: {for_log}"), cx);
                }
                Ok(Err(e)) => {
                    log::push(
                        LogLevel::Error,
                        format!("Save failed: {e}"),
                        cx,
                    );
                    crate::menus::notify_error(
                        format!("Couldn't save {for_log}. {e}"),
                        cx,
                    );
                }
                Err(e) => {
                    log::push(
                        LogLevel::Error,
                        format!("Save task panicked: {e}"),
                        cx,
                    );
                }
            }
            cx.notify();
        });
    }

    /// Copy this pair's file toward one side, advance the parent folder
    /// session to its next difference, and open that pair in a new tab.
    pub fn copy_file_and_open_next(
        &mut self,
        to: Side,
        cx: &mut Context<Self>,
    ) {
        let (Some(parent), Some(manager)) =
            (self.parent.clone(), self.manager.clone())
        else {
            log::push(
                LogLevel::Warning,
                "Copy file and open next needs a parent folder session."
                    .to_string(),
                cx,
            );
            return;
        };
        let (source, target) = match to {
            Side::Right => (&self.left, &self.right),
            Side::Left => (&self.right, &self.left),
        };
        let (src, dst) =
            (source.path().to_path_buf(), target.path().to_path_buf());
        let join = crate::runtime::spawn_on_tokio(async move {
            fs_err::tokio::copy(&src, &dst)
                .await
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("{e}"))
        });
        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let result = match join.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                if let Err(e) = result {
                    cx.update(|cx| {
                        log::push(
                            LogLevel::Error,
                            format!("Copy file failed: {e}"),
                            cx,
                        );
                    });
                    return;
                }
                // Advance the parent selection, then open the newly selected
                // pair so the user lands on the next difference immediately.
                let next = parent.update(&mut cx, |state, cx| {
                    state.select_next_difference(cx);
                    state.selected_endpoints()
                });
                let Ok(Some((left, right))) = next else {
                    return;
                };
                let _ = manager.update(&mut cx, |m, cx| {
                    m.add_text_diff_tab(left, right, Some(parent.clone()), cx)
                });
            }
        })
        .detach();
    }
}

/// File-bar metadata line: modified · size · format · line endings.
fn meta_label(meta: Option<&FileMeta>, ending: LineEnding) -> String {
    let size = meta
        .map(|m| format_size(m.size))
        .unwrap_or_else(|| "—".to_string());
    let modified = meta
        .map(|m| m.modified.clone())
        .unwrap_or_else(|| "—".to_string());
    format!("{modified} · {size} · Text · {}", line_ending_label(ending))
}

impl gpui::Focusable for TextDiffSession {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// Thumbnail strip element
// ---------------------------------------------------------------------------

/// A narrow vertical strip that maps the whole display list onto its
/// height: difference rows paint as colored runs, the viewport shows as a
/// white outline, and the selection as a triangle marker. Clicking or
/// dragging navigates.
struct ThumbnailStrip {
    id: ElementId,
    /// (start fraction, end fraction, color) per run.
    runs: Vec<(f32, f32, Hsla)>,
    /// Viewport top fraction and height fraction.
    viewport: (f32, f32),
    /// Selection marker fraction.
    marker: Option<f32>,
    background: Hsla,
    viewport_color: Hsla,
    marker_color: Hsla,
    width: Pixels,
    dragging: Rc<Cell<bool>>,
    on_navigate: NavigateFn,
}

/// Callback from the thumbnail strip: navigate to a vertical fraction of
/// the display list.
type NavigateFn = Rc<dyn Fn(f32, &mut Window, &mut App)>;

impl IntoElement for ThumbnailStrip {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ThumbnailStrip {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(
        &self,
    ) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = self.width.into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Hitbox,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let top = bounds.origin.y;
        let height = bounds.size.height.as_f32();
        window.paint_quad(fill(bounds, self.background));
        // One quad per merged run, positioned by fraction of the height.
        for (start, end, color) in &self.runs {
            let y = top + px(height * start);
            let h = px((height * (end - start)).max(1.));
            window.paint_quad(fill(
                Bounds {
                    origin: point(bounds.origin.x, y),
                    size: size(bounds.size.width, h),
                },
                *color,
            ));
        }
        // The viewport rectangle as an outline.
        let (vtop, vheight) = self.viewport;
        if vheight > 0. {
            let rect = Bounds {
                origin: point(bounds.origin.x, top + px(height * vtop)),
                size: size(bounds.size.width, px((height * vheight).max(2.))),
            };
            window.paint_quad(outline(
                rect,
                self.viewport_color,
                gpui::BorderStyle::Solid,
            ));
        }
        // The selection marker: a small right-pointing triangle.
        if let Some(frac) = self.marker {
            let y = top + px(height * frac);
            let mut path = PathBuilder::fill();
            path.move_to(point(bounds.origin.x, y - px(4.)));
            path.line_to(point(bounds.origin.x + px(6.), y));
            path.line_to(point(bounds.origin.x, y + px(4.)));
            path.close();
            if let Ok(built) = path.build() {
                window.paint_path(built, self.marker_color);
            }
        }
        if hitbox.is_hovered(window) {
            window.set_cursor_style(CursorStyle::PointingHand, hitbox);
        }
        // Click or drag anywhere on the strip navigates to that fraction.
        let dragging = self.dragging.clone();
        let on_navigate = self.on_navigate.clone();
        let hitbox_id = hitbox.id;
        window.on_mouse_event({
            let dragging = dragging.clone();
            let on_navigate = on_navigate.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if matches!(phase, DispatchPhase::Capture)
                    && event.button == MouseButton::Left
                    && hitbox_id.is_hovered(window)
                {
                    dragging.set(true);
                    let frac = ((event.position.y - top).as_f32() / height)
                        .clamp(0., 1.);
                    on_navigate(frac, window, cx);
                }
            }
        });
        window.on_mouse_event({
            let dragging = dragging.clone();
            let on_navigate = on_navigate.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if matches!(phase, DispatchPhase::Capture)
                    && dragging.get()
                    && event.pressed_button == Some(MouseButton::Left)
                {
                    let frac = ((event.position.y - top).as_f32() / height)
                        .clamp(0., 1.);
                    on_navigate(frac, window, cx);
                }
            }
        });
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
            if matches!(phase, DispatchPhase::Capture) {
                dragging.set(false);
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Text compare view
// ---------------------------------------------------------------------------

/// The side-by-side text compare view (UI spec section 5).
///
/// Renders the session's aligned rows as two scroll-synchronized virtual
/// lists with per-pane file bars, an optional thumbnail strip, read-only
/// detail panes, and a status bar. All state lives in the session entity;
/// the view only owns the list states and the path inputs.
pub struct TextDiffView {
    /// The compare session this view renders.
    state: Entity<TextDiffSession>,
    /// Left pane's virtual list.
    left_list: ListState,
    /// Right pane's virtual list.
    right_list: ListState,
    /// Left file-bar path input, created on first render.
    left_path_input: Option<Entity<InputState>>,
    /// Right file-bar path input, created on first render.
    right_path_input: Option<Entity<InputState>>,
    /// Subscriptions for the path inputs' Enter commits.
    _path_subs: Vec<Subscription>,
    /// Re-render whenever the session notifies.
    _state_sub: Subscription,
    /// Drag flag shared with the thumbnail strip element.
    thumbnail_drag: Rc<Cell<bool>>,
    /// Display-row count the lists were last reset to.
    list_count: usize,
    /// Wrap mode the lists were last reset to.
    list_wrap: bool,
    /// Difference fills for the left editor.
    left_decs: Option<RangeDecorationCollection>,
    /// Difference fills for the right editor.
    right_decs: Option<RangeDecorationCollection>,
    /// Model revision the decorations were last rebuilt for.
    decoration_rev: usize,
    /// Editable detail inputs, created on first Text-detail render.
    detail_inputs: Option<(Entity<InputState>, Entity<InputState>)>,
    /// The `(left, right)` line numbers the detail inputs show.
    detail_seeded: Option<(Option<usize>, Option<usize>)>,
    /// Subscriptions for the detail inputs' Enter commits.
    _detail_subs: Vec<Subscription>,
    /// A detail-line commit waiting for a render (needs a window).
    pending_detail_commit: Option<(Side, usize, String)>,
}

impl TextDiffView {
    /// Create a view over an existing session; starts the initial load.
    pub fn new(
        state: Entity<TextDiffSession>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Line stepping and section walking are scoped to the text view's
        // key context so they never shadow the folder view's plain j/k.
        cx.bind_keys([
            KeyBinding::new("j", NextLine, Some("TextCompare")),
            KeyBinding::new("k", PrevLine, Some("TextCompare")),
            KeyBinding::new("down", NextLine, Some("TextCompare")),
            KeyBinding::new("up", PrevLine, Some("TextCompare")),
            KeyBinding::new("cmd-shift-]", NextSection, Some("TextCompare")),
            KeyBinding::new("cmd-shift-[", PrevSection, Some("TextCompare")),
        ]);

        let left_list = ListState::new(0, ListAlignment::Top, px(100.));
        let right_list = ListState::new(0, ListAlignment::Top, px(100.));
        // Wheel-scrolling one pane scrolls the other to the same logical
        // offset. The handler runs while the source list still holds a
        // mutable borrow of its own state, so reading the offset and
        // scrolling the target must be deferred to the next cycle.
        // `scroll_to` never fires the handler, so this cannot loop.
        left_list.set_scroll_handler({
            let (src, dst) = (left_list.clone(), right_list.clone());
            let weak = cx.entity().downgrade();
            move |_: &ListScrollEvent, _window: &mut Window, cx: &mut App| {
                let (src, dst, weak) =
                    (src.clone(), dst.clone(), weak.clone());
                cx.defer(move |cx| {
                    let top = src.logical_scroll_top();
                    dst.scroll_to(top);
                    let _ = weak.update(cx, |_, cx| cx.notify());
                });
            }
        });
        right_list.set_scroll_handler({
            let (src, dst) = (right_list.clone(), left_list.clone());
            let weak = cx.entity().downgrade();
            move |_: &ListScrollEvent, _window: &mut Window, cx: &mut App| {
                let (src, dst, weak) =
                    (src.clone(), dst.clone(), weak.clone());
                cx.defer(move |cx| {
                    let top = src.logical_scroll_top();
                    dst.scroll_to(top);
                    let _ = weak.update(cx, |_, cx| cx.notify());
                });
            }
        });

        let _state_sub = cx.observe(&state, |_, _, cx| cx.notify());
        state.update(cx, |session, cx| session.ensure_loaded(cx));
        Self {
            state,
            left_list,
            right_list,
            left_path_input: None,
            right_path_input: None,
            _path_subs: Vec::new(),
            _state_sub,
            thumbnail_drag: Rc::new(Cell::new(false)),
            list_count: 0,
            list_wrap: false,
            left_decs: None,
            right_decs: None,
            decoration_rev: usize::MAX,
            detail_inputs: None,
            detail_seeded: None,
            _detail_subs: Vec::new(),
            pending_detail_commit: None,
        }
    }

    /// The session's focus handle, shared with the tab entity.
    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle()
    }

    /// The tab title.
    pub fn title(&self, cx: &App) -> SharedString {
        self.state.read(cx).title().clone()
    }

    /// The underlying session, for the window root's menu sync.
    pub fn text_state(&self) -> Entity<TextDiffSession> {
        self.state.clone()
    }

    /// Scroll both panes so the display row is visible.
    fn reveal(&self, target: Option<usize>) {
        if let Some(ix) = target {
            self.left_list.scroll_to_reveal_item(ix);
            self.right_list.scroll_to_reveal_item(ix);
        }
    }

    // -- Action handlers ---------------------------------------------------

    fn handle_next_line(
        &mut self,
        _: &NextLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.select_line_delta(1, cx));
        self.reveal(target);
    }

    fn handle_prev_line(
        &mut self,
        _: &PrevLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target =
            self.state.update(cx, |s, cx| s.select_line_delta(-1, cx));
        self.reveal(target);
    }

    fn handle_next_difference(
        &mut self,
        _: &NextDifference,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.next_difference(cx));
        self.reveal(target);
    }

    fn handle_prev_difference(
        &mut self,
        _: &PrevDifference,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.prev_difference(cx));
        self.reveal(target);
    }

    fn handle_next_section(
        &mut self,
        _: &NextSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.next_section(cx));
        self.reveal(target);
    }

    fn handle_prev_section(
        &mut self,
        _: &PrevSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.state.update(cx, |s, cx| s.prev_section(cx));
        self.reveal(target);
    }

    fn handle_toggle_wrap(
        &mut self,
        _: &ToggleWordWrap,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.toggle_wrap(cx));
    }

    fn handle_toggle_thumbnails(
        &mut self,
        _: &ShowThumbnails,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.toggle_thumbnail(cx));
    }

    fn handle_reload(
        &mut self,
        _: &ReloadCompare,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // "Reload / recompare files": with unsaved buffer edits the
        // re-diff runs on the buffers so the edits survive; otherwise the
        // files are re-read from disk.
        self.state.update(cx, |s, cx| {
            if s.is_dirty() {
                s.recompare(cx);
            } else {
                s.load(cx);
            }
        });
    }

    fn handle_swap_sides(
        &mut self,
        _: &SwapSides,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.swap_sides(cx));
        self.sync_path_inputs(window, cx);
    }

    /// Re-read both path inputs from the (possibly swapped) session.
    fn sync_path_inputs(&self, window: &mut Window, cx: &mut Context<Self>) {
        let (left, right) = {
            let session = self.state.read(cx);
            (session.left_path(), session.right_path())
        };
        if let Some(input) = &self.left_path_input {
            input.update(cx, |input, cx| {
                input.set_value(left.clone(), window, cx);
            });
        }
        if let Some(input) = &self.right_path_input {
            input.update(cx, |input, cx| input.set_value(right, window, cx));
        }
    }

    // -- Edit-mode handlers --------------------------------------------------

    fn handle_toggle_editing(
        &mut self,
        _: &ToggleTextEdit,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.toggle_editing(cx));
    }

    /// The focused side's editor, when edit mode has created one.
    fn focused_editor(&self, cx: &App) -> Option<(Side, Entity<EditorState>)> {
        let session = self.state.read(cx);
        let side = session.focus_side();
        session.editor(side).map(|ed| (side, ed.clone()))
    }

    fn handle_select_section(
        &mut self,
        _: &SelectSection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((side, editor)) = self.focused_editor(cx) else {
            return;
        };
        let range = self
            .state
            .read(cx)
            .selected_section()
            .and_then(|s| self.state.read(cx).section_line_range(s, side));
        if let Some((lo, hi)) = range {
            select_editor_lines(&editor, lo, hi, cx);
        }
    }

    fn handle_delete_line(
        &mut self,
        _: &DeleteLine,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((side, editor)) = self.focused_editor(cx) else {
            return;
        };
        let Some(line) = self.state.read(cx).selected_line_number(side) else {
            return;
        };
        replace_editor_lines(&editor, line, line, "", window, cx);
        self.after_editor_edit(side, cx);
    }

    fn handle_insert_line_before(
        &mut self,
        _: &InsertLineBefore,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.insert_blank_line(true, window, cx);
    }

    fn handle_insert_line_after(
        &mut self,
        _: &InsertLineAfter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.insert_blank_line(false, window, cx);
    }

    fn insert_blank_line(
        &mut self,
        before: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((side, editor)) = self.focused_editor(cx) else {
            return;
        };
        let Some(line) = self.state.read(cx).selected_line_number(side) else {
            return;
        };
        let at = if before { line } else { line + 1 };
        insert_at_line_start(&editor, at, "\n", window, cx);
        self.after_editor_edit(side, cx);
    }

    /// Mark the edited side dirty and re-diff after an editor mutation.
    fn after_editor_edit(&mut self, side: Side, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.mark_side_dirty(side, cx);
            s.recompare(cx);
        });
    }

    fn apply_convert(
        &mut self,
        op: ConvertOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((side, editor)) = self.focused_editor(cx) else {
            log::push(
                LogLevel::Warning,
                "Convert file needs edit mode: toggle Edit first.".to_string(),
                cx,
            );
            return;
        };
        let converted =
            editor.update(cx, |ed, _| convert_text(&ed.value(), op));
        editor.update(cx, |ed, cx| ed.replace_all(converted, window, cx));
        self.after_editor_edit(side, cx);
    }

    fn handle_convert(
        &mut self,
        op: ConvertOp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_convert(op, window, cx);
    }

    fn handle_compare_selection(
        &mut self,
        _: &CompareSelectionToClipboard,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (side, selection, manager) = {
            let session = self.state.read(cx);
            let side = session.focus_side();
            let mut selection = session
                .editor(side)
                .map(|ed| ed.read(cx).selected_value().to_string())
                .unwrap_or_default();
            if selection.is_empty() {
                selection =
                    session.selected_line_text(side).unwrap_or_default();
            }
            (side, selection, session.manager())
        };
        let Some(manager) = manager else {
            log::push(
                LogLevel::Warning,
                "Compare selection to clipboard needs a session manager."
                    .to_string(),
                cx,
            );
            return;
        };
        let clipboard = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let (left, right) = match side {
            Side::Left => (selection, clipboard),
            Side::Right => (clipboard, selection),
        };
        manager.update(cx, |m, cx| {
            m.add_text_diff_tab_from_texts(
                &left,
                &right,
                "selection vs clipboard".into(),
                cx,
            );
        });
    }

    // -- Alignment and section handlers -------------------------------------

    fn handle_set_align_anchor(
        &mut self,
        action: &SetAlignAnchor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.set_align_anchor(action.side, cx));
    }

    fn handle_align_with(
        &mut self,
        _: &AlignWithAnchor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.align_with_selected(cx));
    }

    fn handle_clear_alignments(
        &mut self,
        _: &ClearAlignments,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.clear_alignments(cx));
    }

    fn handle_isolate(
        &mut self,
        _: &IsolateSelection,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.isolate_selected(cx));
    }

    /// Copy the selected section's lines toward one side, replacing the
    /// target side's lines (or inserting where it has none), then
    /// re-diff. Requires edit mode so the target buffer exists.
    fn handle_copy_section(
        &mut self,
        to: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (source_text, target_range, insert_after, editor) = {
            let session = self.state.read(cx);
            let Some(section) = session.selected_section() else {
                return;
            };
            let from = if to == Side::Left {
                Side::Right
            } else {
                Side::Left
            };
            (
                session.section_text(section, from),
                session.section_line_range(section, to),
                session.section_insert_after(section, to),
                session.editor(to).cloned(),
            )
        };
        let Some(editor) = editor else {
            log::push(
                LogLevel::Warning,
                "Copying sections needs edit mode: toggle Edit first."
                    .to_string(),
                cx,
            );
            return;
        };
        match target_range {
            Some((lo, hi)) => {
                replace_editor_lines(&editor, lo, hi, &source_text, window, cx)
            }
            None => insert_at_line_start(
                &editor,
                insert_after + 1,
                &source_text,
                window,
                cx,
            ),
        }
        self.after_editor_edit(to, cx);
    }

    fn handle_copy_file_next(
        &mut self,
        to: Side,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.copy_file_and_open_next(to, cx));
    }

    fn handle_set_detail_mode(
        &mut self,
        action: &SetDetailMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.set_detail_mode(action.mode, cx));
    }

    // -- Dialogs -----------------------------------------------------------

    fn handle_report(
        &mut self,
        _: &SessionReport,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.state.read(cx).report_text();
        // Suggest the left file's directory as the save location.
        let dir = std::path::PathBuf::from(self.state.read(cx).left_path())
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let preview = text.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Session report")
                .child(
                    div()
                        .id(ElementId::Name("report-preview".into()))
                        .max_h(px(384.))
                        .overflow_y_scroll()
                        .child(preview.clone()),
                )
                .footer(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("report_cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| {
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("report_save")
                                .label("Save to file…")
                                .primary()
                                .on_click({
                                    let text = text.clone();
                                    let dir = dir.clone();
                                    move |_, window, cx| {
                                        window.close_dialog(cx);
                                        save_report(&text, &dir, cx);
                                    }
                                }),
                        ),
                )
        });
    }

    fn handle_info(
        &mut self,
        _: &SessionInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let stats = self.state.read(cx).info_stats();
        let (left, right) = {
            let session = self.state.read(cx);
            (session.left_path(), session.right_path())
        };
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("Session info").child(
                v_flex()
                    .gap_1()
                    .text_sm()
                    .child(format!("Left:  {left}"))
                    .child(format!("Right: {right}"))
                    .child(format!("Left lines:  {}", stats.left_lines))
                    .child(format!("Right lines: {}", stats.right_lines))
                    .child(format!("Difference sections: {}", stats.sections))
                    .child(format!("Inserted lines: {}", stats.insertions))
                    .child(format!("Deleted lines: {}", stats.deletions))
                    .child(format!("Changed lines: {}", stats.changes)),
            )
        });
    }
}

// ---------------------------------------------------------------------------
// Editor mutation helpers
// ---------------------------------------------------------------------------

/// Byte range of 1-based inclusive lines `lo..=hi` in the editor's rope,
/// spanning the trailing newline of `hi` when there is one.
fn editor_line_byte_range(
    editor: &Entity<EditorState>,
    lo: usize,
    hi_inclusive: usize,
    cx: &App,
) -> (usize, usize) {
    editor.read_with(cx, |ed, _| {
        let rope = ed.text();
        let n = rope.lines_len();
        let lo0 = lo.saturating_sub(1);
        let b0 = if lo0 < n {
            rope.line_start_offset(lo0)
        } else {
            rope.len()
        };
        let b1 = if hi_inclusive < n {
            rope.line_start_offset(hi_inclusive)
        } else {
            rope.len()
        };
        (b0, b1.max(b0))
    })
}

/// Replace 1-based inclusive lines `lo..=hi` with `text`, newlines
/// included: the replaced range spans the trailing newline of `hi`.
fn replace_editor_lines(
    editor: &Entity<EditorState>,
    lo: usize,
    hi_inclusive: usize,
    text: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let (b0, b1) = editor_line_byte_range(editor, lo, hi_inclusive, cx);
    editor.update(cx, |ed, cx| {
        ed.set_selected_range(b0..b1, cx);
        ed.replace(text, window, cx);
    });
}

/// Insert `text` at the start of 1-based line `line` (or at the end of
/// the buffer when the line does not exist).
fn insert_at_line_start(
    editor: &Entity<EditorState>,
    line: usize,
    text: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let (b, _) =
        editor_line_byte_range(editor, line, line.saturating_sub(1), cx);
    editor.update(cx, |ed, cx| {
        ed.set_selected_range(b..b, cx);
        ed.replace(text, window, cx);
    });
}

/// Select 1-based inclusive lines `lo..=hi` in the editor.
fn select_editor_lines(
    editor: &Entity<EditorState>,
    lo: usize,
    hi_inclusive: usize,
    cx: &mut App,
) {
    let (b0, b1) = editor_line_byte_range(editor, lo, hi_inclusive, cx);
    editor.update(cx, |ed, cx| ed.set_selected_range(b0..b1, cx));
}

/// Fill decorations over the difference lines of one side, computed
/// against the model (which matches the editor text right after a
/// recompare; the collection then tracks further edits).
fn line_decorations(
    lines: &[LineInfo],
    rows: &[DiffRow],
    side: Side,
    important: Hsla,
    minor: Hsla,
) -> Vec<RangeDecoration> {
    let mut minor_by_line: std::collections::HashMap<usize, bool> =
        std::collections::HashMap::new();
    for row in rows {
        if !row.status.is_difference() {
            continue;
        }
        if let Some(n) = match side {
            Side::Left => row.left,
            Side::Right => row.right,
        } {
            let entry = minor_by_line.entry(n).or_insert(true);
            *entry &= row.minor;
        }
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    for line in lines {
        if let Some(&is_minor) = minor_by_line.get(&line.number) {
            let mut color = if is_minor { minor } else { important };
            color.a = 0.18;
            out.push(
                RangeDecoration::new(offset..offset + line.content.len())
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(color),
            );
        }
        offset += line.content.len() + 1;
    }
    out
}

/// Prompt for a path and write the report; the outcome goes to the log.
fn save_report(text: &str, dir: &std::path::Path, cx: &mut App) {
    let text = text.to_string();
    let receiver = cx.prompt_for_new_path(dir, Some("cocomo-report.txt"));
    cx.spawn(async move |cx| {
        let path = match receiver.await {
            Ok(Ok(Some(path))) => path,
            // The user cancelled the save dialog.
            Ok(Ok(None)) => return,
            Ok(Err(e)) => {
                cx.update(|cx| {
                    log::push(
                        LogLevel::Error,
                        format!("Report save failed: {e}"),
                        cx,
                    );
                });
                return;
            }
            Err(_) => return,
        };
        let for_log = path.display().to_string();
        let result = crate::runtime::spawn_on_tokio(async move {
            fs_err::tokio::write(&path, text.as_bytes())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))
        })
        .await;
        cx.update(|cx| match result {
            Ok(Ok(())) => log::push(
                LogLevel::Info,
                format!("Report saved: {for_log}"),
                cx,
            ),
            Ok(Err(e)) => log::push(
                LogLevel::Error,
                format!("Report save failed: {e}"),
                cx,
            ),
            Err(e) => log::push(
                LogLevel::Error,
                format!("Report save task panicked: {e}"),
                cx,
            ),
        });
    })
    .detach();
}

impl TextDiffView {
    /// Render the view's toolbar row (row 2) for the window root.
    pub fn render_view_toolbar(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (word_wrap, has_sections, editing) = {
            let session = self.state.read(cx);
            (
                session.is_word_wrap(),
                session.section_count() > 0,
                session.is_editing(),
            )
        };

        let wrap_btn = Button::new("text2_wrap")
            .icon(IconName::TextWrap)
            .ghost()
            .small()
            .toggled(word_wrap)
            .tooltip("Toggle word wrap")
            .accessibility_label("Toggle word wrap")
            .on_click({
                let state = self.state.clone();
                move |_, _, cx| {
                    state.update(cx, |s, cx| s.toggle_wrap(cx));
                }
            });

        let prev_section_btn = {
            let state = self.state.clone();
            let left = self.left_list.clone();
            let right = self.right_list.clone();
            Button::new("text2_prev_section")
                .icon(IconName::ChevronsUp)
                .ghost()
                .small()
                .disabled(!has_sections)
                .tooltip("Previous difference section")
                .accessibility_label("Previous difference section")
                .on_click(move |_, _, cx| {
                    let target = state.update(cx, |s, cx| s.prev_section(cx));
                    if let Some(ix) = target {
                        left.scroll_to_reveal_item(ix);
                        right.scroll_to_reveal_item(ix);
                    }
                })
        };
        let next_section_btn = {
            let state = self.state.clone();
            let left = self.left_list.clone();
            let right = self.right_list.clone();
            Button::new("text2_next_section")
                .icon(IconName::ChevronsDown)
                .ghost()
                .small()
                .disabled(!has_sections)
                .tooltip("Next difference section")
                .accessibility_label("Next difference section")
                .on_click(move |_, _, cx| {
                    let target = state.update(cx, |s, cx| s.next_section(cx));
                    if let Some(ix) = target {
                        left.scroll_to_reveal_item(ix);
                        right.scroll_to_reveal_item(ix);
                    }
                })
        };

        let swap_btn = {
            let state = self.state.clone();
            let left_input = self.left_path_input.clone();
            let right_input = self.right_path_input.clone();
            Button::new("text2_swap")
                .icon(IconName::ArrowLeftRight)
                .ghost()
                .small()
                .tooltip("Swap sides")
                .accessibility_label("Swap sides")
                .on_click(move |_, window, cx| {
                    state.update(cx, |s, cx| s.swap_sides(cx));
                    let (l, r) = {
                        let s = state.read(cx);
                        (s.left_path(), s.right_path())
                    };
                    if let Some(input) = &left_input {
                        input.update(cx, |i, cx| {
                            i.set_value(l.clone(), window, cx);
                        });
                    }
                    if let Some(input) = &right_input {
                        input.update(cx, |i, cx| i.set_value(r, window, cx));
                    }
                })
        };

        let edit_btn = {
            let state = self.state.clone();
            Button::new("text2_edit")
                .icon(IconName::Pencil)
                .ghost()
                .small()
                .toggled(editing)
                .tooltip(if editing {
                    "Leave edit mode (edits are kept)"
                } else {
                    "Edit the two files"
                })
                .accessibility_label("Toggle edit mode")
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.toggle_editing(cx));
                })
        };

        let reload_btn = {
            let state = self.state.clone();
            Button::new("text2_reload")
                .icon(IconName::RefreshCw)
                .ghost()
                .small()
                .tooltip("Reload / recompare files")
                .accessibility_label("Reload comparison")
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.load(cx));
                })
        };

        Toolbar::new("text-view-toolbar")
            .child(
                ToolbarGroup::new("text-view-toolbar-view")
                    .label("View")
                    .child(wrap_btn),
            )
            .child(
                ToolbarGroup::new("text-view-toolbar-sections")
                    .label("Sections")
                    .child(prev_section_btn)
                    .child(next_section_btn),
            )
            .child(
                ToolbarGroup::new("text-view-toolbar-pair")
                    .label("Pair")
                    .child(swap_btn),
            )
            .child(
                ToolbarGroup::new("text-view-toolbar-edit")
                    .label("Edit")
                    .child(edit_btn),
            )
            .child(
                ToolbarGroup::new("text-view-toolbar-reload")
                    .label("Reload")
                    .child(reload_btn),
            )
            .into_any_element()
    }
}

impl Render for TextDiffView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Create the path inputs on first render; Enter commits a new
        // path and reloads that side.
        if self.left_path_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Left file path")
            });
            let sub = cx.subscribe(&input, |this, input, event, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().trim().to_string();
                    if !value.is_empty() {
                        this.state.update(cx, |s, cx| {
                            s.set_side_path(Side::Left, &value, cx);
                        });
                    }
                }
            });
            self._path_subs.push(sub);
            self.left_path_input = Some(input);
        }
        if self.right_path_input.is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Right file path")
            });
            let sub = cx.subscribe(&input, |this, input, event, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let value = input.read(cx).value().trim().to_string();
                    if !value.is_empty() {
                        this.state.update(cx, |s, cx| {
                            s.set_side_path(Side::Right, &value, cx);
                        });
                    }
                }
            });
            self._path_subs.push(sub);
            self.right_path_input = Some(input);
        }

        // Apply a detail-line commit that arrived from an input
        // subscription; the mutation needs this render's window.
        if let Some((side, line, text)) = self.pending_detail_commit.take() {
            let editor = self.state.read(cx).editor(side).cloned();
            if let Some(editor) = editor {
                replace_editor_lines(
                    &editor,
                    line,
                    line,
                    &format!("{text}\n"),
                    window,
                    cx,
                );
                self.after_editor_edit(side, cx);
            }
        }

        // Edit mode swaps the diff lists for editable editors. They are
        // created here (the session has no window) and attached to the
        // session, which tracks their dirty and focus state.
        let editing = self.state.read(cx).is_editing();
        if editing && self.state.read(cx).left_editor().is_none() {
            let left = cx.new(|cx| EditorState::new(window, cx));
            let right = cx.new(|cx| EditorState::new(window, cx));
            let left_decs = left.update(cx, |ed, cx| {
                ed.create_range_decorations_collection(Vec::new(), cx)
            });
            let right_decs = right.update(cx, |ed, cx| {
                ed.create_range_decorations_collection(Vec::new(), cx)
            });
            self.state
                .update(cx, |s, cx| s.attach_editors(left, right, cx));
            self.left_decs = Some(left_decs);
            self.right_decs = Some(right_decs);
            self.decoration_rev = usize::MAX;
        }
        // Re-seed the editors after a load; `set_value` clears undo
        // history and emits no change event, matching a fresh file read.
        if self.state.read(cx).editors_stale() {
            let editors = {
                let session = self.state.read(cx);
                (
                    session.left_editor().cloned(),
                    session.right_editor().cloned(),
                )
            };
            if let (Some(left), Some(right)) = editors {
                let (left_text, right_text, wrap) = {
                    let session = self.state.read(cx);
                    (
                        session.side_text(Side::Left, cx),
                        session.side_text(Side::Right, cx),
                        session.is_word_wrap(),
                    )
                };
                left.update(cx, |ed, cx| {
                    ed.set_value(left_text, window, cx);
                    ed.set_soft_wrap(wrap, window, cx);
                });
                right.update(cx, |ed, cx| {
                    ed.set_value(right_text, window, cx);
                    ed.set_soft_wrap(wrap, window, cx);
                });
                self.state.update(cx, |s, _| s.mark_editors_seeded());
                self.decoration_rev = usize::MAX;
            }
        }
        let left_input = self.left_path_input.clone().expect("just created");
        let right_input = self.right_path_input.clone().expect("just created");

        // Snapshot the session so the list closures own cheap Arc clones
        // and no borrow of `cx` outlives this scope head.
        let session = self.state.read(cx);
        let rows = session.rows.clone();
        let display = session.display.clone();
        let row_section = session.row_section.clone();
        let is_section_start = session.is_section_start.clone();
        let left_lines = session.left_lines.clone();
        let right_lines = session.right_lines.clone();
        let selected = session.selected;
        let word_wrap = session.word_wrap;
        let show_thumbnail = session.show_thumbnail;
        let loading = session.is_loading();
        let error_text = session.error().map(str::to_string);
        let left_path = session.left_path();
        let right_path = session.right_path();
        let left_meta = session.left_meta_label();
        let right_meta = session.right_meta_label();
        let left_enc = session.left_encoding_label();
        let right_enc = session.right_encoding_label();
        let left_end = session.left_ending_label();
        let right_end = session.right_ending_label();
        let left_hist = session.left_history().to_vec();
        let right_hist = session.right_history().to_vec();
        let overrides = session.overrides.clone();
        let section_count = session.sections.len();
        let alignment = session.alignment_label();
        let state_label = session.state_label();
        let load_label = session.load_label();
        let selected_row = session.selected_row().copied();
        let detail_mode = session.detail_mode();
        let model_rev = session.model_rev();
        let menu_flags = RowMenuFlags {
            anchor: session.align_anchor(),
            has_overrides: session.has_align_overrides(),
            can_copy_file_next: session.parent.is_some()
                && session.manager.is_some(),
        };

        // Seed the inputs once from the endpoints; later edits belong to
        // the user (swap re-syncs them through `sync_path_inputs`).
        if left_input.read(cx).value().is_empty() && !left_path.is_empty() {
            left_input.update(cx, |i, cx| {
                i.set_value(left_path.clone(), window, cx);
            });
        }
        if right_input.read(cx).value().is_empty() && !right_path.is_empty() {
            right_input.update(cx, |i, cx| {
                i.set_value(right_path.clone(), window, cx);
            });
        }

        // Reset the lists only when the row count or wrap mode changed;
        // a reset per frame would destroy the scroll position.
        let count = display.len();
        let row_h = px(window.rem_size().as_f32() * 1.5);
        if self.list_count != count || self.list_wrap != word_wrap {
            if word_wrap {
                self.left_list.reset(count);
                self.right_list.reset(count);
            } else {
                self.left_list.reset_with_uniform_height(count, row_h);
                self.right_list.reset_with_uniform_height(count, row_h);
            }
            self.list_count = count;
            self.list_wrap = word_wrap;
        }

        let theme = cx.theme().clone();
        let important = ColorRole::Important.resolve(&theme, &overrides);
        let minor = ColorRole::Minor.resolve(&theme, &overrides);
        let mono = theme.mono_font_family.clone();
        let fg = theme.foreground;
        let muted = theme.muted;
        let muted_fg = theme.muted_foreground;
        let list_active = theme.list_active;
        let list_hover = theme.list_hover;
        let border = theme.border;
        let danger = theme.danger;

        // Refresh the editor difference fills whenever the model changed;
        // the ranges track subsequent edits on their own.
        if editing && model_rev != self.decoration_rev {
            self.decoration_rev = model_rev;
            if let Some(decs) = &self.left_decs {
                decs.set(
                    line_decorations(
                        &left_lines,
                        &rows,
                        Side::Left,
                        important,
                        minor,
                    ),
                    cx,
                );
            }
            if let Some(decs) = &self.right_decs {
                decs.set(
                    line_decorations(
                        &right_lines,
                        &rows,
                        Side::Right,
                        important,
                        minor,
                    ),
                    cx,
                );
            }
        }

        let left_el = list(self.left_list.clone(), {
            let rows = rows.clone();
            let display = display.clone();
            let row_section = row_section.clone();
            let is_section_start = is_section_start.clone();
            let lines = left_lines.clone();
            let state = self.state.clone();
            let mono = mono.clone();
            move |d, _window, _cx| {
                render_diff_row(
                    Side::Left,
                    d,
                    &display,
                    &rows,
                    &row_section,
                    &is_section_start,
                    &lines,
                    selected,
                    word_wrap,
                    row_h,
                    important,
                    minor,
                    muted,
                    muted_fg,
                    list_active,
                    list_hover,
                    fg,
                    &mono,
                    &state,
                    menu_flags,
                )
            }
        })
        .size_full()
        .min_h_0();
        let right_el = list(self.right_list.clone(), {
            let rows = rows.clone();
            let display = display.clone();
            let row_section = row_section.clone();
            let is_section_start = is_section_start.clone();
            let lines = right_lines.clone();
            let state = self.state.clone();
            let mono = mono.clone();
            move |d, _window, _cx| {
                render_diff_row(
                    Side::Right,
                    d,
                    &display,
                    &rows,
                    &row_section,
                    &is_section_start,
                    &lines,
                    selected,
                    word_wrap,
                    row_h,
                    important,
                    minor,
                    muted,
                    muted_fg,
                    list_active,
                    list_hover,
                    fg,
                    &mono,
                    &state,
                    menu_flags,
                )
            }
        })
        .size_full()
        .min_h_0();

        // The thumbnail maps the whole display list onto its height.
        let strip = show_thumbnail.then(|| {
            let runs: Vec<(f32, f32, Hsla)> = thumbnail_runs(&display, &rows)
                .into_iter()
                .map(|(s, e, role)| (s, e, role.resolve(&theme, &overrides)))
                .collect();
            let scroll =
                self.left_list.scroll_px_offset_for_scrollbar().y.as_f32();
            let max_off = self.left_list.max_offset_for_scrollbar().y.as_f32();
            let vp_h = self.left_list.viewport_bounds().size.height.as_f32();
            let total = max_off + vp_h;
            let viewport = if total > 0. {
                ((-scroll).max(0.) / total, (vp_h / total).min(1.))
            } else {
                (0., 1.)
            };
            let marker = selected
                .map(|s| (s as f32 + 0.5) / display.len().max(1) as f32);
            let on_navigate: NavigateFn = {
                let state = self.state.clone();
                let left = self.left_list.clone();
                let right = self.right_list.clone();
                Rc::new(move |frac, _window, cx| {
                    let target =
                        state.update(cx, |s, cx| s.select_frac(frac, cx));
                    if let Some(ix) = target {
                        left.scroll_to_reveal_item(ix);
                        right.scroll_to_reveal_item(ix);
                    }
                })
            };
            div().flex_shrink_0().h_full().child(ThumbnailStrip {
                id: ElementId::Name("text_thumbnail".into()),
                runs,
                viewport,
                marker,
                background: theme.muted,
                viewport_color: theme.foreground,
                marker_color: theme.accent,
                width: px(12.),
                dragging: self.thumbnail_drag.clone(),
                on_navigate,
            })
        });

        // Detail panes below the diff: editable text, a hex dump, or an
        // alignment caret, per the selected detail mode.
        let selected_line = |lines: &[LineInfo], n: Option<usize>| -> String {
            n.and_then(|n| lines.get(n - 1))
                .map(|l| l.content.clone())
                .unwrap_or_default()
        };
        let details = match detail_mode {
            DetailMode::Text => {
                if self.detail_inputs.is_none() {
                    let l = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("Left line — Enter commits")
                    });
                    let r = cx.new(|cx| {
                        InputState::new(window, cx)
                            .placeholder("Right line — Enter commits")
                    });
                    for (side, input) in [(Side::Left, &l), (Side::Right, &r)]
                    {
                        let sub = cx.subscribe(
                            input,
                            move |this, input, event, cx| {
                                if matches!(
                                    event,
                                    InputEvent::PressEnter { .. }
                                ) {
                                    let value =
                                        input.read(cx).value().to_string();
                                    let line = this
                                        .state
                                        .read(cx)
                                        .selected_line_number(side);
                                    if let Some(line) = line {
                                        this.pending_detail_commit =
                                            Some((side, line, value));
                                        cx.notify();
                                    }
                                }
                            },
                        );
                        self._detail_subs.push(sub);
                    }
                    self.detail_inputs = Some((l, r));
                    self.detail_seeded = None;
                }
                let (left_in, right_in) =
                    self.detail_inputs.clone().expect("just created");
                let key = (
                    selected_row.and_then(|r| r.left),
                    selected_row.and_then(|r| r.right),
                );
                if self.detail_seeded != Some(key) {
                    self.detail_seeded = Some(key);
                    let lt = selected_line(&left_lines, key.0);
                    let rt = selected_line(&right_lines, key.1);
                    left_in.update(cx, |i, cx| {
                        i.set_value(lt, window, cx);
                    });
                    right_in.update(cx, |i, cx| {
                        i.set_value(rt, window, cx);
                    });
                }
                h_flex()
                    .flex_shrink_0()
                    .h(px(64.))
                    .items_center()
                    .border_t_1()
                    .border_color(border)
                    .bg(muted)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .child(Input::new(&left_in).small()),
                    )
                    .child(div().w(px(1.)).bg(border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .child(Input::new(&right_in).small()),
                    )
            }
            DetailMode::Hex => h_flex()
                .flex_shrink_0()
                .h(px(64.))
                .border_t_1()
                .border_color(border)
                .bg(muted)
                .child(detail_half(
                    hex_dump(&selected_line(
                        &left_lines,
                        selected_row.and_then(|r| r.left),
                    )),
                    &mono,
                ))
                .child(div().w(px(1.)).bg(border))
                .child(detail_half(
                    hex_dump(&selected_line(
                        &right_lines,
                        selected_row.and_then(|r| r.right),
                    )),
                    &mono,
                )),
            DetailMode::Alignment => {
                let lt = selected_line(
                    &left_lines,
                    selected_row.and_then(|r| r.left),
                );
                let rt = selected_line(
                    &right_lines,
                    selected_row.and_then(|r| r.right),
                );
                let caret = first_difference_caret(&lt, &rt);
                h_flex()
                    .flex_shrink_0()
                    .h(px(64.))
                    .border_t_1()
                    .border_color(border)
                    .bg(muted)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .overflow_hidden()
                            .font_family(mono.clone())
                            .text_xs()
                            .child(lt.clone())
                            .child(caret.clone()),
                    )
                    .child(div().w(px(1.)).bg(border))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_1()
                            .overflow_hidden()
                            .font_family(mono.clone())
                            .text_xs()
                            .child(rt)
                            .child(caret),
                    )
            }
        };

        let mut bar = StatusBar::new();
        bar = bar.left(
            h_flex()
                .items_center()
                .gap_3()
                .child(format!("{section_count} difference section(s)"))
                .child(format!("Alignment {alignment}"))
                .child(state_label.to_string()),
        );
        let right_status = if loading {
            h_flex()
                .items_center()
                .gap_1p5()
                .child(Spinner::new().xsmall())
                .child("Loading…")
                .into_any_element()
        } else if let Some(err) = &error_text {
            div()
                .text_color(danger)
                .child(err.clone())
                .into_any_element()
        } else if let Some(label) = &load_label {
            div().child(label.clone()).into_any_element()
        } else {
            div().child("Ready").into_any_element()
        };
        let status_bar = bar.right(right_status);

        let focus = self.state.read(cx).focus_handle();
        // In edit mode each pane shows its editor instead of the diff
        // list; the scroll-synced lists stay mounted behind it.
        let left_body: AnyElement =
            match (editing, self.state.read(cx).left_editor()) {
                (true, Some(ed)) => {
                    Editor::new(ed).appearance(false).into_any_element()
                }
                _ => left_el.into_any_element(),
            };
        let right_body: AnyElement =
            match (editing, self.state.read(cx).right_editor()) {
                (true, Some(ed)) => {
                    Editor::new(ed).appearance(false).into_any_element()
                }
                _ => right_el.into_any_element(),
            };
        div()
            .key_context("TextCompare")
            .track_focus(&focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(fg)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when_some(strip, |row, strip| row.child(strip))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(
                                h_resizable("text-panes")
                                    .child(resizable_panel().child(
                                        render_pane(
                                            Side::Left,
                                            &left_input,
                                            &left_meta,
                                            left_enc,
                                            left_end,
                                            &left_hist,
                                            left_body,
                                            &theme,
                                            &self.state,
                                        ),
                                    ))
                                    .child(resizable_panel().child(
                                        render_pane(
                                            Side::Right,
                                            &right_input,
                                            &right_meta,
                                            right_enc,
                                            right_end,
                                            &right_hist,
                                            right_body,
                                            &theme,
                                            &self.state,
                                        ),
                                    )),
                            ),
                    ),
            )
            .child(details)
            .child(status_bar)
            .on_action(cx.listener(Self::handle_next_line))
            .on_action(cx.listener(Self::handle_prev_line))
            .on_action(cx.listener(Self::handle_next_difference))
            .on_action(cx.listener(Self::handle_prev_difference))
            .on_action(cx.listener(Self::handle_next_section))
            .on_action(cx.listener(Self::handle_prev_section))
            .on_action(cx.listener(Self::handle_toggle_wrap))
            .on_action(cx.listener(Self::handle_toggle_thumbnails))
            .on_action(cx.listener(Self::handle_reload))
            .on_action(cx.listener(Self::handle_swap_sides))
            .on_action(cx.listener(Self::handle_report))
            .on_action(cx.listener(Self::handle_info))
            .on_action(cx.listener(Self::handle_toggle_editing))
            .on_action(cx.listener(Self::handle_select_section))
            .on_action(cx.listener(Self::handle_delete_line))
            .on_action(cx.listener(Self::handle_insert_line_before))
            .on_action(cx.listener(Self::handle_insert_line_after))
            .on_action(cx.listener(|this, _: &ConvertTrimTrailing, w, cx| {
                this.handle_convert(ConvertOp::TrimTrailing, w, cx);
            }))
            .on_action(cx.listener(|this, _: &ConvertSpacesToTabs, w, cx| {
                this.handle_convert(ConvertOp::SpacesToTabs, w, cx);
            }))
            .on_action(cx.listener(|this, _: &ConvertTabsToSpaces, w, cx| {
                this.handle_convert(ConvertOp::TabsToSpaces, w, cx);
            }))
            .on_action(cx.listener(|this, _: &ConvertEndingsPc, w, cx| {
                this.handle_convert(ConvertOp::EndingsPc, w, cx);
            }))
            .on_action(cx.listener(|this, _: &ConvertEndingsMac, w, cx| {
                this.handle_convert(ConvertOp::EndingsMac, w, cx);
            }))
            .on_action(cx.listener(|this, _: &ConvertEndingsUnix, w, cx| {
                this.handle_convert(ConvertOp::EndingsUnix, w, cx);
            }))
            .on_action(cx.listener(Self::handle_compare_selection))
            .on_action(cx.listener(Self::handle_set_align_anchor))
            .on_action(cx.listener(Self::handle_align_with))
            .on_action(cx.listener(Self::handle_clear_alignments))
            .on_action(cx.listener(Self::handle_isolate))
            .on_action(cx.listener(|this, _: &CopySectionRight, w, cx| {
                this.handle_copy_section(Side::Right, w, cx);
            }))
            .on_action(cx.listener(|this, _: &CopySectionLeft, w, cx| {
                this.handle_copy_section(Side::Left, w, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyFileRightNext, w, cx| {
                this.handle_copy_file_next(Side::Right, w, cx);
            }))
            .on_action(cx.listener(|this, _: &CopyFileLeftNext, w, cx| {
                this.handle_copy_file_next(Side::Left, w, cx);
            }))
            .on_action(cx.listener(Self::handle_set_detail_mode))
    }
}

/// One half of the read-only detail row.
fn detail_half(text: String, mono: &SharedString) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .px_2()
        .py_1()
        .overflow_hidden()
        .font_family(mono.clone())
        .text_xs()
        .child(text)
}

/// One text pane: file bar (history, path input, encoding, line
/// endings, metadata) above the diff list.
#[allow(clippy::too_many_arguments)]
fn render_pane(
    side: Side,
    input: &Entity<InputState>,
    meta: &str,
    enc: &'static str,
    ending: &'static str,
    history: &[String],
    body: AnyElement,
    theme: &Theme,
    state: &Entity<TextDiffSession>,
) -> impl IntoElement {
    let side_key = if side == Side::Left { "l" } else { "r" };
    let history = history.to_vec();
    v_flex()
        .size_full()
        .min_h_0()
        .child(
            v_flex()
                .flex_shrink_0()
                .px_2()
                .py_1()
                .gap_0p5()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new(ElementId::Name(
                                format!("text_hist_{side_key}").into(),
                            ))
                            .icon(IconName::ChevronDown)
                            .ghost()
                            .xsmall()
                            .disabled(history.is_empty())
                            .tooltip("Path history")
                            .accessibility_label("Path history")
                            .dropdown_menu({
                                let input = input.clone();
                                let state = state.clone();
                                move |menu, _, _| {
                                    let mut menu = menu;
                                    for entry in history.iter().rev() {
                                        let input = input.clone();
                                        let state = state.clone();
                                        let entry = entry.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(entry.clone())
                                                .on_click(
                                                    move |_, window, cx| {
                                                        input.update(
                                                            cx,
                                                            |i, cx| {
                                                                i.set_value(
                                                                    entry
                                                                        .clone(
                                                                        ),
                                                                    window,
                                                                    cx,
                                                                );
                                                            },
                                                        );
                                                        state.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_side_path(
                                                                side, &entry,
                                                                cx,
                                                            );
                                                        },
                                                    );
                                                    },
                                                ),
                                        );
                                    }
                                    menu
                                }
                            }),
                        )
                        .child(
                            Input::new(input)
                                .small()
                                .flex_1()
                                .min_w_0()
                                .text_color(theme.foreground),
                        )
                        .child(
                            Button::new(ElementId::Name(
                                format!("text_enc_{side_key}").into(),
                            ))
                            .label(enc)
                            .ghost()
                            .xsmall()
                            .tooltip("File encoding")
                            .accessibility_label("File encoding")
                            .dropdown_menu({
                                let state = state.clone();
                                move |menu, _, _| {
                                    let menu = menu.item(
                                        PopupMenuItem::new("UTF-8")
                                            .checked(enc == "UTF-8")
                                            .on_click({
                                                let state = state.clone();
                                                move |_, _, cx| {
                                                    state.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_encoding(
                                                        side,
                                                        TextEncoding::Utf8,
                                                        cx,
                                                    );
                                                        },
                                                    );
                                                }
                                            }),
                                    );
                                    menu.item(
                                        PopupMenuItem::new("ANSI")
                                            .checked(enc == "ANSI")
                                            .on_click({
                                                let state = state.clone();
                                                move |_, _, cx| {
                                                    state.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_encoding(
                                                        side,
                                                        TextEncoding::Latin1,
                                                        cx,
                                                    );
                                                        },
                                                    );
                                                }
                                            }),
                                    )
                                }
                            }),
                        )
                        .child(
                            Button::new(ElementId::Name(
                                format!("text_end_{side_key}").into(),
                            ))
                            .label(ending)
                            .ghost()
                            .xsmall()
                            .tooltip("Line endings")
                            .accessibility_label("Line endings")
                            .dropdown_menu({
                                let state = state.clone();
                                move |menu, _, _| {
                                    let menu = menu.item(
                                        PopupMenuItem::new("PC")
                                            .checked(ending == "PC")
                                            .on_click({
                                                let state = state.clone();
                                                move |_, _, cx| {
                                                    state.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_line_ending(
                                                        side,
                                                        LineEnding::Crlf,
                                                        cx,
                                                    );
                                                        },
                                                    );
                                                }
                                            }),
                                    );
                                    let menu = menu.item(
                                        PopupMenuItem::new("Mac")
                                            .checked(ending == "Mac")
                                            .on_click({
                                                let state = state.clone();
                                                move |_, _, cx| {
                                                    state.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_line_ending(
                                                                side,
                                                                LineEnding::Cr,
                                                                cx,
                                                            );
                                                        },
                                                    );
                                                }
                                            }),
                                    );
                                    menu.item(
                                        PopupMenuItem::new("Unix")
                                            .checked(ending == "Unix")
                                            .on_click({
                                                let state = state.clone();
                                                move |_, _, cx| {
                                                    state.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_line_ending(
                                                                side,
                                                                LineEnding::Lf,
                                                                cx,
                                                            );
                                                        },
                                                    );
                                                }
                                            }),
                                    )
                                }
                            }),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(meta.to_string()),
                ),
        )
        .child(div().flex_1().min_h_0().overflow_hidden().child(body))
}

/// Presence flags for the per-row context menu, snapshotted per frame.
#[derive(Clone, Copy)]
struct RowMenuFlags {
    /// The pending alignment anchor, if a line is marked.
    anchor: Option<(Side, usize)>,
    /// Whether alignment overrides are in effect.
    has_overrides: bool,
    /// Whether "copy file and open next" can run (child session).
    can_copy_file_next: bool,
}

/// Render one display row of one pane: the collapsed-section header or
/// an aligned diff row with gutter arrow (or collapse box), line number,
/// and content.
#[allow(clippy::too_many_arguments)]
fn render_diff_row(
    side: Side,
    d: usize,
    display: &[DisplayRow],
    rows: &[DiffRow],
    row_section: &[usize],
    is_section_start: &[bool],
    lines: &[LineInfo],
    selected: Option<usize>,
    word_wrap: bool,
    row_h: Pixels,
    important: Hsla,
    minor: Hsla,
    muted: Hsla,
    muted_fg: Hsla,
    list_active: Hsla,
    list_hover: Hsla,
    foreground: Hsla,
    mono: &SharedString,
    state: &Entity<TextDiffSession>,
    menu_flags: RowMenuFlags,
) -> AnyElement {
    let side_key = if side == Side::Left { "l" } else { "r" };
    let Some(item) = display.get(d) else {
        return div().size_full().into_any_element();
    };
    match item {
        DisplayRow::Filtered { section, count } => {
            let state = state.clone();
            let section = *section;
            div()
                .id((
                    ElementId::from("text_filtered"),
                    format!("{side_key}-{d}"),
                ))
                .w_full()
                .flex()
                .flex_shrink_0()
                .justify_center()
                .py_0p5()
                .text_xs()
                .text_color(muted_fg)
                .hover(|el| el.bg(list_hover))
                .child(format!("[+] {count} FILTERED LINES"))
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.toggle_section(section, cx));
                })
                .into_any_element()
        }
        DisplayRow::Line(r) => {
            let Some(row) = rows.get(*r) else {
                return div().size_full().into_any_element();
            };
            let number = match side {
                Side::Left => row.left,
                Side::Right => row.right,
            };
            // The blank counterpart of an inserted/deleted row stays
            // muted so the row alignment remains visible in both panes.
            let Some(number) = number else {
                return div()
                    .id((
                        ElementId::from("text_row"),
                        format!("{side_key}-{d}"),
                    ))
                    .w_full()
                    .h(row_h)
                    .flex_shrink_0()
                    .bg(muted)
                    .into_any_element();
            };
            let si = row_section.get(*r).copied().unwrap_or(usize::MAX);
            let show_collapse = si != usize::MAX
                && is_section_start.get(*r).copied().unwrap_or(false);
            let gutter: AnyElement = if show_collapse {
                let state = state.clone();
                div()
                    .id((
                        ElementId::from("text_collapse"),
                        format!("{side_key}-{d}"),
                    ))
                    .w(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .text_color(muted_fg)
                    .hover(|el| el.text_color(foreground))
                    .child("[-]")
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| s.toggle_section(si, cx));
                    })
                    .into_any_element()
            } else {
                div()
                    .w(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .justify_center()
                    .text_color(muted_fg)
                    .child(row.status.gutter_arrow().unwrap_or("").to_string())
                    .into_any_element()
            };
            let content = lines
                .get(number - 1)
                .map(|l| l.content.as_str())
                .unwrap_or("");
            let fg = if row.status == RowStatus::Same {
                foreground
            } else if row.minor {
                minor
            } else {
                important
            };
            // Difference rows tint their background with the severity
            // color at low alpha, per the spec's 18% background.
            let diff_bg = row.status.is_difference().then(|| {
                let mut c = if row.minor { minor } else { important };
                c.a = 0.18;
                c
            });
            div()
                .id((ElementId::from("text_row"), format!("{side_key}-{d}")))
                .w_full()
                .flex()
                .flex_shrink_0()
                .text_color(fg)
                .font_family(mono.clone())
                .text_xs()
                .when_some(diff_bg, |el, c| el.bg(c))
                .when(selected == Some(d), |el| el.bg(list_active))
                .hover(|el| el.bg(list_hover))
                .when(!word_wrap, |el| {
                    el.h(row_h).overflow_hidden().whitespace_nowrap()
                })
                .when(word_wrap, |el| el.py_0p5())
                .child(gutter)
                .child(
                    div()
                        .w(px(40.))
                        .flex_shrink_0()
                        .pr_1()
                        .text_right()
                        .text_color(muted_fg)
                        .child(number.to_string()),
                )
                .child(div().flex_1().min_w_0().child(content.to_string()))
                .on_click({
                    let state = state.clone();
                    move |_, window, cx| {
                        let focus = state.read(cx).focus_handle();
                        window.focus(&focus, cx);
                        state.update(cx, |s, cx| s.select_display(d, cx));
                    }
                })
                // Right-click focuses the session so the menu's actions
                // dispatch through the view's `TextCompare` key context.
                .on_mouse_down(MouseButton::Right, {
                    let state = state.clone();
                    move |_, window, cx| {
                        let focus = state.read(cx).focus_handle();
                        window.focus(&focus, cx);
                    }
                })
                .context_menu(move |menu, _, _| {
                    let mut menu = menu;
                    match menu_flags.anchor {
                        Some((anchor_side, _)) => {
                            if anchor_side != side {
                                menu = menu.menu(
                                    "Align with marked line",
                                    Box::new(AlignWithAnchor),
                                );
                            }
                        }
                        None => {
                            menu = menu.menu(
                                "Mark line for alignment",
                                Box::new(SetAlignAnchor { side }),
                            );
                        }
                    }
                    if menu_flags.has_overrides {
                        menu = menu.menu(
                            "Clear alignment overrides",
                            Box::new(ClearAlignments),
                        );
                    }
                    menu = menu
                        .separator()
                        .menu(
                            "Copy section → right",
                            Box::new(CopySectionRight),
                        )
                        .menu("Copy section ← left", Box::new(CopySectionLeft))
                        .menu("Isolate section", Box::new(IsolateSelection));
                    if menu_flags.can_copy_file_next {
                        menu = menu
                            .separator()
                            .menu(
                                "Copy file to right & open next",
                                Box::new(CopyFileRightNext),
                            )
                            .menu(
                                "Copy file to left & open next",
                                Box::new(CopyFileLeftNext),
                            );
                    }
                    menu
                })
                .into_any_element()
        }
    }
}

// ---------------------------------------------------------------------------
// Tab factory
// ---------------------------------------------------------------------------

/// Create a text compare tab view for the given file pair.
pub(crate) fn new_text_diff_view(
    left: Endpoint,
    right: Endpoint,
    grammar: Option<Grammar>,
    parent: Option<WeakEntity<AppState>>,
    manager: Option<Entity<GuiSessionManager>>,
    cx: &mut App,
) -> Entity<TextDiffView> {
    // Grammar-aware token classification arrives with the editing phase.
    let _ = grammar;
    let title = format!("{} vs {}", base_name(&left), base_name(&right));
    let session = cx.new(|cx| {
        TextDiffSession::new(left, right, title.into(), parent, manager, cx)
    });
    cx.new(|cx| TextDiffView::new(session, cx))
}

/// Create a text compare tab view over two in-memory texts ("compare
/// selection to clipboard").
pub(crate) fn new_text_diff_view_from_texts(
    left_text: &str,
    right_text: &str,
    title: SharedString,
    manager: Option<Entity<GuiSessionManager>>,
    cx: &mut App,
) -> Entity<TextDiffView> {
    let session = cx.new(|cx| {
        TextDiffSession::from_texts(left_text, right_text, title, manager, cx)
    });
    cx.new(|cx| TextDiffView::new(session, cx))
}

/// File name of an endpoint, for the tab title.
fn base_name(endpoint: &Endpoint) -> String {
    endpoint
        .path()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| endpoint.display())
}

// ---------------------------------------------------------------------------
// Session rules dialog
// ---------------------------------------------------------------------------

/// Body of the text session's rules dialog. A `Render` entity so the
/// checkboxes repaint as they are toggled (dialog content is otherwise
/// built only once when the dialog opens).
struct TextRulesBody {
    session: Entity<TextDiffSession>,
    ignore_case: bool,
    ignore_whitespace: bool,
    ignore_blank_lines: bool,
    ignore_comments: bool,
    ignore_numeric: bool,
}

impl TextRulesBody {
    fn new(session: Entity<TextDiffSession>, cx: &mut Context<Self>) -> Self {
        let settings = session.read(cx).compare_settings();
        Self {
            ignore_case: settings.ignore_case,
            ignore_whitespace: !matches!(
                settings.whitespace_mode,
                WhitespaceMode::Sensitive
            ),
            ignore_blank_lines: settings.ignore_blank_lines,
            ignore_comments: settings.ignore_comments,
            ignore_numeric: settings.ignore_numeric_changes,
            session,
        }
    }

    /// Push the drafted rules into the session and re-diff.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let settings = TextCompareSettings {
            ignore_case: self.ignore_case,
            whitespace_mode: if self.ignore_whitespace {
                WhitespaceMode::Insensitive
            } else {
                WhitespaceMode::Sensitive
            },
            ignore_blank_lines: self.ignore_blank_lines,
            ignore_comments: self.ignore_comments,
            ignore_numeric_changes: self.ignore_numeric,
            ..TextCompareSettings::new()
        };
        self.session
            .update(cx, |s, cx| s.set_compare_settings(settings, cx));
    }

    fn toggle(
        &mut self,
        field: fn(&mut Self, bool),
        checked: bool,
        cx: &mut Context<Self>,
    ) {
        field(self, checked);
        cx.notify();
    }
}

impl Render for TextRulesBody {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row = |label: &'static str,
                   checked: bool,
                   handler: fn(&mut Self, bool)| {
            Checkbox::new(label)
                .label(label)
                .checked(checked)
                .on_change(cx.listener(move |this, checked, _, cx| {
                    this.toggle(handler, *checked, cx);
                }))
        };
        v_flex()
            .w_full()
            .gap_2()
            .child(row("Ignore case", self.ignore_case, |this, v| {
                this.ignore_case = v
            }))
            .child(row(
                "Ignore whitespace",
                self.ignore_whitespace,
                |this, v| this.ignore_whitespace = v,
            ))
            .child(row(
                "Ignore blank lines",
                self.ignore_blank_lines,
                |this, v| this.ignore_blank_lines = v,
            ))
            .child(row(
                "Ignore comment lines",
                self.ignore_comments,
                |this, v| this.ignore_comments = v,
            ))
            .child(row(
                "Treat numeric changes as equal",
                self.ignore_numeric,
                |this, v| this.ignore_numeric = v,
            ))
            .child(
                h_flex().justify_end().child(
                    Button::new("rules_apply")
                        .label("Apply")
                        .primary()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.apply(cx);
                        })),
                ),
            )
    }
}

/// Open the rules dialog for a text compare session.
pub(crate) fn open_text_rules_dialog(
    session: Entity<TextDiffSession>,
    window: &mut Window,
    cx: &mut App,
) {
    let body = cx.new(|cx| TextRulesBody::new(session, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("Session rules")
            .w(px(420.))
            .child(body.clone())
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use cocomo_lib::Token;
    use gpui::TestAppContext;

    use super::*;

    fn line(number: usize, content: &str) -> LineInfo {
        LineInfo::new(number, content.to_string())
    }

    fn token_line(
        number: usize,
        content: &str,
        importance: Importance,
    ) -> LineInfo {
        let mut info = line(number, content);
        info.tokens.push(Token {
            text: content.to_string(),
            importance,
            offset: 0,
        });
        info
    }

    fn diff_with(
        differences: Vec<TextDifference>,
        left_line_count: usize,
        right_line_count: usize,
    ) -> TextDiff {
        TextDiff {
            differences,
            left_line_count,
            right_line_count,
            max_importance: Importance::Code,
        }
    }

    #[test]
    fn align_rows_pairs_context_around_a_changed_line() {
        let diff = diff_with(
            vec![TextDifference::LineDifferent(line(2, "b"), line(2, "B"))],
            3,
            3,
        );
        let rows = align_rows(&diff);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, RowStatus::Same);
        assert_eq!(rows[1].status, RowStatus::Changed);
        assert_eq!(rows[1].left, Some(2));
        assert_eq!(rows[1].right, Some(2));
        assert_eq!(rows[2].status, RowStatus::Same);
    }

    #[test]
    fn align_rows_gives_added_lines_a_blank_left_cell() {
        let diff = diff_with(
            vec![TextDifference::LinesAdded(2, vec![line(2, "n")])],
            2,
            3,
        );
        let rows = align_rows(&diff);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, RowStatus::Same);
        assert_eq!(rows[1].status, RowStatus::Inserted);
        assert_eq!(rows[1].left, None);
        assert_eq!(rows[1].right, Some(2));
        assert_eq!(rows[2].status, RowStatus::Same);
        assert_eq!(rows[2].left, Some(2));
        assert_eq!(rows[2].right, Some(3));
    }

    #[test]
    fn align_rows_gives_removed_lines_a_blank_right_cell() {
        let diff = diff_with(
            vec![TextDifference::LinesRemoved(2, vec![line(2, "x")])],
            3,
            2,
        );
        let rows = align_rows(&diff);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].status, RowStatus::Deleted);
        assert_eq!(rows[1].left, Some(2));
        assert_eq!(rows[1].right, None);
        assert_eq!(rows[2].status, RowStatus::Same);
    }

    #[test]
    fn align_rows_splits_an_uneven_changed_block() {
        let diff = diff_with(
            vec![TextDifference::LinesChanged(
                2,
                vec![line(2, "b"), line(3, "c")],
                vec![line(2, "X")],
            )],
            3,
            2,
        );
        let rows = align_rows(&diff);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, RowStatus::Same);
        assert_eq!(rows[1].status, RowStatus::Changed);
        assert_eq!(rows[2].status, RowStatus::Deleted);
    }

    #[test]
    fn identical_texts_align_to_same_rows() {
        let diff = diff_with(Vec::new(), 2, 2);
        let rows = align_rows(&diff);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.status == RowStatus::Same));
    }

    #[test]
    fn minor_requires_tokens_at_comment_or_lower() {
        // No tokens (no grammar applied) is *not* minor.
        assert!(!lines_are_minor([&line(1, "a")]));
        assert!(lines_are_minor([&token_line(1, "a", Importance::Comment)]));
        assert!(lines_are_minor([&token_line(1, "a", Importance::Ignored)]));
        assert!(!lines_are_minor([&token_line(1, "a", Importance::Data)]));
        // A mixed pair is only minor when *both* lines are.
        assert!(!lines_are_minor([
            &token_line(1, "a", Importance::Comment),
            &token_line(1, "b", Importance::Code),
        ]));
    }

    fn row(
        left: Option<usize>,
        right: Option<usize>,
        status: RowStatus,
        minor: bool,
    ) -> DiffRow {
        DiffRow {
            left,
            right,
            status,
            minor,
        }
    }

    #[test]
    fn diff_sections_aggregate_kind_and_minor() {
        let rows = vec![
            row(Some(1), Some(1), RowStatus::Same, false),
            row(None, Some(2), RowStatus::Inserted, true),
            row(None, Some(3), RowStatus::Inserted, true),
            row(Some(2), Some(4), RowStatus::Same, false),
            row(Some(3), None, RowStatus::Deleted, false),
            row(Some(4), Some(5), RowStatus::Changed, false),
        ];
        let sections = diff_sections(&rows);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].kind, SectionKind::Insert);
        assert!(sections[0].minor);
        assert_eq!(sections[0].start_row, 1);
        assert_eq!(sections[0].end_row, 3);
        // A run with both a delete and a change aggregates to Change.
        assert_eq!(sections[1].kind, SectionKind::Change);
        assert!(!sections[1].minor);
        assert_eq!(sections[1].start_row, 4);
        assert_eq!(sections[1].end_row, 6);
    }

    #[test]
    fn display_rows_replaces_collapsed_sections_with_headers() {
        let rows = vec![
            row(Some(1), Some(1), RowStatus::Same, false),
            row(Some(2), Some(2), RowStatus::Changed, false),
            row(Some(3), Some(3), RowStatus::Changed, false),
            row(Some(4), Some(4), RowStatus::Same, false),
        ];
        let sections = diff_sections(&rows);
        let (display, line_display, section_display) =
            display_rows(&rows, &sections, &HashSet::new());
        assert_eq!(display.len(), 4);
        // Expanded sections have no display index of their own.
        assert_eq!(section_display[0], usize::MAX);
        let collapsed = HashSet::from([0usize]);
        let (display, line_display_c, section_display) =
            display_rows(&rows, &sections, &collapsed);
        assert_eq!(display.len(), 3);
        assert_eq!(
            display[1],
            DisplayRow::Filtered {
                section: 0,
                count: 2
            }
        );
        assert_eq!(section_display[0], 1);
        assert_eq!(line_display_c[1], usize::MAX);
        assert_eq!(line_display[1], 1);
    }

    #[test]
    fn difference_navigation_wraps_around() {
        let rows = vec![
            row(Some(1), Some(1), RowStatus::Same, false),
            row(Some(2), Some(2), RowStatus::Changed, false),
            row(Some(3), Some(3), RowStatus::Same, false),
            row(None, Some(4), RowStatus::Inserted, false),
        ];
        let display: Vec<DisplayRow> =
            (0..rows.len()).map(DisplayRow::Line).collect();
        assert_eq!(next_difference_index(&display, &rows, 0), Some(1));
        assert_eq!(next_difference_index(&display, &rows, 2), Some(3));
        // `from` is inclusive: standing on a difference keeps you there.
        assert_eq!(next_difference_index(&display, &rows, 3), Some(3));
        // Past the end the scan wraps back to the first difference.
        assert_eq!(next_difference_index(&display, &rows, 4), Some(1));
        assert_eq!(prev_difference_index(&display, &rows, 2), Some(1));
        assert_eq!(prev_difference_index(&display, &rows, 1), Some(3));
        assert_eq!(next_difference_index(&[], &[], 0), None);
    }

    #[test]
    fn section_navigation_wraps_around() {
        let section_display = vec![1usize, 4];
        assert_eq!(next_section_index(&section_display, None), Some(1));
        assert_eq!(next_section_index(&section_display, Some(1)), Some(4));
        assert_eq!(next_section_index(&section_display, Some(4)), Some(1));
        assert_eq!(prev_section_index(&section_display, None), Some(4));
        assert_eq!(prev_section_index(&section_display, Some(4)), Some(1));
        assert_eq!(prev_section_index(&section_display, Some(1)), Some(4));
    }

    #[test]
    fn thumbnail_runs_merge_adjacent_rows_of_equal_severity() {
        let rows = vec![
            row(Some(1), Some(1), RowStatus::Same, false),
            row(Some(2), Some(2), RowStatus::Changed, false),
            row(Some(3), Some(3), RowStatus::Inserted, false),
            row(Some(4), Some(4), RowStatus::Same, false),
            row(Some(5), Some(5), RowStatus::Deleted, true),
        ];
        let display: Vec<DisplayRow> =
            (0..rows.len()).map(DisplayRow::Line).collect();
        let runs = thumbnail_runs(&display, &rows);
        assert_eq!(runs.len(), 2);
        assert!((runs[0].0 - 0.2).abs() < f32::EPSILON);
        assert!((runs[0].1 - 0.6).abs() < f32::EPSILON);
        assert_eq!(runs[0].2, ColorRole::Important);
        assert_eq!(runs[1].2, ColorRole::Minor);
        assert!(thumbnail_runs(&[], &[]).is_empty());
    }

    #[test]
    fn decode_bytes_handles_both_encodings() {
        // Latin-1 maps bytes 1:1, so 0xE9 is é.
        assert_eq!(
            decode_bytes(&[0x65, 0xE9], TextEncoding::Latin1),
            "e\u{e9}"
        );
        // UTF-8 decoding is lossy, never fatal.
        assert_eq!(decode_bytes(b"a\xffb", TextEncoding::Utf8), "a\u{fffd}b");
    }

    #[test]
    fn split_lines_handles_all_endings_without_trailing_empty() {
        let split = split_lines("a\r\nb\rc\nd\n");
        let contents: Vec<&str> =
            split.iter().map(|l| l.content.as_str()).collect();
        assert_eq!(contents, vec!["a", "b", "c", "d"]);
        let numbers: Vec<usize> =
            split_lines("a\nb").iter().map(|l| l.number).collect();
        assert_eq!(numbers, vec![1, 2]);
        assert!(split_lines("").is_empty());
    }

    #[test]
    fn detect_line_ending_finds_the_first_convention() {
        assert_eq!(detect_line_ending(b"a\r\nb"), LineEnding::Crlf);
        assert_eq!(detect_line_ending(b"a\rb"), LineEnding::Cr);
        assert_eq!(detect_line_ending(b"a\nb"), LineEnding::Lf);
        assert_eq!(detect_line_ending(b"ab"), LineEnding::Lf);
    }

    #[test]
    fn labels_match_the_spec() {
        assert_eq!(RowStatus::Inserted.gutter_arrow(), Some("→"));
        assert_eq!(RowStatus::Deleted.gutter_arrow(), Some("←"));
        assert_eq!(RowStatus::Changed.gutter_arrow(), Some("↔"));
        assert_eq!(RowStatus::Same.gutter_arrow(), None);
        assert_eq!(RowStatus::Changed.label(), "Change");
        assert_eq!(encoding_label(TextEncoding::Latin1), "ANSI");
        assert_eq!(encoding_label(TextEncoding::Utf8), "UTF-8");
        assert_eq!(line_ending_label(LineEnding::Crlf), "PC");
        assert_eq!(line_ending_label(LineEnding::Cr), "Mac");
        assert_eq!(line_ending_label(LineEnding::Lf), "Unix");
    }

    #[gpui::test]
    fn session_model_compares_reports_and_navigates(cx: &mut TestAppContext) {
        let session = cx.new(|cx| {
            TextDiffSession::for_test("a\nb\nc\nd\n", "a\nX\nY\nd\n", cx)
        });

        cx.update(|cx| {
            let s = session.read(cx);
            let stats = s.info_stats();
            assert_eq!(stats.left_lines, 4);
            assert_eq!(stats.right_lines, 4);
            assert_eq!(stats.sections, 1);
            assert_eq!(stats.changes, 2);
            assert_eq!(s.section_count(), 1);
            assert_eq!(s.alignment_label(), "—:—");
            assert_eq!(s.load_label().as_deref(), Some("Loaded in 1 ms"));
            let report = s.report_text();
            assert!(report.contains("Changed lines: 2"));
            assert!(report.contains("#1 Change"));
        });

        // Navigation walks to the first difference row.
        let target =
            cx.update(|cx| session.update(cx, |s, cx| s.next_difference(cx)));
        assert_eq!(target, Some(1));
        cx.update(|cx| {
            session.update(cx, |s, cx| {
                assert_eq!(s.state_label(), "Change");
                assert_eq!(s.alignment_label(), "2:2");
                // Collapsing the section replaces its two rows with one
                // filtered-lines header.
                s.toggle_section(0, cx);
                assert_eq!(s.display.len(), s.rows.len() - 1);
                assert!(matches!(s.display[1], DisplayRow::Filtered { .. }));
                s.toggle_section(0, cx);
                assert_eq!(s.display.len(), s.rows.len());
            });
        });

        // View options toggle.
        cx.update(|cx| {
            session.update(cx, |s, cx| {
                s.toggle_wrap(cx);
                s.toggle_thumbnail(cx);
                assert!(s.is_word_wrap());
                assert!(s.is_show_thumbnail());
            });
        });

        // Swapping sides exchanges the endpoints and their state.
        cx.update(|cx| {
            session.update(cx, |s, cx| {
                s.swap_sides(cx);
                assert_eq!(s.left_path(), "/right.txt");
                assert_eq!(s.right_path(), "/left.txt");
            });
        });
    }

    #[gpui::test]
    fn encoding_and_ending_selectors_update_the_session(
        cx: &mut TestAppContext,
    ) {
        // The left file holds UTF-8 `café`, the right the mojibake
        // `cafÃ©`. Under UTF-8 the lines differ; re-reading the left side
        // as Latin-1 turns its UTF-8 bytes into the same mojibake, making
        // The sides equal.
        let session = cx.new(|cx| {
            TextDiffSession::for_test("caf\u{e9}\n", "caf\u{c3}\u{a9}\n", cx)
        });
        cx.update(|cx| {
            assert_eq!(session.read(cx).info_stats().changes, 1);
        });
        cx.update(|cx| {
            session.update(cx, |s, cx| {
                s.set_encoding(Side::Left, TextEncoding::Latin1, cx);
                assert_eq!(s.left_encoding_label(), "ANSI");
                assert_eq!(s.info_stats().changes, 0);
                s.set_line_ending(Side::Left, LineEnding::Crlf, cx);
                assert_eq!(s.left_ending_label(), "PC");
            });
        });
    }

    #[gpui::test]
    fn text_view_handles_navigation_and_view_actions(cx: &mut TestAppContext) {
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        use crate::{
            menus::{ShowThumbnails, ToggleWordWrap},
            ui::NextDifference,
        };

        cx.update(gpui::init);

        let session = cx
            .new(|cx| TextDiffSession::for_test("a\nb\nc\n", "a\nX\nc\n", cx));
        let (window, view) = cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|cx| TextDiffView::new(session.clone(), cx)),
            )
            .expect("failed to open the test window")
        });

        // Focus the view so dispatched actions reach its handlers, then
        // walk to the only difference.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let focus = view.read(cx).focus_handle(cx);
            focus.focus(window, cx);
            window.dispatch_action(Box::new(NextDifference), cx);
        })
        .expect("failed to dispatch the next-difference action");

        cx.update(|cx| {
            let s = session.read(cx);
            let row = s.selected.unwrap();
            match s.display[row] {
                DisplayRow::Line(r) => {
                    assert_eq!(s.rows[r].status, RowStatus::Changed)
                }
                DisplayRow::Filtered { .. } => panic!("expected a line row"),
            }
        });

        // The View menu actions toggle the session's display options.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(Box::new(ToggleWordWrap), cx);
            window.dispatch_action(Box::new(ShowThumbnails), cx);
        })
        .expect("failed to dispatch the view actions");

        cx.update(|cx| {
            let s = session.read(cx);
            assert!(s.is_word_wrap());
            assert!(s.is_show_thumbnail());
        });
    }

    #[test]
    fn align_overrides_merge_and_swap_rows() {
        // A deleted and an inserted row merge into one changed row.
        let mut rows = vec![
            DiffRow {
                left: Some(1),
                right: Some(1),
                status: RowStatus::Same,
                minor: false,
            },
            DiffRow {
                left: Some(2),
                right: None,
                status: RowStatus::Deleted,
                minor: false,
            },
            DiffRow {
                left: None,
                right: Some(2),
                status: RowStatus::Inserted,
                minor: false,
            },
        ];
        apply_align_overrides(&mut rows, &[(2, 2)]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].status, RowStatus::Changed);
        assert_eq!((rows[1].left, rows[1].right), (Some(2), Some(2)));

        // Two changed rows swap partners so the anchor pair shares a row.
        let mut rows = vec![
            DiffRow {
                left: Some(1),
                right: Some(2),
                status: RowStatus::Changed,
                minor: false,
            },
            DiffRow {
                left: Some(2),
                right: Some(1),
                status: RowStatus::Changed,
                minor: false,
            },
        ];
        apply_align_overrides(&mut rows, &[(1, 1)]);
        assert_eq!((rows[0].left, rows[0].right), (Some(1), Some(1)));
        assert_eq!((rows[1].left, rows[1].right), (Some(2), Some(2)));

        // An out-of-range override changes nothing.
        let before = rows.clone();
        apply_align_overrides(&mut rows, &[(9, 9)]);
        assert_eq!(rows.len(), before.len());
    }

    #[gpui::test]
    fn align_override_forces_a_changed_row(cx: &mut App) {
        // The swapped `b`/`c` lines align as two changed rows; the
        // override re-pairs left 2 with right 3.
        let session = cx
            .new(|cx| TextDiffSession::for_test("a\nb\nc\n", "a\nc\nb\n", cx));
        session.update(cx, |s, cx| {
            s.align_overrides.push((2, 3));
            s.recompare(cx);
            assert!(s.rows.iter().any(|r| r.left == Some(2)
                && r.right == Some(3)
                && r.status == RowStatus::Changed));
            s.clear_alignments(cx);
            assert!(!s.has_align_overrides());
            // Back to the base alignment: the forced pair is gone.
            assert!(
                !s.rows
                    .iter()
                    .any(|r| r.left == Some(2) && r.right == Some(3))
            );
        });
    }

    #[test]
    fn convert_ops_rewrite_the_buffer() {
        assert_eq!(
            convert_text("a  \nb\t\n", ConvertOp::TrimTrailing),
            "a\nb\n"
        );
        assert_eq!(convert_text("    x\n", ConvertOp::SpacesToTabs), "\tx\n");
        assert_eq!(convert_text("  y\n", ConvertOp::SpacesToTabs), "  y\n");
        assert_eq!(convert_text("\tz\n", ConvertOp::TabsToSpaces), "    z\n");
        assert_eq!(convert_text("a\nb", ConvertOp::EndingsPc), "a\r\nb");
        assert_eq!(convert_text("a\r\nb\n", ConvertOp::EndingsMac), "a\rb\r");
        assert_eq!(convert_text("a\rb", ConvertOp::EndingsUnix), "a\nb");
    }

    #[test]
    fn encode_text_applies_ending_and_encoding() {
        assert_eq!(
            encode_text("a\nb\n", LineEnding::Crlf, TextEncoding::Utf8),
            b"a\r\nb\r\n"
        );
        assert_eq!(
            encode_text("a\r\nb", LineEnding::Lf, TextEncoding::Utf8),
            b"a\nb"
        );
        assert_eq!(
            encode_text("\u{e9}\n", LineEnding::Lf, TextEncoding::Latin1),
            vec![0xE9, b'\n']
        );
    }

    #[test]
    fn hex_dump_and_alignment_caret() {
        let dump = hex_dump("ab?");
        assert!(dump.starts_with("0000  61 62 3F"), "got: {dump}");
        assert!(dump.contains("|ab?|"));
        assert_eq!(hex_dump(""), "—");
        assert_eq!(first_difference_caret("abc", "abd"), "  ^");
        assert_eq!(first_difference_caret("", ""), "^");
    }

    #[gpui::test]
    fn isolate_and_section_helpers_use_the_selected_section(cx: &mut App) {
        let session = cx.new(|cx| {
            TextDiffSession::for_test("a\nb\nc\nd\ne\n", "a\nX\nc\nY\ne\n", cx)
        });
        session.update(cx, |s, cx| {
            assert_eq!(s.section_count(), 2);
            s.next_difference(cx);
            let keep = s.selected_section().expect("row is in a section");
            assert_eq!(s.section_line_range(keep, Side::Left), Some((2, 2)));
            assert_eq!(s.section_text(keep, Side::Left), "b\n");
            assert_eq!(s.section_text(keep, Side::Right), "X\n");
            s.isolate_selected(cx);
            assert_eq!(s.collapsed.len(), s.sections.len() - 1);
            assert!(!s.collapsed.contains(&keep));
        });
    }

    #[gpui::test]
    fn from_texts_compares_without_io(cx: &mut App) {
        let session = cx.new(|cx| {
            TextDiffSession::from_texts(
                "sel\n",
                "clip\n",
                "t".into(),
                None,
                cx,
            )
        });
        assert_eq!(session.read(cx).info_stats().changes, 1);
    }

    /// Minimal renderable root for windows that only need a `Window`.
    struct TestRoot;

    impl gpui::Render for TestRoot {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn recompare_uses_edited_buffers(cx: &mut TestAppContext) {
        use gpui::{Bounds, WindowBounds, WindowOptions};

        cx.update(gpui::init);
        let session =
            cx.new(|cx| TextDiffSession::for_test("a\nb\n", "a\nX\n", cx));
        let (window, _root) = cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|_| TestRoot),
            )
            .expect("failed to open the test window")
        });
        cx.update_window(window, |_, window, cx| {
            let left = cx.new(|cx| EditorState::new(window, cx));
            let right = cx.new(|cx| EditorState::new(window, cx));
            left.update(cx, |ed, cx| {
                ed.set_value("a\nX\n", window, cx);
            });
            right.update(cx, |ed, cx| {
                ed.set_value("a\nX\n", window, cx);
            });
            session.update(cx, |s, cx| {
                s.attach_editors(left, right, cx);
                // Stand in for the view's render-time seeding.
                s.mark_editors_seeded();
                s.recompare(cx);
            });
        })
        .expect("failed to attach the editors");
        cx.update(|cx| {
            let s = session.read(cx);
            assert_eq!(s.info_stats().changes, 0);
            assert_eq!(s.side_text(Side::Left, cx), "a\nX\n");
            // `set_value` emits no change event, so nothing is dirty.
            assert!(!s.is_dirty());
            session.update(cx, |s, cx| {
                s.mark_side_dirty(Side::Left, cx);
                assert!(s.is_dirty());
            });
        });
    }

    #[gpui::test]
    fn edit_mode_actions_drive_the_buffers(cx: &mut TestAppContext) {
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        cx.update(gpui::init);
        let session =
            cx.new(|cx| TextDiffSession::for_test("a  \nb\n", "a\nb\n", cx));
        let (window, view) = cx.update(|cx| {
            gpui::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(
                        Bounds::maximized(None, cx),
                    )),
                    ..Default::default()
                },
                cx,
                |_, cx| cx.new(|cx| TextDiffView::new(session.clone(), cx)),
            )
            .expect("failed to open the test window")
        });

        // Enter edit mode; the view creates and seeds the editors.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let focus = view.read(cx).focus_handle(cx);
            focus.focus(window, cx);
            window.dispatch_action(Box::new(ToggleTextEdit), cx);
            window.render_frame(cx);
        })
        .expect("failed to toggle edit mode");
        cx.update(|cx| {
            let s = session.read(cx);
            assert!(s.is_editing());
            assert!(s.left_editor().is_some());
            assert_eq!(s.side_text(Side::Left, cx), "a  \nb\n");
        });

        // Trim trailing whitespace on the focused (left) side; the
        // re-diff runs on the buffers and the edit marks the tab dirty.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(Box::new(ConvertTrimTrailing), cx);
        })
        .expect("failed to convert the left buffer");
        cx.update(|cx| {
            let s = session.read(cx);
            assert_eq!(s.side_text(Side::Left, cx), "a\nb\n");
            assert_eq!(s.info_stats().changes, 0);
            assert!(s.is_dirty());
        });

        // The detail-mode selector switches the detail panes.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(
                Box::new(SetDetailMode {
                    mode: DetailMode::Hex,
                }),
                cx,
            );
        })
        .expect("failed to switch the detail mode");
        cx.update(|cx| {
            assert_eq!(session.read(cx).detail_mode(), DetailMode::Hex);
        });
    }
}
