// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Unified diff generation and patch application.
//!
//! This module provides two core capabilities:
//!
//! - **Generation**: [`generate_unified_diff`] converts two text contents into
//!   a standard unified diff (`---`/`+++`/`@@`) suitable for piping to `patch`
//!   or `git apply`.
//!
//! - **Application**: [`apply_patch`] parses a unified diff and applies it to
//!   a target file, returning a [`PatchResult`] that indicates whether all
//!   hunks landed cleanly, some required fuzz, or some failed entirely.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use fs_err::tokio as tokio_fs;
use thiserror::Error;

use crate::text::{TextCompareSettings, compare_texts};

// ---------------------------------------------------------------------------
// Patch errors
// ---------------------------------------------------------------------------

/// Errors that can occur during patch parsing or application.
#[derive(Debug, Error)]
pub enum PatchError {
    /// The unified diff header is missing or malformed.
    #[error("malformed diff header: {0}")]
    MalformedHeader(String),

    /// A hunk header (`@@ ... @@`) is missing or malformed.
    #[error("malformed hunk header at line {0}: {1}")]
    MalformedHunkHeader(usize, String),

    /// A hunk references lines outside the target file range.
    #[error("hunk {0} out of bounds: target file has {1} lines")]
    HunkOutOfBounds(usize, usize),

    /// A hunk's context lines do not match the target file content.
    #[error("hunk {0} failed at line {1}: context mismatch")]
    HunkMismatch(usize, usize),

    /// The target file could not be read or written.
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The patch text is empty or contains no hunks.
    #[error("empty patch: no hunks found")]
    EmptyPatch,
}

// ---------------------------------------------------------------------------
// Patch result
// ---------------------------------------------------------------------------

/// Outcome of applying a patch to a file.
#[derive(Debug)]
pub enum PatchResult {
    /// All hunks applied successfully without any issues.
    Applied,

    /// Some hunks failed to apply. The Vec contains (hunk_index, reason)
    /// pairs for each failed hunk.
    HunkFailed(Vec<(usize, String)>),

    /// All hunks applied but some required position fuzzing. The Vec
    /// contains the 0-based indices of hunks that were fuzzed.
    FuzzApplied(Vec<usize>),
}

impl fmt::Display for PatchResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PatchResult::Applied => write!(f, "patch applied cleanly"),
            PatchResult::HunkFailed(failures) => {
                write!(f, "{} hunk(s) failed", failures.len())?;
                for (idx, reason) in failures {
                    write!(f, "\n  hunk {idx}: {reason}")?;
                }
                Ok(())
            }
            PatchResult::FuzzApplied(indices) => {
                write!(
                    f,
                    "patch applied with fuzz on {} hunk(s)",
                    indices.len()
                )?;
                for idx in indices {
                    write!(f, " {idx}")?;
                }
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: parsed hunk representation
// ---------------------------------------------------------------------------

/// A single hunk from a unified diff, ready for application.
#[derive(Debug)]
pub(crate) struct PatchHunk {
    /// 1-based starting line in the old file.
    pub(crate) old_start: usize,
    /// Number of lines this hunk covers in the old file.
    pub(crate) old_lines: usize,
    /// 1-based starting line in the new file (parsed but not used for
    /// application).
    #[allow(dead_code)]
    pub(crate) new_start: usize,
    /// Number of lines this hunk covers in the new file.
    #[allow(dead_code)]
    pub(crate) new_lines: usize,
    /// Operations to apply: keep context, remove, or add lines.
    pub(crate) lines: Vec<HunkLine>,
}

/// A single line within a hunk.
#[derive(Debug)]
pub(crate) enum HunkLine {
    /// A context line (unchanged). Content must match.
    Context(String),
    /// A line to remove from the target.
    Remove,
    /// A line to add to the target.
    Add(String),
}

// ---------------------------------------------------------------------------
// Unified diff generation
// ---------------------------------------------------------------------------

/// Number of context lines to include around changes.
const CONTEXT_LINES: usize = 3;

/// Generate a unified diff string from two text contents.
///
/// Produces standard unified diff output with:
/// - File header lines (`--- left`, `+++ right`)
/// - Hunk headers (`@@ -old_start,old_count +new_start,new_count @@`)
/// - Context lines around changes
///
/// Lines that are identical are included as context around changes.
/// When changes are adjacent (within 2×context lines), hunks are merged.
///
/// # Arguments
///
/// - `left_path`: Display path for the "old" file (used in `---` header).
/// - `right_path`: Display path for the "new" file (used in `+++` header).
/// - `left`: Original text content.
/// - `right`: Modified text content.
///
/// # Returns
///
/// Returns an empty string if the texts are identical.
pub fn generate_unified_diff(
    left_path: &str,
    right_path: &str,
    left: &str,
    right: &str,
) -> String {
    generate_unified_diff_with_settings(
        left_path,
        right_path,
        left,
        right,
        &TextCompareSettings::new(),
    )
}

/// Generate a unified diff with custom comparison settings.
///
/// When settings like `ignore_case` make the texts equal, an empty string
/// is returned. Otherwise the raw (non-normalized) lines are used for
/// output so the diff reflects the actual character-level differences.
pub fn generate_unified_diff_with_settings(
    left_path: &str,
    right_path: &str,
    left: &str,
    right: &str,
    settings: &TextCompareSettings,
) -> String {
    // Fast path: identical texts produce no diff.
    if left == right {
        return String::new();
    }

    // Use the library's text comparison to determine if there are
    // semantic differences. If settings suppress all differences,
    // return empty.
    let diff = compare_texts(left, right, settings);
    if diff.is_empty() {
        return String::new();
    }

    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();

    // Collect changed ranges by walking diff::slice on raw lines.
    let changes: Vec<diff::Result<&str>> =
        diff::slice(&left_lines, &right_lines)
            .into_iter()
            .map(|c| match c {
                diff::Result::Left(s) => diff::Result::Left(*s),
                diff::Result::Right(s) => diff::Result::Right(*s),
                diff::Result::Both(a, b) => diff::Result::Both(*a, *b),
            })
            .collect();
    let ranges = collect_change_ranges(&changes);

    if ranges.is_empty() {
        return String::new();
    }

    // Merge adjacent ranges and expand with context.
    let merged = merge_and_expand_ranges(&ranges);

    let mut output = String::new();
    output.push_str("--- ");
    output.push_str(left_path);
    output.push('\n');
    output.push_str("+++ ");
    output.push_str(right_path);
    output.push('\n');

    // Emit hunks. Track how many lines we've already consumed to handle
    // the diff::slice walk correctly.
    let mut last_right_end: usize = 0;

    for (l_start, l_end, r_start, r_end) in &merged {
        let l_count = l_end - l_start;
        let r_count = r_end - r_start;

        // Write hunk header using 1-based line numbers.
        output.push_str("@@ -");
        output.push_str(&format!("{}", l_start + 1));
        if l_count != 1 {
            output.push(',');
            output.push_str(&format!("{}", l_count));
        }
        output.push_str(" +");
        output.push_str(&format!("{}", r_start + 1));
        if r_count != 1 {
            output.push(',');
            output.push_str(&format!("{}", r_count));
        }
        output.push_str(" @@\n");

        // Walk the diff::slice results to emit the hunk body.
        emit_hunk_body(
            &mut output,
            &changes,
            &left_lines,
            &right_lines,
            *l_start,
            *l_end,
            *r_start,
            *r_end,
            last_right_end,
        );

        last_right_end = *r_end;
    }

    output
}

/// Walk the diff::slice results and collect ranges of changed lines.
///
/// Returns `(left_start_0, left_end_0, right_start_0, right_end_0)`
/// tuples for each contiguous block of changes. Ranges are exclusive
/// on the upper bound.
fn collect_change_ranges(
    changes: &[diff::Result<&str>],
) -> Vec<(usize, usize, usize, usize)> {
    let mut ranges = Vec::new();
    let mut in_change = false;
    let mut l_start = 0usize;
    let mut r_start = 0usize;
    let mut l_pos = 0usize;
    let mut r_pos = 0usize;

    for change in changes {
        match change {
            diff::Result::Left(_) => {
                if !in_change {
                    l_start = l_pos;
                    r_start = r_pos;
                    in_change = true;
                }
                l_pos += 1;
            }
            diff::Result::Right(_) => {
                if !in_change {
                    l_start = l_pos;
                    r_start = r_pos;
                    in_change = true;
                }
                r_pos += 1;
            }
            diff::Result::Both(..) => {
                if in_change {
                    ranges.push((l_start, l_pos, r_start, r_pos));
                    in_change = false;
                }
                l_pos += 1;
                r_pos += 1;
            }
        }
    }

    // Flush trailing change.
    if in_change {
        ranges.push((l_start, l_pos, r_start, r_pos));
    }

    ranges
}

/// Merge adjacent change ranges and expand with context lines.
///
/// Two ranges are merged when the gap between them is less than
/// 2×[`CONTEXT_LINES`], so that their context windows overlap.
fn merge_and_expand_ranges(
    ranges: &[(usize, usize, usize, usize)],
) -> Vec<(usize, usize, usize, usize)> {
    if ranges.is_empty() {
        return Vec::new();
    }

    // Expand each range with context.
    let expanded: Vec<(usize, usize, usize, usize)> = ranges
        .iter()
        .map(|(ls, le, rs, re)| {
            (
                ls.saturating_sub(CONTEXT_LINES),
                le + CONTEXT_LINES,
                rs.saturating_sub(CONTEXT_LINES),
                re + CONTEXT_LINES,
            )
        })
        .collect();

    // Merge overlapping/adjacent expanded ranges.
    let mut merged = vec![expanded[0]];
    for &next in &expanded[1..] {
        let last = merged.last_mut().unwrap();
        if next.0 <= last.1 && next.2 <= last.3 {
            last.1 = last.1.max(next.1);
            last.3 = last.3.max(next.3);
        } else {
            merged.push(next);
        }
    }

    merged
}

/// Emit the body of a unified diff hunk by re-walking the diff::slice
/// results and only outputting lines that fall within the merged range.
fn emit_hunk_body(
    output: &mut String,
    changes: &[diff::Result<&str>],
    left_lines: &[&str],
    right_lines: &[&str],
    l_start: usize,
    l_end: usize,
    r_start: usize,
    r_end: usize,
    _prev_right_end: usize,
) {
    let mut l_pos = 0usize;
    let mut r_pos = 0usize;

    for change in changes {
        match change {
            diff::Result::Left(left_line) => {
                if l_pos >= l_start && l_pos < l_end {
                    output.push('-');
                    output.push_str(left_line);
                    output.push('\n');
                }
                l_pos += 1;
            }
            diff::Result::Right(right_line) => {
                if r_pos >= r_start && r_pos < r_end {
                    output.push('+');
                    output.push_str(right_line);
                    output.push('\n');
                }
                r_pos += 1;
            }
            diff::Result::Both(line, _) => {
                if l_pos >= l_start
                    && l_pos < l_end
                    && r_pos >= r_start
                    && r_pos < r_end
                {
                    output.push(' ');
                    output.push_str(line);
                    output.push('\n');
                }
                l_pos += 1;
                r_pos += 1;
            }
        }
    }

    let _ = (right_lines, left_lines);
}

// ---------------------------------------------------------------------------
// Patch parsing
// ---------------------------------------------------------------------------

/// Parse a unified diff string into a list of hunks.
pub(crate) fn parse_patch(
    patch_text: &str,
) -> Result<Vec<PatchHunk>, PatchError> {
    let lines: Vec<&str> = patch_text.lines().collect();
    let mut hunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Skip file header and metadata lines.
        if line.starts_with("---")
            || line.starts_with("+++")
            || line.starts_with("index ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
            || line.starts_with("copy from ")
            || line.starts_with("copy to ")
            || line.starts_with("diff --git")
        {
            i += 1;
            continue;
        }

        if line.starts_with("@@") {
            let mut hunk = parse_hunk_header(line, i + 1)?;
            i += 1;

            let mut hunk_lines = Vec::new();
            let mut old_count = hunk.old_lines;
            let mut new_count = hunk.new_lines;

            while i < lines.len()
                && !lines[i].starts_with("@@")
                && !lines[i].starts_with("--- ")
                && !lines[i].starts_with("diff --git")
                && (old_count > 0 || new_count > 0)
            {
                let cur_line = lines[i];
                if cur_line.starts_with(' ') {
                    hunk_lines
                        .push(HunkLine::Context(cur_line[1..].to_string()));
                    old_count = old_count.saturating_sub(1);
                    new_count = new_count.saturating_sub(1);
                } else if cur_line.starts_with('-') {
                    hunk_lines.push(HunkLine::Remove);
                    old_count = old_count.saturating_sub(1);
                } else if cur_line.starts_with('+') {
                    hunk_lines.push(HunkLine::Add(cur_line[1..].to_string()));
                    new_count = new_count.saturating_sub(1);
                } else if cur_line == "\\ No newline at end of file" {
                    // Marker; keep track but don't add as content line.
                } else {
                    // Fallback: treat as context line for robustness.
                    hunk_lines.push(HunkLine::Context(cur_line.to_string()));
                    old_count = old_count.saturating_sub(1);
                    new_count = new_count.saturating_sub(1);
                }
                i += 1;
            }

            hunk.lines = hunk_lines;
            hunks.push(hunk);
        } else {
            i += 1;
        }
    }

    if hunks.is_empty() {
        return Err(PatchError::EmptyPatch);
    }

    Ok(hunks)
}

/// Parse a single `@@` hunk header line.
fn parse_hunk_header(
    line: &str,
    line_num: usize,
) -> Result<PatchHunk, PatchError> {
    let line = line.trim();
    if !line.starts_with("@@") || !line.ends_with("@@") {
        return Err(PatchError::MalformedHunkHeader(
            line_num,
            line.to_string(),
        ));
    }

    let inner = &line[2..line.len() - 2].trim();
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() < 2 {
        return Err(PatchError::MalformedHunkHeader(
            line_num,
            line.to_string(),
        ));
    }

    let (old_start, old_lines) = parse_hunk_range(parts[0], line_num, "old")?;
    let (new_start, new_lines) = parse_hunk_range(parts[1], line_num, "new")?;

    Ok(PatchHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        lines: Vec::new(),
    })
}

/// Parse a range specifier like `-123,456` into (start, count).
fn parse_hunk_range(
    spec: &str,
    line_num: usize,
    label: &str,
) -> Result<(usize, usize), PatchError> {
    if !spec.starts_with('-') && !spec.starts_with('+') {
        return Err(PatchError::MalformedHunkHeader(
            line_num,
            format!("{spec} (invalid {label} range)"),
        ));
    }

    let nums = &spec[1..];
    let parts: Vec<&str> = nums.split(',').collect();

    match parts.as_slice() {
        [start_str] => {
            let start: usize = start_str.parse().map_err(|_| {
                PatchError::MalformedHunkHeader(
                    line_num,
                    format!("{spec} (invalid start)"),
                )
            })?;
            Ok((start, 1))
        }
        [start_str, count_str] => {
            let start: usize = start_str.parse().map_err(|_| {
                PatchError::MalformedHunkHeader(
                    line_num,
                    format!("{spec} (invalid start)"),
                )
            })?;
            let count: usize = count_str.parse().map_err(|_| {
                PatchError::MalformedHunkHeader(
                    line_num,
                    format!("{spec} (invalid count)"),
                )
            })?;
            Ok((start, count))
        }
        _ => Err(PatchError::MalformedHunkHeader(
            line_num,
            format!("{spec} (malformed range)"),
        )),
    }
}

// ---------------------------------------------------------------------------
// Patch application
// ---------------------------------------------------------------------------

/// Apply a unified diff patch to a file on disk.
///
/// Reads the target file, parses the patch, and applies all hunks.
/// Supports a fuzz factor of 2 for context line mismatches.
///
/// # Arguments
///
/// - `target_path`: Path to the file to patch.
/// - `patch_text`: The unified diff text to apply.
///
/// # Returns
///
/// - `PatchResult::Applied` if all hunks applied cleanly.
/// - `PatchResult::FuzzApplied` if some hunks required position fuzzing.
/// - `PatchResult::HunkFailed` if one or more hunks failed to apply.
pub async fn apply_patch(
    target_path: &Path,
    patch_text: &str,
) -> Result<PatchResult, PatchError> {
    let content =
        tokio_fs::read_to_string(target_path).await.map_err(|e| {
            PatchError::Io {
                path: target_path.to_path_buf(),
                source: e,
            }
        })?;

    let hunks = parse_patch(patch_text)?;

    let mut result_lines: Vec<String> =
        content.lines().map(String::from).collect();

    let mut failed = Vec::new();
    let mut fuzzed = Vec::new();

    // Track cumulative offset so later hunks apply at adjusted positions.
    let mut offset: isize = 0;

    for (hunk_idx, hunk) in hunks.iter().enumerate() {
        let actual_start = (hunk.old_start as isize - 1 + offset) as usize;

        match apply_hunk(&mut result_lines, hunk, actual_start, hunk_idx) {
            Ok((lines_delta, fuzz_used)) => {
                offset += lines_delta;
                if fuzz_used {
                    fuzzed.push(hunk_idx);
                }
            }
            Err(reason) => {
                failed.push((hunk_idx, reason));
            }
        }
    }

    if !failed.is_empty() {
        return Ok(PatchResult::HunkFailed(failed));
    }

    // Rebuild the file content.
    let mut output = String::new();
    for (idx, line) in result_lines.iter().enumerate() {
        if idx > 0 {
            output.push('\n');
        }
        output.push_str(line);
    }

    // Preserve trailing newline if the original had one.
    if content.ends_with('\n') && !output.ends_with('\n') {
        output.push('\n');
    }

    tokio_fs::write(target_path, &output).await.map_err(|e| {
        PatchError::Io {
            path: target_path.to_path_buf(),
            source: e,
        }
    })?;

    if !fuzzed.is_empty() {
        Ok(PatchResult::FuzzApplied(fuzzed))
    } else {
        Ok(PatchResult::Applied)
    }
}

/// Apply a single hunk to the target lines.
///
/// Returns the net change in line count and whether fuzz was used.
fn apply_hunk(
    target: &mut Vec<String>,
    hunk: &PatchHunk,
    start: usize,
    hunk_idx: usize,
) -> Result<(isize, bool), String> {
    let max_fuzz = 2;

    // Try exact match first, then fuzz positions: +1, -1, +2, -2.
    let fuzz_steps: Vec<isize> = [0, 1, -1, 2, -2].to_vec();
    let mut used_fuzz = false;
    let mut chosen_pos = start;

    for fuzz in &fuzz_steps {
        let adjusted = if *fuzz >= 0 {
            start + (*fuzz as usize)
        } else {
            start.saturating_sub((-fuzz) as usize)
        };

        if match_hunk(target, hunk, adjusted) {
            chosen_pos = adjusted;
            if *fuzz != 0 {
                used_fuzz = true;
            }
            break;
        }

        if *fuzz == max_fuzz {
            return Err(format!(
                "hunk {hunk_idx} failed at line {}: context mismatch",
                chosen_pos + 1
            ));
        }
    }

    // Build the new lines for this hunk.
    let mut new_lines = Vec::new();
    for line in &hunk.lines {
        match line {
            HunkLine::Context(content) | HunkLine::Add(content) => {
                new_lines.push(content.clone());
            }
            HunkLine::Remove => {
                // Skip removed lines.
            }
        }
    }

    let old_count = hunk.old_lines;
    let new_count = new_lines.len();

    // Replace the old range.
    let end = (chosen_pos + old_count).min(target.len());
    target.splice(chosen_pos..end, new_lines);

    let delta = (new_count as isize) - (old_count as isize);
    Ok((delta, used_fuzz))
}

/// Check if a hunk matches the target file at the given position.
fn match_hunk(target: &[String], hunk: &PatchHunk, start: usize) -> bool {
    if start > target.len() {
        return false;
    }

    let mut target_pos = start;

    for line in &hunk.lines {
        match line {
            HunkLine::Context(content) => {
                if target_pos >= target.len() {
                    return false;
                }
                if target[target_pos] != *content {
                    return false;
                }
                target_pos += 1;
            }
            HunkLine::Remove => {
                // Consumes a position in the old file but nothing
                // needs to match (the content is about to be removed).
                target_pos += 1;
            }
            HunkLine::Add(_) => {
                // Does not consume an old file position.
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextCompareSettings;

    // -- Generation tests --

    #[test]
    fn identical_texts_produce_empty_diff() {
        let text = "hello\nworld\n";
        let diff = generate_unified_diff("a.txt", "b.txt", text, text);
        assert!(diff.is_empty());
    }

    #[test]
    fn single_line_change_produces_diff() {
        let left = "hello\nworld\nfoo";
        let right = "hello\nWORLD\nfoo";
        let diff = generate_unified_diff("left.txt", "right.txt", left, right);
        assert!(!diff.is_empty(), "expected non-empty diff");
        assert!(diff.contains("--- left.txt"));
        assert!(diff.contains("+++ right.txt"));
        assert!(diff.contains("@@"));
        assert!(diff.contains("-world"));
        assert!(diff.contains("+WORLD"));
    }

    #[test]
    fn added_line_in_diff() {
        let left = "a\nb\nc";
        let right = "a\nb\nX\nc";
        let diff = generate_unified_diff("left.txt", "right.txt", left, right);
        assert!(!diff.is_empty(), "expected non-empty diff");
        assert!(diff.contains("+X"));
    }

    #[test]
    fn removed_line_in_diff() {
        let left = "a\nX\nb\nc";
        let right = "a\nb\nc";
        let diff = generate_unified_diff("left.txt", "right.txt", left, right);
        assert!(!diff.is_empty(), "expected non-empty diff");
        assert!(diff.contains("-X"));
    }

    #[test]
    fn empty_left_text() {
        let right = "hello\nworld";
        let diff = generate_unified_diff("left.txt", "right.txt", "", right);
        assert!(!diff.is_empty(), "expected non-empty diff for additions");
    }

    #[test]
    fn empty_right_text() {
        let left = "hello\nworld";
        let diff = generate_unified_diff("left.txt", "right.txt", left, "");
        assert!(!diff.is_empty(), "expected non-empty diff for removals");
    }

    #[test]
    fn diff_header_contains_paths() {
        let diff =
            generate_unified_diff("left.txt", "right.txt", "a\nb", "a\nc");
        assert!(diff.contains("--- left.txt"));
        assert!(diff.contains("+++ right.txt"));
    }

    #[test]
    fn diff_with_ignore_case_produces_empty() {
        let left = "Hello\nWorld";
        let right = "HELLO\nWORLD";

        let settings = TextCompareSettings {
            ignore_case: true,
            ..TextCompareSettings::new()
        };
        let diff = generate_unified_diff_with_settings(
            "left.txt",
            "right.txt",
            left,
            right,
            &settings,
        );
        // With ignore_case, texts are considered equal.
        assert!(diff.is_empty());
    }

    #[test]
    fn large_file_with_single_change() {
        // Build files with 100 lines where only line 50 differs.
        let mut left_lines = Vec::new();
        let mut right_lines = Vec::new();
        for i in 1..=100 {
            if i == 50 {
                left_lines.push(format!("ORIGINAL {}", i));
                right_lines.push(format!("MODIFIED {}", i));
            } else {
                left_lines.push(format!("line {}", i));
                right_lines.push(format!("line {}", i));
            }
        }
        let left = left_lines.join("\n");
        let right = right_lines.join("\n");
        let diff =
            generate_unified_diff("left.txt", "right.txt", &left, &right);
        assert!(!diff.is_empty());
        assert!(diff.contains("-ORIGINAL 50"));
        assert!(diff.contains("+MODIFIED 50"));
    }

    // -- Parsing tests --

    #[test]
    fn parse_simple_patch() {
        let patch = "\
--- a/file.txt
+++ b/file.txt
@@ -1,5 +1,5 @@
 line 1
-line 2
+line 2 modified
 line 3
 line 4
 line 5";

        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_lines, 5);
        assert_eq!(hunks[0].new_start, 1);
        assert_eq!(hunks[0].new_lines, 5);
    }

    #[test]
    fn parse_patch_with_additions() {
        let patch = "\
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,4 @@
 a
+b
 c
 d";

        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
    }

    #[test]
    fn parse_patch_with_removals() {
        let patch = "\
--- a/file.txt
+++ b/file.txt
@@ -1,4 +1,3 @@
 a
-b
 c
 d";

        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
    }

    #[test]
    fn parse_empty_patch_returns_error() {
        let err = parse_patch("just some text\nno hunks here").unwrap_err();
        assert!(matches!(err, PatchError::EmptyPatch));
    }

    #[test]
    fn parse_malformed_hunk_header() {
        let err = parse_patch("@@ bad header").unwrap_err();
        assert!(matches!(err, PatchError::MalformedHunkHeader(_, _)));
    }

    #[test]
    fn parse_multiple_hunks() {
        let patch = "\
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c
@@ -10,3 +10,3 @@
 x
-y
+Y
 z";

        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 2);
    }

    #[test]
    fn parse_patch_with_git_header() {
        let patch = "\
diff --git a/file.txt b/file.txt
index abc1234..def5678 100644
--- a/file.txt
+++ b/file.txt
@@ -1,3 +1,3 @@
 a
-b
+B
 c";

        let hunks = parse_patch(patch).unwrap();
        assert_eq!(hunks.len(), 1);
    }

    // -- Hunk range parsing --

    #[test]
    fn parse_range_with_count() {
        let (start, count) = parse_hunk_range("-123,456", 1, "old").unwrap();
        assert_eq!(start, 123);
        assert_eq!(count, 456);
    }

    #[test]
    fn parse_range_without_count() {
        let (start, count) = parse_hunk_range("+42", 1, "new").unwrap();
        assert_eq!(start, 42);
        assert_eq!(count, 1);
    }

    #[test]
    fn parse_range_zero_count() {
        let (start, count) = parse_hunk_range("-0,0", 1, "old").unwrap();
        assert_eq!(start, 0);
        assert_eq!(count, 0);
    }

    #[test]
    fn parse_range_invalid_start() {
        let err = parse_hunk_range("-abc", 1, "old").unwrap_err();
        assert!(matches!(err, PatchError::MalformedHunkHeader(_, _)));
    }

    // -- Hunk matching --

    #[test]
    fn hunk_matches_exact() {
        let target = vec![
            "line 1".to_string(),
            "line 2".to_string(),
            "line 3".to_string(),
        ];
        let hunk = PatchHunk {
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 3,
            lines: vec![
                HunkLine::Context("line 1".to_string()),
                HunkLine::Remove,
                HunkLine::Add("line 2 modified".to_string()),
                HunkLine::Context("line 3".to_string()),
            ],
        };
        assert!(match_hunk(&target, &hunk, 0));
    }

    #[test]
    fn hunk_mismatches_context() {
        let target = vec![
            "line 1".to_string(),
            "WRONG".to_string(),
            "line 3".to_string(),
        ];
        let hunk = PatchHunk {
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 3,
            lines: vec![
                HunkLine::Context("line 1".to_string()),
                HunkLine::Context("line 2".to_string()),
                HunkLine::Context("line 3".to_string()),
            ],
        };
        assert!(!match_hunk(&target, &hunk, 0));
    }

    #[test]
    fn hunk_out_of_bounds() {
        let target = vec!["line 1".to_string()];
        let hunk = PatchHunk {
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec![HunkLine::Context("line 1".to_string())],
        };
        assert!(!match_hunk(&target, &hunk, 10));
    }

    // -- Display tests --

    #[test]
    fn patch_result_display_applied() {
        let result = PatchResult::Applied;
        assert_eq!(format!("{result}"), "patch applied cleanly");
    }

    #[test]
    fn patch_result_display_hunk_failed() {
        let result =
            PatchResult::HunkFailed(vec![(0, "context mismatch".to_string())]);
        let display = format!("{result}");
        assert!(display.contains("1 hunk(s) failed"));
        assert!(display.contains("context mismatch"));
    }

    #[test]
    fn patch_result_display_fuzz_applied() {
        let result = PatchResult::FuzzApplied(vec![0, 2]);
        let display = format!("{result}");
        assert!(display.contains("fuzz"));
    }

    // -- Change range collection --

    fn to_raw_changes<'a>(
        changes: Vec<diff::Result<&&'a str>>,
    ) -> Vec<diff::Result<&'a str>> {
        changes
            .into_iter()
            .map(|c| match c {
                diff::Result::Left(s) => diff::Result::Left(*s),
                diff::Result::Right(s) => diff::Result::Right(*s),
                diff::Result::Both(a, b) => diff::Result::Both(*a, *b),
            })
            .collect()
    }

    #[test]
    fn collect_ranges_identical() {
        let left = vec!["a", "b", "c"];
        let right = vec!["a", "b", "c"];
        let changes = to_raw_changes(diff::slice(&left, &right));
        let ranges = collect_change_ranges(&changes);
        assert!(ranges.is_empty());
    }

    #[test]
    fn collect_ranges_single_change() {
        let left = vec!["a", "b", "c"];
        let right = vec!["a", "X", "c"];
        let changes = to_raw_changes(diff::slice(&left, &right));
        let ranges = collect_change_ranges(&changes);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn collect_ranges_addition() {
        let left = vec!["a", "c"];
        let right = vec!["a", "b", "c"];
        let changes = to_raw_changes(diff::slice(&left, &right));
        let ranges = collect_change_ranges(&changes);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn collect_ranges_removal() {
        let left = vec!["a", "b", "c"];
        let right = vec!["a", "c"];
        let changes = to_raw_changes(diff::slice(&left, &right));
        let ranges = collect_change_ranges(&changes);
        assert_eq!(ranges.len(), 1);
    }

    #[test]
    fn collect_ranges_multiple_blocks() {
        let left = vec!["a", "b", "c", "d", "e"];
        let right = vec!["a", "X", "c", "Y", "e"];
        let changes = to_raw_changes(diff::slice(&left, &right));
        let ranges = collect_change_ranges(&changes);
        // Two separate change blocks (b->X and d->Y).
        assert_eq!(ranges.len(), 2);
    }
}
