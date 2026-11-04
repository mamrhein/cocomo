// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{collections::HashMap, io::Write};

use cocomo_lib::compare::{DirEntry, DirEntryStatus};

use super::filter::{should_show_entry, should_show_in_tree};
use crate::cli::{DirCompareArgs, SummaryMode};

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

/// Per-status counts computed from the entries that are actually visible in
/// the output, after applying all display filters.
pub(super) struct Summary {
    by_status: HashMap<DirEntryStatus, usize>,
}

impl Summary {
    /// Return the total number of visible entries.
    pub(super) fn total(&self) -> usize {
        self.by_status.values().sum()
    }

    /// Return the number of visible entries whose status bitmap
    /// intersects `mask`.
    fn count_matching(&self, mask: DirEntryStatus) -> usize {
        self.by_status
            .iter()
            .filter(|(status, _)| status.intersects(mask))
            .map(|(_, count)| *count)
            .sum()
    }

    /// Return the number of visible entries whose sides are identical or
    /// equal.
    pub(super) fn same_count(&self) -> usize {
        self.count_matching(DirEntryStatus::EQUAL)
    }

    /// Return the number of visible entries whose sides differ.
    pub(super) fn different_count(&self) -> usize {
        self.count_matching(DirEntryStatus::DIFFERENT)
    }

    /// Return the number of visible orphan entries.
    pub(super) fn orphan_count(&self) -> usize {
        self.count_matching(DirEntryStatus::ORPHAN)
    }

    /// Return the count for a specific status, or zero if absent.
    pub(super) fn get(&self, status: DirEntryStatus) -> usize {
        self.by_status.get(&status).copied().unwrap_or(0)
    }
}

/// Flatten the entry tree, apply display filters, and count visible entries
/// in a single recursive pass.
///
/// Returns a flat `Vec` of visible entries (for flat output modes) and a
/// [`Summary`] with per-status counts.
///
/// When `use_tree_filter` is `true` (text tree output), only visible
/// directories are descended into, matching the behavior of
/// [`render_tree_entries`]. When `false` (CSV, JSON), all subdirectories are
/// always visited, so that children of filtered-out directories are still
/// counted if they pass the filter themselves.
///
/// `args.max_depth` (1-based, the top level being depth 1) restricts which
/// entries are collected; the comparison itself always covers the full tree.
pub(super) fn collect_and_count(
    entries: &[DirEntry],
    args: &DirCompareArgs,
    use_tree_filter: bool,
) -> (Vec<DirEntry>, Summary) {
    let mut visible = Vec::new();
    let mut by_status = HashMap::new();
    collect_and_count_into(
        entries,
        1,
        args,
        use_tree_filter,
        &mut visible,
        &mut by_status,
    );
    (visible, Summary { by_status })
}

/// Recursive worker for [`collect_and_count`]. `depth` is the 1-based depth
/// of `entries` in the tree.
pub(super) fn collect_and_count_into(
    entries: &[DirEntry],
    depth: usize,
    args: &DirCompareArgs,
    use_tree_filter: bool,
    visible: &mut Vec<DirEntry>,
    by_status: &mut HashMap<DirEntryStatus, usize>,
) {
    // Entries deeper than the max output depth are neither collected nor
    // counted.
    if args.max_depth.is_some_and(|max| depth > max) {
        return;
    }
    let descend = args.max_depth.is_none_or(|max| depth < max);

    for entry in entries {
        let is_visible = if use_tree_filter {
            should_show_in_tree(entry, args)
        } else {
            should_show_entry(entry, args)
        };

        if is_visible {
            visible.push(entry.clone());
            *by_status.entry(entry.status).or_insert(0) += 1;
        }

        // Recurse into subdirectories.
        // Tree mode: only descend into visible directories (matching
        // render_tree_entries). Flat mode: always descend, since children
        // are shown independently of parent visibility.
        if descend
            && let Some(sub) = &entry.sub_entries
            && (!use_tree_filter
                || (is_visible && super::filter::is_dir_entry(entry)))
        {
            collect_and_count_into(
                &sub.entries,
                depth + 1,
                args,
                use_tree_filter,
                visible,
                by_status,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Summary printing
// ---------------------------------------------------------------------------

pub(super) fn print_summary(summary: &Summary, mode: SummaryMode) {
    let mut stdout = std::io::stdout();
    print_summary_to(summary, mode, &mut stdout);
}

/// Like [`print_summary`] but writes to the given writer, enabling unit tests.
pub(super) fn print_summary_to(
    summary: &Summary,
    mode: SummaryMode,
    output: &mut dyn Write,
) {
    writeln!(output,).unwrap();
    match mode {
        SummaryMode::Brief => {
            writeln!(
                output,
                "Summary: {} total, {} same, {} different, {} orphans",
                summary.total(),
                summary.same_count(),
                summary.different_count(),
                summary.orphan_count(),
            )
            .unwrap();
        }
        SummaryMode::Full => {
            print_full_summary_to(summary, output);
        }
    }
}

/// Print a per-status breakdown table with counts and percentages.
pub(super) fn print_full_summary_to(
    summary: &Summary,
    output: &mut dyn Write,
) {
    let total = summary.total();
    writeln!(output, "Summary breakdown:").unwrap();
    // The percentage column is right-aligned in a fixed 10-character field so
    // that the header label and all data values share the same right edge.
    writeln!(
        output,
        "  {:<20} {:>6}   {:>10}",
        "Status", "Count", "% of total"
    )
    .unwrap();
    let mut statuses: Vec<DirEntryStatus> =
        summary.by_status.keys().copied().collect();
    statuses.sort();
    for status in statuses {
        let count = summary.get(status);
        if count > 0 {
            let pct = if total > 0 {
                (count as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            let pct_str = format!("{:.1}%", pct);
            // Convert to String: Rust's {:<N} padding only works for
            // &str/String (pad_str fast path), not custom Display
            // types whose fmt() uses write!() without f.pad(). See
            // Rust std::fmt internals.
            let status_str = status.to_string();
            writeln!(
                output,
                "  {:<20} {:>6}   {:>10}",
                status_str, count, pct_str
            )
            .unwrap();
        }
    }
    // Separator spans the status (20) + space (1) + count (6) = 27 columns.
    let sep = "─".repeat(27);
    writeln!(output, "  {sep}").unwrap();
    let total_pct = format!("{:.1}%", 100.0);
    let total_line =
        format!("  {:<20} {:>6}   {:>10}", "Total", total, total_pct);
    writeln!(output, "{}", total_line).unwrap();
}

/// Wrapper around [`print_summary_to`] that writes to a `Vec<u8>` buffer,
/// enabling unit testing of summary output without touching stdout.
#[cfg(test)]
pub(super) fn print_summary_to_vec(
    summary: &Summary,
    mode: SummaryMode,
    buf: &mut Vec<u8>,
) {
    print_summary_to(summary, mode, buf);
}

#[cfg(test)]
mod tests {
    use super::{
        super::test_helpers::{
            make_nested_comparison, make_nested_comparison_with_hidden,
            make_test_comparison,
        },
        collect_and_count, print_summary_to_vec,
    };
    use crate::cli::{DirCompareArgs, SummaryMode};

    /// Build a `DirCompareArgs` with all flags set to their defaults, suitable
    /// for unit tests that need an args instance.
    fn test_args() -> DirCompareArgs {
        DirCompareArgs::test_defaults()
    }

    #[test]
    fn summary_brief_prints_aggregate_line() {
        let comp = make_test_comparison();
        let (_, summary) =
            collect_and_count(&comp.entries, &test_args(), false);
        let mut buf = Vec::new();
        print_summary_to_vec(&summary, SummaryMode::Brief, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Summary:"));
        assert!(output.contains("total"));
        assert!(output.contains("same"));
        assert!(output.contains("different"));
        assert!(output.contains("orphans"));
    }

    #[test]
    fn summary_full_prints_breakdown_table() {
        let comp = make_test_comparison();
        let (_, summary) =
            collect_and_count(&comp.entries, &test_args(), false);
        let mut buf = Vec::new();
        print_summary_to_vec(&summary, SummaryMode::Full, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Summary breakdown:"));
        assert!(output.contains("Status"));
        assert!(output.contains("Count"));
        // "equal" should appear with count 1 and 25.0%.
        assert!(output.contains("equal"));
        assert!(output.contains("25.0%"));
        // "Total" row should appear with 100.0%.
        assert!(output.contains("Total"));
    }

    #[test]
    fn summary_full_only_shows_nonzero_statuses() {
        let comp = make_test_comparison();
        let (_, summary) =
            collect_and_count(&comp.entries, &test_args(), false);
        let mut buf = Vec::new();
        print_summary_to_vec(&summary, SummaryMode::Full, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        // The fixture carries no `Same` bit, so "same" must not appear as a
        // status row.
        assert!(!output.contains("same"));
    }

    #[test]
    fn summary_reflects_show_different_filter() {
        // With --show-different, only entries carrying a newer-side bit
        // should be counted in the summary.
        let comp = make_test_comparison();
        let mut args = test_args();
        args.show_different = true;
        let (visible, summary) =
            collect_and_count(&comp.entries, &args, false);
        assert_eq!(visible.len(), 1); // only diff.txt (DIFFERENT)
        assert_eq!(summary.total(), 1);
        assert_eq!(summary.same_count(), 0);
        assert_eq!(summary.different_count(), 1);
        assert_eq!(summary.orphan_count(), 0);
    }

    #[test]
    fn summary_reflects_show_orphans_filter() {
        // With --show-orphans, only entries carrying an orphan bit should
        // be counted.
        let comp = make_test_comparison();
        let mut args = test_args();
        args.show_orphans = true;
        let (visible, summary) =
            collect_and_count(&comp.entries, &args, false);
        assert_eq!(visible.len(), 2); // only_left.txt + only_right.txt
        assert_eq!(summary.total(), 2);
        assert_eq!(summary.same_count(), 0);
        assert_eq!(summary.different_count(), 0);
        assert_eq!(summary.orphan_count(), 2);
    }

    #[test]
    fn summary_counts_nested_entries_recursively() {
        // The summary should count entries at all levels, not just the first
        // level of the compared directories.
        let comp = make_nested_comparison();
        let mut args = test_args();
        // Count the full tree, ignoring the default output depth limit.
        args.max_depth = None;
        let (visible, summary) =
            collect_and_count(&comp.entries, &args, false);
        // Root level: 1 dir + 1 file = 2 entries
        // Sub-level (sub/): 1 equal + 1 diff + 1 left orphan = 3 entries
        // Total: 5
        assert_eq!(visible.len(), 5);
        assert_eq!(summary.total(), 5);
        assert_eq!(summary.same_count(), 3); // sub/ + same.txt (root) + same.txt (sub)
        assert_eq!(summary.different_count(), 1); // diff.txt (sub)
        assert_eq!(summary.orphan_count(), 1); // only_left.txt (sub)
    }

    #[test]
    fn summary_tree_mode_counts_only_visible_dirs() {
        // In tree mode, directories are always shown but children of filtered
        // directories are not counted.
        let comp = make_nested_comparison();
        let mut args = test_args();
        // Count the full tree, ignoring the default output depth limit.
        args.max_depth = None;
        let (visible, summary) = collect_and_count(&comp.entries, &args, true);
        // Root level: 1 dir + 1 file = 2 entries (all visible with default
        // args) Sub-level (sub/): 3 entries
        // Total: 5
        assert_eq!(visible.len(), 5);
        assert_eq!(summary.total(), 5);
    }

    #[test]
    fn summary_tree_mode_excludes_children_of_filtered_dirs() {
        // With --excl-hidden, a hidden directory and its children should not
        // be counted in tree mode.
        let comp = make_nested_comparison_with_hidden();
        let mut args = test_args();
        args.excl_hidden = true;
        let (visible, summary) = collect_and_count(&comp.entries, &args, true);
        // Root: sub/ (visible, Equal) + visible.txt (visible, Equal) = 2
        // .hidden_dir/ is filtered out, so its children are not counted.
        assert!(!visible.iter().any(|e| e.name.starts_with('.')));
        assert_eq!(summary.total(), 2);
    }

    #[test]
    fn summary_flat_mode_includes_children_of_filtered_dirs() {
        // In flat mode, children of filtered directories ARE counted if they
        // pass the name filter themselves.
        let comp = make_nested_comparison_with_hidden();
        let mut args = test_args();
        args.excl_hidden = true;
        // Count the full tree, ignoring the default output depth limit.
        args.max_depth = None;
        let (_visible, summary) =
            collect_and_count(&comp.entries, &args, false);
        // Root: sub/ (Equal) + visible.txt (Equal) + .hidden_dir/ (filtered
        // out) .hidden_dir/child.txt (passes name filter, counted)
        assert_eq!(summary.total(), 3); // sub, visible.txt, child.txt
        assert_eq!(summary.same_count(), 3);
    }

    #[test]
    fn summary_respects_max_depth() {
        // The default output depth of 1 counts only the top level; a deeper
        // limit or none counts the nested children as well.
        let comp = make_nested_comparison();
        let mut args = test_args();
        args.max_depth = Some(1);
        let (visible, summary) =
            collect_and_count(&comp.entries, &args, false);
        // Root level only: the sub/ directory row + same.txt.
        assert_eq!(visible.len(), 2);
        assert_eq!(summary.total(), 2);

        args.max_depth = Some(2);
        let (_, summary) = collect_and_count(&comp.entries, &args, false);
        assert_eq!(summary.total(), 5);

        // The same holds for the tree filter mode.
        args.max_depth = Some(1);
        let (_, summary) = collect_and_count(&comp.entries, &args, true);
        assert_eq!(summary.total(), 2);
        args.max_depth = None;
        let (_, summary) = collect_and_count(&comp.entries, &args, true);
        assert_eq!(summary.total(), 5);
    }

    #[test]
    fn summary_full_table_lists_each_status_bitmap_once() {
        // The full breakdown iterates the observed exact bitmaps, sorted
        // by their raw value; each appears in exactly one row.
        let comp = make_test_comparison();
        let (_, summary) =
            collect_and_count(&comp.entries, &test_args(), false);
        let mut buf = Vec::new();
        print_summary_to_vec(&summary, SummaryMode::Full, &mut buf);
        let output = String::from_utf8(buf).unwrap();
        for status_name in [
            "equal",
            "left_newer,right_newer",
            "left_orphan",
            "right_orphan",
        ] {
            assert!(
                output.contains(status_name),
                "missing status row: {status_name}"
            );
        }
        // Every visible entry is counted in exactly one row.
        let row_count = output
            .lines()
            .filter(|line| {
                ["equal", "left_newer", "left_orphan", "right_orphan"]
                    .iter()
                    .any(|name| line.trim_start().starts_with(name))
            })
            .count();
        assert_eq!(row_count, summary.total());
    }
}
