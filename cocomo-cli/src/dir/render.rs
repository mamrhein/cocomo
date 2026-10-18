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
        Colorizer, format_date_display, format_modified, format_size,
        terminal_width, truncate_name,
    },
    filter::{is_dir_entry, should_show_in_tree},
    summary::{Summary, count_visible_tree_children},
};
use crate::cli::DirCompareArgs;

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub(super) fn print_comparison_text(
    entries: &[DirEntry],
    args: &DirCompareArgs,
    output: &mut dyn Write,
) {
    let colorizer = Colorizer::new(args.color);

    if entries.is_empty() {
        writeln!(output, "No matching entries.").unwrap();
        return;
    }

    let visible: Vec<&DirEntry> = entries.iter().collect();

    let show_mtime = !args.no_mtime;

    // Calculate the dynamic name column width based on terminal size. The
    // fixed overhead is the inter-column spaces and all non-name columns.
    // When not on a TTY (piped/redirected output) we fall back to a 40-column
    // default so that CI and integration tests remain deterministic.
    // Column layout adapted to the TUI's main area:
    //   dir indicator + space before name = 2
    //   left metadata column (size + date) = 25 (space + 24)
    //   status symbol = 2 (space + symbol)
    //   right metadata column (size + date) = 25 (space + 24)
    let fixed_width = 2
        + 24  // space + left metadata (23)
        + 2   // space + status (2)
        + 24; // space + right metadata (23)
    let name_width = match terminal_width() {
        Some(term_w) => term_w.saturating_sub(fixed_width).max(10),
        None => 40,
    };

    // Combined header for size+date metadata column, matching the TUI's
    // layout (24 chars).
    let meta_header = if show_mtime {
        format!("{:^6} {:^16}", "Size", "Modified")
    } else {
        format!("{:^6}", "Size")
    };

    // Print header — adapted to the TUI's column layout:
    //   T Name | Size Modified | S | Size Modified
    let mut header = format!("{} {:<w$}", "T", "Name", w = name_width);
    header += &format!(" {:^23}", meta_header);
    header += &format!(" {}", "S");
    header += &format!(" {:^23}", meta_header);
    writeln!(output, "{}", header).unwrap();

    // Separator line matching column widths.
    let sep_dashes = "-".repeat(name_width);
    let mut sep = format!("{} {:<w$}", "-", sep_dashes, w = name_width);
    sep += &format!(" {:^23}", "-----------------------");
    sep += &format!(" {}", "-");
    sep += &format!(" {:^23}", "-----------------------");
    writeln!(output, "{}", sep).unwrap();

    for entry in &visible {
        let status_sym =
            colorizer.colorize(entry.status.symbol(), entry.status);
        let is_dir = is_dir_entry(entry);
        let display_name = entry.name.clone();
        let display_name = truncate_name(&display_name, name_width);

        // Format combined left metadata: "size date" (24-char column,
        // matching the TUI's `format!("{:>6} {}", size, date)`).
        let left_meta = entry
            .left
            .as_ref()
            .map(|l| {
                let size_str = format_size(l.size);
                let date_str = format_date_display(&l.modified);
                if show_mtime {
                    format!("{:>6} {}", size_str, date_str)
                } else {
                    format!("{:>6}", size_str)
                }
            })
            .unwrap_or_default();

        // Format combined right metadata.
        let right_meta = entry
            .right
            .as_ref()
            .map(|r| {
                let size_str = format_size(r.size);
                let date_str = format_date_display(&r.modified);
                if show_mtime {
                    format!("{:>6} {}", size_str, date_str)
                } else {
                    format!("{:>6}", size_str)
                }
            })
            .unwrap_or_default();

        // Directory type indicator (matching the TUI's 📁 for dirs).
        let dir_indicator = if is_dir { "\u{1F5C0}" } else { " " };
        let mut row =
            format!("{} {:<w$}", dir_indicator, display_name, w = name_width);

        row += &format!(" {:<23}", left_meta);
        row += &format!(" {}", status_sym);
        row += &format!(" {:<23}", right_meta);

        writeln!(output, "{}", row).unwrap();
    }
}

/// Recursively render a directory comparison as an indented tree using
/// box-drawing characters, with optional ANSI color on the status symbols.
pub(super) fn render_tree_entries(
    entries: &[DirEntry],
    prefix: &str,
    output: &mut dyn Write,
    colorizer: &Colorizer,
    args: &DirCompareArgs,
    name_width: usize,
) {
    let visible: Vec<&DirEntry> = entries
        .iter()
        .filter(|e| should_show_in_tree(e, args))
        .collect();

    let show_mtime = !args.no_mtime;
    let prefix_len = prefix.chars().count();
    // Total name area = name_width + 2 (matching flat output).
    // Within the name area: prefix + connector (2) + space (1) + name_field.
    let name_field_width = name_width
        .saturating_sub(prefix_len)
        .saturating_sub(1)
        .max(1);

    let count = visible.len();
    for (i, entry) in visible.iter().enumerate() {
        let is_last = i == count - 1;
        let connector = if is_last {
            "\u{2514}\u{2500}" // └─
        } else {
            "\u{251c}\u{2500}" // ├─
        };

        let is_dir = is_dir_entry(entry);

        // Build display name (including "/" for directories and child count).
        let display_name = if is_dir {
            if let Some(sub) = &entry.sub_entries {
                let child_count =
                    count_visible_tree_children(&sub.entries, args);
                format!("{}/ ({})", entry.name, child_count)
            } else {
                format!("{}/", entry.name)
            }
        } else {
            entry.name.clone()
        };
        let display_name = truncate_name(&display_name, name_field_width);

        // Format metadata columns (matching flat output format).
        let left_meta = entry
            .left
            .as_ref()
            .map(|l| {
                let size_str = format_size(l.size);
                let date_str = format_date_display(&l.modified);
                if show_mtime {
                    format!("{:>6} {}", size_str, date_str)
                } else {
                    format!("{:>6}", size_str)
                }
            })
            .unwrap_or_default();

        let right_meta = entry
            .right
            .as_ref()
            .map(|r| {
                let size_str = format_size(r.size);
                let date_str = format_date_display(&r.modified);
                if show_mtime {
                    format!("{:>6} {}", size_str, date_str)
                } else {
                    format!("{:>6}", size_str)
                }
            })
            .unwrap_or_default();

        let status_sym =
            colorizer.colorize(entry.status.symbol(), entry.status);

        let mut row = format!(
            "{prefix}{connector} {:<w$}",
            display_name,
            w = name_field_width
        );
        row += &format!(" {:<23}", left_meta);
        row += &format!(" {}", status_sym);
        row += &format!(" {:<23}", right_meta);
        writeln!(output, "{}", row).unwrap();

        // Recurse into subdirectories.
        if is_dir && let Some(sub) = &entry.sub_entries {
            let child_prefix = format!(
                "{prefix}{}",
                if is_last { "  " } else { "\u{2502} " }
            );
            render_tree_entries(
                &sub.entries,
                &child_prefix,
                output,
                colorizer,
                args,
                name_width,
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
    buf: &mut Vec<u8>,
    colorizer: &Colorizer,
    args: &DirCompareArgs,
    name_width: usize,
) {
    render_tree_entries(entries, prefix, buf, colorizer, args, name_width);
}

/// Wrapper around [`print_comparison_text`] that writes to a `Vec<u8>` buffer,
/// enabling unit testing of text output without touching stdout.
#[cfg(test)]
pub(super) fn render_comparison_text_to_vec(
    entries: &[DirEntry],
    args: &DirCompareArgs,
    buf: &mut Vec<u8>,
) {
    print_comparison_text(entries, args, buf);
}

/// Print the comparison result as a hierarchical tree (text format only).
pub(super) fn print_comparison_tree(
    comparison: &cocomo_lib::compare::DirComparison,
    args: &DirCompareArgs,
) {
    let colorizer = Colorizer::new(args.color);
    let mut stdout = std::io::stdout();

    let show_mtime = !args.no_mtime;
    // Fixed overhead: dir indicator + space (2) + left metadata (24) +
    // status (2) + right metadata (24) = 52. No path column in tree mode.
    let fixed_width = 2 + 24 + 2 + 24;
    let name_width = match terminal_width() {
        Some(term_w) => term_w.saturating_sub(fixed_width).max(10),
        None => 40,
    };

    // Header row — matching the flat output format, without the "T" type
    // column (tree mode shows directory types via "/" suffix).
    let meta_header = if show_mtime {
        format!("{:^6} {:^16}", "Size", "Modified")
    } else {
        format!("{:^6}", "Size")
    };
    let mut header = format!("{:<w$}", "Name", w = name_width + 2);
    header += &format!(" {:^23}", meta_header);
    header += &format!(" {}", "S");
    header += &format!(" {:^23}", meta_header);
    writeln!(stdout, "{}", header).unwrap();

    // Separator line matching column widths.
    let mut sep = "-".repeat(name_width + 2);
    sep += &format!(" {:^23}", "-----------------------");
    sep += &format!(" {}", "-");
    sep += &format!(" {:^23}", "-----------------------");
    writeln!(stdout, "{}", sep).unwrap();

    // Root of the tree.
    writeln!(stdout, ".").unwrap();

    render_tree_entries(
        &comparison.entries,
        "",
        &mut stdout,
        &colorizer,
        args,
        name_width,
    );
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
    use super::{
        super::{display::Colorizer, test_helpers::make_test_comparison},
        render_comparison_text_to_vec, render_tree_entries_to_vec,
    };
    use crate::cli::{ColorMode, DirCompareArgs};

    /// Build a `DirCompareArgs` with all flags set to their defaults, suitable
    /// for unit tests that need an args instance.
    fn test_args() -> DirCompareArgs {
        DirCompareArgs::test_defaults()
    }

    #[test]
    fn tree_rendering_produces_box_drawing_chars() {
        let comp = make_test_comparison();
        let mut buf = Vec::new();
        render_tree_entries_to_vec(
            &comp.entries,
            "",
            &mut buf,
            &Colorizer::new(ColorMode::Never),
            &test_args(),
            40,
        );
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains('\u{251c}')); // ├
        assert!(output.contains('\u{2514}')); // └
        assert!(output.contains("same.txt"));
        assert!(output.contains("diff.txt"));
        assert!(output.contains("only_left.txt"));
        assert!(output.contains("only_right.txt"));
        // Metadata columns should be present (size + modified date).
        assert!(output.contains("2024-03-15"));
    }

    #[test]
    fn tree_rendering_with_color_includes_ansi() {
        let comp = make_test_comparison();
        let mut buf = Vec::new();
        render_tree_entries_to_vec(
            &comp.entries,
            "",
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
    fn text_mtime_columns_by_default() {
        let comp = make_test_comparison();
        let mut args = test_args();
        args.tree = false;
        let mut buf = Vec::new();
        render_comparison_text_to_vec(&comp.entries, &args, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Size"));
        assert!(output.contains("Modified"));
    }

    #[test]
    fn text_no_mtime_hides_timestamp_columns() {
        let comp = make_test_comparison();
        let mut args = test_args();
        args.tree = false;
        args.no_mtime = true;
        let mut buf = Vec::new();
        render_comparison_text_to_vec(&comp.entries, &args, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(!output.contains("Modified"));
    }

    #[test]
    fn text_mtime_displays_formatted_rfc3339() {
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
        let mut args = test_args();
        args.tree = false;
        let mut buf = Vec::new();
        render_comparison_text_to_vec(&comp.entries, &args, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        // The formatted date should appear (truncated to 16 chars to match
        // the TUI's column width), not the raw RFC 3339 string.
        assert!(output.contains("2024-03-15 14:30"));
        assert!(!output.contains("2024-03-15T14:30:00+00:00"));
    }
}
