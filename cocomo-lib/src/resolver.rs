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

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use thiserror::Error;

use crate::{
    local::LocalFs,
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
// Endpoint handle
// ---------------------------------------------------------------------------

/// One side of a comparison: the provider servicing it and the location
/// within that provider.
///
/// An endpoint is a cheap handle: the provider is shared (`Arc`), so
/// navigating into a subdirectory or up to the parent clones only the
/// parsed [`Url`]. Local endpoints all share one process-wide [`LocalFs`],
/// so two local sides of a comparison address the same provider instance
/// and keep the same-provider fast paths of compare and sync.
#[derive(Clone)]
pub struct Endpoint {
    fs: Arc<Provider>,
    url: Url,
}

impl Endpoint {
    /// The provider instance shared by every local endpoint.
    fn local_provider() -> Arc<Provider> {
        static LOCAL: OnceLock<Arc<Provider>> = OnceLock::new();
        LOCAL
            .get_or_init(|| Arc::new(Provider::Local(LocalFs::new("local"))))
            .clone()
    }

    /// A local endpoint at `path`.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self {
            fs: Self::local_provider(),
            url: Url::from_path(path.into()),
        }
    }

    /// An endpoint on an already-resolved provider, located by `url`.
    pub fn new(fs: Arc<Provider>, url: Url) -> Self {
        Self { fs, url }
    }

    /// Rebuild an endpoint from the provider reference and path recorded
    /// in a saved session.
    ///
    /// The reference is the bare `file` scheme (or the legacy `local`
    /// label) for local endpoints, otherwise the URL authority without the
    /// path. Remote refs re-resolve their credentials through `prompter`
    /// for whatever the profile store, the environment, and the keychain
    /// do not supply.
    pub fn from_ref(
        provider: &str,
        path: PathBuf,
        prompter: &dyn Prompter,
    ) -> Result<Self, ResolverError> {
        if provider == Url::LOCAL_SCHEME || provider == "local" {
            return Ok(Self::local(path));
        }
        let mut url = Url::parse(provider)?;
        url.path = path;
        let fs = Arc::new(resolve_provider(&url, None, prompter)?);
        Ok(Self { fs, url })
    }

    /// The provider servicing this side.
    pub fn fs(&self) -> &Provider {
        &self.fs
    }

    /// A shared handle to the servicing provider.
    pub fn fs_arc(&self) -> Arc<Provider> {
        self.fs.clone()
    }

    /// The path within the provider.
    pub fn path(&self) -> &Path {
        &self.url.path
    }

    /// Whether this side is on the local filesystem.
    pub fn is_local(&self) -> bool {
        self.url.scheme == Url::LOCAL_SCHEME
    }

    /// Return `true` when both sides are serviced by the same provider
    /// instance, which enables the same-filesystem fast paths.
    pub fn same_provider(&self, other: &Endpoint) -> bool {
        Arc::ptr_eq(&self.fs, &other.fs)
    }

    /// Display form: the plain path for local endpoints, the full URL for
    /// remote ones.
    pub fn display(&self) -> String {
        self.url.to_string()
    }

    /// The same endpoint located at `path`.
    pub fn with_path(&self, path: impl Into<PathBuf>) -> Self {
        let mut url = self.url.clone();
        url.path = path.into();
        Self {
            fs: self.fs.clone(),
            url,
        }
    }

    /// The same endpoint one directory up; `None` at the provider root.
    pub fn parent(&self) -> Option<Self> {
        Some(self.with_path(self.url.path.parent()?))
    }

    /// The same endpoint with `name` appended to its path.
    pub fn join(&self, name: &str) -> Self {
        let mut path = self.url.path.clone();
        path.push(name);
        self.with_path(path)
    }

    /// The provider identity recorded in saved sessions: the bare `file`
    /// scheme for local endpoints, the scheme plus authority (without the
    /// path) for remote ones.
    pub fn provider_ref(&self) -> String {
        if self.is_local() {
            return Url::LOCAL_SCHEME.to_string();
        }
        let host = self.url.host.clone().unwrap_or_default();
        // Bracket IPv6 literals so the authority re-parses as a host.
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host
        };
        match self.url.port {
            Some(port) => {
                format!("{}://{host}:{port}", self.url.scheme)
            }
            None => format!("{}://{host}", self.url.scheme),
        }
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Endpoint {
    /// Two endpoints are equal when they address the same provider
    /// instance at the same location.
    fn eq(&self, other: &Self) -> bool {
        self.same_provider(other) && self.url == other.url
    }
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

    #[test]
    fn local_endpoints_share_one_provider_instance() {
        let left = Endpoint::local("/a");
        let right = Endpoint::local("/b");
        assert!(left.same_provider(&right));
        assert!(left.is_local());
        assert_eq!(left.provider_ref(), Url::LOCAL_SCHEME);
    }

    #[test]
    fn a_saved_local_ref_rebuilds_a_local_endpoint() {
        for provider in ["local", "file"] {
            let endpoint =
                Endpoint::from_ref(provider, "/x/y".into(), &NullPrompter)
                    .unwrap();
            assert!(endpoint.is_local());
            assert_eq!(endpoint.path(), Path::new("/x/y"));
        }
    }

    #[test]
    fn a_remote_ref_records_scheme_host_and_port() {
        let endpoint = Endpoint::new(
            Endpoint::local("/").fs_arc(),
            Url::parse("ftp://host.example:2121/pub").unwrap(),
        );
        assert_eq!(endpoint.provider_ref(), "ftp://host.example:2121");
        assert_eq!(endpoint.path(), Path::new("pub"));
    }

    #[test]
    fn navigation_keeps_the_provider_and_updates_the_path() {
        let root = Endpoint::local("/a/b");
        let child = root.join("c");
        assert!(root.same_provider(&child));
        assert_eq!(child.path(), Path::new("/a/b/c"));
        assert_eq!(child.parent().unwrap(), root);
    }
}
