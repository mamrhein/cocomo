// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared endpoint resolution for the frontends.
//!
//! Every frontend (CLI, TUI, GUI) accepts path-like endpoint arguments —
//! bare paths, `file://` URLs, or remote URLs such as `ftp://host/pub` —
//! and must turn them into live [`Provider`] instances. This module owns
//! that production resolution: it parses the argument with [`Url::parse`],
//! consults the default profile store, the environment, and the
//! password-safe, and hands the remainder to [`Provider::resolve`].
//!
//! The interactive [`Prompter`] is supplied by the caller so that a
//! terminal frontend can pass [`TtyPrompter`](crate::TtyPrompter) while a
//! GUI passes [`NullPrompter`](crate::NullPrompter) (or, in the future, a
//! dialog-backed prompter).
//!
//! The profile store is only opened for remote endpoints, so local-only
//! runs neither read nor create the profile configuration files.

use std::{path::PathBuf, sync::Arc};

use thiserror::Error;

use crate::{
    profile::{ProfileError, ProfileStore},
    provider::{Provider, ProviderError},
    secrets::{Prompter, Secrets},
    url::{Url, UrlError},
};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur while resolving an endpoint argument to a
/// [`Provider`].
#[derive(Debug, Error)]
pub enum ResolverError {
    /// The argument could not be parsed as a URL or path.
    #[error(transparent)]
    Url(#[from] UrlError),

    /// No provider could be resolved for the parsed URL.
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// The profile store could not be opened or the named profile could
    /// not be loaded.
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

// ---------------------------------------------------------------------------
// Endpoint identity
// ---------------------------------------------------------------------------

/// The identity of the endpoint a [`Url`] addresses: two URLs with equal
/// identities are serviced by the same provider, so one filesystem
/// instance can serve both sides of a comparison.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointIdentity {
    /// URL scheme (`file`, `ftp`, `sftp`, …).
    pub scheme: String,
    /// Authority host, `None` for local URLs.
    pub host: Option<String>,
    /// Effective port (explicit or scheme default), `None` for local URLs.
    pub port: Option<u16>,
}

/// Return the identity of the endpoint `url` addresses.
pub fn endpoint_identity(url: &Url) -> EndpointIdentity {
    EndpointIdentity {
        scheme: url.scheme.clone(),
        host: url.host.clone(),
        port: url.effective_port(),
    }
}

// ---------------------------------------------------------------------------
// Provider resolution
// ---------------------------------------------------------------------------

/// Resolve the provider that services `url`, letting [`Provider::resolve`]
/// consult the default profile store, the environment, and the
/// password-safe. `prompter` supplies interactive credential entry where
/// the frontend has a suitable input channel.
pub fn resolve_provider(
    url: &Url,
    profile_id: Option<&str>,
    prompter: &dyn Prompter,
) -> Result<Provider, ResolverError> {
    let store = if url.scheme == Url::LOCAL_SCHEME {
        None
    } else {
        Some(ProfileStore::open_default()?)
    };
    let secrets = Secrets::new();
    let provider = Provider::resolve(
        url,
        store.as_ref(),
        profile_id,
        &secrets,
        prompter,
    )?;
    Ok(provider)
}

/// The filesystem(s) addressing the two sides of a pair command. Both
/// variants hold `Arc`s, so the size difference is a single pointer and no
/// padding is needed.
pub enum EndpointPair<N> {
    /// One instance addressing both sides: the endpoints share an
    /// identity, so one connection services both and cross-boundary
    /// transfers keep the same-provider fast path.
    Shared(Arc<N>),
    /// Two distinct instances: the endpoints address different providers,
    /// so each side resolves and connects on its own.
    Separate(Arc<N>, Arc<N>),
}

/// Resolve one path-like argument into a filesystem instance and the path
/// within it.
pub fn resolve_endpoint(
    arg: &str,
    profile_id: Option<&str>,
    prompter: &dyn Prompter,
) -> Result<(Arc<Provider>, PathBuf), ResolverError> {
    let url = Url::parse(arg)?;
    let fs = Arc::new(resolve_provider(&url, profile_id, prompter)?);
    Ok((fs, url.path))
}

/// Resolve a pair of path-like arguments into the filesystem(s) that
/// service them plus the in-provider path of each side. Endpoints with
/// equal identities share one instance (one connection); equal identities
/// resolved per side would give two providers with the same label, aliasing
/// the shared `ContentCache` keys. Different identities resolve to one
/// instance per side, so a mixed pair such as `ftp://host/pub` vs. `./src`
/// is accepted and each side runs against its own backend.
pub fn resolve_endpoint_pair(
    left_arg: &str,
    right_arg: &str,
    profile_id: Option<&str>,
    prompter: &dyn Prompter,
) -> Result<(EndpointPair<Provider>, PathBuf, PathBuf), ResolverError> {
    let left_url = Url::parse(left_arg)?;
    let right_url = Url::parse(right_arg)?;
    let pair = if endpoint_identity(&left_url) == endpoint_identity(&right_url)
    {
        EndpointPair::Shared(Arc::new(resolve_provider(
            &left_url, profile_id, prompter,
        )?))
    } else {
        EndpointPair::Separate(
            Arc::new(resolve_provider(&left_url, profile_id, prompter)?),
            Arc::new(resolve_provider(&right_url, profile_id, prompter)?),
        )
    };
    Ok((pair, left_url.path, right_url.path))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NullPrompter;

    #[test]
    fn bare_paths_share_the_local_identity() {
        let left = Url::parse("./src").unwrap();
        let right = Url::parse("/tmp/other").unwrap();
        assert_eq!(endpoint_identity(&left), endpoint_identity(&right));
    }

    #[test]
    fn default_port_matches_the_explicit_default() {
        let implicit = Url::parse("ftp://host.example/pub").unwrap();
        let explicit = Url::parse("ftp://host.example:21/pub").unwrap();
        assert_eq!(endpoint_identity(&implicit), endpoint_identity(&explicit));
    }

    #[test]
    fn a_non_default_port_is_a_different_identity() {
        let standard = Url::parse("ftp://host.example/pub").unwrap();
        let custom = Url::parse("ftp://host.example:2121/pub").unwrap();
        assert_ne!(endpoint_identity(&standard), endpoint_identity(&custom));
    }

    #[test]
    fn different_hosts_are_different_identities() {
        let a = Url::parse("sftp://host-a.example/data").unwrap();
        let b = Url::parse("sftp://host-b.example/data").unwrap();
        assert_ne!(endpoint_identity(&a), endpoint_identity(&b));
    }

    #[test]
    fn a_local_path_resolves_to_the_local_provider() {
        let (fs, path) =
            resolve_endpoint("./some/dir", None, &NullPrompter).unwrap();
        assert!(matches!(*fs, Provider::Local(_)));
        assert_eq!(path, PathBuf::from("./some/dir"));
    }

    #[test]
    fn a_local_pair_shares_one_provider_instance() {
        let (pair, left, right) =
            resolve_endpoint_pair("./a", "./b", None, &NullPrompter).unwrap();
        assert!(matches!(pair, EndpointPair::Shared(_)));
        assert_eq!(left, PathBuf::from("./a"));
        assert_eq!(right, PathBuf::from("./b"));
    }

    #[test]
    fn credentials_in_a_url_are_refused() {
        let err = resolve_endpoint(
            "ftp://user:pw@host.example/p",
            None,
            &NullPrompter,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ResolverError::Url(UrlError::CredentialsInUrl { .. })
        ));
    }

    #[test]
    fn an_unsupported_scheme_is_refused() {
        let err =
            resolve_endpoint("gopher://host.example/p", None, &NullPrompter)
                .unwrap_err();
        assert!(matches!(
            err,
            ResolverError::Url(UrlError::UnsupportedScheme { .. })
        ));
    }
}
x
