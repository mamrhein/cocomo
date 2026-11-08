// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Line-based text comparison engine.
//!
//! Compares two text contents and produces a sequence of [`TextDifference`]
//! items that describe added, removed, and changed lines. Supports
//! syntax-aware comparison via [`Grammar`][crate::grammar::Grammar] and
//! configurable normalization through [`TextCompareSettings`].
//!
//! [`TextDiff::merged_rows`] additionally derives the aligned side-by-side
//! row model used by diff viewers: one [`MergedDiffRow`] per display row,
//! with a blank (`None`) cell wherever the other side carries the
//! difference.

use std::{collections::HashSet, fmt};

use crate::grammar::{Grammar, Importance};

/// Granularity at which text comparison operates.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlignmentMode {
    /// Compare at line granularity (default).
    #[default]
    Line,
    /// Compare at word granularity within lines.
    Word,
    /// Compare at character granularity.
    Character,
}

/// How whitespace differences should be handled during comparison.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WhitespaceMode {
    /// Treat all whitespace as significant.
    #[default]
    Sensitive,
    /// Trim leading and trailing whitespace before comparison.
    Trim,
    /// Ignore all whitespace differences.
    Insensitive,
}

/// Configuration for text comparison.
#[derive(Clone, Debug, Default)]
pub struct TextCompareSettings {
    /// Whether to perform a case-insensitive comparison.
    pub ignore_case: bool,
    /// How to handle whitespace differences.
    pub whitespace_mode: WhitespaceMode,
    /// Whether to skip blank lines during comparison.
    pub ignore_blank_lines: bool,
    /// Whether to skip comment lines. Requires a grammar to be set.
    pub ignore_comments: bool,
    /// Whether to treat numeric differences as equal. Compares numeric
    /// values within a tolerance when both sides contain a number.
    pub ignore_numeric_changes: bool,
    /// Regex patterns whose matches should be ignored during comparison.
    /// Each pattern is applied before comparing a line.
    pub ignore_regexes: Vec<regex::Regex>,
    /// Granularity of comparison.
    pub alignment_mode: AlignmentMode,
    /// Optional grammar for syntax-aware classification and normalization.
    pub grammar: Option<Grammar>,
}

impl TextCompareSettings {
    /// Create settings with all defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Normalize a line for comparison based on these settings.
    /// Returns the transformed text.
    pub(crate) fn normalize_line(&self, line: &str) -> String {
        let mut result = line.to_string();

        // Apply grammar normalization when available.
        if let Some(g) = &self.grammar {
            result = g.normalize_line(&result);
        }

        // Whitespace handling.
        match self.whitespace_mode {
            WhitespaceMode::Sensitive => {}
            WhitespaceMode::Trim => {
                result = result.trim().to_string();
            }
            WhitespaceMode::Insensitive => {
                result = result.split_whitespace().collect();
            }
        }

        // Case normalization.
        if self.ignore_case {
            result = result.to_lowercase();
        }

        // Apply ignore regexes.
        for re in &self.ignore_regexes {
            result = re.replace_all(&result, "").to_string();
        }

        result
    }

    /// Check whether a line should be skipped entirely (blank or comment).
    pub(crate) fn should_skip(&self, line: &str) -> bool {
        if self.ignore_blank_lines && line.trim().is_empty() {
            return true;
        }
        if self.ignore_comments
            && let Some(g) = &self.grammar
            && g.classify_line(line) == Importance::Comment
        {
            return true;
        }
        false
    }
}

/// Granularity at which to display differences within a changed line.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiffGranularity {
    /// The entire line is different.
    #[default]
    Line,
    /// Specific words within the line differ.
    Word,
    /// Specific character ranges within the line differ.
    Character,
}

/// Information about a single line in a text file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LineInfo {
    /// 1-based line number in the original file.
    pub number: usize,
    /// Raw line content (without line ending).
    pub content: String,
    /// Tokenized segments produced by a grammar. Empty when no grammar is
    /// applied.
    pub tokens: Vec<Token>,
}

/// A classified segment of text within a [`LineInfo`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Token {
    /// The text content of this token.
    pub text: String,
    /// Importance level assigned by the grammar.
    pub importance: Importance,
    /// Byte offset within the line where this token starts.
    pub offset: usize,
}

impl LineInfo {
    /// Create a new `LineInfo` with no tokens.
    pub fn new(number: usize, content: String) -> Self {
        Self {
            number,
            content,
            tokens: Vec::new(),
        }
    }

    /// Tokenize this line using the given grammar.
    pub fn with_grammar(mut self, grammar: &Grammar) -> Self {
        let importance = grammar.classify_line(&self.content);
        self.tokens.push(Token {
            text: self.content.clone(),
            importance,
            offset: 0,
        });
        self
    }
}

/// A difference found between two texts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TextDifference {
    /// A single line exists in both texts but has different content.
    LineDifferent(LineInfo, LineInfo),
    /// Lines exist only in the right (new) text. The usize is the
    /// 1-based starting line number in the right text.
    LinesAdded(usize, Vec<LineInfo>),
    /// Lines exist only in the left (old) text. The usize is the
    /// 1-based starting line number in the left text.
    LinesRemoved(usize, Vec<LineInfo>),
    /// A block of lines was changed. Contains the starting line number in
    /// the left text, the removed lines, and the added lines.
    LinesChanged(usize, Vec<LineInfo>, Vec<LineInfo>),
    /// A blank line exists in both texts but was classified differently
    /// (e.g., different importance). Rare; kept for completeness.
    BlankLine,
}

impl fmt::Display for TextDifference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TextDifference::LineDifferent(left, right) => {
                write!(
                    f,
                    "line {}: {} -> {}",
                    left.number, left.content, right.content
                )
            }
            TextDifference::LinesAdded(start, lines) => {
                write!(f, "added {} lines starting at {}", lines.len(), start)
            }
            TextDifference::LinesRemoved(start, lines) => {
                write!(
                    f,
                    "removed {} lines starting at {}",
                    lines.len(),
                    start
                )
            }
            TextDifference::LinesChanged(start, removed, added) => {
                write!(
                    f,
                    "changed {} -> {} lines starting at {}",
                    removed.len(),
                    added.len(),
                    start
                )
            }
            TextDifference::BlankLine => {
                write!(f, "blank line difference")
            }
        }
    }
}

/// Alignment status of one row in a merged side-by-side view.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MergedRowStatus {
    /// Both sides show the same line.
    #[default]
    Same,
    /// Both sides show a line and both participate in the same
    /// difference.
    Changed,
    /// Only the right side carries the line.
    Inserted,
    /// Only the left side carries the line.
    Deleted,
}

impl MergedRowStatus {
    /// Whether the row participates in a difference.
    pub fn is_difference(self) -> bool {
        self != Self::Same
    }
}

/// One row of the aligned side-by-side view derived from a [`TextDiff`].
///
/// The cell of the side that lacks a line at this position is `None`; the
/// viewer renders it as a blank filler cell.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MergedDiffRow {
    /// Left (old) line, `None` for an inserted row.
    pub left: Option<LineInfo>,
    /// Right (new) line, `None` for a deleted row.
    pub right: Option<LineInfo>,
    /// Alignment status of the row.
    pub status: MergedRowStatus,
}

/// Result of a text comparison containing the sequence of differences.
#[derive(Clone, Debug, Default)]
pub struct TextDiff {
    /// All differences found between the two texts.
    pub differences: Vec<TextDifference>,
    /// Total number of lines in the left (old) text.
    pub left_line_count: usize,
    /// Total number of lines in the right (new) text.
    pub right_line_count: usize,
    /// Highest importance level among all differences.
    pub max_importance: Importance,
    /// Grammar-classified left lines that participated in the comparison,
    /// in file order, with their original 1-based line numbers. Skipped
    /// lines (blank or comment, per the settings) are absent.
    pub left_filtered: Vec<LineInfo>,
    /// Right counterpart of [`Self::left_filtered`].
    pub right_filtered: Vec<LineInfo>,
}

impl TextDiff {
    /// Return `true` when the texts are identical (no differences).
    pub fn is_empty(&self) -> bool {
        self.differences.is_empty()
    }

    /// Return the set of 1-based line numbers that participate in a
    /// difference, from either side.
    pub fn difference_line_numbers(&self) -> HashSet<usize> {
        let (mut numbers, right_numbers) =
            self.difference_line_numbers_by_side();
        numbers.extend(right_numbers);
        numbers
    }

    /// Return the 1-based line numbers that participate in a difference,
    /// split by side: the left (old) text's numbers first, the right (new)
    /// text's numbers second.
    ///
    /// Unlike [`Self::difference_line_numbers`], this keeps track of *which*
    /// text a differing line belongs to, which is what a side-by-side view
    /// needs to tell a left-only line from a right-only or changed one.
    pub fn difference_line_numbers_by_side(
        &self,
    ) -> (HashSet<usize>, HashSet<usize>) {
        let mut left_numbers = HashSet::new();
        let mut right_numbers = HashSet::new();
        for diff in &self.differences {
            match diff {
                TextDifference::LineDifferent(left, right) => {
                    left_numbers.insert(left.number);
                    right_numbers.insert(right.number);
                }
                TextDifference::LinesAdded(_, lines) => {
                    right_numbers.extend(lines.iter().map(|l| l.number));
                }
                TextDifference::LinesRemoved(_, lines) => {
                    left_numbers.extend(lines.iter().map(|l| l.number));
                }
                TextDifference::LinesChanged(_, removed, added) => {
                    left_numbers.extend(removed.iter().map(|l| l.number));
                    right_numbers.extend(added.iter().map(|l| l.number));
                }
                TextDifference::BlankLine => {}
            }
        }
        (left_numbers, right_numbers)
    }

    /// Return the aligned side-by-side rows for a viewer: one
    /// [`MergedDiffRow`] per display row, with blank (`None`) counterpart
    /// cells for inserted and deleted lines.
    ///
    /// The rows are derived from [`Self::differences`] and the comparison's
    /// line lists ([`Self::left_filtered`] / [`Self::right_filtered`], or
    /// just the line counts for a hand-built diff), so a viewer never has
    /// to re-derive the alignment itself.
    ///
    /// `LinesChanged` blocks pair their removed and added lines
    /// positionally; the remainder of the longer half is emitted as
    /// `Deleted`/`Inserted` filler rows. The rare
    /// [`TextDifference::BlankLine`] variant carries no line numbers and is
    /// skipped.
    pub fn merged_rows(&self) -> Vec<MergedDiffRow> {
        let mut rows = Vec::new();
        // Cursors are 0-based line numbers in the *raw* files; the filtered
        // indices advance only over lines that took part in the comparison.
        let (mut l, mut r) = (1usize, 1usize);
        let (mut li, mut ri) = (0usize, 0usize);
        for diff in &self.differences {
            match diff {
                TextDifference::LineDifferent(left, right) => {
                    Self::push_context(
                        &mut rows,
                        &mut l,
                        &mut r,
                        &mut li,
                        &mut ri,
                        left.number,
                        right.number,
                        &self.left_filtered,
                        &self.right_filtered,
                    );
                    rows.push(MergedDiffRow {
                        left: Some(left.clone()),
                        right: Some(right.clone()),
                        status: MergedRowStatus::Changed,
                    });
                    l += 1;
                    r += 1;
                    li += 1;
                    ri += 1;
                }
                TextDifference::LinesAdded(start, lines) => {
                    Self::push_context(
                        &mut rows,
                        &mut l,
                        &mut r,
                        &mut li,
                        &mut ri,
                        *start,
                        *start,
                        &self.left_filtered,
                        &self.right_filtered,
                    );
                    for line in lines {
                        rows.push(MergedDiffRow {
                            left: None,
                            right: Some(line.clone()),
                            status: MergedRowStatus::Inserted,
                        });
                        r += 1;
                        ri += 1;
                    }
                }
                TextDifference::LinesRemoved(start, lines) => {
                    Self::push_context(
                        &mut rows,
                        &mut l,
                        &mut r,
                        &mut li,
                        &mut ri,
                        *start,
                        *start,
                        &self.left_filtered,
                        &self.right_filtered,
                    );
                    for line in lines {
                        rows.push(MergedDiffRow {
                            left: Some(line.clone()),
                            right: None,
                            status: MergedRowStatus::Deleted,
                        });
                        l += 1;
                        li += 1;
                    }
                }
                TextDifference::LinesChanged(start, removed, added) => {
                    // Keep the right cursor's filtered partner aligned
                    // with the removed block's start.
                    let right_target = *start + r.saturating_sub(l);
                    Self::push_context(
                        &mut rows,
                        &mut l,
                        &mut r,
                        &mut li,
                        &mut ri,
                        *start,
                        right_target,
                        &self.left_filtered,
                        &self.right_filtered,
                    );
                    let paired = removed.len().min(added.len());
                    for i in 0..paired {
                        rows.push(MergedDiffRow {
                            left: Some(removed[i].clone()),
                            right: Some(added[i].clone()),
                            status: MergedRowStatus::Changed,
                        });
                    }
                    for line in &removed[paired..] {
                        rows.push(MergedDiffRow {
                            left: Some(line.clone()),
                            right: None,
                            status: MergedRowStatus::Deleted,
                        });
                    }
                    for line in &added[paired..] {
                        rows.push(MergedDiffRow {
                            left: None,
                            right: Some(line.clone()),
                            status: MergedRowStatus::Inserted,
                        });
                    }
                    l += removed.len();
                    r += added.len();
                    li += removed.len();
                    ri += added.len();
                }
                // A blank-line classification difference carries no line
                // numbers to align on; ignore it.
                TextDifference::BlankLine => {}
            }
        }
        Self::push_context(
            &mut rows,
            &mut l,
            &mut r,
            &mut li,
            &mut ri,
            self.left_line_count + 1,
            self.right_line_count + 1,
            &self.left_filtered,
            &self.right_filtered,
        );
        rows
    }

    /// Emit aligned context rows until both raw line cursors reach their
    /// (exclusive) targets. Pairwise rows are `Same`; when one side still
    /// has lines before its target, the leftovers are emitted as
    /// `Deleted`/`Inserted` filler rows.
    ///
    /// A side's current line takes its content from the filtered list when
    /// it took part in the comparison there (advancing the filtered
    /// index), and is rebuilt as a plain unclassified line otherwise
    /// (settings-skipped lines are absent from the filtered lists).
    #[allow(clippy::too_many_arguments)]
    fn push_context(
        rows: &mut Vec<MergedDiffRow>,
        l: &mut usize,
        r: &mut usize,
        li: &mut usize,
        ri: &mut usize,
        left_target: usize,
        right_target: usize,
        left_filtered: &[LineInfo],
        right_filtered: &[LineInfo],
    ) {
        while *l < left_target || *r < right_target {
            let left = if *l < left_target {
                Some(take_filtered_line(left_filtered, li, *l))
            } else {
                None
            };
            let right = if *r < right_target {
                Some(take_filtered_line(right_filtered, ri, *r))
            } else {
                None
            };
            let status = match (&left, &right) {
                (Some(_), Some(_)) => MergedRowStatus::Same,
                (Some(_), None) => MergedRowStatus::Deleted,
                (None, Some(_)) => MergedRowStatus::Inserted,
                (None, None) => unreachable!("loop guard advances a side"),
            };
            rows.push(MergedDiffRow {
                left,
                right,
                status,
            });
            *l += usize::from(*l < left_target);
            *r += usize::from(*r < right_target);
        }
    }

    /// Return the first merged-row index (0-based, as produced by
    /// [`Self::merged_rows`]) of each contiguous difference group.
    ///
    /// These are the jump targets for next/previous-difference navigation:
    /// a merged row belongs to a difference when its status reports one,
    /// and adjacent such rows collapse into a single group.
    pub fn group_starts(&self) -> Vec<usize> {
        let mut groups: Vec<usize> = Vec::new();
        let mut prev_diff = false;
        for (i, row) in self.merged_rows().iter().enumerate() {
            let is_diff = row.status.is_difference();
            if is_diff && !prev_diff {
                groups.push(i);
            }
            prev_diff = is_diff;
        }
        groups
    }

    /// Return `(group, total)` for the merged row under the cursor: the
    /// 1-based index of the difference group the row falls in, and the
    /// total number of groups.
    ///
    /// Returns `(0, total)` when `row` precedes the first group and `(0, 0)`
    /// when there are no differences.
    pub fn position_of(&self, row: usize) -> (usize, usize) {
        let groups = self.group_starts();
        let total = groups.len();
        if total == 0 {
            return (0, 0);
        }
        // The cursor sits in the last group that starts at or before it.
        for (i, &start) in groups.iter().enumerate().rev() {
            if row >= start {
                return (i + 1, total);
            }
        }
        (0, total)
    }

    /// Iterate over differences, skipping those below the given importance
    /// threshold.
    pub fn filter_by_importance(
        &self,
        min: Importance,
    ) -> impl Iterator<Item = &TextDifference> {
        self.differences.iter().filter(move |diff| match diff {
            TextDifference::LineDifferent(left, right) => {
                left.tokens.iter().any(|t| t.importance >= min)
                    || right.tokens.iter().any(|t| t.importance >= min)
            }
            TextDifference::LinesAdded(_, lines)
            | TextDifference::LinesRemoved(_, lines) => lines
                .iter()
                .any(|l| l.tokens.iter().any(|t| t.importance >= min)),
            TextDifference::LinesChanged(_, removed, added) => {
                removed
                    .iter()
                    .any(|l| l.tokens.iter().any(|t| t.importance >= min))
                    || added
                        .iter()
                        .any(|l| l.tokens.iter().any(|t| t.importance >= min))
            }
            TextDifference::BlankLine => false,
        })
    }
}

/// Compare two text contents and return a `TextDiff` describing all
/// differences.
///
/// The comparison uses the `diff` crate's LCS algorithm on normalized lines.
/// Lines are preprocessed according to `settings` (whitespace trimming,
/// case folding, comment skipping, etc.) before the diff is computed.
pub fn compare_texts(
    left: &str,
    right: &str,
    settings: &TextCompareSettings,
) -> TextDiff {
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();

    // Build filtered lists of (original_1_based_line_number, raw_line).
    let left_filtered: Vec<(usize, &str)> = left_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !settings.should_skip(line))
        .map(|(i, line)| (i + 1, *line))
        .collect();

    let right_filtered: Vec<(usize, &str)> = right_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !settings.should_skip(line))
        .map(|(i, line)| (i + 1, *line))
        .collect();

    // Pre-compute normalized forms for comparison. Use a wrapper type so
    // that `diff::slice` can use `PartialEq` with custom numeric logic.
    let ignore_numeric = settings.ignore_numeric_changes;
    let left_norm: Vec<NormLine> = left_filtered
        .iter()
        .map(|(_, line)| {
            NormLine(settings.normalize_line(line), ignore_numeric)
        })
        .collect();
    let right_norm: Vec<NormLine> = right_filtered
        .iter()
        .map(|(_, line)| {
            NormLine(settings.normalize_line(line), ignore_numeric)
        })
        .collect();

    // Compare using diff::slice on the wrapper type.
    let changes = diff::slice(&left_norm[..], &right_norm[..]);

    // Use the settings grammar or fall back to plain text.
    let default_grammar = Grammar::plain_text();
    let grammar = settings.grammar.as_ref().unwrap_or(&default_grammar);

    // Walk the diff results and build structured differences.
    let mut differences = Vec::new();
    let mut max_importance = Importance::Ignored;

    let mut removed_buf: Vec<LineInfo> = Vec::new();
    let mut added_buf: Vec<LineInfo> = Vec::new();
    let mut left_start: Option<usize> = None;

    let mut left_idx = 0usize;
    let mut right_idx = 0usize;

    for change in changes {
        match change {
            diff::Result::Left(_) => {
                let (orig_num, raw) = left_filtered[left_idx];
                let info = LineInfo::new(orig_num, raw.to_string())
                    .with_grammar(grammar);
                update_max_importance(&mut max_importance, &info);
                left_start.get_or_insert(orig_num);
                removed_buf.push(info);
                left_idx += 1;
            }
            diff::Result::Right(_) => {
                let (orig_num, raw) = right_filtered[right_idx];
                let info = LineInfo::new(orig_num, raw.to_string())
                    .with_grammar(grammar);
                update_max_importance(&mut max_importance, &info);
                added_buf.push(info);
                right_idx += 1;
            }
            diff::Result::Both(..) => {
                flush_changes(
                    &mut differences,
                    left_start.take(),
                    std::mem::take(&mut removed_buf),
                    std::mem::take(&mut added_buf),
                );
                left_idx += 1;
                right_idx += 1;
            }
        }
    }

    // Flush remaining changes at end of input.
    flush_changes(&mut differences, left_start, removed_buf, added_buf);

    // Tokenize the lines that survived filtering for the merged row model
    // (the difference blocks own the classification of the differing
    // lines).
    let left_filtered = left_filtered
        .iter()
        .map(|(num, raw)| {
            LineInfo::new(*num, (*raw).to_string()).with_grammar(grammar)
        })
        .collect();
    let right_filtered = right_filtered
        .iter()
        .map(|(num, raw)| {
            LineInfo::new(*num, (*raw).to_string()).with_grammar(grammar)
        })
        .collect();

    TextDiff {
        differences,
        left_line_count: left_lines.len(),
        right_line_count: right_lines.len(),
        max_importance,
        left_filtered,
        right_filtered,
    }
}

/// Take the filtered line at the current filtered index when it is the
/// cursor's raw line number, advancing the index; otherwise build a plain
/// unclassified line for a settings-skipped raw line.
fn take_filtered_line(
    filtered: &[LineInfo],
    idx: &mut usize,
    number: usize,
) -> LineInfo {
    if let Some(info) = filtered.get(*idx)
        && info.number == number
    {
        *idx += 1;
        return info.clone();
    }
    LineInfo::new(number, String::new())
}

/// Wrapper around a normalized line that implements `PartialEq` with optional
/// numeric comparison semantics.
struct NormLine(String, bool); // (normalized_text, ignore_numeric)

impl PartialEq for NormLine {
    fn eq(&self, other: &Self) -> bool {
        if self.1
            && let (Ok(x), Ok(y)) =
                (self.0.trim().parse::<f64>(), other.0.trim().parse::<f64>())
            && (x - y).abs() < 1e-9
        {
            return true;
        }
        self.0 == other.0
    }
}

impl Eq for NormLine {}

/// Update `max_importance` if any token in the line has higher importance.
fn update_max_importance(max: &mut Importance, line: &LineInfo) {
    for token in &line.tokens {
        if token.importance > *max {
            *max = token.importance;
        }
    }
}

// ---------------------------------------------------------------------------
// Flush accumulated removed/added lines into `TextDifference` variants.
// ---------------------------------------------------------------------------

fn flush_changes(
    differences: &mut Vec<TextDifference>,
    left_start: Option<usize>,
    removed: Vec<LineInfo>,
    added: Vec<LineInfo>,
) {
    // Lines only added in right.
    if removed.is_empty() && !added.is_empty() {
        let start = added.first().map(|l| l.number);
        if let Some(start) = start {
            differences.push(TextDifference::LinesAdded(start, added));
        }
        return;
    }
    // Lines only removed from left.
    if !removed.is_empty() && added.is_empty() {
        if let Some(s) = left_start {
            differences.push(TextDifference::LinesRemoved(s, removed));
        }
        return;
    }
    // Lines changed (both removed and added).
    if !removed.is_empty()
        && !added.is_empty()
        && let Some(s) = left_start
    {
        differences.push(TextDifference::LinesChanged(s, removed, added));
    }
}

// ---------------------------------------------------------------------------
// Convenience: compare two lines for equality under settings
// ---------------------------------------------------------------------------

/// Compare two individual lines for equality under the given settings.
/// Returns `true` when the lines are considered equal.
pub fn lines_equal(
    left: &str,
    right: &str,
    settings: &TextCompareSettings,
) -> bool {
    if settings.ignore_numeric_changes
        && let (Ok(l), Ok(r)) =
            (left.trim().parse::<f64>(), right.trim().parse::<f64>())
        && (l - r).abs() < 1e-9
    {
        return true;
    }
    settings.normalize_line(left) == settings.normalize_line(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_settings() -> TextCompareSettings {
        TextCompareSettings::new()
    }

    #[test]
    fn identical_texts_produce_no_diff() {
        let text = "hello\nworld\n";
        let diff = compare_texts(text, text, &default_settings());
        assert!(diff.is_empty());
        assert_eq!(diff.left_line_count, 2);
        assert_eq!(diff.right_line_count, 2);
    }

    #[test]
    fn detects_added_line() {
        let left = "alpha\nbeta\n";
        let right = "alpha\nbeta\ngamma\n";
        let diff = compare_texts(left, right, &default_settings());
        assert!(!diff.is_empty());
        assert_eq!(diff.differences.len(), 1);
    }

    #[test]
    fn detects_removed_line() {
        let left = "alpha\nbeta\ngamma\n";
        let right = "alpha\nbeta\n";
        let diff = compare_texts(left, right, &default_settings());
        assert!(!diff.is_empty());
    }

    #[test]
    fn detects_changed_line() {
        let left = "hello\nworld\n";
        let right = "hello\neveryone\n";
        let diff = compare_texts(left, right, &default_settings());
        assert!(!diff.is_empty());
    }

    #[test]
    fn ignore_case() {
        let mut settings = default_settings();
        settings.ignore_case = true;
        let diff = compare_texts("Hello\n", "hello\n", &settings);
        assert!(diff.is_empty());
    }

    #[test]
    fn ignore_whitespace_trim() {
        let mut settings = default_settings();
        settings.whitespace_mode = WhitespaceMode::Trim;
        let diff = compare_texts("  hello  \n", "hello\n", &settings);
        assert!(diff.is_empty());
    }

    #[test]
    fn ignore_blank_lines() {
        let mut settings = default_settings();
        settings.ignore_blank_lines = true;
        let left = "alpha\n\nbeta\n";
        let right = "alpha\nbeta\n";
        let diff = compare_texts(left, right, &settings);
        assert!(diff.is_empty());
    }

    #[test]
    fn ignore_comments_with_grammar() {
        let mut settings = default_settings();
        settings.ignore_comments = true;
        settings.grammar = Some(Grammar::rust());
        let left = "// old comment\nfn main() {}\n";
        let right = "// new comment\nfn main() {}\n";
        let diff = compare_texts(left, right, &settings);
        assert!(diff.is_empty());
    }

    #[test]
    fn ignore_numeric_changes() {
        let mut settings = default_settings();
        settings.ignore_numeric_changes = true;
        let diff = compare_texts("42\n", "42.0\n", &settings);
        assert!(diff.is_empty());
    }

    #[test]
    fn lines_equal_basic() {
        assert!(lines_equal("hello", "hello", &default_settings()));
        assert!(!lines_equal("hello", "world", &default_settings()));
    }

    #[test]
    fn line_info_with_grammar() {
        let grammar = Grammar::rust();
        let line =
            LineInfo::new(1, "// comment".into()).with_grammar(&grammar);
        assert_eq!(line.tokens.len(), 1);
        assert_eq!(line.tokens[0].importance, Importance::Comment);
    }

    #[test]
    fn text_diff_display() {
        let left = LineInfo::new(1, "old".into());
        let right = LineInfo::new(1, "new".into());
        let diff = TextDifference::LineDifferent(left, right);
        let display = format!("{}", diff);
        assert!(display.contains("old"));
        assert!(display.contains("new"));
    }

    #[test]
    fn max_importance_tracked() {
        let mut settings = default_settings();
        settings.grammar = Some(Grammar::rust());
        let diff = compare_texts(
            "// comment\nfn main() {}\n",
            "// comment\nfn foo() {}\n",
            &settings,
        );
        assert!(!diff.is_empty());
        assert_eq!(diff.max_importance, Importance::Code);
    }

    #[test]
    fn filter_by_importance() {
        let mut settings = default_settings();
        settings.grammar = Some(Grammar::rust());
        let diff = compare_texts("fn main() {}\n", "fn foo() {}\n", &settings);
        // Code-level differences should pass.
        let code_diffs: Vec<_> =
            diff.filter_by_importance(Importance::Code).collect();
        assert!(!code_diffs.is_empty());
    }

    #[test]
    fn empty_text_comparison() {
        let diff = compare_texts("", "", &default_settings());
        assert!(diff.is_empty());
    }

    #[test]
    fn single_line_texts() {
        let diff = compare_texts("a", "b", &default_settings());
        assert!(!diff.is_empty());
    }

    /// A diff with line 3 differing, lines 4 and 5 added on the right, and
    /// line 7 (left) / line 9 (right) differing.
    fn navigation_diff() -> TextDiff {
        let left = "l1\nl2\nA\nl4\nl5\nl6\nB\nl8\nl9\nl10\n";
        let right = "l1\nl2\nB\nX\nY\nl4\nl5\nl6\nA\nl8\nl9\nl10\n";
        compare_texts(left, right, &default_settings())
    }

    #[test]
    fn difference_line_numbers_collects_both_sides() {
        let numbers = navigation_diff().difference_line_numbers();
        for n in [3, 4, 5, 7] {
            assert!(numbers.contains(&n), "missing line {n}");
        }
        assert!(!numbers.contains(&1));
        assert!(!numbers.contains(&10));
    }

    #[test]
    fn difference_line_numbers_by_side_splits_sides() {
        let (left, right) =
            navigation_diff().difference_line_numbers_by_side();
        // Line 3 differs on both sides (left 3 ↔ right 8), lines 4 and 5
        // were added on the right only, line 7 (left) pairs with line 9
        // (right).
        assert_eq!(left, HashSet::from([3, 7]));
        assert_eq!(right, HashSet::from([3, 4, 5, 9]));
    }

    #[test]
    fn group_starts_merges_contiguous_rows() {
        // Merged rows 2, 3, 4 (0-based) form one group, row 8 another.
        assert_eq!(navigation_diff().group_starts(), vec![2, 8]);
    }

    #[test]
    fn group_starts_is_empty_without_differences() {
        assert!(TextDiff::default().group_starts().is_empty());
    }

    #[test]
    fn position_of_reports_one_based_group() {
        let diff = navigation_diff();
        // Before the first group.
        assert_eq!(diff.position_of(0), (0, 2));
        assert_eq!(diff.position_of(3), (1, 2));
        assert_eq!(diff.position_of(11), (2, 2));
    }

    #[test]
    fn position_of_without_differences_is_zero() {
        assert_eq!(TextDiff::default().position_of(0), (0, 0));
    }

    #[test]
    fn merged_rows_pad_insertions_with_blank_left_cells() {
        let diff =
            compare_texts("a\nb\n", "a\nx\ny\nb\n", &default_settings());
        let rows = diff.merged_rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].status, MergedRowStatus::Same);
        assert_eq!(rows[1].status, MergedRowStatus::Inserted);
        assert_eq!(rows[1].left, None);
        assert_eq!(
            rows[1]
                .right
                .as_ref()
                .map(|l| (l.number, l.content.as_str())),
            Some((2, "x"))
        );
        assert_eq!(rows[2].status, MergedRowStatus::Inserted);
        // The trailing context line stays paired after the block; the
        // right line number reflects the shift caused by the insertion.
        assert_eq!(rows[3].status, MergedRowStatus::Same);
        assert_eq!(rows[3].left.as_ref().map(|l| l.number), Some(2));
        assert_eq!(rows[3].right.as_ref().map(|l| l.number), Some(4));
        assert_eq!(
            rows[3].right.as_ref().map(|l| l.content.as_str()),
            Some("b")
        );
    }

    #[test]
    fn merged_rows_pad_deletions_with_blank_right_cells() {
        let diff = compare_texts("a\nx\nb\n", "a\nb\n", &default_settings());
        let rows = diff.merged_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].status, MergedRowStatus::Deleted);
        assert_eq!(rows[1].right, None);
        assert_eq!(
            rows[1]
                .left
                .as_ref()
                .map(|l| (l.number, l.content.as_str())),
            Some((2, "x"))
        );
        assert_eq!(rows[2].status, MergedRowStatus::Same);
    }

    #[test]
    fn merged_rows_pair_changed_blocks_and_pad_the_remainder() {
        // One left line is replaced by two right lines.
        let diff = compare_texts("a\nb\n", "a\nx\ny\n", &default_settings());
        let rows = diff.merged_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, MergedRowStatus::Same);
        // The exact hunk shape depends on the LCS; what must hold is one
        // Changed row followed by one Inserted filler row.
        let statuses: Vec<MergedRowStatus> =
            rows.iter().map(|row| row.status).collect();
        assert!(
            statuses.contains(&MergedRowStatus::Changed)
                && statuses.contains(&MergedRowStatus::Inserted),
            "unexpected statuses: {statuses:?}"
        );
        let changed = rows
            .iter()
            .find(|row| row.status == MergedRowStatus::Changed)
            .unwrap();
        assert!(changed.left.is_some() && changed.right.is_some());
    }

    #[test]
    fn merged_rows_pair_skipped_context_lines() {
        // With blank lines ignored, the blank context row must still be
        // paired on both sides in the merged view.
        let mut settings = default_settings();
        settings.ignore_blank_lines = true;
        let diff = compare_texts("a\n\nb\n", "a\n\nB\n", &settings);
        assert!(!diff.is_empty());
        let rows = diff.merged_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].status, MergedRowStatus::Same);
        assert_eq!(rows[1].left.as_ref().map(|l| l.number), Some(2));
        assert_eq!(rows[1].right.as_ref().map(|l| l.number), Some(2));
        assert_eq!(rows[2].status, MergedRowStatus::Changed);
    }

    #[test]
    fn merged_rows_of_handbuilt_diff_fall_back_to_empty_context() {
        // A hand-built diff carries no filtered line lists; context rows
        // still align by line number, with empty placeholder content.
        let diff = TextDiff {
            differences: vec![TextDifference::LinesAdded(
                2,
                vec![LineInfo::new(2, "x".into())],
            )],
            left_line_count: 2,
            right_line_count: 3,
            ..Default::default()
        };
        let rows = diff.merged_rows();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, MergedRowStatus::Same);
        assert_eq!(rows[0].left.as_ref().map(|l| l.number), Some(1));
        assert_eq!(rows[1].status, MergedRowStatus::Inserted);
        assert_eq!(rows[2].status, MergedRowStatus::Same);
        assert_eq!(rows[2].left.as_ref().map(|l| l.number), Some(2));
        assert_eq!(rows[2].right.as_ref().map(|l| l.number), Some(3));
    }
}
