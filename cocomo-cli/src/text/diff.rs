// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::path::Path;

use cocomo_lib::{TextDifference, text::TextDiff};

use super::truncate;

// ---------------------------------------------------------------------------
// Text diff rendering
// ---------------------------------------------------------------------------

pub(super) fn print_text_diff_table(
    diff: &TextDiff,
    left_name: &str,
    right_name: &str,
) {
    // Column layout adapted to the TUI's text main area:
    //   Num | marker | content | Num | marker | content
    // Widths: [7, 2, 60, 7, 2, 60] with 2-space gaps between groups.
    println!(
        "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
        "Num", "", left_name, "Num", "", right_name
    );
    println!(
        "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
        "----", "--", "", "----", "--", ""
    );

    for difference in &diff.differences {
        match difference {
            TextDifference::LineDifferent(left, right) => {
                println!(
                    "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
                    left.number,
                    "▶ ",
                    truncate(&left.content, 60),
                    right.number,
                    "▶ ",
                    truncate(&right.content, 60),
                );
            }
            TextDifference::LinesAdded(_start, lines) => {
                for line in lines {
                    println!(
                        "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
                        "-",
                        "  ",
                        "",
                        line.number,
                        "▶ ",
                        truncate(&line.content, 60),
                    );
                }
            }
            TextDifference::LinesRemoved(_start, lines) => {
                for line in lines {
                    println!(
                        "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
                        line.number,
                        "▶ ",
                        truncate(&line.content, 60),
                        "-",
                        "  ",
                        "",
                    );
                }
            }
            TextDifference::LinesChanged(_start, removed, added) => {
                for line in removed {
                    println!(
                        "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
                        line.number,
                        "▶ ",
                        truncate(&line.content, 60),
                        "-",
                        "  ",
                        "",
                    );
                }
                for line in added {
                    println!(
                        "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
                        "-",
                        "  ",
                        "",
                        line.number,
                        "▶ ",
                        truncate(&line.content, 60),
                    );
                }
            }
            TextDifference::BlankLine => {
                println!(
                    "{:<6}  {:<2} {:<60}  {:<6}  {:<2} {:<60}",
                    "-", "  ", "(blank)", "-", "  ", ""
                );
            }
        }
    }
}

pub(super) fn generate_unified_diff(
    left_path: &Path,
    right_path: &Path,
    left_content: &str,
    right_content: &str,
    diff: &TextDiff,
) -> String {
    use std::fmt::Write;

    let left_lines: Vec<&str> = left_content.lines().collect();
    let right_lines: Vec<&str> = right_content.lines().collect();

    let mut output = String::new();
    writeln!(
        output,
        "--- {}\n+++ {}",
        left_path.display(),
        right_path.display(),
    )
    .ok();

    // Walk the differences and emit hunks. We need context lines around
    // each change group.
    let context_lines = 3;

    // Build a mapping of which left/right lines are part of changes.
    let mut changed_left: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    let mut changed_right: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    for difference in &diff.differences {
        match difference {
            TextDifference::LineDifferent(left, right) => {
                changed_left.insert(left.number - 1);
                changed_right.insert(right.number - 1);
            }
            TextDifference::LinesAdded(_start, lines) => {
                for line in lines {
                    changed_right.insert(line.number - 1);
                }
            }
            TextDifference::LinesRemoved(_start, lines) => {
                for line in lines {
                    changed_left.insert(line.number - 1);
                }
            }
            TextDifference::LinesChanged(_start, removed, added) => {
                for line in removed {
                    changed_left.insert(line.number - 1);
                }
                for line in added {
                    changed_right.insert(line.number - 1);
                }
            }
            TextDifference::BlankLine => {
                // Blank lines between changes are context.
            }
        }
    }

    // Group into hunks based on left-side changes.
    let mut sorted_left: Vec<usize> = changed_left.iter().copied().collect();
    sorted_left.sort();

    let mut hunks: Vec<(usize, usize)> = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_end: Option<usize> = None;

    for idx in sorted_left {
        let ctx_start = idx.saturating_sub(context_lines);
        let ctx_end = idx + context_lines;

        match (&mut current_start, &mut current_end) {
            (Some(s), Some(e)) if ctx_start <= *e => {
                *e = ctx_end.min(left_lines.len());
            }
            _ => {
                if let (Some(s), Some(e)) =
                    (current_start.take(), current_end.take())
                {
                    hunks.push((s, e));
                }
                current_start = Some(ctx_start);
                current_end = Some(ctx_end.min(left_lines.len()));
            }
        }
    }

    if let (Some(s), Some(e)) = (current_start, current_end) {
        hunks.push((s, e));
    }

    // If no left-side changes (only additions), emit a single hunk at
    // the end.
    if hunks.is_empty() && !changed_right.is_empty() {
        hunks.push((left_lines.len(), left_lines.len()));
    }

    for (hunk_start, hunk_end) in &hunks {
        let left_start = *hunk_start;
        let left_count = *hunk_end - *hunk_start + (context_lines * 2);

        // Approximate the right range.
        let mut right_start = *hunk_start;
        let mut right_count = *hunk_end - *hunk_start + (context_lines * 2);

        // Adjust for pure additions at the end.
        if changed_left.is_empty() {
            right_start = right_lines.len().saturating_sub(context_lines);
            right_count = context_lines + changed_right.len();
        }

        writeln!(output, "@@ -{},+{} @@", left_start + 1, right_start + 1,)
            .ok();

        // Emit context and changed lines from the original text.
        for (i, line) in left_lines.iter().enumerate() {
            if i >= left_count {
                break;
            }
            if changed_left.contains(&i) {
                writeln!(output, "-{}", line).ok();
            } else if i >= hunk_start.saturating_sub(context_lines)
                && i < hunk_end + context_lines
            {
                writeln!(output, " {}", line).ok();
            }
        }

        for (i, line) in right_lines.iter().enumerate() {
            if changed_right.contains(&i) {
                writeln!(output, "+{}", line).ok();
            } else if i >= right_start.saturating_sub(context_lines)
                && i < right_start + right_count
            {
                // Only emit context that wasn't already emitted as left
                // context.
                if !changed_left.contains(&i) {
                    // Avoid duplicate context lines.
                }
            }
        }
    }

    output
}
