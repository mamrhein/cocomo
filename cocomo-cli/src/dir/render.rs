// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::io::Write;

use cocomo_lib::compare::DirEntry;

use super::{
    display::{
        Colorizer, format_modified, format_size, terminal_width, truncate_name,
    },
    filter::{is_dir_entry, should_show_in_tree},
    sort::sort_entry_refs,
    summary::Summary,
};
use crate::cli::DirCompareArgs;

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const SIZE_COL_WIDTH: usize = 6;
const MODIFIED_COL_WIDTH: usize = 19;
const META_DATA_WIDTH: usize = SIZE_COL_WIDTH + 1 + MODIFIED_COL_WIDTH;

/// Recursively render a directory comparison as an indented tree using
/// box-drawing characters, with optional ANSI color on the status symbols.
///
/// The top-most level (`depth == 0`) is rendered flush left without any tree
/// indicator; deeper levels are prefixed with a 3-character indicator
/// (`├─ `, `└─ ` or the continuation `│  ` / `   `).
///
/// When `args.sort` is set, the entries of every level are sorted by the
/// given field before rendering.
pub(super) fn render_tree_entries(
    entries: &[DirEntry],
    prefix: &str,
    depth: usize,
    output: &mut dyn Write,
    colorizer: &Colorizer,
    args: &DirCompareArgs,
    name_col_width: usize,
) {
    let mut visible: Vec<&DirEntry> = entries
        .iter()
        .filter(|e| should_show_in_tree(e, args))
        .collect();
    if let Some(sort) = args.sort {
        sort_entry_refs(&mut visible, sort);
    }

    let prefix_len = prefix.chars().count();
    let indicator_width = usize::from(depth > 0) * 3;
    let name_field_width = name_col_width
        .saturating_sub(prefix_len)
        .saturating_sub(indicator_width)
        .max(1);

    let count = visible.len();
    for (i, entry) in visible.iter().enumerate() {
        let is_last = i == count - 1;
        // Tree indicator: 2 box-drawing chars + 1 space; omitted entirely at
        // the top-most level.
        let indicator = if depth == 0 {
            ""
        } else if is_last {
            "\u{2514}\u{2500} " // └─
        } else {
            "\u{251c}\u{2500} " // ├─
        };

        let is_dir = is_dir_entry(entry);

        // Build display name (directories are marked with a "/" suffix).
        let display_name = if is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let display_name = truncate_name(&display_name, name_field_width);

        // Format metadata columns.
        let left_meta = entry
            .left
            .as_ref()
            .map(|l| {
                let size_str = format_size(l.size);
                let date_str = format_modified(&l.modified);
                format!(
                    "{size_str:>0$} {date_str:<1$}",
                    SIZE_COL_WIDTH, MODIFIED_COL_WIDTH
                )
            })
            .unwrap_or_default();

        let right_meta = entry
            .right
            .as_ref()
            .map(|r| {
                let size_str = format_size(r.size);
                let date_str = format_modified(&r.modified);
                format!(
                    "{size_str:>0$} {date_str:<1$}",
                    SIZE_COL_WIDTH, MODIFIED_COL_WIDTH
                )
            })
            .unwrap_or_default();

        let status_sym =
            colorizer.colorize(entry.status.symbol(), entry.status);

        let row = format!(
            "{}{}{:<3$} {left_meta:<4$} {status_sym} {right_meta:<4$}",
            prefix, indicator, display_name, name_field_width, META_DATA_WIDTH
        );
        writeln!(output, "{}", row).unwrap();

        // Recurse into subdirectories.
        if is_dir && let Some(sub) = &entry.sub_entries {
            // Continuation prefix, as wide as the indicator column; empty at
            // the top-most level since it has no indicator column.
            let continuation = if depth == 0 {
                ""
            } else if is_last {
                "   "
            } else {
                "\u{2502}  " // │ + 2 spaces
            };
            let child_prefix = format!("{prefix}{continuation}");
            render_tree_entries(
                &sub.entries,
                &child_prefix,
                depth + 1,
                output,
                colorizer,
                args,
                name_col_width,
            );
        }
    }
}

/// Wrapper around [`render_tree_entries`] that writes to a `Vec<u8>` buffer,
/// enabling unit testing of tree output without touching stdout.
#[cfg(test)]
pub(super) fn render_tree_entries_to_vec(
    entries: &[DirEntry],
    prefix: &str,
    depth: usize,
    buf: &mut Vec<u8>,
    colorizer: &Colorizer,
    args: &DirCompareArgs,
    name_width: usize,
) {
    render_tree_entries(
        entries, prefix, depth, buf, colorizer, args, name_width,
    );
}

/// Render the comparison result as a hierarchical tree to `output`.
///
/// Prints the column header, then the entry tree. When no entry passes the
/// display filters, a single "No matching entries." line is printed instead.
fn render_comparison_tree(
    comparison: &cocomo_lib::compare::DirComparison,
    args: &DirCompareArgs,
    output: &mut dyn Write,
) {
    // An empty top level means nothing can be visible at deeper levels
    // either, since recursion only descends into visible directories.
    if !comparison
        .entries
        .iter()
        .any(|e| should_show_in_tree(e, args))
    {
        writeln!(output, "No matching entries.").unwrap();
        return;
    }

    let colorizer = Colorizer::new(args.color);

    // Fixed overhead: space + left metadata + space + status + space + right
    // metadata
    let fixed_width = 2 * META_DATA_WIDTH + 4;
    let name_col_width = match terminal_width() {
        Some(term_w) => term_w.saturating_sub(fixed_width).max(10),
        None => 40,
    };

    // Header row
    let meta_header = format!(
        "{:^2$} {:^3$}",
        "Size", "Modified", SIZE_COL_WIDTH, MODIFIED_COL_WIDTH
    );
    let header = format!(
        "{:<1$} {meta_header} S {meta_header}",
        "Name", name_col_width
    );
    writeln!(output, "{}", header).unwrap();

    // Separator line matching column widths.
    let sep = "─".repeat(terminal_width().unwrap_or(40));
    writeln!(output, "{}", sep).unwrap();

    render_tree_entries(
        &comparison.entries,
        "",
        0,
        output,
        &colorizer,
        args,
        name_col_width,
    );
}

/// Print the comparison result as a hierarchical tree (the only text
/// output form).
pub(super) fn print_comparison_tree(
    comparison: &cocomo_lib::compare::DirComparison,
    args: &DirCompareArgs,
) {
    let mut stdout = std::io::stdout();
    render_comparison_tree(comparison, args, &mut stdout);
}

/// Wrapper around [`render_comparison_tree`] that writes to a `Vec<u8>`
/// buffer, enabling unit testing of the full tree output (including the
/// header) without touching stdout.
#[cfg(test)]
pub(super) fn render_comparison_tree_to_vec(
    comparison: &cocomo_lib::compare::DirComparison,
    args: &DirCompareArgs,
    buf: &mut Vec<u8>,
) {
    render_comparison_tree(comparison, args, buf);
}

pub(super) fn print_comparison_csv(entries: &[DirEntry]) {
    // Header row with is_dir column (parity with the report module's CSV).
    println!(
        "status,name,is_dir,left_size,right_size,left_modified,
         right_modified"
    );

    for entry in entries {
        let is_dir = entry
            .left
            .as_ref()
            .map(|l| l.is_dir)
            .or_else(|| entry.right.as_ref().map(|r| r.is_dir))
            .unwrap_or(false);

        let left_size = entry
            .left
            .as_ref()
            .map(|l| l.size.to_string())
            .unwrap_or_default();
        let right_size = entry
            .right
            .as_ref()
            .map(|r| r.size.to_string())
            .unwrap_or_default();
        let left_modified = entry
            .left
            .as_ref()
            .map(|l| format_modified(&l.modified))
            .unwrap_or_default();
        let right_modified = entry
            .right
            .as_ref()
            .map(|r| format_modified(&r.modified))
            .unwrap_or_default();

        // Escape fields that may contain commas.
        let name = escape_csv_field(&entry.name);
        println!(
            "{},{},{},{},{},{},{}",
            entry.status,
            name,
            is_dir,
            left_size,
            right_size,
            escape_csv_field(&left_modified),
            escape_csv_field(&right_modified),
        );
    }
}

pub(super) fn escape_csv_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

pub(super) fn print_comparison_json(entries: &[DirEntry], summary: &Summary) {
    let json_entries: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "status".into(),
                serde_json::Value::String(entry.status.to_string()),
            );
            obj.insert("name".into(), serde_json::json!(entry.name));
            obj.insert(
                "is_dir".into(),
                serde_json::json!(
                    entry.left.as_ref().map(|l| l.is_dir).unwrap_or(false)
                ),
            );

            if let Some(ref left) = entry.left {
                let mut lo = serde_json::Map::new();
                lo.insert("size".into(), serde_json::json!(left.size));
                lo.insert("modified".into(), serde_json::json!(left.modified));
                lo.insert("path".into(), serde_json::json!(left.path));
                if let Some(ref h) = left.hash {
                    lo.insert("hash".into(), serde_json::json!(h));
                }
                obj.insert("left".into(), serde_json::Value::Object(lo));
            } else {
                obj.insert("left".into(), serde_json::Value::Null);
            }

            if let Some(ref right) = entry.right {
                let mut ro = serde_json::Map::new();
                ro.insert("size".into(), serde_json::json!(right.size));
                ro.insert(
                    "modified".into(),
                    serde_json::json!(right.modified),
                );
                ro.insert("path".into(), serde_json::json!(right.path));
                if let Some(ref h) = right.hash {
                    ro.insert("hash".into(), serde_json::json!(h));
                }
                obj.insert("right".into(), serde_json::Value::Object(ro));
            } else {
                obj.insert("right".into(), serde_json::Value::Null);
            }

            serde_json::Value::Object(obj)
        })
        .collect();

    let summary_json = serde_json::json!({
        "total": summary.total(),
        "same": summary.same_count(),
        "different": summary.different_count(),
        "orphans": summary.orphan_count(),
    });

    let output = serde_json::json!({
        "summary": summary_json,
        "entries": json_entries,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&output).unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc};

    use cocomo_lib::compare::{
        DirComparison, DirEntry, DirEntryStatus, EntryInfo,
    };

    use super::{
        super::{
            display::Colorizer,
            test_helpers::{make_nested_comparison, make_test_comparison},
        },
        render_comparison_tree_to_vec, render_tree_entries_to_vec,
    };
    use crate::cli::{ColorMode, DirCompareArgs, SortField};

    /// Build a `DirCompareArgs` with all flags set to their defaults, suitable
    /// for unit tests that need an args instance.
    fn test_args() -> DirCompareArgs {
        DirCompareArgs::test_defaults()
    }

    #[test]
    fn tree_rendering_produces_box_drawing_chars() {
        // Only nested entries carry tree indicators, so use a fixture with a
        // subdirectory.
        let comp = make_nested_comparison();
        let mut buf = Vec::new();
        render_tree_entries_to_vec(
            &comp.entries,
            "",
            0,
            &mut buf,
            &Colorizer::new(ColorMode::Never),
            &test_args(),
            40,
        );
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains('\u{251c}')); // ├
        assert!(output.contains('\u{2514}')); // └
        assert!(output.contains("sub/"));
        assert!(output.contains("same.txt"));
        assert!(output.contains("diff.txt"));
        assert!(output.contains("only_left.txt"));
        // Metadata columns should be present (size + modified date).
        assert!(output.contains("2024-03-15"));
    }

    #[test]
    fn tree_rendering_top_level_has_no_indicator() {
        // The top-most level is rendered flush left, without any tree
        // indicator.
        let comp = make_test_comparison();
        let mut buf = Vec::new();
        render_tree_entries_to_vec(
            &comp.entries,
            "",
            0,
            &mut buf,
            &Colorizer::new(ColorMode::Never),
            &test_args(),
            40,
        );
        let output = String::from_utf8(buf).unwrap();
        assert!(!output.contains('\u{251c}')); // no ├
        assert!(!output.contains('\u{2514}')); // no └
        assert!(!output.contains('\u{2502}')); // no │
        assert!(output.starts_with("same.txt"));
    }

    #[test]
    fn tree_rendering_indicator_width_matches_example() {
        // Build the fixture
        //     level 1a
        //     ├─ level 2a
        //     ├─ level 2b
        //     │  └─ level 3a
        //     └─ level 2c
        //     level 1b
        // and verify the indicator column: 2 box-drawing chars + 1 space at
        // every level below the top-most.
        let info = |name: &str, is_dir: bool| EntryInfo {
            name: name.to_string(),
            path: name.to_string(),
            size: 0,
            modified: String::new(),
            is_dir,
            hash: None,
        };
        let file = |name: &str| DirEntry {
            name: name.to_string(),
            status: DirEntryStatus::Same,
            left: Some(info(name, false)),
            right: Some(info(name, false)),
            center: None,
            sub_entries: None,
        };
        let dir = |name: &str, children: Vec<DirEntry>| DirEntry {
            name: name.to_string(),
            status: DirEntryStatus::Same,
            left: Some(info(name, true)),
            right: Some(info(name, true)),
            center: None,
            sub_entries: Some(Arc::new(DirComparison {
                entries: children,
                counts: HashMap::new(),
                errors: vec![],
            })),
        };
        let comp = DirComparison {
            entries: vec![
                dir(
                    "level 1a",
                    vec![
                        file("level 2a"),
                        dir("level 2b", vec![file("level 3a")]),
                        file("level 2c"),
                    ],
                ),
                file("level 1b"),
            ],
            counts: HashMap::new(),
            errors: vec![],
        };
        let mut buf = Vec::new();
        render_tree_entries_to_vec(
            &comp.entries,
            "",
            0,
            &mut buf,
            &Colorizer::new(ColorMode::Never),
            &test_args(),
            40,
        );
        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 6);
        assert!(lines[0].starts_with("level 1a/"));
        assert!(lines[1].starts_with("\u{251c}\u{2500} level 2a")); // ├─
        assert!(lines[2].starts_with("\u{251c}\u{2500} level 2b")); // ├─
        // Continuation (│ + 2 spaces) aligns with the 3-char indicator.
        assert!(lines[3].starts_with("\u{2502}  \u{2514}\u{2500} level 3a"));
        assert!(lines[4].starts_with("\u{2514}\u{2500} level 2c")); // └─
        assert!(lines[5].starts_with("level 1b"));
    }

    #[test]
    fn tree_rendering_with_color_includes_ansi() {
        let comp = make_test_comparison();
        let mut buf = Vec::new();
        render_tree_entries_to_vec(
            &comp.entries,
            "",
            0,
            &mut buf,
            &Colorizer::new(ColorMode::Always),
            &test_args(),
            40,
        );
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\x1b[32m")); // green for Same
        assert!(output.contains("\x1b[35m")); // magenta for RightOnly
    }

    #[test]
    fn tree_mtime_displays_formatted_rfc3339() {
        let mut comp = make_test_comparison();
        // Replace with RFC 3339 timestamps to verify formatting in output.
        for entry in &mut comp.entries {
            if let Some(left) = &mut entry.left {
                left.modified = "2024-03-15T14:30:00+00:00".to_string();
            }
            if let Some(right) = &mut entry.right {
                right.modified = "2024-03-15T14:30:00+00:00".to_string();
            }
        }
        let mut buf = Vec::new();
        render_comparison_tree_to_vec(&comp, &test_args(), &mut buf);
        let output = String::from_utf8(buf).unwrap();
        // The formatted date should appear (truncated to 16 chars to match
        // the TUI's column width), not the raw RFC 3339 string.
        assert!(output.contains("2024-03-15 14:30"));
        assert!(!output.contains("2024-03-15T14:30:00+00:00"));
    }

    #[test]
    fn tree_empty_output_prints_no_matching_entries() {
        let comp = make_test_comparison();
        let mut args = test_args();
        args.filter = Some("nonexistent*".to_string());
        let mut buf = Vec::new();
        render_comparison_tree_to_vec(&comp, &args, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output, "No matching entries.\n");
    }

    #[test]
    fn tree_sort_by_name_orders_entries_per_level() {
        // Default scan order is same, diff, only_left, only_right; sorting by
        // name must reorder them within the rendered level.
        let comp = make_test_comparison();
        let mut args = test_args();
        args.sort = Some(SortField::Name);
        let mut buf = Vec::new();
        render_comparison_tree_to_vec(&comp, &args, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        let names: Vec<&str> = output
            .lines()
            .skip(2) // header + separator
            .map(|line| line.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["diff.txt", "only_left.txt", "only_right.txt", "same.txt"]
        );
    }
}
