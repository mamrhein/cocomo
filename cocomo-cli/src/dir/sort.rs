// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::cmp::Ordering;

use cocomo_lib::compare::DirEntry;

use crate::cli::SortField;

// ---------------------------------------------------------------------------
// Entry sorting
// ---------------------------------------------------------------------------

/// Compare two entries by the given sort field. Ordering is descending for
/// size and modified time (largest/newest first), ascending otherwise.
fn compare_entries(a: &DirEntry, b: &DirEntry, sort: SortField) -> Ordering {
    match sort {
        SortField::Name => a.name.cmp(&b.name),
        SortField::Size => entry_size(b).cmp(&entry_size(a)),
        SortField::Modified => entry_modified(b).cmp(&entry_modified(a)),
        SortField::Status => a.status.to_string().cmp(&b.status.to_string()),
    }
}

/// Sort a flattened entry list in place by the given field.
pub(super) fn sort_entries(entries: &mut [DirEntry], sort: SortField) {
    entries.sort_by(|a, b| compare_entries(a, b, sort));
}

/// Sort a borrowed entry list in place by the given field (used to order
/// entries within each level of the tree output).
pub(super) fn sort_entry_refs(entries: &mut [&DirEntry], sort: SortField) {
    entries.sort_by(|a, b| compare_entries(a, b, sort));
}

/// Return the file size for an entry, preferring the left side then right.
pub(super) fn entry_size(entry: &DirEntry) -> u64 {
    entry
        .left
        .as_ref()
        .map(|l| l.size)
        .or_else(|| entry.right.as_ref().map(|r| r.size))
        .unwrap_or(0)
}

/// Return the modification timestamp for an entry, preferring left then right.
pub(super) fn entry_modified(entry: &DirEntry) -> String {
    entry
        .left
        .as_ref()
        .map(|l| l.modified.clone())
        .or_else(|| entry.right.as_ref().map(|r| r.modified.clone()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use cocomo_lib::compare::{DirEntry, DirEntryStatus, EntryInfo};

    use super::sort_entries;
    use crate::cli::SortField;

    fn make_entry(
        name: &str,
        status: DirEntryStatus,
        size: u64,
        modified: &str,
    ) -> DirEntry {
        let info = |s: u64, m: &str| EntryInfo {
            name: name.to_string(),
            path: name.to_string(),
            size: s,
            modified: m.to_string(),
            is_dir: false,
            hash: None,
        };
        let left = if status != DirEntryStatus::RightOnly {
            Some(info(size, modified))
        } else {
            None
        };
        let right = if status != DirEntryStatus::LeftOnly {
            Some(info(size, modified))
        } else {
            None
        };
        DirEntry {
            name: name.to_string(),
            status,
            left,
            right,
            center: None,
            sub_entries: None,
        }
    }

    #[test]
    fn sort_entries_by_name_orders_alphabetically() {
        let mut entries = vec![
            make_entry("zebra.txt", DirEntryStatus::Different, 10, ""),
            make_entry("apple.txt", DirEntryStatus::Same, 5, ""),
        ];
        sort_entries(&mut entries, SortField::Name);
        assert_eq!(entries[0].name, "apple.txt");
        assert_eq!(entries[1].name, "zebra.txt");
    }

    #[test]
    fn sort_entries_by_size_largest_first() {
        let mut entries = vec![
            make_entry("small.txt", DirEntryStatus::Same, 10, ""),
            make_entry("large.txt", DirEntryStatus::Same, 1000, ""),
        ];
        sort_entries(&mut entries, SortField::Size);
        assert_eq!(entries[0].name, "large.txt");
        assert_eq!(entries[1].name, "small.txt");
    }

    #[test]
    fn sort_entries_by_status_groups_like_items() {
        let mut entries = vec![
            make_entry("right_only.txt", DirEntryStatus::RightOnly, 5, ""),
            make_entry("same.txt", DirEntryStatus::Same, 5, ""),
        ];
        sort_entries(&mut entries, SortField::Status);
        // Display names: "right only" < "same" alphabetically (r < s).
        assert_eq!(entries[0].status, DirEntryStatus::RightOnly);
        assert_eq!(entries[1].status, DirEntryStatus::Same);
    }

    #[test]
    fn sort_entries_by_modified_newest_first() {
        let mut entries = vec![
            make_entry("old.txt", DirEntryStatus::Same, 5, "2024-01-01"),
            make_entry("new.txt", DirEntryStatus::Same, 5, "2024-06-01"),
        ];
        sort_entries(&mut entries, SortField::Modified);
        assert_eq!(entries[0].name, "new.txt");
        assert_eq!(entries[1].name, "old.txt");
    }

    #[test]
    fn sort_entries_by_name_with_left_only_uses_name() {
        // Entries with only a left side should still sort by name.
        let mut entries = vec![
            make_entry("zzz.txt", DirEntryStatus::LeftOnly, 5, ""),
            make_entry("aaa.txt", DirEntryStatus::LeftOnly, 5, ""),
        ];
        sort_entries(&mut entries, SortField::Name);
        assert_eq!(entries[0].name, "aaa.txt");
        assert_eq!(entries[1].name, "zzz.txt");
    }

    #[test]
    fn sort_entries_by_size_with_only_right_side() {
        // Entries with only a right side should use the right size.
        let mut entries = vec![
            make_entry("small.txt", DirEntryStatus::RightOnly, 10, ""),
            make_entry("large.txt", DirEntryStatus::RightOnly, 1000, ""),
        ];
        sort_entries(&mut entries, SortField::Size);
        assert_eq!(entries[0].name, "large.txt");
        assert_eq!(entries[1].name, "small.txt");
    }
}
