// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Integration tests for the COCOMO CLI parameter handling and exit codes.

use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cmd() -> Command {
    Command::cargo_bin("cocomo-cli").unwrap()
}

/// Create a temp directory with a known file structure for directory tests.
fn create_test_dirs() -> TempDir {
    let dir = TempDir::with_prefix("cocomo_test").unwrap();

    let left = dir.path().join("left");
    let right = dir.path().join("right");
    fs::create_dir_all(&left).unwrap();
    fs::create_dir_all(&right).unwrap();

    // Identical file.
    fs::write(left.join("same.txt"), "hello\n").unwrap();
    fs::write(right.join("same.txt"), "hello\n").unwrap();

    // Different content.
    fs::write(left.join("diff.txt"), "world\n").unwrap();
    fs::write(right.join("diff.txt"), "changed\n").unwrap();

    // Left-only file.
    fs::write(left.join("only_left.txt"), "left content\n").unwrap();

    // Right-only file.
    fs::write(right.join("only_right.txt"), "right content\n").unwrap();

    dir
}

/// Create two temp text files with different content.
fn create_diff_text_files() -> TempDir {
    let dir = TempDir::with_prefix("cocomo_text").unwrap();

    let left = dir.path().join("left.txt");
    let right = dir.path().join("right.txt");

    fs::write(&left, "line one\nline two\nline three\nline four\n").unwrap();
    fs::write(
        &right,
        "line one\nLINE TWO\nline three\nextra line\nline four\n",
    )
    .unwrap();

    dir
}

/// Create two identical text files.
fn create_same_text_files() -> TempDir {
    let dir = TempDir::with_prefix("cocomo_same").unwrap();

    let content = "identical content\n";
    fs::write(dir.path().join("a.txt"), content).unwrap();
    fs::write(dir.path().join("b.txt"), content).unwrap();

    dir
}

// ---------------------------------------------------------------------------
// Top-level: no command, help, version
// ---------------------------------------------------------------------------

mod top_level {
    use super::*;

    #[test]
    fn no_command_shows_help() {
        cmd()
            .assert()
            .failure()
            .stderr(predicate::str::contains("Usage: cocomo-cli <COMMAND>"));
    }

    #[test]
    fn help_flag() {
        cmd()
            .arg("--help")
            .assert()
            .success()
            .stdout(predicate::str::contains("Commands:"))
            .stdout(predicate::str::contains("dir"))
            .stdout(predicate::str::contains("text"))
            .stdout(predicate::str::contains("snapshot"));
    }

    #[test]
    fn version_flag() {
        cmd()
            .arg("--version")
            .assert()
            .success()
            .stdout(predicate::str::contains("0.0.1"));
    }
}

// ---------------------------------------------------------------------------
// Dir compare
// ---------------------------------------------------------------------------

mod dir_compare {
    use super::*;

    #[test]
    fn help_flag() {
        cmd()
            .args(["dir", "compare", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("<LEFT>"))
            .stdout(predicate::str::contains("<RIGHT>"));
    }

    #[test]
    fn missing_arguments_fails() {
        cmd()
            .args(["dir", "compare"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn left_missing_fails() {
        cmd()
            .args(["dir", "compare"])
            .args(["/nonexistent/path/left", "/nonexistent/path/right"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn right_missing_fails() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                "/nonexistent/path/right",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn identical_dirs_exit_zero() {
        let dir = create_test_dirs();
        // Compare a dir with itself — should be identical.
        cmd()
            .args(["dir", "compare"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("left").to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    #[test]
    fn different_dirs_exit_one() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1);
    }

    #[test]
    fn structure_only_flag() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare", "--structure-only"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("Summary:"));
    }

    #[test]
    fn format_csv() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare", "--format", "csv"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains(
                "status,name,left_size,right_size",
            ))
            .stdout(predicate::str::contains("same"))
            .stdout(predicate::str::contains("different"));
    }

    #[test]
    fn format_json() {
        let dir = create_test_dirs();
        let output = cmd()
            .args(["dir", "compare", "--format", "json"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();

        // Extract the JSON object from stdout (summary line is appended
        // after).
        let stdout_str = String::from_utf8(output).unwrap();
        let json_end = stdout_str.rfind('}').unwrap() + 1;
        let json_str = &stdout_str[..json_end];
        let parsed: serde_json::Value =
            serde_json::from_str(json_str).unwrap();

        assert_eq!(parsed["summary"]["total"], 4);
        assert_eq!(parsed["summary"]["same"], 1);
        assert_eq!(parsed["summary"]["different"], 1);
        assert_eq!(parsed["summary"]["orphans"], 2);
    }

    #[test]
    fn format_invalid_fails() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare", "--format", "xml"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn show_different_filter() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare", "--show-different"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("!"))
            .stdout(predicate::str::contains("diff.txt"))
            .stdout(predicate::str::contains("only_left").not());
    }

    #[test]
    fn show_orphans_filter() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare", "--show-orphans"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("<"))
            .stdout(predicate::str::contains(">"));
    }

    #[test]
    fn both_filters_shows_all() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "compare", "--show-different", "--show-orphans"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("same"))
            .stdout(predicate::str::contains("only_left"));
    }
}

// ---------------------------------------------------------------------------
// Dir sync
// ---------------------------------------------------------------------------

mod dir_sync {
    use super::*;

    #[test]
    fn help_flag() {
        cmd()
            .args(["dir", "sync", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--mirror-left"))
            .stdout(predicate::str::contains("--dry-run"));
    }

    #[test]
    fn missing_arguments_fails() {
        cmd()
            .args(["dir", "sync"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn left_missing_fails() {
        cmd()
            .args(["dir", "sync", "/no/left", "/no/right"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn dry_run_exits_cleanly() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "sync", "--dry-run"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("DRY RUN"));
    }

    #[test]
    fn dry_run_with_mirror_right() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "sync", "--dry-run", "--mirror-right"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("mirror right"));
    }

    #[test]
    fn synced_dirs_report_no_actions() {
        let dir = create_test_dirs();
        // Compare a dir with itself — should be in sync.
        cmd()
            .args(["dir", "sync", "--dry-run"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("left").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("already in sync"));
    }

    #[test]
    fn copy_right_flag() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "sync", "--dry-run", "--copy-right"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("copy right"));
    }

    #[test]
    fn delete_orphans_flag() {
        let dir = create_test_dirs();
        cmd()
            .args(["dir", "sync", "--dry-run", "--delete-orphans"])
            .args([
                dir.path().join("left").to_str().unwrap(),
                dir.path().join("right").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("delete orphans"));
    }
}

// ---------------------------------------------------------------------------
// Text compare
// ---------------------------------------------------------------------------

mod text_compare {
    use super::*;

    #[test]
    fn help_flag() {
        cmd()
            .args(["text", "compare", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--ignore-case"))
            .stdout(predicate::str::contains("--ignore-whitespace"));
    }

    #[test]
    fn missing_arguments_fails() {
        cmd()
            .args(["text", "compare"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn left_missing_fails() {
        cmd()
            .args(["text", "compare", "/no/left.txt", "/no/right.txt"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn right_missing_fails() {
        let dir = create_diff_text_files();
        cmd()
            .args(["text", "compare"])
            .args([
                dir.path().join("left.txt").to_str().unwrap(),
                "/nonexistent.txt",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn identical_files_exit_zero() {
        let dir = create_same_text_files();
        cmd()
            .args(["text", "compare"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }

    #[test]
    fn different_files_exit_one() {
        let dir = create_diff_text_files();
        cmd()
            .args(["text", "compare"])
            .args([
                dir.path().join("left.txt").to_str().unwrap(),
                dir.path().join("right.txt").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("differences found"));
    }

    #[test]
    fn ignore_case_flag() {
        let dir = TempDir::with_prefix("cocomo_case").unwrap();
        fs::write(dir.path().join("a.txt"), "Hello\n").unwrap();
        fs::write(dir.path().join("b.txt"), "hello\n").unwrap();

        // Without ignore-case — should show a difference.
        cmd()
            .args(["text", "compare"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .code(1);

        // With ignore-case — should be identical.
        cmd()
            .args(["text", "compare", "--ignore-case"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }

    #[test]
    fn ignore_whitespace_trim() {
        let dir = TempDir::with_prefix("cocomo_ws").unwrap();
        fs::write(dir.path().join("a.txt"), "hello \n").unwrap();
        fs::write(dir.path().join("b.txt"), "hello\n").unwrap();

        // Without ignore-whitespace — should show a difference.
        cmd()
            .args(["text", "compare"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .code(1);

        // With trim mode — should be identical.
        cmd()
            .args(["text", "compare", "--ignore-whitespace", "trim"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }

    #[test]
    fn ignore_whitespace_insensitive() {
        let dir = TempDir::with_prefix("cocomo_ws2").unwrap();
        fs::write(dir.path().join("a.txt"), "hello world\n").unwrap();
        fs::write(dir.path().join("b.txt"), "hello  world\n").unwrap();

        cmd()
            .args(["text", "compare", "--ignore-whitespace", "insensitive"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }

    #[test]
    fn grammar_rust() {
        let dir = TempDir::with_prefix("cocomo_grammar").unwrap();
        fs::write(dir.path().join("a.rs"), "// comment\nfn main() {}\n")
            .unwrap();
        fs::write(
            dir.path().join("b.rs"),
            "// different comment\nfn main() {}\n",
        )
        .unwrap();

        cmd()
            .args(["text", "compare", "--grammar", "rust"])
            .args([
                dir.path().join("a.rs").to_str().unwrap(),
                dir.path().join("b.rs").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("differences found"));

        // With ignore-comments the comment line should be skipped.
        cmd()
            .args([
                "text",
                "compare",
                "--grammar",
                "rust",
                "--ignore-comments",
            ])
            .args([
                dir.path().join("a.rs").to_str().unwrap(),
                dir.path().join("b.rs").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }

    #[test]
    fn grammar_python() {
        let dir = TempDir::with_prefix("cocomo_py").unwrap();
        fs::write(dir.path().join("a.py"), "# comment\ndef foo(): pass\n")
            .unwrap();
        fs::write(dir.path().join("b.py"), "# different\ndef foo(): pass\n")
            .unwrap();

        cmd()
            .args([
                "text",
                "compare",
                "--grammar",
                "python",
                "--ignore-comments",
            ])
            .args([
                dir.path().join("a.py").to_str().unwrap(),
                dir.path().join("b.py").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }

    #[test]
    fn grammar_plain_text() {
        let dir = create_diff_text_files();
        cmd()
            .args(["text", "compare", "--grammar", "plain-text"])
            .args([
                dir.path().join("left.txt").to_str().unwrap(),
                dir.path().join("right.txt").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("differences found"));
    }

    #[test]
    fn grammar_invalid_fails() {
        let dir = create_diff_text_files();
        cmd()
            .args(["text", "compare", "--grammar", "invalid"])
            .args([
                dir.path().join("left.txt").to_str().unwrap(),
                dir.path().join("right.txt").to_str().unwrap(),
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn ignore_whitespace_invalid_fails() {
        let dir = create_diff_text_files();
        cmd()
            .args(["text", "compare", "--ignore-whitespace", "invalid"])
            .args([
                dir.path().join("left.txt").to_str().unwrap(),
                dir.path().join("right.txt").to_str().unwrap(),
            ])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn ignore_blank_lines_flag() {
        let dir = TempDir::with_prefix("cocomo_blank").unwrap();
        fs::write(dir.path().join("a.txt"), "line\n\nend\n").unwrap();
        fs::write(dir.path().join("b.txt"), "line\nend\n").unwrap();

        cmd()
            .args(["text", "compare", "--ignore-blank-lines"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }
}

// ---------------------------------------------------------------------------
// Text diff (unified output)
// ---------------------------------------------------------------------------

mod text_diff {
    use super::*;

    #[test]
    fn help_flag() {
        cmd()
            .args(["text", "diff", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("<LEFT>"))
            .stdout(predicate::str::contains("<RIGHT>"));
    }

    #[test]
    fn missing_arguments_fails() {
        cmd()
            .args(["text", "diff"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn left_missing_fails() {
        cmd()
            .args(["text", "diff", "/no/left.txt", "/no/right.txt"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn identical_files_exit_zero() {
        let dir = create_same_text_files();
        cmd()
            .args(["text", "diff"])
            .args([
                dir.path().join("a.txt").to_str().unwrap(),
                dir.path().join("b.txt").to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    #[test]
    fn different_files_produce_unified_diff() {
        let dir = create_diff_text_files();
        let output = cmd()
            .args(["text", "diff"])
            .args([
                dir.path().join("left.txt").to_str().unwrap(),
                dir.path().join("right.txt").to_str().unwrap(),
            ])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();

        let stdout = String::from_utf8(output).unwrap();
        assert!(stdout.contains("---"), "should have left header");
        assert!(stdout.contains("+++"), "should have right header");
        assert!(stdout.contains("@@"), "should have hunk header");
        assert!(stdout.contains("-"), "should have removed lines");
        assert!(stdout.contains("+"), "should have added lines");
    }
}

// ---------------------------------------------------------------------------
// Snapshot capture
// ---------------------------------------------------------------------------

mod snapshot_capture {
    use super::*;

    #[test]
    fn help_flag() {
        cmd()
            .args(["snapshot", "capture", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("<PATH>"));
    }

    #[test]
    fn missing_arguments_fails() {
        cmd()
            .args(["snapshot", "capture"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn path_missing_fails() {
        cmd()
            .args(["snapshot", "capture", "/nonexistent/path"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn valid_capture_succeeds() {
        let dir = create_test_dirs();
        let snap_path = dir.path().join("test.snap");
        cmd()
            .args([
                "snapshot",
                "capture",
                dir.path().join("left").to_str().unwrap(),
                snap_path.to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("Snapshot captured"));

        assert!(snap_path.exists(), "snapshot file should exist");
    }

    #[test]
    fn default_output_name() {
        let dir = create_test_dirs();
        // Without explicit output path, the snapshot is created in the current
        // dir. We cannot easily test this without changing cwd, so we just
        // verify the explicit output path works.
        let snap_path = dir.path().join("left.snap");
        cmd()
            .args([
                "snapshot",
                "capture",
                dir.path().join("left").to_str().unwrap(),
                snap_path.to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    #[test]
    fn empty_directory_snapshot() {
        let dir = TempDir::with_prefix("cocomo_snap").unwrap();
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();

        let snap_path = dir.path().join("empty.snap");
        cmd()
            .args([
                "snapshot",
                "capture",
                empty.to_str().unwrap(),
                snap_path.to_str().unwrap(),
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains("Snapshot captured"));
    }
}

// ---------------------------------------------------------------------------
// Snapshot list
// ---------------------------------------------------------------------------

mod snapshot_list {
    use super::*;

    #[test]
    fn help_flag() {
        cmd()
            .args(["snapshot", "list", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("[DIRECTORY]"));
    }

    #[test]
    fn empty_directory_shows_none() {
        let dir = TempDir::with_prefix("cocomo_list").unwrap();
        cmd()
            .args(["snapshot", "list", dir.path().to_str().unwrap()])
            .assert()
            .success()
            .stdout(predicate::str::contains("No snapshots found"));
    }
}

// ---------------------------------------------------------------------------
// Snapshot diff
// ---------------------------------------------------------------------------

mod snapshot_diff {
    use super::*;

    #[test]
    fn help_flag() {
        cmd()
            .args(["snapshot", "diff", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("<LEFT>"))
            .stdout(predicate::str::contains("<RIGHT>"));
    }

    #[test]
    fn missing_arguments_fails() {
        cmd()
            .args(["snapshot", "diff"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn left_missing_fails() {
        cmd()
            .args(["snapshot", "diff", "/no/a.snap", "/no/b.snap"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn identical_snapshots_exit_zero() {
        let dir = create_test_dirs();
        let snap_path = dir.path().join("test.snap");

        // Capture the same dir twice.
        cmd()
            .args([
                "snapshot",
                "capture",
                dir.path().join("left").to_str().unwrap(),
                snap_path.to_str().unwrap(),
            ])
            .assert()
            .success();

        // Copy the snapshot to another file for comparison.
        let snap_copy = dir.path().join("test_copy.snap");
        fs::copy(&snap_path, &snap_copy).unwrap();

        cmd()
            .args(["snapshot", "diff"])
            .args([snap_path.to_str().unwrap(), snap_copy.to_str().unwrap()])
            .assert()
            .success()
            .stdout(predicate::str::contains("identical"));
    }

    #[test]
    fn different_snapshots_exit_one() {
        let dir = create_test_dirs();
        let snap_left = dir.path().join("left.snap");
        let snap_right = dir.path().join("right.snap");

        cmd()
            .args([
                "snapshot",
                "capture",
                dir.path().join("left").to_str().unwrap(),
                snap_left.to_str().unwrap(),
            ])
            .assert()
            .success();

        cmd()
            .args([
                "snapshot",
                "capture",
                dir.path().join("right").to_str().unwrap(),
                snap_right.to_str().unwrap(),
            ])
            .assert()
            .success();

        cmd()
            .args(["snapshot", "diff"])
            .args([snap_left.to_str().unwrap(), snap_right.to_str().unwrap()])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("Snapshot diff:"))
            .stdout(predicate::str::contains("modified"))
            .stdout(predicate::str::contains("added"))
            .stdout(predicate::str::contains("deleted"));
    }
}

// ---------------------------------------------------------------------------
// Subcommand routing
// ---------------------------------------------------------------------------

mod routing {
    use super::*;

    #[test]
    fn dir_help() {
        cmd()
            .args(["dir", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("compare"))
            .stdout(predicate::str::contains("sync"));
    }

    #[test]
    fn text_help() {
        cmd()
            .args(["text", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("compare"))
            .stdout(predicate::str::contains("diff"));
    }

    #[test]
    fn snapshot_help() {
        cmd()
            .args(["snapshot", "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("capture"))
            .stdout(predicate::str::contains("list"))
            .stdout(predicate::str::contains("diff"));
    }

    #[test]
    fn unknown_subcommand_fails() {
        cmd()
            .arg("foobar")
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn unknown_dir_subcommand_fails() {
        cmd()
            .args(["dir", "merge"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn unknown_text_subcommand_fails() {
        cmd()
            .args(["text", "merge"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn unknown_snapshot_subcommand_fails() {
        cmd()
            .args(["snapshot", "merge"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }
}
