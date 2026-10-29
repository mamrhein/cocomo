// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use cocomo_lib::compare::{DirEntry, DirEntryStatus};
use globset::Glob;

use crate::cli::DirCompareArgs;

// ---------------------------------------------------------------------------
// Entry filtering
// ---------------------------------------------------------------------------

/// Check whether an entry passes the name/type filters: `--excl-hidden`,
/// `--dirs-only`, and `--filter`.
pub(super) fn matches_name_filter(
    entry: &DirEntry,
    args: &DirCompareArgs,
) -> bool {
    if args.excl_hidden && entry.name.starts_with('.') {
        return false;
    }
    if args.dirs_only && !is_dir_entry(entry) {
        return false;
    }
    if let Some(pattern) = &args.filter
        && let Ok(glob) = Glob::new(pattern)
    {
        let matcher = glob.compile_matcher();
        if !matcher.is_match(&entry.name) {
            return false;
        }
    }
    true
}

/// Check whether an entry passes the status display filters:
/// `--show-different` and `--show-orphans`.
pub(super) fn matches_status_filter(
    entry: &DirEntry,
    args: &DirCompareArgs,
) -> bool {
    if args.show_different && args.show_orphans {
        return true;
    }

    if args.show_different {
        matches!(
            entry.status,
            DirEntryStatus::Different | DirEntryStatus::Similar
        )
    } else if args.show_orphans {
        matches!(
            entry.status,
            DirEntryStatus::LeftOnly | DirEntryStatus::RightOnly
        )
    } else {
        true
    }
}

/// Check whether an entry should be shown in CSV or JSON output.
/// Applies both name/type and status filters.
pub(super) fn should_show_entry(
    entry: &DirEntry,
    args: &DirCompareArgs,
) -> bool {
    matches_name_filter(entry, args) && matches_status_filter(entry, args)
}

/// Check whether an entry should be shown in tree mode.
///
/// The name/type filter applies to all entries (so hidden or non-matching
/// directories are excluded entirely). Directories are always shown as
/// structural nodes unless filtered by name/type, preserving the tree
/// skeleton when status filters like `--show-different` are active.
pub(super) fn should_show_in_tree(
    entry: &DirEntry,
    args: &DirCompareArgs,
) -> bool {
    if !matches_name_filter(entry, args) {
        return false;
    }
    is_dir_entry(entry) || matches_status_filter(entry, args)
}

/// Return `true` when the entry is a directory on any side.
pub(super) fn is_dir_entry(entry: &DirEntry) -> bool {
    entry.left.as_ref().map(|l| l.is_dir).unwrap_or(false)
        || entry.right.as_ref().map(|r| r.is_dir).unwrap_or(false)
        || entry.center.as_ref().map(|c| c.is_dir).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use cocomo_lib::compare::{DirEntry, DirEntryStatus, EntryInfo};

    use super::{
        super::test_helpers::{
            make_dir_entry, make_hidden_file_entry, make_test_comparison,
        },
        is_dir_entry, matches_name_filter, should_show_in_tree,
    };
    use crate::cli::DirCompareArgs;

    /// Build a `DirCompareArgs` with all flags set to their defaults, suitable
    /// for unit tests that need an args instance.
    fn test_args() -> DirCompareArgs {
        DirCompareArgs::test_defaults()
    }

    #[test]
    fn is_dir_entry_detects_files_as_non_dirs() {
        let comp = make_test_comparison();
        for entry in &comp.entries {
            assert!(!is_dir_entry(entry));
        }
    }

    #[test]
    fn is_dir_entry_detects_directory_on_left_only() {
        let dir_info = EntryInfo {
            name: "mydir".to_string(),
            path: "mydir".to_string(),
            size: 4096,
            modified: "2024-03-15 14:30".to_string(),
            is_dir: true,
            hash: None,
        };
        let entry = DirEntry {
            name: "mydir".to_string(),
            status: DirEntryStatus::LeftOnly,
            left: Some(dir_info),
            right: None,
            center: None,
            sub_entries: None,
            name_conflict: false,
        };
        assert!(is_dir_entry(&entry));
    }

    #[test]
    fn excl_hidden_filters_dotfiles() {
        let entry = make_hidden_file_entry();
        let mut args = test_args();
        args.excl_hidden = true;
        assert!(!matches_name_filter(&entry, &args));
    }

    #[test]
    fn excl_hidden_keeps_visible_files() {
        let comp = make_test_comparison();
        let entry = &comp.entries[0]; // "same.txt"
        let mut args = test_args();
        args.excl_hidden = true;
        assert!(matches_name_filter(entry, &args));
    }

    #[test]
    fn dirs_only_filters_files() {
        let comp = make_test_comparison();
        let entry = &comp.entries[0]; // "same.txt" — a file
        let mut args = test_args();
        args.dirs_only = true;
        assert!(!matches_name_filter(entry, &args));
    }

    #[test]
    fn dirs_only_keeps_directories() {
        let entry = make_dir_entry();
        let mut args = test_args();
        args.dirs_only = true;
        assert!(matches_name_filter(&entry, &args));
    }

    #[test]
    fn filter_glob_matches_txt() {
        let comp = make_test_comparison();
        let entry = &comp.entries[0]; // "same.txt"
        let mut args = test_args();
        args.filter = Some("*.txt".to_string());
        assert!(matches_name_filter(entry, &args));
    }

    #[test]
    fn filter_glob_excludes_unmatched() {
        let comp = make_test_comparison();
        let entry = &comp.entries[0]; // "same.txt"
        let mut args = test_args();
        args.filter = Some("*.log".to_string());
        assert!(!matches_name_filter(entry, &args));
    }

    #[test]
    fn should_show_in_tree_preserves_dirs_with_show_different() {
        let dir_entry = make_dir_entry();
        let mut args = test_args();
        args.show_different = true;
        // A directory with LeftOnly status should be shown in tree mode even
        // though LeftOnly is not a "different" status.
        assert!(should_show_in_tree(&dir_entry, &args));
    }

    #[test]
    fn should_show_in_tree_excludes_hidden_dirs() {
        let hidden_dir = DirEntry {
            name: ".git".to_string(),
            status: DirEntryStatus::LeftOnly,
            left: Some(EntryInfo {
                name: ".git".to_string(),
                path: ".git".to_string(),
                size: 4096,
                modified: "2024-03-15 14:30".to_string(),
                is_dir: true,
                hash: None,
            }),
            right: None,
            center: None,
            sub_entries: None,
            name_conflict: false,
        };
        let mut args = test_args();
        args.excl_hidden = true;
        assert!(!should_show_in_tree(&hidden_dir, &args));
    }
}
