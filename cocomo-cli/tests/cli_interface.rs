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

// ---------------------------------------------------------------------------
// Top-level: no command, help, version
// ---------------------------------------------------------------------------

mod top_level {
    use super::*;

    #[test]
    fn no_command_shows_help() {
        #[cfg(windows)]
        let exe = ".exe";
        #[cfg(not(windows))]
        let exe = "";
        cmd()
            .assert()
            .failure()
            .stderr(predicate::str::contains(format!(
                "Usage: cocomo-cli{} <COMMAND>",
                exe
            )));
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
    fn format_invalid_fails() {
        let left = "/tmp/left";
        let right = "/tmp/right";
        cmd()
            .args(["dir", "compare", "--format", "xml"])
            .args([left, right])
            .assert()
            .failure()
            .stderr(predicate::str::contains("error:"));
    }

    #[test]
    fn report_invalid_format_fails() {
        let left = "/tmp/left";
        let right = "/tmp/right";
        let report_path = "/tmp/report.txt";
        cmd()
            .args(["dir", "compare", "--report-format", "invalid"])
            .arg("--report")
            .arg(report_path)
            .args([left, right])
            .assert()
            .failure();
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
