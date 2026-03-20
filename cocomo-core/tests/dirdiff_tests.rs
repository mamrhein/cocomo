// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::fs;

use cocomo_core::{By, DiffItemType, DiffSide, DirDiff, FSItem};
use filetime::{set_file_mtime, FileTime};

async fn setup_test_dirs() -> (tempfile::TempDir, tempfile::TempDir) {
    let left_dir = tempfile::tempdir().unwrap();
    let right_dir = tempfile::tempdir().unwrap();

    let left_path = left_dir.path();
    let right_path = right_dir.path();

    // 1. LeftOnly
    fs::write(left_path.join("left_only.txt"), "left only content").unwrap();

    // 2. RightOnly
    fs::write(right_path.join("right_only.txt"), "right only content")
        .unwrap();

    // 3. Same (Metadata)
    let same_path_left = left_path.join("same.txt");
    let same_path_right = right_path.join("same.txt");
    fs::write(&same_path_left, "same content").unwrap();
    fs::write(&same_path_right, "same content").unwrap();
    let now = FileTime::from_unix_time(1000000, 0);
    set_file_mtime(&same_path_left, now).unwrap();
    set_file_mtime(&same_path_right, now).unwrap();

    // 4. Different (Newer on Left)
    let left_newer_path = left_path.join("diff_left_newer.txt");
    let right_older_path = right_path.join("diff_left_newer.txt");
    fs::write(&left_newer_path, "content changed").unwrap();
    fs::write(&right_older_path, "content").unwrap();
    set_file_mtime(&left_newer_path, FileTime::from_unix_time(2000000, 0))
        .unwrap();
    set_file_mtime(&right_older_path, FileTime::from_unix_time(1000000, 0))
        .unwrap();

    // 5. Different (Newer on Right)
    let left_older_path = left_path.join("diff_right_newer.txt");
    let right_newer_path = right_path.join("diff_right_newer.txt");
    fs::write(&left_older_path, "content").unwrap();
    fs::write(&right_newer_path, "content changed").unwrap();
    set_file_mtime(&left_older_path, FileTime::from_unix_time(1000000, 0))
        .unwrap();
    set_file_mtime(&right_newer_path, FileTime::from_unix_time(2000000, 0))
        .unwrap();

    // 6. Different (Same mtime, different size -> newer: None)
    let left_diff_size = left_path.join("diff_size.txt");
    let right_diff_size = right_path.join("diff_size.txt");
    fs::write(&left_diff_size, "short").unwrap();
    fs::write(&right_diff_size, "much longer content").unwrap();
    set_file_mtime(&left_diff_size, now).unwrap();
    set_file_mtime(&right_diff_size, now).unwrap();

    (left_dir, right_dir)
}

#[tokio::test]
async fn test_dirdiff_all_variants() {
    let (left_dir, right_dir) = setup_test_dirs().await;

    let left_fsitem = FSItem::new(left_dir.path()).await;
    let right_fsitem = FSItem::new(right_dir.path()).await;

    let diff = DirDiff::new(&Some(left_fsitem), &Some(right_fsitem))
        .await
        .unwrap();

    let mut found_left_only = false;
    let mut found_right_only = false;
    let mut found_same = false;
    let mut found_left_newer = false;
    let mut found_right_newer = false;
    let mut found_diff_no_newer = false;

    for item in &diff.items {
        let name = item.name().to_str().unwrap();
        match name {
            "left_only.txt" => {
                assert_eq!(item.diff_item_type, DiffItemType::LeftOnly);
                found_left_only = true;
            }
            "right_only.txt" => {
                assert_eq!(item.diff_item_type, DiffItemType::RightOnly);
                found_right_only = true;
            }
            "same.txt" => {
                assert_eq!(
                    item.diff_item_type,
                    DiffItemType::Same { by: By::Metadata }
                );
                found_same = true;
            }
            "diff_left_newer.txt" => {
                assert_eq!(
                    item.diff_item_type,
                    DiffItemType::Different {
                        newer: Some(DiffSide::Left)
                    }
                );
                found_left_newer = true;
            }
            "diff_right_newer.txt" => {
                assert_eq!(
                    item.diff_item_type,
                    DiffItemType::Different {
                        newer: Some(DiffSide::Right)
                    }
                );
                found_right_newer = true;
            }
            "diff_size.txt" => {
                assert_eq!(
                    item.diff_item_type,
                    DiffItemType::Different { newer: None }
                );
                found_diff_no_newer = true;
            }
            _ => {}
        }
    }

    assert!(found_left_only, "LeftOnly not found");
    assert!(found_right_only, "RightOnly not found");
    assert!(found_same, "Same not found");
    assert!(found_left_newer, "Left newer not found");
    assert!(found_right_newer, "Right newer not found");
    assert!(found_diff_no_newer, "Different (no newer) not found");
}
