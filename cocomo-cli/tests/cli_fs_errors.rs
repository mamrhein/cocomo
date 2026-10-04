// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Integration tests for CLI error handling with an injected [`MockFs`].
//!
//! The command logic runs in-process against a mock filesystem, so errors
//! that cannot be reproduced reliably on a real filesystem (permission
//! denials mid-walk, missing roots) can be injected deterministically.
//! Errors at the root of an endpoint propagate as `CliError::Fs`; errors
//! encountered mid-walk are collected by the scan and surfaced as
//! `CliError::FsErrors`.

use std::{path::PathBuf, sync::Arc};

use clap::Parser;
use cocomo_cli::{Cli, CliError, DiffResult, EndpointResolver, run};
use cocomo_lib::{FsError, FsOperation, MockFs, ProviderId, Url};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A resolver that serves every endpoint from one shared [`MockFs`], so
/// both sides of a pair command observe the same in-memory tree.
struct MockResolver {
    fs: Arc<MockFs>,
}

impl EndpointResolver for MockResolver {
    type Fs = MockFs;

    fn resolve(
        &self,
        _url: &Url,
        _profile_id: Option<&str>,
    ) -> Result<Arc<MockFs>, CliError> {
        Ok(self.fs.clone())
    }

    fn provider_id(&self, _fs: &MockFs) -> ProviderId {
        ProviderId::local()
    }
}

/// Parse `args` and run the command against `mock`.
async fn run_with_mock(
    mock: MockFs,
    args: &[&str],
) -> Result<DiffResult, CliError> {
    let cli = Cli::try_parse_from(args).unwrap();
    run(&cli.command, &MockResolver { fs: Arc::new(mock) }).await
}

// ---------------------------------------------------------------------------
// dir compare
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dir_compare_missing_root_returns_fs_error() {
    let mock = MockFs::new("mock");

    let result =
        run_with_mock(mock, &["cocomo", "dir", "compare", "/left", "/right"])
            .await;

    match result {
        Err(CliError::Fs(FsError::NotFound { path })) => {
            assert_eq!(path, PathBuf::from("/left"));
        }
        other => panic!("expected Fs(NotFound), got {other:?}"),
    }
}

#[tokio::test]
async fn dir_compare_surfaces_mid_walk_file_error() {
    // The file exists in the tree (so the scan lists it) but every
    // operation on it fails, so resolving it mid-walk degrades to a
    // collected error.
    let mock = MockFs::new("mock")
        .with_dir("/left")
        .with_file("/left/blocked.txt", "secret")
        .with_error(
            "/left/blocked.txt",
            FsError::PermissionDenied {
                operation: FsOperation::Read,
                path: PathBuf::from("/left/blocked.txt"),
            },
        )
        .with_dir("/right");

    let result =
        run_with_mock(mock, &["cocomo", "dir", "compare", "/left", "/right"])
            .await;

    match result {
        Err(CliError::FsErrors(errors)) => {
            assert_eq!(errors.len(), 1);
            assert!(matches!(errors[0], FsError::PermissionDenied { .. }));
        }
        other => panic!("expected FsErrors, got {other:?}"),
    }
}

#[tokio::test]
async fn dir_compare_surfaces_mid_walk_subdir_error() {
    // The subdirectory exists in the tree but cannot be read, so the scan
    // collects the error and skips the subtree.
    let mock = MockFs::new("mock")
        .with_dir("/left")
        .with_dir("/left/sub")
        .with_error(
            "/left/sub",
            FsError::PermissionDenied {
                operation: FsOperation::ReadDir,
                path: PathBuf::from("/left/sub"),
            },
        )
        .with_dir("/right");

    let result =
        run_with_mock(mock, &["cocomo", "dir", "compare", "/left", "/right"])
            .await;

    match result {
        Err(CliError::FsErrors(errors)) => {
            assert_eq!(errors.len(), 1);
            assert!(matches!(errors[0], FsError::PermissionDenied { .. }));
        }
        other => panic!("expected FsErrors, got {other:?}"),
    }
}

#[tokio::test]
async fn dir_compare_identical_trees_report_no_diffs() {
    let mock = MockFs::new("mock")
        .with_dir("/left")
        .with_file("/left/same.txt", "hello\n")
        .with_dir("/right")
        .with_file("/right/same.txt", "hello\n");

    let result =
        run_with_mock(mock, &["cocomo", "dir", "compare", "/left", "/right"])
            .await;

    match result {
        Ok(DiffResult::NoDiffs) => {}
        other => panic!("expected Ok(NoDiffs), got {other:?}"),
    }
}

#[tokio::test]
async fn dir_compare_differing_trees_report_diffs() {
    let mock = MockFs::new("mock")
        .with_dir("/left")
        .with_file("/left/diff.txt", "world\n")
        .with_dir("/right")
        .with_file("/right/diff.txt", "changed\n");

    let result =
        run_with_mock(mock, &["cocomo", "dir", "compare", "/left", "/right"])
            .await;

    match result {
        Ok(DiffResult::HasDiffs) => {}
        other => panic!("expected Ok(HasDiffs), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// dir sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dir_sync_surfaces_mid_walk_error() {
    let mock = MockFs::new("mock")
        .with_dir("/left")
        .with_file("/left/blocked.txt", "secret")
        .with_error(
            "/left/blocked.txt",
            FsError::PermissionDenied {
                operation: FsOperation::Read,
                path: PathBuf::from("/left/blocked.txt"),
            },
        )
        .with_dir("/right");

    let result = run_with_mock(
        mock,
        &["cocomo", "dir", "sync", "/left", "/right", "--dry-run"],
    )
    .await;

    match result {
        Err(CliError::FsErrors(errors)) => {
            assert_eq!(errors.len(), 1);
            assert!(matches!(errors[0], FsError::PermissionDenied { .. }));
        }
        other => panic!("expected FsErrors, got {other:?}"),
    }
}

#[tokio::test]
async fn dir_sync_missing_root_returns_fs_error() {
    let mock = MockFs::new("mock");

    let result = run_with_mock(
        mock,
        &["cocomo", "dir", "sync", "/left", "/right", "--dry-run"],
    )
    .await;

    match result {
        Err(CliError::Fs(FsError::NotFound { path })) => {
            assert_eq!(path, PathBuf::from("/left"));
        }
        other => panic!("expected Fs(NotFound), got {other:?}"),
    }
}

#[tokio::test]
async fn dir_sync_non_dry_run_aborts_on_mid_walk_error() {
    // Without `--dry-run` the sync must refuse to execute on top of an
    // incomplete comparison instead of transferring a partial plan.
    let mock = MockFs::new("mock")
        .with_dir("/left")
        .with_file("/left/left_only.txt", "only left")
        .with_dir("/left/blocked")
        .with_error(
            "/left/blocked",
            FsError::PermissionDenied {
                operation: FsOperation::ReadDir,
                path: PathBuf::from("/left/blocked"),
            },
        )
        .with_dir("/right");

    let result =
        run_with_mock(mock, &["cocomo", "dir", "sync", "/left", "/right"])
            .await;

    match result {
        Err(CliError::FsErrors(errors)) => {
            assert_eq!(errors.len(), 1);
            assert!(matches!(errors[0], FsError::PermissionDenied { .. }));
        }
        other => panic!("expected FsErrors, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// text compare
// ---------------------------------------------------------------------------

#[tokio::test]
async fn text_compare_missing_file_returns_fs_error() {
    let mock = MockFs::new("mock");

    let result = run_with_mock(
        mock,
        &["cocomo", "text", "compare", "/left.txt", "/right.txt"],
    )
    .await;

    match result {
        Err(CliError::Fs(FsError::NotFound { path })) => {
            assert_eq!(path, PathBuf::from("/left.txt"));
        }
        other => panic!("expected Fs(NotFound), got {other:?}"),
    }
}

#[tokio::test]
async fn text_compare_injected_read_error_returns_fs_error() {
    let mock = MockFs::new("mock")
        .with_file("/left.txt", "secret")
        .with_error(
            "/left.txt",
            FsError::PermissionDenied {
                operation: FsOperation::Read,
                path: PathBuf::from("/left.txt"),
            },
        )
        .with_file("/right.txt", "public");

    let result = run_with_mock(
        mock,
        &["cocomo", "text", "compare", "/left.txt", "/right.txt"],
    )
    .await;

    match result {
        Err(CliError::Fs(FsError::PermissionDenied { path, .. })) => {
            assert_eq!(path, PathBuf::from("/left.txt"));
        }
        other => panic!("expected Fs(PermissionDenied), got {other:?}"),
    }
}

#[tokio::test]
async fn text_compare_identical_files_report_no_diffs() {
    let mock = MockFs::new("mock")
        .with_file("/left.txt", "same\n")
        .with_file("/right.txt", "same\n");

    let result = run_with_mock(
        mock,
        &["cocomo", "text", "compare", "/left.txt", "/right.txt"],
    )
    .await;

    match result {
        Ok(DiffResult::NoDiffs) => {}
        other => panic!("expected Ok(NoDiffs), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// snapshot capture
// ---------------------------------------------------------------------------

#[tokio::test]
async fn snapshot_capture_missing_root_returns_fs_error() {
    let mock = MockFs::new("mock");

    let result =
        run_with_mock(mock, &["cocomo", "snapshot", "capture", "/data"]).await;

    match result {
        Err(CliError::Fs(FsError::NotFound { path })) => {
            assert_eq!(path, PathBuf::from("/data"));
        }
        other => panic!("expected Fs(NotFound), got {other:?}"),
    }
}

#[tokio::test]
async fn snapshot_capture_clean_tree_succeeds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let output = tmp.path().join("data.snap");

    let mock = MockFs::new("mock")
        .with_dir("/data")
        .with_file("/data/a.txt", "alpha");

    let result = run_with_mock(
        mock,
        &[
            "cocomo",
            "snapshot",
            "capture",
            "/data",
            output.to_str().unwrap(),
        ],
    )
    .await;

    match result {
        Ok(DiffResult::NoDiffs) => {}
        other => panic!("expected Ok(NoDiffs), got {other:?}"),
    }
    assert!(output.exists());
}

#[tokio::test]
async fn snapshot_capture_mid_walk_error_writes_no_file() {
    // A partial scan must abort the capture before any snapshot file is
    // written, so no misleading empty snapshot ends up on disk.
    let tmp = tempfile::TempDir::new().unwrap();
    let output = tmp.path().join("data.snap");

    let mock = MockFs::new("mock")
        .with_dir("/data")
        .with_file("/data/a.txt", "alpha")
        .with_dir("/data/sub")
        .with_error(
            "/data/sub",
            FsError::PermissionDenied {
                operation: FsOperation::ReadDir,
                path: PathBuf::from("/data/sub"),
            },
        );

    let result = run_with_mock(
        mock,
        &[
            "cocomo",
            "snapshot",
            "capture",
            "/data",
            output.to_str().unwrap(),
        ],
    )
    .await;

    match result {
        Err(CliError::FsErrors(errors)) => {
            assert_eq!(errors.len(), 1);
            assert!(matches!(errors[0], FsError::PermissionDenied { .. }));
        }
        other => panic!("expected FsErrors, got {other:?}"),
    }
    assert!(!output.exists());
}
