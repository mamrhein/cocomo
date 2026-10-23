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
//! [`Startup`] decision — canonicalize and validate the paths, then run
//! the initial comparison — so [`crate::run`] only has to seed
//! [`APP_STATE`](crate::state::APP_STATE) and enter the loop.

use std::path::PathBuf;

use clap::Parser;

use crate::{controller::compare, error::TuiError, state::SessionView};

/// cocomo — COmpare, COpy & MOve directories and files.
#[derive(Parser, Debug)]
#[command(name = "cocomo-tui", version, about)]
pub(crate) struct Cli {
    /// Left directory path.
    #[arg(requires = "right")]
    pub left: Option<PathBuf>,

    /// Right directory path.
    pub right: Option<PathBuf>,

    /// Compare file content (using hashing).
    #[arg(short = 'c', long, default_value_t = false)]
    pub content: bool,
}

/// The startup decision derived from the parsed [`Cli`] arguments.
pub(crate) enum Startup {
    /// No paths were given: start at the welcome screen.
    Welcome,

    /// The paths were given and the initial comparison succeeded: start
    /// with this session. Boxed to keep the enum small (`SessionView` is
    /// a large struct).
    Session(Box<SessionView>),

    /// The paths were valid but the initial comparison failed: start at
    /// the welcome screen and surface the typed error in the UI, instead
    /// of adding a session whose comparison failed.
    ComparisonFailed(TuiError),
}

impl Cli {
    /// Resolve the parsed arguments into a [`Startup`] decision:
    /// canonicalize and validate the paths, then run the initial
    /// comparison.
    ///
    /// Argument-level problems (a path that cannot be resolved or is not
    /// a directory) are returned as errors so the binary exits with a
    /// message; a comparison that fails on otherwise valid paths is
    /// reported as [`Startup::ComparisonFailed`] so the UI can show it.
    pub(crate) async fn startup(&self) -> anyhow::Result<Startup> {
        // clap's `requires` guarantees the positional pair comes together,
        // so anything but "both present" means "no paths given".
        let (Some(left), Some(right)) =
            (self.left.as_deref(), self.right.as_deref())
        else {
            return Ok(Startup::Welcome);
        };

        let left = fs_err::tokio::canonicalize(left).await?;
        let right = fs_err::tokio::canonicalize(right).await?;

        // Validate that both paths exist and are of the same type.
        let left_meta = fs_err::tokio::metadata(&left).await?;
        let right_meta = fs_err::tokio::metadata(&right).await?;

        if left_meta.is_file() && right_meta.is_file() {
            return Err(anyhow::anyhow!(
                "Comparing individual files is not yet implemented. Please \
                 provide directories."
            ));
        }

        if !left_meta.is_dir() || !right_meta.is_dir() {
            return Err(anyhow::anyhow!(
                "Both paths must be directories, or both must be files."
            ));
        }

        match compare(&left, &right, self.content).await {
            Ok(comparison) => {
                let mut session =
                    SessionView::new_dir_compare(left, right, self.content);
                session.set_comparison(Some(comparison));
                Ok(Startup::Session(Box::new(session)))
            }
            Err(err) => Ok(Startup::ComparisonFailed(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_specified_pair_is_a_parse_error() {
        assert!(Cli::try_parse_from(["cocomo-tui", "/a"]).is_err());
    }

    #[test]
    fn no_paths_parses() {
        let cli = Cli::try_parse_from(["cocomo-tui"]).unwrap();
        assert!(cli.left.is_none());
        assert!(cli.right.is_none());
        assert!(!cli.content);
    }

    #[test]
    fn full_pair_parses() {
        let cli =
            Cli::try_parse_from(["cocomo-tui", "/a", "/b", "-c"]).unwrap();
        assert_eq!(cli.left.as_deref(), Some(std::path::Path::new("/a")));
        assert_eq!(cli.right.as_deref(), Some(std::path::Path::new("/b")));
        assert!(cli.content);
    }

    #[tokio::test]
    async fn no_paths_starts_at_the_welcome_screen() {
        let cli = Cli::try_parse_from(["cocomo-tui"]).unwrap();
        assert!(matches!(cli.startup().await.unwrap(), Startup::Welcome));
    }

    #[tokio::test]
    async fn two_directories_start_with_a_session() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-tui",
            left.path().to_str().unwrap(),
            right.path().to_str().unwrap(),
        ])
        .unwrap();
        assert!(matches!(cli.startup().await.unwrap(), Startup::Session(_)));
    }

    #[tokio::test]
    async fn missing_path_is_a_startup_error() {
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-tui",
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
    async fn two_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left.txt");
        let right = dir.path().join("right.txt");
        fs_err::tokio::write(&left, b"a").await.unwrap();
        fs_err::tokio::write(&right, b"b").await.unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-tui",
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
