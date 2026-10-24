// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use cocomo_lib::{
    error::FsError, profile::ProfileError, provider::ProviderError,
    resolver::ResolverError, url::UrlError,
};

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Result of a command that may or may not find differences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffResult {
    NoDiffs,
    HasDiffs,
}

/// Errors that can occur while resolving an endpoint or reading from a
/// resolved provider.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error(transparent)]
    Url(#[from] UrlError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    /// Non-fatal filesystem errors collected while walking the trees. The
    /// result may be incomplete; each error is printed to stderr before
    /// this variant is returned.
    #[error("operation incomplete due to filesystem errors")]
    FsErrors(Vec<FsError>),
}

/// Map a shared [`ResolverError`] onto the corresponding [`CliError`]
/// variant so endpoint resolution failures display exactly like the
/// individual errors they wrap.
impl From<ResolverError> for CliError {
    fn from(err: ResolverError) -> Self {
        match err {
            ResolverError::Url(e) => Self::Url(e),
            ResolverError::Provider(e) => Self::Provider(e),
            ResolverError::Profile(e) => Self::Profile(e),
        }
    }
}

/// Convert a filesystem error into a [`CliError`], printing the individual
/// errors of an incomplete operation to stderr before returning them.
pub(crate) fn fs_error_to_cli(e: FsError) -> CliError {
    match e {
        FsError::Incomplete { errors } => {
            for err in &errors {
                eprintln!("error: {err}");
            }
            CliError::FsErrors(errors)
        }
        other => CliError::Fs(other),
    }
}
