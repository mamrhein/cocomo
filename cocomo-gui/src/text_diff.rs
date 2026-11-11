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
    cell::Cell, collections::HashSet, path::Path, rc::Rc, sync::Arc,
    time::Instant,
};

use chrono::{DateTime, Local, Utc};
use cocomo_lib::{
    CancellationToken, ColorOverrides, Endpoint, FsError, Grammar, Importance,
    LineEnding, LineInfo, MergedRowStatus, NodeFileSystem,
    TextCompareSettings, TextDiff, TextEncoding, WhitespaceMode,
    compare_texts, format_size,
};
use gpui::{
    Action, AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, Div,
    Element, ElementId, Entity, ExternalPaths, FocusHandle, GlobalElementId,
    Hitbox, Hsla, InspectorElementId, IntoElement, LayoutId, ListAlignment,
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
    color::{ColorFields, ColorRole, Side},
    file_formats::FileFormatRegistry,
    log::{self, LogLevel},
    menus::{
        NextSection, PrevSection, ReloadCompare, SessionInfo, SessionReport,
        ShowThumbnails, StopCompare, ToggleWordWrap,
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
        NextDifferenceFiles,
        PrevDifferenceFiles,
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
/// Delegates the alignment itself to [`TextDiff::merged_rows`], which
/// pairs context and pads blocks of differing length with blank cells;
/// this function only projects each merged row onto the GUI's [`DiffRow`]
/// and computes the `minor` flag from the row's tokens.
pub(crate) fn align_rows(diff: &TextDiff) -> Vec<DiffRow> {
    diff.merged_rows()
        .iter()
        .map(|row| {
            let status = match row.status {
                MergedRowStatus::Same => RowStatus::Same,
                MergedRowStatus::Changed => RowStatus::Changed,
                MergedRowStatus::Inserted => RowStatus::Inserted,
                MergedRowStatus::Deleted => RowStatus::Deleted,
            };
            let minor = row.status.is_difference()
                && lines_are_minor(row.left.iter().chain(row.right.iter()));
            DiffRow {
                left: row.left.as_ref().map(|line| line.number),
                right: row.right.as_ref().map(|line| line.number),
                status,
                minor,
            }
        })
        .collect()
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
    /// Non-interactive header standing in for `count` context rows that
    /// the section context depth hides between differences.
    Context { count: usize },
}

/// Build the display list from the rows, the collapsed sections, and the
/// context depth.
///
/// Returns the display rows, the row-index → display-index map, and the
/// section-index → display-index map (`usize::MAX` for absent entries).
///
/// With a context depth of `Some(n)`, every maximal run of context rows
/// (rows outside any section) longer than `2n` is folded to its first
/// and last `n` rows plus a [`DisplayRow::Context`] header for the
/// hidden middle; `Some(0)` hides whole context runs. `None` keeps the
/// full context.
pub(crate) fn display_rows(
    rows: &[DiffRow],
    sections: &[DiffSection],
    collapsed: &HashSet<usize>,
    context_depth: Option<usize>,
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
    let push_line =
        |display: &mut Vec<DisplayRow>, line_display: &mut Vec<usize>, r| {
            line_display[r] = display.len();
            display.push(DisplayRow::Line(r));
        };
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
        } else if si != usize::MAX {
            push_line(&mut display, &mut line_display, i);
            i += 1;
        } else {
            // Maximal context run [i, end): fold it when a context
            // depth is set and the run is longer than the kept edges.
            let mut end = i;
            while end < rows.len() && section_of_row[end] == usize::MAX {
                end += 1;
            }
            let len = end - i;
            let fold = context_depth.is_some_and(|n| len > 2 * n);
            if fold {
                let keep = context_depth.unwrap_or(0);
                for r in i..i + keep {
                    push_line(&mut display, &mut line_display, r);
                }
                display.push(DisplayRow::Context {
                    count: len - 2 * keep,
                });
                for r in end - keep..end {
                    push_line(&mut display, &mut line_display, r);
                }
            } else {
                for r in i..end {
                    push_line(&mut display, &mut line_display, r);
                }
            }
            i = end;
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
pub(crate) fn detect_line_ending(bytes: &[u8]) -> LineEnding {
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

/// A source language with a built-in classification grammar and a
/// tree-sitter editor language.
///
/// The classification grammar drives minor/important row coloring and
/// comment skipping in the diff; the editor language names the tree-sitter
/// grammar the edit-mode editors highlight with. Extensions outside this
/// small set fall back to plain text (no classification, no highlighting).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lang {
    Rust,
    Python,
    C,
}

impl Lang {
    /// Detect a language from a file path's extension.
    fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::from_extension(&ext)
    }

    /// Detect a language from a lowercase extension.
    fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "py" | "pyi" => Some(Self::Python),
            "c" | "h" => Some(Self::C),
            _ => None,
        }
    }

    /// Detect a language from a registered format name. A user who
    /// registers a `"Rust"` format forces Rust classification and
    /// highlighting regardless of the file's extension.
    fn from_format_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "rust" => Some(Self::Rust),
            "python" => Some(Self::Python),
            "c" | "c/c++" | "cpp" | "c++" => Some(Self::C),
            _ => None,
        }
    }

    /// The cocomo-lib classification grammar for this language.
    fn grammar(self) -> Grammar {
        match self {
            Self::Rust => Grammar::rust(),
            Self::Python => Grammar::python(),
            Self::C => Grammar::c(),
        }
    }

    /// The tree-sitter editor language name for the edit-mode editors.
    fn editor_name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::C => "c",
        }
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
    /// Left format-name override from the format switcher (`None` keeps
    /// the registry's extension-derived format).
    left_format: Option<String>,
    /// Right format-name override from the format switcher.
    right_format: Option<String>,
    /// Per-session auto-substitutions applied to both sides during
    /// comparison (the rules dialog's replacement list).
    replacements: Vec<(String, String)>,
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
    /// Context lines kept around each difference (`None` shows the
    /// full context, `Some(0)` shows only the differences).
    context_depth: Option<usize>,
    /// Selected display index.
    selected: Option<usize>,
    /// Whether a load is in flight.
    loading: bool,
    /// Monotonic counter that invalidates results of stopped or
    /// superseded loads.
    load_revision: u64,
    /// Cancellation token of the in-flight load, if any.
    cancel_token: Option<CancellationToken>,
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
            left_format: None,
            right_format: None,
            replacements: Vec::new(),
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
            context_depth: None,
            selected: None,
            loading: false,
            load_revision: 0,
            cancel_token: None,
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
            DisplayRow::Filtered { .. } | DisplayRow::Context { .. } => None,
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
        self.load_revision += 1;
        let revision = self.load_revision;
        let token = CancellationToken::new();
        self.cancel_token = Some(token.clone());
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
            // The providers take no token, so the most a stop can save
            // here is the second read.
            if token.is_cancelled() {
                return Err(anyhow::anyhow!("aborted"));
            }
            let right = read_side(right).await?;
            Ok::<_, anyhow::Error>((
                left,
                right,
                started.elapsed().as_millis(),
            ))
        });

        cx.spawn(move |_, cx: &mut gpui::AsyncApp| {
            let async_app = cx.clone();
            async move {
                let result = match join_handle.await {
                    Ok(r) => r,
                    Err(e) => Err(anyhow::anyhow!("task panicked: {e}")),
                };
                async_app.update(|cx| {
                    if let Some(entity) = weak.upgrade() {
                        entity.update(cx, |session, cx| {
                            // A stop or a newer load superseded this
                            // one; drop the result without touching the
                            // session state.
                            if revision != session.load_revision {
                                return;
                            }
                            session.cancel_token = None;
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

    /// Abort the in-flight load, discarding its result.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.loading {
            return;
        }
        self.load_revision += 1;
        if let Some(token) = self.cancel_token.take() {
            token.cancel();
        }
        self.loading = false;
        log::push(LogLevel::Info, "Comparison aborted", cx);
        cx.notify();
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
        self.left_lines =
            Arc::new(self.tokenize_lines(&split_lines(&left_text)));
        self.right_lines =
            Arc::new(self.tokenize_lines(&split_lines(&right_text)));
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

    /// Rebuild the display list from rows, collapsed sections, and the
    /// context depth, keeping the selection within bounds.
    fn rebuild_display(&mut self) {
        let (display, _line_display, section_display) = display_rows(
            &self.rows,
            &self.sections,
            &self.collapsed,
            self.context_depth,
        );
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
                Side::Left => {
                    self.left = updated;
                    // A new file re-derives its format and language from
                    // its extension; a stale override would mislabel it.
                    self.left_format = None;
                }
                Side::Right => {
                    self.right = updated;
                    self.right_format = None;
                }
            }
            self.rebuild_grammar();
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
        std::mem::swap(&mut self.left_format, &mut self.right_format);
        self.rebuild_grammar();
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

    /// Context lines kept around each difference, or `None` for the
    /// full context.
    pub fn context_depth(&self) -> Option<usize> {
        self.context_depth
    }

    /// Set the section context depth: keep `depth` context lines around
    /// each difference (`Some(0)` shows the differences only, `None`
    /// shows the full context) and rebuild the display list.
    pub fn set_context_depth(
        &mut self,
        depth: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if self.context_depth == depth {
            return;
        }
        self.context_depth = depth;
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

    // -- Format, grammar, and replacements ---------------------------------

    /// The detected language for one side: the format override when it
    /// names a language, otherwise the side's file extension.
    fn side_lang(&self, side: Side) -> Option<Lang> {
        let (path, format) = match side {
            Side::Left => (self.left.path(), self.left_format.as_deref()),
            Side::Right => (self.right.path(), self.right_format.as_deref()),
        };
        format
            .and_then(Lang::from_format_name)
            .or_else(|| Lang::from_path(path))
    }

    /// The language the comparison classifies with: the left side's, or
    /// the right side's when the left has no recognized language.
    fn comparison_lang(&self) -> Option<Lang> {
        self.side_lang(Side::Left)
            .or_else(|| self.side_lang(Side::Right))
    }

    /// Rebuild the comparison grammar from the detected language and the
    /// replacement list, storing it in the compare settings.
    ///
    /// Replacements become grammar normalization rules: they rewrite text
    /// for the comparison only, leaving the displayed content untouched,
    /// so a substituted-equal pair aligns as identical.
    fn rebuild_grammar(&mut self) {
        let mut grammar = self
            .comparison_lang()
            .map(Lang::grammar)
            .unwrap_or_else(Grammar::plain_text);
        for (from, to) in &self.replacements {
            if from.is_empty() {
                continue;
            }
            if let Ok(pattern) = regex::Regex::new(&regex::escape(from)) {
                grammar.add_rule(
                    format!("replace:{from}"),
                    pattern,
                    Importance::Code,
                    Some(to.clone()),
                );
            }
        }
        self.compare_settings.grammar = Some(grammar);
    }

    /// The tree-sitter editor language name for one side.
    pub fn editor_lang(&self, side: Side) -> SharedString {
        self.side_lang(side)
            .map(Lang::editor_name)
            .unwrap_or("text")
            .into()
    }

    /// Left format label: the override or the registry's format name.
    pub fn left_format_label(&self, cx: &App) -> String {
        self.format_label(Side::Left, cx)
    }

    /// Right format label: the override or the registry's format name.
    pub fn right_format_label(&self, cx: &App) -> String {
        self.format_label(Side::Right, cx)
    }

    /// The format name shown in a pane's file bar.
    fn format_label(&self, side: Side, cx: &App) -> String {
        let override_name = match side {
            Side::Left => self.left_format.as_deref(),
            Side::Right => self.right_format.as_deref(),
        };
        match override_name {
            Some(name) => name.to_string(),
            None => {
                let path = match side {
                    Side::Left => self.left.path(),
                    Side::Right => self.right.path(),
                };
                FileFormatRegistry::format_name(path, cx)
            }
        }
    }

    /// Switch one side's format and re-derive the grammar. The view
    /// re-applies the editor highlighter from `editor_lang`.
    pub fn set_format(
        &mut self,
        side: Side,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        match side {
            Side::Left => self.left_format = Some(name.to_string()),
            Side::Right => self.right_format = Some(name.to_string()),
        }
        self.rebuild_grammar();
        if self.left_raw.is_some() || self.right_raw.is_some() {
            self.recompare(cx);
        } else {
            cx.notify();
        }
    }

    /// The current replacement list, for the rules dialog.
    pub fn replacements(&self) -> &[(String, String)] {
        &self.replacements
    }

    /// Replace the auto-substitution table and re-diff.
    pub fn set_replacements(
        &mut self,
        replacements: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        self.replacements = replacements;
        self.rebuild_grammar();
        self.recompare(cx);
    }

    /// Apply a format and grammar to a freshly created session before its
    /// first load. `grammar` is the caller's explicit choice; when `None`
    /// the language is detected from the endpoints.
    fn init_grammar(&mut self, grammar: Option<Grammar>) {
        match grammar {
            Some(g) => self.compare_settings.grammar = Some(g),
            None => self.rebuild_grammar(),
        }
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

    /// Re-point this session at a new owning manager.
    ///
    /// Called when the tab moves to another window: follow-up tabs and
    /// dirty marks must go to the window the tab now lives in.
    pub fn set_manager(&mut self, manager: Entity<GuiSessionManager>) {
        self.manager = Some(manager);
    }

    /// The folder session this diff was opened from, if any.
    pub fn parent(&self) -> Option<WeakEntity<AppState>> {
        self.parent.clone()
    }

    /// Whether the parent folder session is still open.
    pub fn has_live_parent(&self, cx: &App) -> bool {
        self.parent.as_ref().is_some_and(|parent| {
            parent.upgrade().is_some_and(|parent| {
                // A cleared parent has no comparison to step through.
                parent.read(cx).has_comparison()
            })
        })
    }

    /// Clear the comparison: drop content, diff, selection, and
    /// alignment state, keeping the endpoints and settings (Session ▸
    /// Clear session).
    ///
    /// An in-flight load is aborted first so its result cannot
    /// repopulate the cleared view; nothing auto-reloads because
    /// `ensure_loaded` only runs at view creation.
    pub fn clear_session(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.load_revision += 1;
            if let Some(token) = self.cancel_token.take() {
                token.cancel();
            }
        }
        self.loading = false;
        self.left_raw = None;
        self.right_raw = None;
        self.left_lines = Arc::new(Vec::new());
        self.right_lines = Arc::new(Vec::new());
        self.left_meta = None;
        self.right_meta = None;
        self.diff = None;
        self.rows = Arc::new(Vec::new());
        self.sections = Arc::new(Vec::new());
        self.display = Arc::new(Vec::new());
        self.section_display = Arc::new(Vec::new());
        self.row_section = Arc::new(Vec::new());
        self.is_section_start = Arc::new(Vec::new());
        self.collapsed.clear();
        self.selected = None;
        self.error = None;
        self.load_ms = None;
        self.align_overrides.clear();
        self.align_anchor = None;
        // Leave edit mode; the editors re-seed from the (now empty)
        // model the next time editing is entered.
        self.editing = false;
        self.left_dirty = false;
        self.right_dirty = false;
        self.editors_stale = true;
        self.model_rev += 1;
        cx.notify();
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
        self.left_lines =
            Arc::new(self.tokenize_lines(&split_lines(&left_text)));
        self.right_lines =
            Arc::new(self.tokenize_lines(&split_lines(&right_text)));
        self.diff = Some(compare_texts(
            &left_text,
            &right_text,
            &self.compare_settings,
        ));
        self.rebuild_model();
        cx.notify();
    }

    /// Classify each line with the comparison grammar so the diff rows can
    /// color content by importance. A no-op when no grammar is set.
    fn tokenize_lines(&self, lines: &[LineInfo]) -> Vec<LineInfo> {
        match &self.compare_settings.grammar {
            Some(grammar) => lines
                .iter()
                .map(|line| {
                    LineInfo::new(line.number, line.content.clone())
                        .with_grammar(grammar)
                })
                .collect(),
            None => lines.to_vec(),
        }
    }

    /// Replace the session's compare rules and re-diff.
    ///
    /// The grammar is rebuilt from the detected language and the
    /// replacement list afterward, so a rules-dialog apply (which builds
    /// fresh settings) never drops syntax classification.
    pub fn set_compare_settings(
        &mut self,
        settings: TextCompareSettings,
        cx: &mut Context<Self>,
    ) {
        self.compare_settings = settings;
        self.rebuild_grammar();
        self.recompare(cx);
    }

    /// The current compare rules, for the rules dialog.
    pub fn compare_settings(&self) -> &TextCompareSettings {
        &self.compare_settings
    }

    /// The per-session color overrides, for the rules dialog.
    pub fn overrides(&self) -> &ColorOverrides {
        &self.overrides
    }

    /// Replace the per-session color overrides (spec section 7.1).
    pub fn set_overrides(
        &mut self,
        overrides: ColorOverrides,
        cx: &mut Context<Self>,
    ) {
        self.overrides = overrides;
        cx.notify();
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
        (hi > 0).then_some((lo, hi))
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
        entity.update(cx, |session, cx| {
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
                manager.update(&mut cx, |m, cx| {
                    m.add_text_diff_tab(left, right, Some(parent.clone()), cx)
                });
            }
        })
        .detach();
    }

    /// Step the parent folder session to its next (`direction > 0`) or
    /// previous differing pair and open that pair in a new tab.
    ///
    /// This is Search ▸ Next/Previous Difference Files for a child text
    /// session (spec section 5.4); it moves the *parent's* selection, not
    /// this session's rows.
    pub fn step_difference_files(
        &mut self,
        direction: isize,
        cx: &mut Context<Self>,
    ) {
        let (Some(parent), Some(manager)) =
            (self.parent.clone(), self.manager.clone())
        else {
            log::push(
                LogLevel::Warning,
                "Next/Previous Difference Files needs a parent folder \
                 session."
                    .to_string(),
                cx,
            );
            return;
        };
        let next = parent.update(cx, |state, cx| {
            if direction > 0 {
                state.select_next_difference(cx);
            } else {
                state.select_previous_difference(cx);
            }
            state.selected_endpoints()
        });
        let Ok(Some((left, right))) = next else {
            return;
        };
        manager.update(cx, |m, cx| {
            m.add_text_diff_tab(left, right, Some(parent.clone()), cx)
        });
    }
}

/// File-bar metadata line: modified · size · line endings. The format
/// name lives in its own switcher in the file-bar row, not here.
fn meta_label(meta: Option<&FileMeta>, ending: LineEnding) -> String {
    let size = meta
        .map(|m| format_size(m.size))
        .unwrap_or_else(|| "—".to_string());
    let modified = meta
        .map(|m| m.modified.clone())
        .unwrap_or_else(|| "—".to_string());
    format!("{modified} · {size} · {}", line_ending_label(ending))
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
// Hatched shading element
// ---------------------------------------------------------------------------

/// A diagonal-hatch fill for the blank counterpart of an inserted or
/// deleted row and for filtered/context headers, per the spec's "hatched
/// shading for filtered/blank alignment areas." It fills its bounds with a
/// background, then paints clipped 45° lines.
struct Hatch {
    id: ElementId,
    background: Hsla,
    line: Hsla,
}

impl IntoElement for Hatch {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Hatch {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        window.paint_quad(fill(bounds, self.background));
        let spacing = px(6.).as_f32();
        let w = bounds.size.width.as_f32();
        let h = bounds.size.height.as_f32();
        let ox = bounds.origin.x;
        let oy = bounds.origin.y;
        window.with_content_mask(
            Some(gpui::ContentMask { bounds }),
            |window| {
                // Start one height to the left so the top-left corner is
                // covered; each line runs bottom-left to top-right.
                let mut x = -h;
                while x < w {
                    let mut path = PathBuilder::stroke(px(1.));
                    path.move_to(point(ox + px(x), oy + px(h)));
                    path.line_to(point(ox + px(x + h), oy));
                    if let Ok(built) = path.build() {
                        window.paint_path(built, self.line);
                    }
                    x += spacing;
                }
            },
        );
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
    /// The editor language names the highlighters were last set to,
    /// `(left, right)`; refreshed when a format switch changes them.
    applied_langs: Option<(SharedString, SharedString)>,
}

impl TextDiffView {
    /// Create a view over an existing session; starts the initial load.
    pub fn new(
        state: Entity<TextDiffSession>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Line stepping and section walking are scoped to the text view's
        // key context so they never shadow the folder view's plain j/k.
        // They are registered centrally by `crate::commands::view_bindings`
        // so a keymap rebuild can restore them.

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
            applied_langs: None,
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

    /// The compared endpoints, for "Compare files using ▸".
    pub fn pair_endpoints(
        &self,
        cx: &App,
    ) -> (cocomo_lib::Endpoint, cocomo_lib::Endpoint) {
        let session = self.state.read(cx);
        (session.left_endpoint(), session.right_endpoint())
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

    /// Handle the StopCompare action dispatched from the Session menu:
    /// abort the in-flight load.
    fn handle_stop_compare(
        &mut self,
        _: &StopCompare,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |s, cx| s.stop(cx));
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

    fn handle_next_difference_files(
        &mut self,
        _: &NextDifferenceFiles,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.step_difference_files(1, cx));
    }

    fn handle_prev_difference_files(
        &mut self,
        _: &PrevDifferenceFiles,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state
            .update(cx, |s, cx| s.step_difference_files(-1, cx));
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
            // Span the line *and* its newline so the fill reaches the line
            // break (a selected newline paints a one-cell tail), matching
            // the full-row fill the list view gives difference rows.
            let end = offset + line.content.len() + 1;
            out.push(
                RangeDecoration::new(offset..end)
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
        // The format switcher acts on the focused side (the file the user
        // is working on); outside edit mode that is the left side.
        let (format_label, formats) = {
            let session = self.state.read(cx);
            let side = session.focus_side();
            let label = session.format_label(side, cx);
            let formats: Vec<(String, bool)> = FileFormatRegistry::all(cx)
                .into_iter()
                .map(|f| (f.name, f.view.is_available()))
                .collect();
            (label, formats)
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
                    // Same contract as the `ReloadCompare` command: with
                    // unsaved buffer edits the re-diff runs on the
                    // buffers so the edits survive.
                    state.update(cx, |s, cx| {
                        if s.is_dirty() {
                            s.recompare(cx);
                        } else {
                            s.load(cx);
                        }
                    });
                })
        };

        let format_btn = {
            let state = self.state.clone();
            let current = format_label.clone();
            Button::new("text2_format")
                .label(format_label)
                .ghost()
                .small()
                .tooltip("Change file format (focused side)")
                .accessibility_label("Change file format")
                .dropdown_menu(move |menu, _, _| {
                    let mut menu = menu;
                    for (name, available) in formats.iter() {
                        let is_current = *name == current;
                        let name = name.clone();
                        let state = state.clone();
                        menu = menu.item(
                            PopupMenuItem::new(name.clone())
                                .checked(is_current)
                                .disabled(!*available)
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |s, cx| {
                                        let side = s.focus_side();
                                        s.set_format(side, &name, cx);
                                    });
                                }),
                        );
                    }
                    menu
                })
        };

        Toolbar::new("text-view-toolbar")
            .child(
                ToolbarGroup::new("text-view-toolbar-format")
                    .label("Format")
                    .content(h_flex().items_center().child(format_btn)),
            )
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
            let (left_lang, right_lang) = {
                let session = self.state.read(cx);
                (
                    session.editor_lang(Side::Left),
                    session.editor_lang(Side::Right),
                )
            };
            let left = cx.new(|cx| {
                EditorState::new(window, cx).language(left_lang.clone())
            });
            let right = cx.new(|cx| {
                EditorState::new(window, cx).language(right_lang.clone())
            });
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
            self.applied_langs = Some((left_lang, right_lang));
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
        // Re-apply the tree-sitter highlighter when a format switch (or a
        // path change) moved a side to a different language.
        if editing {
            let editors = {
                let session = self.state.read(cx);
                (
                    session.left_editor().cloned(),
                    session.right_editor().cloned(),
                    session.editor_lang(Side::Left),
                    session.editor_lang(Side::Right),
                )
            };
            if let (Some(left), Some(right)) = (editors.0, editors.1) {
                let (left_lang, right_lang) = (editors.2, editors.3);
                let stale = self
                    .applied_langs
                    .as_ref()
                    .is_none_or(|(l, r)| *l != left_lang || *r != right_lang);
                if stale {
                    left.update(cx, |ed, cx| {
                        ed.set_highlighter(left_lang.clone(), cx);
                    });
                    right.update(cx, |ed, cx| {
                        ed.set_highlighter(right_lang.clone(), cx);
                    });
                    self.applied_langs = Some((left_lang, right_lang));
                }
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
        let left_format = session.left_format_label(cx);
        let right_format = session.right_format_label(cx);
        let left_enc = session.left_encoding_label();
        let right_enc = session.right_encoding_label();
        let left_end = session.left_ending_label();
        let right_end = session.right_ending_label();
        let left_hist = session.left_history().to_vec();
        let right_hist = session.right_history().to_vec();
        let overrides = crate::color::merge_overrides(&session.overrides, cx);
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
        // The format switcher lists every registered format; only text-view
        // formats are selectable until the other views ship.
        let formats: Vec<(String, bool)> = FileFormatRegistry::all(cx)
            .into_iter()
            .map(|f| (f.name, f.view.is_available()))
            .collect();

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
        let table_even = theme.table_even;
        let border = theme.border;
        let danger = theme.danger;
        // Comment-colored context lines reuse the editor's syntax palette so
        // the list and the edit-mode editors agree on what a comment looks
        // like.
        let syntax_comment = theme
            .highlight_theme
            .style
            .syntax
            .style("comment")
            .and_then(|s| s.color)
            .unwrap_or(muted_fg);

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
                    table_even,
                    fg,
                    syntax_comment,
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
                    table_even,
                    fg,
                    syntax_comment,
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
        // The Text and Alignment detail panes share one pair of editable
        // inputs; Alignment adds a difference caret under each line.
        // Outside edit mode the inputs are disabled, so a commit can never
        // silently no-op against a missing editor.
        let editable_details =
            matches!(detail_mode, DetailMode::Text | DetailMode::Alignment);
        if editable_details && self.detail_inputs.is_none() {
            let l = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Left line — Enter commits")
            });
            let r = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Right line — Enter commits")
            });
            for (side, input) in [(Side::Left, &l), (Side::Right, &r)] {
                let sub =
                    cx.subscribe(input, move |this, input, event, cx| {
                        if matches!(event, InputEvent::PressEnter { .. }) {
                            let value = input.read(cx).value().to_string();
                            let line =
                                this.state.read(cx).selected_line_number(side);
                            if let Some(line) = line {
                                this.pending_detail_commit =
                                    Some((side, line, value));
                                cx.notify();
                            }
                        }
                    });
                self._detail_subs.push(sub);
            }
            self.detail_inputs = Some((l, r));
            self.detail_seeded = None;
        }
        let detail_key = (
            selected_row.and_then(|r| r.left),
            selected_row.and_then(|r| r.right),
        );
        if editable_details {
            let (left_in, right_in) =
                self.detail_inputs.clone().expect("just created");
            if self.detail_seeded != Some(detail_key) {
                self.detail_seeded = Some(detail_key);
                let lt = selected_line(&left_lines, detail_key.0);
                let rt = selected_line(&right_lines, detail_key.1);
                left_in.update(cx, |i, cx| i.set_value(lt, window, cx));
                right_in.update(cx, |i, cx| i.set_value(rt, window, cx));
            }
        }
        let details = match detail_mode {
            DetailMode::Text => {
                let (left_in, right_in) =
                    self.detail_inputs.clone().expect("just created");
                h_flex()
                    .flex_shrink_0()
                    .h(px(64.))
                    .items_center()
                    .border_t_1()
                    .border_color(border)
                    .bg(muted)
                    .child(div().flex_1().min_w_0().px_2().child(
                        Input::new(&left_in).small().disabled(!editing),
                    ))
                    .child(div().w(px(1.)).bg(border))
                    .child(div().flex_1().min_w_0().px_2().child(
                        Input::new(&right_in).small().disabled(!editing),
                    ))
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
                let (left_in, right_in) =
                    self.detail_inputs.clone().expect("just created");
                let lt = selected_line(&left_lines, detail_key.0);
                let rt = selected_line(&right_lines, detail_key.1);
                let caret = first_difference_caret(&lt, &rt);
                h_flex()
                    .flex_shrink_0()
                    .h(px(64.))
                    .items_center()
                    .border_t_1()
                    .border_color(border)
                    .bg(muted)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .gap_0p5()
                            .child(
                                Input::new(&left_in)
                                    .small()
                                    .disabled(!editing),
                            )
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(caret.clone()),
                            ),
                    )
                    .child(div().w(px(1.)).bg(border))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .gap_0p5()
                            .child(
                                Input::new(&right_in)
                                    .small()
                                    .disabled(!editing),
                            )
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(caret),
                            ),
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
                        // `h_flex` centers on the cross axis, so the panes
                        // column needs `h_full` to resolve the lists against
                        // the row's height instead of collapsing to content
                        // height (a 0px list viewport).
                        div()
                            .flex_1()
                            .h_full()
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
                                            &left_format,
                                            &formats,
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
                                            &right_format,
                                            &formats,
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
            .on_action(cx.listener(Self::handle_stop_compare))
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
            .on_action(cx.listener(Self::handle_next_difference_files))
            .on_action(cx.listener(Self::handle_prev_difference_files))
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
/// Point `side` at the first dropped path, when it is a file.
///
/// A drop that cannot be honored explains itself in the log and as a
/// notification, so a rejected drop never looks like a dead target.
fn accept_text_pane_drop(
    side: Side,
    state: &Entity<TextDiffSession>,
    paths: &ExternalPaths,
    cx: &mut App,
) {
    let Some(path) = paths.paths().first() else {
        return;
    };
    match fs_err::metadata(path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            let message = format!("{} is not a file.", path.display());
            log::push(LogLevel::Error, message.clone(), cx);
            crate::menus::notify_error(message, cx);
            return;
        }
        Err(e) => {
            let message = format!("Couldn't read {}: {e}", path.display());
            log::push(LogLevel::Error, message.clone(), cx);
            crate::menus::notify_error(message, cx);
            return;
        }
    }
    state.update(cx, |session, cx| {
        session.set_side_path(side, &path.to_string_lossy(), cx);
    });
}

#[allow(clippy::too_many_arguments)]
fn render_pane(
    side: Side,
    input: &Entity<InputState>,
    meta: &str,
    format: &str,
    formats: &[(String, bool)],
    enc: &'static str,
    ending: &'static str,
    history: &[String],
    body: AnyElement,
    theme: &Theme,
    state: &Entity<TextDiffSession>,
) -> impl IntoElement {
    let side_key = if side == Side::Left { "l" } else { "r" };
    let history = history.to_vec();
    let formats = formats.to_vec();
    let drop_state = state.clone();
    let primary = theme.primary;
    v_flex()
        .id(ElementId::Name(format!("text-pane-{side_key}").into()))
        .size_full()
        .min_h_0()
        // Dropping a file onto a pane repoints that side, matching the
        // folder view's drop targets.
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.border_color(primary)
        })
        .on_drop::<ExternalPaths>(move |paths, _, cx| {
            accept_text_pane_drop(side, &drop_state, paths, cx);
        })
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
                                format!("text_format_{side_key}").into(),
                            ))
                            .label(format.to_string())
                            .ghost()
                            .xsmall()
                            .tooltip("File format")
                            .accessibility_label("File format")
                            .dropdown_menu({
                                let state = state.clone();
                                let current = format.to_string();
                                move |menu, _, _| {
                                    let mut menu = menu;
                                    for (name, available) in formats.iter() {
                                        let is_current = *name == current;
                                        let name = name.clone();
                                        let state = state.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(name.clone())
                                                .checked(is_current)
                                                // Only the text view can
                                                // open a file today, so a
                                                // table/hex/picture format
                                                // stays visible but inert.
                                                .disabled(!*available)
                                                .on_click(move |_, _, cx| {
                                                    state.update(
                                                        cx,
                                                        |s, cx| {
                                                            s.set_format(
                                                                side, &name,
                                                                cx,
                                                            );
                                                        },
                                                    );
                                                }),
                                        );
                                    }
                                    menu
                                }
                            }),
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
    table_even: Hsla,
    foreground: Hsla,
    syntax_comment: Hsla,
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
                .relative()
                .w_full()
                .flex_shrink_0()
                .overflow_hidden()
                .hover(|el| el.bg(list_hover))
                .child(div().absolute().inset_0().child(Hatch {
                    id: ElementId::Name(
                        format!("text_filtered_hatch-{side_key}-{d}").into(),
                    ),
                    background: muted,
                    line: muted_fg.opacity(0.25),
                }))
                .child(
                    h_flex()
                        .w_full()
                        .justify_center()
                        .py_0p5()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(format!("[+] {count} FILTERED LINES")),
                )
                .on_click(move |_, _, cx| {
                    state.update(cx, |s, cx| s.toggle_section(section, cx));
                })
                .into_any_element()
        }
        DisplayRow::Context { count } => div()
            .id((ElementId::from("text_context"), format!("{side_key}-{d}")))
            .relative()
            .w_full()
            .flex_shrink_0()
            .overflow_hidden()
            .child(div().absolute().inset_0().child(Hatch {
                id: ElementId::Name(
                    format!("text_context_hatch-{side_key}-{d}").into(),
                ),
                background: muted,
                line: muted_fg.opacity(0.25),
            }))
            .child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .py_0p5()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(format!("{count} context lines hidden")),
            )
            .into_any_element(),
        DisplayRow::Line(r) => {
            let Some(row) = rows.get(*r) else {
                return div().size_full().into_any_element();
            };
            let number = match side {
                Side::Left => row.left,
                Side::Right => row.right,
            };
            // The blank counterpart of an inserted/deleted row is hatched
            // so the row alignment stays visible in both panes (spec
            // section 5.2).
            let Some(number) = number else {
                return div()
                    .id((
                        ElementId::from("text_row"),
                        format!("{side_key}-{d}"),
                    ))
                    .w_full()
                    .h(row_h)
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(Hatch {
                        id: ElementId::Name(
                            format!("text_blank_hatch-{side_key}-{d}").into(),
                        ),
                        background: muted,
                        line: muted_fg.opacity(0.3),
                    })
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
                // Context lines take their color from the grammar's line
                // classification: comments read as comments, ignored text
                // as muted, everything else as the default foreground.
                match lines.get(number - 1).and_then(|l| l.tokens.first()) {
                    Some(t) if t.importance == Importance::Comment => {
                        syntax_comment
                    }
                    Some(t) if t.importance == Importance::Ignored => muted_fg,
                    _ => foreground,
                }
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
                // Alternating row backgrounds (spec section 7.4-7);
                // difference tints, selection, and hover override.
                .when(
                    d % 2 == 1 && diff_bg.is_none() && selected != Some(d),
                    |el| el.bg(table_even),
                )
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
                .context_menu(move |menu, _, cx| {
                    let mut menu = menu;
                    match menu_flags.anchor {
                        Some((anchor_side, _)) => {
                            if anchor_side != side
                                && crate::commands::is_visible(
                                    "edit.align_with",
                                    cx,
                                )
                            {
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
                    if menu_flags.has_overrides
                        && crate::commands::is_visible(
                            "edit.clear_alignments",
                            cx,
                        )
                    {
                        menu = menu.menu(
                            "Clear alignment overrides",
                            Box::new(ClearAlignments),
                        );
                    }
                    menu = menu.separator();
                    if crate::commands::is_visible(
                        "actions.copy_section_right",
                        cx,
                    ) {
                        menu = menu.menu(
                            "Copy section → right",
                            Box::new(CopySectionRight),
                        );
                    }
                    if crate::commands::is_visible(
                        "actions.copy_section_left",
                        cx,
                    ) {
                        menu = menu.menu(
                            "Copy section ← left",
                            Box::new(CopySectionLeft),
                        );
                    }
                    if crate::commands::is_visible("edit.isolate_section", cx)
                    {
                        menu = menu.menu(
                            "Isolate section",
                            Box::new(IsolateSelection),
                        );
                    }
                    if menu_flags.can_copy_file_next {
                        if crate::commands::is_visible(
                            "actions.copy_file_right_next",
                            cx,
                        ) {
                            menu = menu.menu(
                                "Copy file to right & open next",
                                Box::new(CopyFileRightNext),
                            );
                        }
                        if crate::commands::is_visible(
                            "actions.copy_file_left_next",
                            cx,
                        ) {
                            menu = menu.menu(
                                "Copy file to left & open next",
                                Box::new(CopyFileLeftNext),
                            );
                        }
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
    // The grammar (when the caller supplies one) drives classification and
    // editor highlighting; otherwise the language is detected from the
    // endpoints' extensions.
    let title = left.pair_title(&right);
    let session = cx.new(|cx| {
        TextDiffSession::new(left, right, title.into(), parent, manager, cx)
    });
    session.update(cx, |s, _| s.init_grammar(grammar));
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

// ---------------------------------------------------------------------------
// Session rules dialog
// ---------------------------------------------------------------------------

/// Body of the text session's rules dialog: compare rules, per-session
/// color overrides, and the auto-substitution (replacement) list. A
/// `Render` entity so the controls repaint as they are edited (dialog
/// content is otherwise built only once when the dialog opens).
struct TextRulesBody {
    session: Entity<TextDiffSession>,
    ignore_case: bool,
    ignore_whitespace: bool,
    ignore_blank_lines: bool,
    ignore_comments: bool,
    ignore_numeric: bool,
    colors: ColorFields,
    /// `(find, replace)` input pairs for the replacement table.
    replacement_rows: Vec<(Entity<InputState>, Entity<InputState>)>,
    /// A validation problem to show above the Apply button, if any.
    error: Option<String>,
}

impl TextRulesBody {
    fn new(
        session: Entity<TextDiffSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (settings, overrides, replacements) = {
            let session = session.read(cx);
            (
                session.compare_settings().clone(),
                session.overrides().clone(),
                session.replacements().to_vec(),
            )
        };
        let replacement_rows = replacements
            .into_iter()
            .map(|(from, to)| {
                let from_in = cx.new(|cx| {
                    let mut s =
                        InputState::new(window, cx).placeholder("Find");
                    s.set_value(from, window, cx);
                    s
                });
                let to_in = cx.new(|cx| {
                    let mut s =
                        InputState::new(window, cx).placeholder("Replace");
                    s.set_value(to, window, cx);
                    s
                });
                (from_in, to_in)
            })
            .collect();
        Self {
            ignore_case: settings.ignore_case,
            ignore_whitespace: !matches!(
                settings.whitespace_mode,
                WhitespaceMode::Sensitive
            ),
            ignore_blank_lines: settings.ignore_blank_lines,
            ignore_comments: settings.ignore_comments,
            ignore_numeric: settings.ignore_numeric_changes,
            colors: ColorFields::new(&overrides, window, cx),
            replacement_rows,
            error: None,
            session,
        }
    }

    /// Push the drafted rules, colors, and replacements into the session.
    ///
    /// An invalid color aborts the apply and leaves the dialog open with a
    /// message, so a typo never silently drops a preference.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let colors = match self.colors.collect(cx) {
            Ok(colors) => colors,
            Err(e) => {
                self.error = Some(e);
                cx.notify();
                return;
            }
        };
        let mut replacements = Vec::new();
        for (from_in, to_in) in &self.replacement_rows {
            let from = from_in.read(cx).value().to_string();
            if from.is_empty() {
                continue;
            }
            let to = to_in.read(cx).value().to_string();
            replacements.push((from, to));
        }
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
        self.error = None;
        self.session.update(cx, |s, cx| {
            s.set_compare_settings(settings, cx);
            s.set_overrides(colors, cx);
            s.set_replacements(replacements, cx);
        });
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

    /// Append an empty find/replace row.
    fn add_replacement(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from_in =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let to_in =
            cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));
        self.replacement_rows.push((from_in, to_in));
        cx.notify();
    }
}

impl Render for TextRulesBody {
    fn render(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let heading = |label: &'static str| {
            div()
                .text_xs()
                .font_weight(gpui::FontWeight::BOLD)
                .text_color(theme.muted_foreground)
                .child(label)
        };
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
        let replacement_rows = v_flex().gap_1().children(
            self.replacement_rows.iter().enumerate().map(
                |(ix, (from_in, to_in))| {
                    h_flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(from_in).small()),
                        )
                        .child(div().text_xs().child("→"))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(Input::new(to_in).small()),
                        )
                        .child(
                            Button::new(ElementId::Name(
                                format!("rep_rm_{ix}").into(),
                            ))
                            .icon(IconName::Close)
                            .ghost()
                            .xsmall()
                            .tooltip("Remove substitution")
                            .accessibility_label("Remove substitution")
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    if ix < this.replacement_rows.len() {
                                        this.replacement_rows.remove(ix);
                                        cx.notify();
                                    }
                                }),
                            ),
                        )
                },
            ),
        );
        v_flex()
            .w_full()
            .gap_3()
            .child(heading("Compare rules"))
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
            .child(div().w_full().h_px().bg(theme.border))
            .child(heading("Colors"))
            .child(self.colors.render())
            .child(div().w_full().h_px().bg(theme.border))
            .child(heading("Replacements"))
            .child(div().text_xs().text_color(theme.muted_foreground).child(
                "Auto-substitutions applied to both sides during comparison; \
                 a substituted-equal pair aligns as identical.",
            ))
            .child(replacement_rows)
            .child(
                Button::new("rep_add")
                    .label("Add substitution")
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_replacement(window, cx);
                    })),
            )
            .when_some(self.error.clone(), |this, err| {
                this.child(div().text_sm().text_color(theme.danger).child(err))
            })
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
    let body = cx.new(|cx| TextRulesBody::new(session, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.title("Session rules").w(px(480.)).child(
            div()
                .id(ElementId::Name("text_rules_scroll".into()))
                .max_h(px(520.))
                .overflow_y_scroll()
                .child(body.clone()),
        )
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use cocomo_lib::{TextDifference, Token};
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
            ..Default::default()
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
            display_rows(&rows, &sections, &HashSet::new(), None);
        assert_eq!(display.len(), 4);
        // Expanded sections have no display index of their own.
        assert_eq!(section_display[0], usize::MAX);
        let collapsed = HashSet::from([0usize]);
        let (display, line_display_c, section_display) =
            display_rows(&rows, &sections, &collapsed, None);
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
    fn display_rows_folds_context_runs_to_the_depth() {
        // Four context rows, one difference, four context rows.
        let mut rows = vec![row(Some(1), Some(1), RowStatus::Same, false); 4];
        rows.push(row(Some(5), Some(5), RowStatus::Changed, false));
        rows.extend(vec![row(Some(6), Some(6), RowStatus::Same, false); 4]);
        let sections = diff_sections(&rows);

        // Full context: every row is displayed.
        let (display, ..) =
            display_rows(&rows, &sections, &HashSet::new(), None);
        assert_eq!(display.len(), 9);
        assert!(
            !display
                .iter()
                .any(|d| matches!(d, DisplayRow::Context { .. }))
        );

        // One context line per edge: each 4-row run folds to two rows
        // plus a header for the hidden middle.
        let (display, line_display, _) =
            display_rows(&rows, &sections, &HashSet::new(), Some(1));
        assert_eq!(
            display,
            vec![
                DisplayRow::Line(0),
                DisplayRow::Context { count: 2 },
                DisplayRow::Line(3),
                DisplayRow::Line(4),
                DisplayRow::Line(5),
                DisplayRow::Context { count: 2 },
                DisplayRow::Line(8),
            ]
        );
        assert_eq!(line_display[2], usize::MAX);

        // Differences only: whole context runs become headers.
        let (display, ..) =
            display_rows(&rows, &sections, &HashSet::new(), Some(0));
        assert_eq!(
            display,
            vec![
                DisplayRow::Context { count: 4 },
                DisplayRow::Line(4),
                DisplayRow::Context { count: 4 },
            ]
        );

        // A depth that already covers the run keeps everything.
        let (display, ..) =
            display_rows(&rows, &sections, &HashSet::new(), Some(2));
        assert_eq!(display.len(), 9);
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
    fn context_depth_folds_context_runs(cx: &mut TestAppContext) {
        let session = cx.new(|cx| {
            TextDiffSession::for_test(
                "a\nb\nc\nd\ne\nf\n",
                "a\nb\nc\nX\ne\nf\n",
                cx,
            )
        });

        cx.update(|cx| {
            session.update(cx, |s, cx| {
                // Full context by default: all six aligned rows.
                assert_eq!(s.context_depth(), None);
                assert_eq!(s.display.len(), 6);

                // Differences only: both context runs become headers.
                s.set_context_depth(Some(0), cx);
                assert_eq!(s.context_depth(), Some(0));
                assert_eq!(
                    s.display.to_vec(),
                    vec![
                        DisplayRow::Context { count: 3 },
                        DisplayRow::Line(3),
                        DisplayRow::Context { count: 2 },
                    ]
                );

                // Restoring the full context rebuilds the row list.
                s.set_context_depth(None, cx);
                assert_eq!(s.display.len(), 6);
            });
        });
    }

    #[gpui::test]
    fn stop_aborts_an_in_flight_load(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(crate::log::LogBuffer::default()));
        let session = cx.new(|cx| TextDiffSession::for_test("a\n", "b\n", cx));
        session.update(cx, |s, cx| {
            // Simulate an in-flight load without spawning the real task.
            s.loading = true;
            s.load_revision = 7;
            let token = CancellationToken::new();
            s.cancel_token = Some(token.clone());

            s.stop(cx);

            assert!(!s.is_loading());
            // The bumped revision drops the in-flight result on arrival.
            assert_eq!(s.load_revision, 8);
            assert!(s.cancel_token.is_none());
            assert!(token.is_cancelled());
        });
        // Stopping an idle session is a no-op.
        session.update(cx, |s, cx| {
            s.stop(cx);
            assert_eq!(s.load_revision, 8);
        });
    }

    #[gpui::test]
    fn clear_session_drops_content_and_keeps_paths(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(crate::log::LogBuffer::default()));
        let session =
            cx.new(|cx| TextDiffSession::for_test("a\nb\n", "a\nX\n", cx));
        session.update(cx, |s, cx| {
            assert!(s.can_save());
            assert!(!s.display.is_empty());

            s.clear_session(cx);

            assert!(!s.can_save());
            assert!(s.left_raw.is_none());
            assert!(s.right_raw.is_none());
            assert!(s.diff.is_none());
            assert!(s.display.is_empty());
            assert!(s.selected.is_none());
            assert!(!s.is_editing());
            assert!(s.editors_stale());
            // The endpoints survive the clear, so Reload can refetch.
            assert_eq!(s.left_path(), "/left.txt");
            assert_eq!(s.right_path(), "/right.txt");
        });
    }

    #[gpui::test]
    fn clear_session_aborts_an_in_flight_load(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_global(crate::log::LogBuffer::default()));
        let session = cx.new(|cx| TextDiffSession::for_test("a\n", "b\n", cx));
        session.update(cx, |s, cx| {
            // Simulate an in-flight load without spawning the real task.
            s.loading = true;
            s.load_revision = 3;
            let token = CancellationToken::new();
            s.cancel_token = Some(token.clone());

            s.clear_session(cx);

            assert!(!s.is_loading());
            // The bumped revision drops the in-flight result on arrival.
            assert_eq!(s.load_revision, 4);
            assert!(s.cancel_token.is_none());
            assert!(token.is_cancelled());
        });
    }

    #[gpui::test]
    fn side_text_returns_the_loaded_text(cx: &mut TestAppContext) {
        let session =
            cx.new(|cx| TextDiffSession::for_test("a\nb\n", "a\nX\n", cx));
        cx.update(|cx| {
            assert_eq!(session.read(cx).side_text(Side::Left, cx), "a\nb\n");
            assert_eq!(session.read(cx).side_text(Side::Right, cx), "a\nX\n");
        });
        // After a clear there is no text to compare against.
        session.update(cx, |s, cx| s.clear_session(cx));
        cx.update(|cx| {
            assert_eq!(session.read(cx).side_text(Side::Left, cx), "");
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

        cx.update(|cx| {
            gpui::init(cx);
            // The view reads the options global when it renders.
            cx.set_global(crate::options::AppOptions::default());
        });

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
                DisplayRow::Filtered { .. } | DisplayRow::Context { .. } => {
                    panic!("expected a line row")
                }
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

    /// Regression: the diff lists must resolve against the row's height.
    /// `h_flex` centers its children on the cross axis, so without
    /// `h_full` on the panes wrapper the resizable group collapses to a
    /// 0px-tall list viewport and both panes render blank even though the
    /// model has rows.
    #[gpui::test]
    fn diff_list_viewport_gets_a_nonzero_height(cx: &mut TestAppContext) {
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        cx.update(|cx| {
            gpui::init(cx);
            // The view reads the options global when it renders.
            cx.set_global(crate::options::AppOptions::default());
        });

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

        // Two frames: the resizable group settles its panel sizes on the
        // frame after its first prepaint.
        for _ in 0..2 {
            cx.update_window(window, |_, window, cx| {
                window.render_frame(cx);
            })
            .expect("failed to render the test window");
        }

        cx.update(|cx| {
            let view = view.read(cx);
            for (side, list) in
                [("left", &view.left_list), ("right", &view.right_list)]
            {
                let height = list.viewport_bounds().size.height;
                assert!(
                    height.as_f32() > 0.,
                    "the {side} diff list viewport collapsed to {height:?}"
                );
            }
        });
    }

    /// The Edit menu's alignment commands (promoted from the row context
    /// menu in phase 2) must reach the focused text view's handlers when
    /// dispatched as menu items do.
    #[gpui::test]
    fn edit_menu_alignment_actions_dispatch_through_the_view(
        cx: &mut TestAppContext,
    ) {
        use gpui::{Bounds, WindowBounds, WindowOptions, test::TestWindowExt};

        use crate::{color::Side, ui::NextDifference};

        cx.update(|cx| {
            gpui::init(cx);
            cx.set_global(crate::options::AppOptions::default());
            cx.set_global(crate::log::LogBuffer::default());
        });

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

        // Walk to the only difference, mark its left line, then align it
        // with the row's right line — the menu's two-step flow.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            let focus = view.read(cx).focus_handle(cx);
            focus.focus(window, cx);
            window.dispatch_action(Box::new(NextDifference), cx);
            window.dispatch_action(
                Box::new(SetAlignAnchor { side: Side::Left }),
                cx,
            );
            window.dispatch_action(Box::new(AlignWithAnchor), cx);
        })
        .expect("failed to dispatch the alignment actions");

        cx.update(|cx| {
            assert!(session.read(cx).has_align_overrides());
        });

        // Clear alignment overrides drops the pair again.
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(Box::new(ClearAlignments), cx);
        })
        .expect("failed to dispatch the clear-alignments action");

        cx.update(|cx| {
            let s = session.read(cx);
            assert!(!s.has_align_overrides());
            assert!(s.align_anchor().is_none());
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

        cx.update(|cx| {
            gpui::init(cx);
            // The view reads the options global when it renders.
            cx.set_global(crate::options::AppOptions::default());
        });
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

        cx.update(|cx| {
            gpui::init(cx);
            // The view reads the options global when it renders.
            cx.set_global(crate::options::AppOptions::default());
        });
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

    #[gpui::test]
    fn rust_grammar_marks_comment_differences_minor(cx: &mut App) {
        let session =
            cx.new(|cx| TextDiffSession::for_test("// a\n", "// b\n", cx));
        // With no grammar the comment lines are unclassified, so the
        // difference stays important.
        session.update(cx, |s, _| {
            let changed = s
                .rows
                .iter()
                .find(|r| r.status == RowStatus::Changed)
                .expect("one changed row");
            assert!(!changed.minor);
        });
        // A Rust grammar classifies both lines as comments, making the
        // change minor (spec section 5.2).
        session.update(cx, |s, cx| {
            s.compare_settings.grammar = Some(Grammar::rust());
            s.recompare(cx);
            let changed = s
                .rows
                .iter()
                .find(|r| r.status == RowStatus::Changed)
                .expect("one changed row");
            assert!(changed.minor);
        });
    }

    #[gpui::test]
    fn replacements_align_substituted_pairs(cx: &mut App) {
        let session =
            cx.new(|cx| TextDiffSession::for_test("foo\n", "bar\n", cx));
        session.update(cx, |s, _| {
            assert_eq!(s.info_stats().changes, 1);
        });
        // Substituting "foo"→"bar" on both sides makes the pair compare
        // equal, so it no longer counts as a difference.
        session.update(cx, |s, cx| {
            s.set_replacements(vec![("foo".into(), "bar".into())], cx);
            assert_eq!(s.info_stats().changes, 0);
            assert_eq!(s.replacements().len(), 1);
        });
    }

    #[gpui::test]
    fn format_override_drives_editor_language(cx: &mut App) {
        let session = cx.new(|cx| {
            TextDiffSession::for_test("fn main() {}\n", "fn main() { }\n", cx)
        });
        // A `.txt` endpoint has no language until the format forces one.
        session.update(cx, |s, _| {
            assert_eq!(s.editor_lang(Side::Left).as_ref(), "text");
        });
        session.update(cx, |s, cx| {
            s.set_format(Side::Left, "Rust", cx);
            assert_eq!(s.editor_lang(Side::Left).as_ref(), "rust");
            assert_eq!(s.left_format_label(cx), "Rust");
        });
    }

    #[gpui::test]
    fn word_wrap_is_per_session(cx: &mut App) {
        let left = cx.new(|cx| TextDiffSession::for_test("a\n", "b\n", cx));
        let right = cx.new(|cx| TextDiffSession::for_test("a\n", "b\n", cx));
        left.update(cx, |s, cx| s.toggle_wrap(cx));
        assert!(left.read(cx).is_word_wrap());
        assert!(!right.read(cx).is_word_wrap());
    }

    #[gpui::test]
    fn step_difference_files_without_parent_is_safe(cx: &mut App) {
        use gpui::AppContext as _;
        cx.set_global(crate::log::LogBuffer::default());
        let session = cx.new(|cx| TextDiffSession::for_test("a\n", "b\n", cx));
        // No parent or manager: the command logs a warning and returns
        // without panicking or changing the session.
        session.update(cx, |s, cx| s.step_difference_files(1, cx));
        session.update(cx, |s, cx| s.step_difference_files(-1, cx));
        assert_eq!(session.read(cx).info_stats().changes, 1);
    }
}
