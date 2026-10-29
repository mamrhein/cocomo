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
//! Unlike the TUI, the GUI does not run the initial comparison eagerly:
//! the folder-compare view loads its comparison asynchronously and
//! surfaces failures in the UI (see [`crate::state::AppState`]), which
//! is the GUI equivalent of the TUI's startup comparison failures.
//! Argument-level problems (unresolvable endpoints, wrong types) still
//! exit with a message before the window is opened.
//!
//! Remote endpoints resolve through the shared `cocomo-lib` resolver
//! (profile store, environment, then keychain). A GUI has no reliable
//! terminal, so credential entry never prompts: whatever the profile
//! store, the environment, and the keychain do not supply must come from
//! a profile given with `--profile`. A dialog-backed prompter is planned
//! as a follow-up.

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
    /// Left directory path or URL (e.g. `ftp://host/pub/src`).
    #[arg(requires = "right")]
    pub left: Option<String>,

    /// Right directory path or URL (e.g. `ftp://host/pub/src`).
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

    /// A valid directory pair was given: open a folder-compare tab for
    /// these endpoints. The comparison itself runs asynchronously once
    /// the tab is rendered.
    Session {
        /// Left-side directory endpoint.
        left: Endpoint,
        /// Right-side directory endpoint.
        right: Endpoint,
        /// Whether to compare file content (hashing) instead of using
        /// metadata only.
        content: bool,
    },
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
    /// both sides to endpoints and validate them.
    ///
    /// Must be called with a tokio runtime entered (the GUI creates its
    /// runtime before resolving startup so remote providers can connect).
    ///
    /// Argument-level problems (an unparsable URL, an endpoint that
    /// cannot be resolved or is not a directory) are returned as errors
    /// so the binary exits with a message; a comparison that fails later
    /// on otherwise valid endpoints is surfaced in the UI by the
    /// session's [`AppState`](crate::state::AppState).
    pub(crate) async fn startup(&self) -> Result<Startup> {
        // clap's `requires` guarantees the positional pair comes together,
        // so anything but "both present" means "no paths given".
        let (Some(left), Some(right)) =
            (self.left.as_deref(), self.right.as_deref())
        else {
            return Ok(Startup::Welcome);
        };

        let left = resolve_startup_side(left, self.profile.as_deref()).await?;
        let right =
            resolve_startup_side(right, self.profile.as_deref()).await?;

        // Validate that both endpoints exist and are of the same type.
        let left_meta = left.fs().metadata_path(left.path()).await?;
        let right_meta = right.fs().metadata_path(right.path()).await?;

        if left_meta.is_file() && right_meta.is_file() {
            return Err(anyhow::anyhow!(
                "Comparing individual files is not yet implemented. Please \
                 provide directories."
            ));
        }

        if !left_meta.is_dir || !right_meta.is_dir {
            return Err(anyhow::anyhow!(
                "Both paths must be directories, or both must be files."
            ));
        }

        Ok(Startup::Session {
            left,
            right,
            content: self.content,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_specified_pair_is_a_parse_error() {
        assert!(Cli::try_parse_from(["cocomo-gui", "/a"]).is_err());
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
        assert!(matches!(
            cli.startup().await.unwrap(),
            Startup::Session { .. }
        ));
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
        assert!(matches!(
            cli.startup().await.unwrap(),
            Startup::Session { .. }
        ));
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
    async fn two_files_are_rejected() {
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
        let Err(err) = cli.startup().await else {
            panic!("a file pair must fail startup");
        };
        assert!(
            err.to_string().contains("not yet implemented"),
            "error should explain that files are unsupported: {err}"
        );
    }
}
