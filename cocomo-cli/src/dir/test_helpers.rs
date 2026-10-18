// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared test fixtures for directory comparison tests.

use std::{collections::HashMap, sync::Arc};

use cocomo_lib::compare::{
    DirComparison, DirEntry, DirEntryStatus, EntryInfo,
};

/// Build a flat `DirComparison` with the four standard test entries.
pub(super) fn make_test_comparison() -> DirComparison {
    let same = EntryInfo {
        name: "same.txt".to_string(),
        path: "same.txt".to_string(),
        size: 6,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("abc".to_string()),
    };
    let diff = EntryInfo {
        name: "diff.txt".to_string(),
        path: "diff.txt".to_string(),
        size: 6,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("abc".to_string()),
    };
    let diff_right = EntryInfo {
        name: "diff.txt".to_string(),
        path: "diff.txt".to_string(),
        size: 8,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("def".to_string()),
    };
    let left_only = EntryInfo {
        name: "only_left.txt".to_string(),
        path: "only_left.txt".to_string(),
        size: 13,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: None,
    };
    let right_only = EntryInfo {
        name: "only_right.txt".to_string(),
        path: "only_right.txt".to_string(),
        size: 13,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: None,
    };

    let mut counts = HashMap::new();
    counts.insert(DirEntryStatus::Same, 1);
    counts.insert(DirEntryStatus::Different, 1);
    counts.insert(DirEntryStatus::LeftOnly, 1);
    counts.insert(DirEntryStatus::RightOnly, 1);

    DirComparison {
        entries: vec![
            DirEntry {
                name: "same.txt".to_string(),
                status: DirEntryStatus::Same,
                left: Some(same.clone()),
                right: Some(same),
                center: None,
                sub_entries: None,
            },
            DirEntry {
                name: "diff.txt".to_string(),
                status: DirEntryStatus::Different,
                left: Some(diff.clone()),
                right: Some(diff_right),
                center: None,
                sub_entries: None,
            },
            DirEntry {
                name: "only_left.txt".to_string(),
                status: DirEntryStatus::LeftOnly,
                left: Some(left_only),
                right: None,
                center: None,
                sub_entries: None,
            },
            DirEntry {
                name: "only_right.txt".to_string(),
                status: DirEntryStatus::RightOnly,
                left: None,
                right: Some(right_only),
                center: None,
                sub_entries: None,
            },
        ],
        counts,
        errors: vec![],
    }
}

/// Build a hidden file entry for filter tests.
pub(super) fn make_hidden_file_entry() -> DirEntry {
    DirEntry {
        name: ".hidden.txt".to_string(),
        status: DirEntryStatus::Same,
        left: Some(EntryInfo {
            name: ".hidden.txt".to_string(),
            path: ".hidden.txt".to_string(),
            size: 4,
            modified: "2024-03-15 14:30".to_string(),
            is_dir: false,
            hash: None,
        }),
        right: Some(EntryInfo {
            name: ".hidden.txt".to_string(),
            path: ".hidden.txt".to_string(),
            size: 4,
            modified: "2024-03-15 14:30".to_string(),
            is_dir: false,
            hash: None,
        }),
        center: None,
        sub_entries: None,
    }
}

/// Build a directory entry for filter and tree tests.
pub(super) fn make_dir_entry() -> DirEntry {
    DirEntry {
        name: "mydir".to_string(),
        status: DirEntryStatus::LeftOnly,
        left: Some(EntryInfo {
            name: "mydir".to_string(),
            path: "mydir".to_string(),
            size: 4096,
            modified: "2024-03-15 14:30".to_string(),
            is_dir: true,
            hash: None,
        }),
        right: None,
        center: None,
        sub_entries: None,
    }
}

/// Build a comparison with nested subdirectories for testing recursive
/// counting.
pub(super) fn make_nested_comparison() -> DirComparison {
    let dir_info = EntryInfo {
        name: "sub".to_string(),
        path: "sub".to_string(),
        size: 4096,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: true,
        hash: None,
    };
    // Sub-entries for the "sub" directory.
    let sub_same = EntryInfo {
        name: "same.txt".to_string(),
        path: "sub/same.txt".to_string(),
        size: 6,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("abc".to_string()),
    };
    let sub_diff_left = EntryInfo {
        name: "diff.txt".to_string(),
        path: "sub/diff.txt".to_string(),
        size: 6,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("abc".to_string()),
    };
    let sub_diff_right = EntryInfo {
        name: "diff.txt".to_string(),
        path: "sub/diff.txt".to_string(),
        size: 8,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("def".to_string()),
    };
    let sub_left_only = EntryInfo {
        name: "only_left.txt".to_string(),
        path: "sub/only_left.txt".to_string(),
        size: 13,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: None,
    };

    let sub_comparison = DirComparison {
        entries: vec![
            DirEntry {
                name: "same.txt".to_string(),
                status: DirEntryStatus::Same,
                left: Some(sub_same.clone()),
                right: Some(sub_same),
                center: None,
                sub_entries: None,
            },
            DirEntry {
                name: "diff.txt".to_string(),
                status: DirEntryStatus::Different,
                left: Some(sub_diff_left),
                right: Some(sub_diff_right),
                center: None,
                sub_entries: None,
            },
            DirEntry {
                name: "only_left.txt".to_string(),
                status: DirEntryStatus::LeftOnly,
                left: Some(sub_left_only),
                right: None,
                center: None,
                sub_entries: None,
            },
        ],
        counts: HashMap::new(),
        errors: vec![],
    };

    let same = EntryInfo {
        name: "same.txt".to_string(),
        path: "same.txt".to_string(),
        size: 6,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("abc".to_string()),
    };

    DirComparison {
        entries: vec![
            DirEntry {
                name: "sub".to_string(),
                status: DirEntryStatus::Same,
                left: Some(dir_info.clone()),
                right: Some(dir_info.clone()),
                center: None,
                sub_entries: Some(Arc::new(sub_comparison)),
            },
            DirEntry {
                name: "same.txt".to_string(),
                status: DirEntryStatus::Same,
                left: Some(same.clone()),
                right: Some(same),
                center: None,
                sub_entries: None,
            },
        ],
        counts: HashMap::new(),
        errors: vec![],
    }
}

/// Build a comparison with a hidden directory for testing tree-aware
/// filtering.
pub(super) fn make_nested_comparison_with_hidden() -> DirComparison {
    let hidden_dir = EntryInfo {
        name: ".hidden_dir".to_string(),
        path: ".hidden_dir".to_string(),
        size: 4096,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: true,
        hash: None,
    };
    let visible_dir = EntryInfo {
        name: "sub".to_string(),
        path: "sub".to_string(),
        size: 4096,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: true,
        hash: None,
    };
    let visible_file = EntryInfo {
        name: "visible.txt".to_string(),
        path: "visible.txt".to_string(),
        size: 6,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("abc".to_string()),
    };
    let child_file = EntryInfo {
        name: "child.txt".to_string(),
        path: ".hidden_dir/child.txt".to_string(),
        size: 6,
        modified: "2024-03-15 14:30".to_string(),
        is_dir: false,
        hash: Some("abc".to_string()),
    };

    let hidden_sub = DirComparison {
        entries: vec![DirEntry {
            name: "child.txt".to_string(),
            status: DirEntryStatus::Same,
            left: Some(child_file),
            right: Some(EntryInfo {
                name: "child.txt".to_string(),
                path: ".hidden_dir/child.txt".to_string(),
                size: 6,
                modified: "2024-03-15 14:30".to_string(),
                is_dir: false,
                hash: Some("abc".to_string()),
            }),
            center: None,
            sub_entries: None,
        }],
        counts: HashMap::new(),
        errors: vec![],
    };
    let visible_sub = DirComparison {
        entries: vec![],
        counts: HashMap::new(),
        errors: vec![],
    };

    DirComparison {
        entries: vec![
            DirEntry {
                name: ".hidden_dir".to_string(),
                status: DirEntryStatus::Same,
                left: Some(hidden_dir),
                right: Some(EntryInfo {
                    name: ".hidden_dir".to_string(),
                    path: ".hidden_dir".to_string(),
                    size: 4096,
                    modified: "2024-03-15 14:30".to_string(),
                    is_dir: true,
                    hash: None,
                }),
                center: None,
                sub_entries: Some(Arc::new(hidden_sub)),
            },
            DirEntry {
                name: "sub".to_string(),
                status: DirEntryStatus::Same,
                left: Some(visible_dir),
                right: Some(EntryInfo {
                    name: "sub".to_string(),
                    path: "sub".to_string(),
                    size: 4096,
                    modified: "2024-03-15 14:30".to_string(),
                    is_dir: true,
                    hash: None,
                }),
                center: None,
                sub_entries: Some(Arc::new(visible_sub)),
            },
            DirEntry {
                name: "visible.txt".to_string(),
                status: DirEntryStatus::Same,
                left: Some(visible_file),
                right: Some(EntryInfo {
                    name: "visible.txt".to_string(),
                    path: "visible.txt".to_string(),
                    size: 6,
                    modified: "2024-03-15 14:30".to_string(),
                    is_dir: false,
                    hash: Some("abc".to_string()),
                }),
                center: None,
                sub_entries: None,
            },
        ],
        counts: HashMap::new(),
        errors: vec![],
    }
}
