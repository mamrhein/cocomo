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
//! [`Startup`] decision — canonicalize and validate the paths — so
//! `main` only has to seed the session manager and open the window.
//!
//! Unlike the TUI, the GUI does not run the initial comparison eagerly:
//! the folder-compare view loads its comparison asynchronously and
//! surfaces failures in the UI (see [`crate::state::AppState`]), which
//! is the GUI equivalent of the TUI's `Startup::ComparisonFailed`
//! handling. Argument-level problems (unresolvable paths, wrong types)
//! still exit with a message before the window is opened.

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

/// cocomo — COmpare, COpy & MOve directories and files.
#[derive(Parser, Debug)]
#[command(name = "cocomo-gui", version, about)]
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
    /// No paths were given: start with an empty untitled tab (the GUI
    /// equivalent of the TUI's welcome screen).
    Welcome,

    /// A valid directory pair was given: open a folder-compare tab for
    /// these paths. The comparison itself runs asynchronously once the
    /// tab is rendered.
    Session {
        /// Canonicalized left directory path.
        left: PathBuf,
        /// Canonicalized right directory path.
        right: PathBuf,
        /// Whether to compare file content (hashing) instead of using
        /// metadata only.
        content: bool,
    },
}

impl Cli {
    /// Resolve the parsed arguments into a [`Startup`] decision:
    /// canonicalize and validate the paths.
    ///
    /// Argument-level problems (a path that cannot be resolved or is not
    /// a directory) are returned as errors so the binary exits with a
    /// message; a comparison that fails later on otherwise valid paths is
    /// surfaced in the UI by the session's
    /// [`AppState`](crate::state::AppState).
    pub(crate) fn startup(&self) -> Result<Startup> {
        // clap's `requires` guarantees the positional pair comes together,
        // so anything but "both present" means "no paths given".
        let (Some(left), Some(right)) =
            (self.left.as_deref(), self.right.as_deref())
        else {
            return Ok(Startup::Welcome);
        };

        let left = fs_err::canonicalize(left)?;
        let right = fs_err::canonicalize(right)?;

        // Validate that both paths exist and are of the same type.
        let left_meta = fs_err::metadata(&left)?;
        let right_meta = fs_err::metadata(&right)?;

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
    }

    #[test]
    fn full_pair_parses() {
        let cli =
            Cli::try_parse_from(["cocomo-gui", "/a", "/b", "-c"]).unwrap();
        assert_eq!(cli.left.as_deref(), Some(std::path::Path::new("/a")));
        assert_eq!(cli.right.as_deref(), Some(std::path::Path::new("/b")));
        assert!(cli.content);
    }

    #[test]
    fn no_paths_starts_at_the_welcome_screen() {
        let cli = Cli::try_parse_from(["cocomo-gui"]).unwrap();
        assert!(matches!(cli.startup().unwrap(), Startup::Welcome));
    }

    #[test]
    fn two_directories_start_with_a_session() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            left.path().to_str().unwrap(),
            right.path().to_str().unwrap(),
        ])
        .unwrap();
        assert!(matches!(cli.startup().unwrap(), Startup::Session { .. }));
    }

    #[test]
    fn content_flag_reaches_the_session() {
        let left = tempfile::tempdir().unwrap();
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            left.path().to_str().unwrap(),
            right.path().to_str().unwrap(),
            "-c",
        ])
        .unwrap();
        let Startup::Session { content, .. } = cli.startup().unwrap() else {
            panic!("a directory pair must start a session");
        };
        assert!(content);
    }

    #[test]
    fn missing_path_is_a_startup_error() {
        let right = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            "/definitely-not-here",
            right.path().to_str().unwrap(),
        ])
        .unwrap();
        let Err(err) = cli.startup() else {
            panic!("a missing path must fail startup");
        };
        assert!(
            err.to_string().contains("definitely-not-here"),
            "error should name the offending path: {err}"
        );
    }

    #[test]
    fn two_files_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let left = dir.path().join("left.txt");
        let right = dir.path().join("right.txt");
        fs_err::write(&left, b"a").unwrap();
        fs_err::write(&right, b"b").unwrap();
        let cli = Cli::try_parse_from([
            "cocomo-gui",
            left.to_str().unwrap(),
            right.to_str().unwrap(),
        ])
        .unwrap();
        let Err(err) = cli.startup() else {
            panic!("a file pair must fail startup");
        };
        assert!(
            err.to_string().contains("not yet implemented"),
            "error should explain that files are unsupported: {err}"
        );
    }
}
