// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Command-line argument definitions and startup policy.
//!
//! [`Cli`] parses the arguments; [`Cli::startup`] resolves them into a
//! [`Startup`] decision — parse each side as a path or URL, resolve it to
//! an [`Endpoint`], and validate the pair — so `main` only has to seed the
//! session manager and open the window.
//!
//! Zero, one, or two paths are accepted: none opens the welcome screen,
//! one opens the view matching the node type with only the left side set,
//! and two open the pair in that view.
//!
//! Unlike the TUI, the GUI does not run the initial comparison eagerly:
//! the folder-compare view loads its comparison asynchronously and
//! surfaces failures in the UI (see [`crate::state::AppState`]), which
//! is the GUI equivalent of the TUI's startup comparison failures.
//! Argument-level problems (unresolvable endpoints, wrong types) still
//! exit with a message before the window is opened.
//!
//! Remote endpoints resolve through the shared `cocomo-lib` resolver
//! (profile store, environment, then keychain). Command-line endpoints
//! are resolved before any window exists, so a [`NullPrompter`] backs
//! that first resolution: whatever the profile store, the environment,
//! and the keychain do not supply must come from a profile given with
//! `--profile`. Once the window is up, runtime opens use the
//! dialog-backed [`crate::credentials::DialogPrompter`].

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use cocomo_lib::{
    Endpoint, NodeFileSystem, NullPrompter, Url, resolve_provider,
};

/// cocomo — COmpare, COpy & MOve directories and files.
#[derive(Parser, Debug)]
#[command(name = "cocomo-gui", version, about)]
pub(crate) struct Cli {
    /// Left directory or file path or URL (e.g. `ftp://host/pub/src`). A
    /// single path opens the matching view with an empty right side.
    pub left: Option<String>,

    /// Right directory or file path or URL (e.g. `ftp://host/pub/src`).
    pub right: Option<String>,

    /// Compare file content (using hashing).
    #[arg(short = 'c', long, default_value_t = false)]
    pub content: bool,

    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(short = 'P', long)]
    pub profile: Option<String>,
}

/// The startup decision derived from the parsed [`Cli`] arguments.
pub(crate) enum Startup {
    /// No paths were given: start with the Home tab, the start screen
    /// for launching new sessions and reopening saved ones.
    Welcome,

    /// Valid paths were given: open a tab for them at startup. Boxed to
    /// keep the enum small (`StartupSession` is larger than the other
    /// variants' payloads).
    Session(Box<StartupSession>),
}

/// The session a run starts with: the view's sides and compare mode.
///
/// The comparison itself runs asynchronously once the tab is rendered.
pub(crate) struct StartupSession {
    /// Left-side endpoint.
    pub left: Endpoint,
    /// Right-side endpoint; `None` when only one path was given, leaving
    /// the right side of the view empty for the user to fill in.
    pub right: Option<Endpoint>,
    /// Whether the given nodes are directories: `true` opens the
    /// folder-compare view, `false` the file view the left file's format
    /// maps to.
    pub is_dir: bool,
    /// Whether to compare file content (hashing) instead of using
    /// metadata only.
    pub content: bool,
}

/// Resolve one startup argument into an [`Endpoint`].
///
/// Local paths are canonicalized so the session shows absolute paths and
/// symlinks are followed; remote URLs resolve their provider through the
/// shared resolver chain, authenticated with `profile_id` if given. A
/// [`NullPrompter`] backs the resolution: the GUI must never block on a
/// terminal prompt.
async fn resolve_startup_side(
    arg: &str,
    profile_id: Option<&str>,
) -> Result<Endpoint> {
    let url = Url::parse(arg)?;
    if url.scheme == Url::LOCAL_SCHEME {
        let path = fs_err::tokio::canonicalize(&url.path).await?;
        return Ok(Endpoint::local(path));
    }
    let fs = Arc::new(resolve_provider(&url, profile_id, &NullPrompter)?);
    Ok(Endpoint::new(fs, url))
}

impl Cli {
    /// Resolve the parsed arguments into a [`Startup`] decision: resolve
    /// the given sides to endpoints and validate them.
    ///
    /// Must be called with a tokio runtime entered (the GUI creates its
    /// runtime before resolving startup so remote providers can connect).
    ///
    /// Argument-level problems (an unparsable URL, an endpoint that
    /// cannot be resolved, a mixed directory/file pair) are returned as
    /// errors so the binary exits with a message; a comparison that fails
    /// later on otherwise valid endpoints is surfaced in the UI by the
    /// session's [`AppState`](crate::state::AppState).
    pub(crate) async fn startup(&self) -> Result<Startup> {
        let profile = self.profile.as_deref();
        // clap fills the positionals in order, so a missing left means no
        // paths were given at all.
        let Some(left) = self.left.as_deref() else {
            return Ok(Startup::Welcome);
        };

        let left = resolve_startup_side(left, profile).await?;
        let left_meta = left.fs().metadata_path(left.path()).await?;

        // One path: the view opens with only the left side set.
        let Some(right) = self.right.as_deref() else {
            return Ok(Startup::Session(Box::new(StartupSession {
                left,
                right: None,
                is_dir: left_meta.is_dir,
                content: self.content,
            })));
        };

        let right = resolve_startup_side(right, profile).await?;
        let right_meta = right.fs().metadata_path(right.path()).await?;

        // Validate that both endpoints exist and are of the same type.
        let same_kind = (left_meta.is_dir && right_meta.is_dir)
            || (left_meta.is_file() && right_meta.is_file());
        if !same_kind {
            return Err(anyhow::anyhow!(
                "Both paths must be directories, or both must be files."
            ));
        }

        Ok(Startup::Session(Box::new(StartupSession {
            left,
            right: Some(right),
            is_dir: left_meta.is_dir,
            content: self.content,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_path_parses() {
        let cli = Cli::try_parse_from(["cocomo-gui", "/a"]).unwrap();
        assert_eq!(cli.left.as_deref(), Some("/a"));
        assert!(cli.right.is_none());
    }

    #[test]
    fn no_paths_parses() {
        let cli = Cli::try_parse_from(["cocomo-gui"]).unwrap();
        assert!(cli.left.is_none());
        assert!(cli.right.is_none());
        assert!(!cli.content);
        assert!(cli.profile.is_none());
    }

    #[test]
    fn full_pair_parses() {
        let cli =
            Cli::try_parse_from(["cocomo-gui", "/a", "/b", "-c"]).unwrap();
        assert_eq!(cli.left.as_deref(), Some("/a"));
        assert_eq!(cli.right.as_deref(), Some("/b"));
        assert!(cli.content);
    }

    #[test]
    fn url_pair_and_profile_parse() {
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            "ftp://host.example/pub",
            "./mirror",
            "--profile",
            "prod",
        ])
        .unwrap();
        assert_eq!(cli.left.as_deref(), Some("ftp://host.example/pub"));
        assert_eq!(cli.right.as_deref(), Some("./mirror"));
        assert_eq!(cli.profile.as_deref(), Some("prod"));
    }

    #[tokio::test]
    async fn no_paths_starts_at_the_welcome_screen() {
        let cli = Cli::try_parse_from(["cocomo-gui"]).unwrap();
        assert!(matches!(cli.startup().await.unwrap(), Startup::Welcome));
    }

    #[tokio::test]
    async fn two_directories_start_with_a_session() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            left.path().to_str().unwrap(),
            right.path().to_str().unwrap(),
        ])
        .unwrap();
        let Startup::Session(session) = cli.startup().await.unwrap() else {
            panic!("a directory pair must start a session");
        };
        assert!(session.is_dir);
        assert!(session.right.is_some());
    }

    #[tokio::test]
    async fn one_directory_leaves_the_right_side_empty() {
        let left = tempfile::tempdir().unwrap();
        let cli =
            Cli::try_parse_from(["cocomo-gui", left.path().to_str().unwrap()])
                .unwrap();
        let Startup::Session(session) = cli.startup().await.unwrap() else {
            panic!("a single directory must start a session");
        };
        assert!(session.is_dir);
        assert!(session.right.is_none());
    }

    #[tokio::test]
    async fn one_file_leaves_the_right_side_empty() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("left.txt");
        fs_err::tokio::write(&file, b"a").await.unwrap();
        let cli = Cli::try_parse_from(["cocomo-gui", file.to_str().unwrap()])
            .unwrap();
        let Startup::Session(session) = cli.startup().await.unwrap() else {
            panic!("a single file must start a session");
        };
        assert!(!session.is_dir);
        assert!(session.right.is_none());
    }

    #[tokio::test]
    async fn file_url_pair_starts_a_session() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            &format!("file://{}", left.path().display()),
            &format!("file://{}", right.path().display()),
        ])
        .unwrap();
        assert!(matches!(cli.startup().await.unwrap(), Startup::Session(_)));
    }

    #[tokio::test]
    async fn missing_single_path_is_a_startup_error() {
        let cli = Cli::try_parse_from(["cocomo-gui", "/definitely-not-here"])
            .unwrap();
        let Err(err) = cli.startup().await else {
            panic!("a missing path must fail startup");
        };
        assert!(
            err.to_string().contains("definitely-not-here"),
            "error should name the offending path: {err}"
        );
    }

    #[tokio::test]
    async fn missing_path_is_a_startup_error() {
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            "/definitely-not-here",
            right.path().to_str().unwrap(),
        ])
        .unwrap();
        let Err(err) = cli.startup().await else {
            panic!("a missing path must fail startup");
        };
        assert!(
            err.to_string().contains("definitely-not-here"),
            "error should name the offending path: {err}"
        );
    }

    #[tokio::test]
    async fn unsupported_scheme_is_a_startup_error() {
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            "gopher://host.example/pub",
            right.path().to_str().unwrap(),
        ])
        .unwrap();
        let Err(err) = cli.startup().await else {
            panic!("an unsupported scheme must fail startup");
        };
        assert!(
            err.to_string().contains("unsupported URL scheme"),
            "error should name the offending scheme: {err}"
        );
    }

    #[tokio::test]
    async fn two_files_start_a_file_session() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left.txt");
        let right = dir.path().join("right.txt");
        fs_err::tokio::write(&left, b"a").await.unwrap();
        fs_err::tokio::write(&right, b"b").await.unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            left.to_str().unwrap(),
            right.to_str().unwrap(),
        ])
        .unwrap();
        let Startup::Session(session) = cli.startup().await.unwrap() else {
            panic!("a file pair must start a session");
        };
        assert!(!session.is_dir);
        assert!(session.right.is_some());
    }

    #[tokio::test]
    async fn mixed_dir_and_file_pair_is_a_startup_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("left.txt");
        fs_err::tokio::write(&file, b"a").await.unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            dir.path().to_str().unwrap(),
            file.to_str().unwrap(),
        ])
        .unwrap();
        let Err(err) = cli.startup().await else {
            panic!("a mixed pair must fail startup");
        };
        assert!(
            err.to_string().contains("both must be files"),
            "error should explain the expected types: {err}"
        );
    }
}
