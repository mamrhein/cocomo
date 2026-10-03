// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Integration tests for endpoint resolution in the COCOMO CLI: endpoint
//! URL and bare-path formats are parsed at the argument level, mixed
//! (cross-provider) pairs resolve per side and reach I/O, the
//! scaffolding `s3` and `webdav` schemes are gated with a clean error
//! instead of a panic, and remote endpoints without credentials fail
//! rather than resolving anonymously (OQ3).

use std::{env, fs, path::Path, time::Duration};

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cmd() -> Command {
    Command::cargo_bin("cocomo-cli").unwrap()
}

/// Turn into a `file://` URL argument as a user would type it.
fn file_url(path: impl AsRef<Path>) -> String {
    format!("file://{}", path.as_ref().display())
}

/// Create a temp directory with two directory trees that differ in content.
fn create_test_dirs() -> TempDir {
    let dir = TempDir::with_prefix("cocomo_ep").unwrap();

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

/// Create two identical text files in a fresh temp directory.
fn create_same_text_files() -> TempDir {
    let dir = TempDir::with_prefix("cocomo_ep_same").unwrap();
    fs::write(dir.path().join("a.txt"), "identical content\n").unwrap();
    fs::write(dir.path().join("b.txt"), "identical content\n").unwrap();
    dir
}

// ---------------------------------------------------------------------------
// Cross-provider pairs: mixed pairs resolve per side and reach I/O
// ---------------------------------------------------------------------------

mod cross_provider_pairs {
    use super::*;

    #[test]
    fn local_vs_remote_pair_reaches_remote_auth() {
        // The mixed pair passes the endpoint check and resolves per
        // side; the remote side fails credential resolution (no TTY, no
        // secret) instead of a cross-provider refusal. The `.invalid`
        // host is unreachable by design (RFC 6761), which keeps the run
        // bounded even if the environment does supply a
        // `COCOMO_FTP_PASSWORD`.
        cmd()
            .env_remove("COCOMO_FTP_USER")
            .env_remove("COCOMO_FTP_PASSWORD")
            .timeout(Duration::from_secs(30))
            .args(["dir", "compare", "./src", "ftp://ftp.invalid/pub/src"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("authentication required"))
            .stderr(predicate::str::contains("cross-provider").not());
    }

    #[test]
    fn remote_vs_local_sync_reaches_remote_auth() {
        cmd()
            .env_remove("COCOMO_FTP_USER")
            .env_remove("COCOMO_FTP_PASSWORD")
            .timeout(Duration::from_secs(30))
            .args([
                "dir",
                "sync",
                "ftp://ftp.invalid/pub/mirror",
                "./mirror",
                "--mirror-right",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("authentication required"))
            .stderr(predicate::str::contains("cross-provider").not());
    }

    #[test]
    fn two_remote_hosts_resolve_per_side() {
        // Different hosts address two providers, even under one scheme;
        // each side resolves on its own and the first fails credential
        // resolution.
        cmd()
            .env_remove("COCOMO_FTP_USER")
            .env_remove("COCOMO_FTP_PASSWORD")
            .timeout(Duration::from_secs(30))
            .args([
                "dir",
                "compare",
                "ftp://host-a.invalid/pub",
                "ftp://host-b.invalid/pub",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("authentication required"))
            .stderr(predicate::str::contains("different providers").not());
    }

    #[test]
    fn same_host_pair_shares_one_provider() {
        // Both URLs share an endpoint identity, so one provider instance
        // services both sides; the run fails on the credential resolution
        // instead (no TTY, no secret), never on a provider-count refusal.
        cmd()
            .env_remove("COCOMO_FTP_USER")
            .env_remove("COCOMO_FTP_PASSWORD")
            .timeout(Duration::from_secs(30))
            .args([
                "dir",
                "compare",
                "ftp://ftp.invalid/pub/a",
                "ftp://ftp.invalid/pub/b",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("authentication required"))
            .stderr(predicate::str::contains("cross-provider").not());
    }

    #[test]
    fn text_compare_resolves_its_sides_on_its_own() {
        // The text commands read each side through its own provider, so a
        // mixed pair is never refused as such; the remote side errors with
        // its own credential problem instead.
        let dir = create_same_text_files();
        cmd()
            .env_remove("COCOMO_FTP_USER")
            .env_remove("COCOMO_FTP_PASSWORD")
            .timeout(Duration::from_secs(30))
            .current_dir(dir.path())
            .args([
                "text",
                "compare",
                "ftp://ftp.invalid/pub/a.txt",
                "./a.txt",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("cross-provider").not());
        cmd()
            .env_remove("COCOMO_FTP_USER")
            .env_remove("COCOMO_FTP_PASSWORD")
            .timeout(Duration::from_secs(30))
            .current_dir(dir.path())
            .args([
                "text",
                "compare",
                "./a.txt",
                "ftp://ftp.invalid/pub/a.txt",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("cross-provider").not());
    }
}

// ---------------------------------------------------------------------------
// Gated schemes: scaffolding backends yield a clean error, never a panic
// ---------------------------------------------------------------------------

mod gated_schemes {
    use super::*;

    #[test]
    fn s3_pair_in_dir_compare_is_gated() {
        // Exit code 2 (not 101) also proves the gate is a clean error
        // rather than the `unimplemented!()` panic of the `S3Fs` skeleton.
        cmd()
            .args([
                "dir",
                "compare",
                "s3://my-bucket/a/main.rs",
                "s3://my-bucket/b/main.rs",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "provider for scheme `s3` is not implemented yet",
            ));
    }

    #[test]
    fn s3_mixed_pair_in_text_diff_is_gated() {
        let dir = create_same_text_files();
        cmd()
            .current_dir(dir.path())
            .args([
                "text",
                "diff",
                "s3://my-bucket/a/main.rs",
                "file://./b.txt",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "provider for scheme `s3` is not implemented yet",
            ));
    }

    #[test]
    fn webdav_pair_is_gated() {
        cmd()
            .args([
                "dir",
                "compare",
                "webdav://host.invalid/pub",
                "webdav://host.invalid/pub",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "provider for scheme `webdav` is not implemented yet",
            ));
    }

    #[test]
    fn webdavs_sync_pair_is_gated() {
        cmd()
            .args([
                "dir",
                "sync",
                "--dry-run",
                "webdavs://host.invalid/pub",
                "webdavs://host.invalid/pub",
            ])
            .assert()
            .code(2)
            .stderr(predicate::str::contains(
                "provider for scheme `webdavs` is not implemented yet",
            ));
    }

    #[test]
    fn unknown_scheme_is_rejected() {
        // A mistyped remote scheme must not silently resolve to a local
        // path.
        cmd()
            .args(["dir", "compare", "smb://host.invalid/share", "./local"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unsupported URL scheme"));
    }
}

// ---------------------------------------------------------------------------
// Rejected endpoint formats (argv-level parsing)
// ---------------------------------------------------------------------------

mod rejected_endpoint_formats {
    use super::*;

    #[test]
    fn empty_argument_is_rejected() {
        cmd()
            .args(["dir", "compare", "", "./dst"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("empty URL"));
    }

    #[test]
    fn userinfo_in_url_is_rejected() {
        cmd()
            .args(["snapshot", "capture", "ftp://user:pass@ftp.invalid/pub"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("contains credentials"));
    }

    #[test]
    fn file_url_with_host_is_rejected() {
        cmd()
            .args(["dir", "compare", "file://host.invalid/tmp/x", "./local"])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("invalid host"));
    }
}

// ---------------------------------------------------------------------------
// End-to-end runs through `file://` and bare-path endpoints
// ---------------------------------------------------------------------------

mod file_endpoints {
    use super::*;

    #[test]
    fn dir_compare_of_different_file_urls_exits_with_one() {
        let dir = create_test_dirs();
        cmd()
            .args([
                "dir",
                "compare",
                &file_url(dir.path().join("left")),
                &file_url(dir.path().join("right")),
            ])
            .assert()
            .code(1);
    }

    #[test]
    fn dir_compare_of_identical_endpoints_exits_with_zero() {
        let dir = create_test_dirs();
        let left = dir.path().join("left");
        // A `file://` URL and a bare path address the same provider, so
        // this pair is accepted (and identical, hence exit code `0`).
        cmd()
            .args(["dir", "compare", &file_url(&left), left.to_str().unwrap()])
            .assert()
            .code(0)
            .stdout(predicate::str::contains("Summary:"));
    }

    #[test]
    fn relative_file_urls_are_accepted() {
        let dir = create_same_text_files();
        cmd()
            .current_dir(dir.path())
            .args(["text", "compare", "file://./a.txt", "file://./b.txt"])
            .assert()
            .code(0)
            .stdout(predicate::str::contains("Files are identical."));
    }

    #[test]
    fn dir_sync_dry_run_with_file_urls() {
        let dir = create_test_dirs();
        cmd()
            .args([
                "dir",
                "sync",
                "--mirror-left",
                "--dry-run",
                &file_url(dir.path().join("left")),
                &file_url(dir.path().join("right")),
            ])
            .assert()
            .code(1)
            .stdout(predicate::str::contains("[DRY RUN]"));
    }

    #[test]
    fn snapshot_capture_from_file_url() {
        let dir = TempDir::with_prefix("cocomo_ep_snap").unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/a.txt"), "hello\n").unwrap();
        cmd()
            .current_dir(dir.path())
            .args(["snapshot", "capture", &file_url(dir.path().join("src"))])
            .assert()
            .code(0)
            .stdout(predicate::str::contains("Snapshot captured"));
        assert!(dir.path().join("src.snap").exists());
    }
}

// ---------------------------------------------------------------------------
// Remote endpoints against a real server (ignored by default)
// ---------------------------------------------------------------------------

/// End-to-end runs against a real FTP server. Run them manually, with a
/// reachable server and working credentials, via
///
/// ```text
/// COCOMO_TEST_FTP_URL=ftp://127.0.0.1/pub \
/// COCOMO_FTP_USER=ftp COCOMO_FTP_PASSWORD=guest \
///     cargo nextest run --run-ignored all --test cli_endpoints
/// ```
mod remote_endpoints {
    use super::*;

    /// URL of a readable directory on a reachable FTP server.
    const FTP_URL_ENV: &str = "COCOMO_TEST_FTP_URL";

    fn ftp_url() -> Option<String> {
        env::var(FTP_URL_ENV).ok().filter(|url| !url.is_empty())
    }

    #[test]
    #[ignore = "requires a reachable FTP server in COCOMO_TEST_FTP_URL"]
    fn dir_compare_of_identical_remote_dirs_exits_with_zero() {
        let Some(url) = ftp_url() else {
            eprintln!("skipping: {FTP_URL_ENV} is not set");
            return;
        };
        // Comparing the directory with itself exercises connect, login,
        // listing, and hashing on one provider and must find no
        // differences.
        cmd().args(["dir", "compare", &url, &url]).assert().code(0);
    }

    #[test]
    #[ignore = "requires a reachable FTP server in COCOMO_TEST_FTP_URL"]
    fn snapshot_capture_from_remote_url() {
        let Some(url) = ftp_url() else {
            eprintln!("skipping: {FTP_URL_ENV} is not set");
            return;
        };
        let dir = TempDir::with_prefix("cocomo_ep_remote").unwrap();
        let out = dir.path().join("remote.snap");
        cmd()
            .args(["snapshot", "capture", &url, out.to_str().unwrap()])
            .assert()
            .code(0)
            .stdout(predicate::str::contains("Snapshot captured"));
    }
}

/// End-to-end runs against a real SFTP server. Run them manually, with a
/// reachable server whose host key is in `~/.ssh/known_hosts` and working
/// credentials, via
///
/// ```text
/// COCOMO_TEST_SFTP_URL=sftp://127.0.0.1/pub \
/// COCOMO_SFTP_USER=user COCOMO_SFTP_PASSWORD=secret \
///     cargo nextest run --run-ignored all --test cli_endpoints
/// ```
mod remote_sftp_endpoints {
    use super::*;

    /// URL of a readable directory on a reachable SFTP server.
    const SFTP_URL_ENV: &str = "COCOMO_TEST_SFTP_URL";

    fn sftp_url() -> Option<String> {
        env::var(SFTP_URL_ENV).ok().filter(|url| !url.is_empty())
    }

    #[test]
    #[ignore = "requires a reachable SFTP server in COCOMO_TEST_SFTP_URL"]
    fn dir_compare_of_identical_remote_dirs_exits_with_zero() {
        let Some(url) = sftp_url() else {
            eprintln!("skipping: {SFTP_URL_ENV} is not set");
            return;
        };
        // Comparing the directory with itself exercises connect, auth,
        // listing, and hashing on one provider and must find no
        // differences.
        cmd().args(["dir", "compare", &url, &url]).assert().code(0);
    }

    #[test]
    #[ignore = "requires a reachable SFTP server in COCOMO_TEST_SFTP_URL"]
    fn snapshot_capture_from_remote_url() {
        let Some(url) = sftp_url() else {
            eprintln!("skipping: {SFTP_URL_ENV} is not set");
            return;
        };
        let dir = TempDir::with_prefix("cocomo_ep_remote").unwrap();
        let out = dir.path().join("remote.snap");
        cmd()
            .args(["snapshot", "capture", &url, out.to_str().unwrap()])
            .assert()
            .code(0)
            .stdout(predicate::str::contains("Snapshot captured"));
    }
}
