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

use std::fmt;

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
        if self.ignore_comments {
            if let Some(g) = &self.grammar
                && g.classify_line(line) == Importance::Comment
            {
                return true;
            }
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
}

impl TextDiff {
    /// Return `true` when the texts are identical (no differences).
    pub fn is_empty(&self) -> bool {
        self.differences.is_empty()
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

    TextDiff {
        differences,
        left_line_count: left_lines.len(),
        right_line_count: right_lines.len(),
        max_importance,
    }
}

/// Wrapper around a normalized line that implements `PartialEq` with optional
/// numeric comparison semantics.
struct NormLine(String, bool); // (normalized_text, ignore_numeric)

impl PartialEq for NormLine {
    fn eq(&self, other: &Self) -> bool {
        if self.1 {
            if let (Ok(x), Ok(y)) =
                (self.0.trim().parse::<f64>(), other.0.trim().parse::<f64>())
                && (x - y).abs() < 1e-9
            {
                return true;
            }
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
    if !removed.is_empty() && !added.is_empty() {
        if let Some(s) = left_start {
            differences.push(TextDifference::LinesChanged(s, removed, added));
        }
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
    if settings.ignore_numeric_changes {
        if let (Ok(l), Ok(r)) =
            (left.trim().parse::<f64>(), right.trim().parse::<f64>())
            && (l - r).abs() < 1e-9
        {
            return true;
        }
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
}
