// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{path::PathBuf, sync::Arc};

use cocomo_lib::{
    FileSystem, ProviderId, WritableFileSystem,
    profile::ProfileStore,
    provider::Provider,
    secrets::{Secrets, TtyPrompter},
    url::Url,
};

use crate::error::CliError;

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Maps CLI endpoint arguments (paths or URLs) to filesystem instances.
///
/// The production binary resolves endpoints to live [`Provider`] instances
/// via [`ProductionResolver`]; tests inject a resolver backed by
/// `cocomo_lib::MockFs` to exercise the command logic against an in-memory
/// tree with injected errors.
pub trait EndpointResolver {
    /// The filesystem type this resolver produces. It must support both the
    /// path-based reads of the text commands and the node-based scan,
    /// compare, and sync pipelines.
    type Fs: FileSystem + WritableFileSystem<Nid = u64>;

    /// Resolve the provider that services `url`, authenticated with
    /// `profile_id` if given. Both sides of a pair command that address the
    /// same endpoint identity receive the same instance, so one connection
    /// services both.
    fn resolve(
        &self,
        url: &Url,
        profile_id: Option<&str>,
    ) -> Result<Arc<Self::Fs>, CliError>;

    /// Return the identity of the endpoint `fs` addresses, as recorded in
    /// snapshots.
    fn provider_id(&self, fs: &Self::Fs) -> ProviderId;
}

/// The production [`EndpointResolver`]: resolves endpoints to live
/// [`Provider`] instances.
pub struct ProductionResolver;

impl EndpointResolver for ProductionResolver {
    type Fs = Provider;

    fn resolve(
        &self,
        url: &Url,
        profile_id: Option<&str>,
    ) -> Result<Arc<Provider>, CliError> {
        Ok(Arc::new(resolve_provider(url, profile_id)?))
    }

    fn provider_id(&self, fs: &Provider) -> ProviderId {
        fs.provider_id()
    }
}

/// Return the identity of the endpoint `url` addresses. Two URLs with
/// equal identities are serviced by the same provider, so one filesystem
/// instance can serve both sides of a comparison.
pub(crate) fn endpoint_identity(
    url: &Url,
) -> (String, Option<String>, Option<u16>) {
    (url.scheme.clone(), url.host.clone(), url.effective_port())
}

/// Resolve the provider that services `url`, letting `Provider::resolve`
/// consult the default profile store, the environment, and the keychain.
/// The store is only opened for remote endpoints, so local-only runs
/// neither read nor create the profile configuration files.
pub(crate) fn resolve_provider(
    url: &Url,
    profile_id: Option<&str>,
) -> Result<Provider, CliError> {
    let store = if url.scheme == Url::LOCAL_SCHEME {
        None
    } else {
        Some(ProfileStore::open_default()?)
    };
    let secrets = Secrets::new();
    let prompter = TtyPrompter::new();
    let provider = Provider::resolve(
        url,
        store.as_ref(),
        profile_id,
        &secrets,
        &prompter,
    )?;
    Ok(provider)
}

/// Resolve one path-like CLI argument into a filesystem instance and the
/// path within it.
pub(crate) fn resolve_endpoint<R: EndpointResolver>(
    resolver: &R,
    arg: &str,
    profile_id: Option<&str>,
) -> Result<(Arc<R::Fs>, PathBuf), CliError> {
    let url = Url::parse(arg)?;
    let fs = resolver.resolve(&url, profile_id)?;
    Ok((fs, url.path))
}

/// The filesystem(s) addressing the two sides of a pair command. Both
/// variants hold `Arc`s, so the size difference is a single pointer and no
/// padding is needed.
pub(crate) enum EndpointPair<N> {
    /// One instance addressing both sides: the endpoints share an identity,
    /// so one connection services both and cross-boundary transfers keep
    /// the same-provider fast path.
    Shared(Arc<N>),
    /// Two distinct instances: the endpoints address different providers,
    /// so each side resolves and connects on its own.
    Separate(Arc<N>, Arc<N>),
}

/// Resolve a pair of path-like CLI arguments into the filesystem(s) that
/// service them plus the in-provider path of each side. Endpoints with
/// equal identities share one instance (one connection); equal identities
/// resolved per side would give two providers with the same label, aliasing
/// the shared `ContentCache` keys. Different identities resolve to one
/// instance per side, so a mixed pair such as `ftp://host/pub` vs. `./src`
/// is accepted and each side runs against its own backend.
pub(crate) fn resolve_endpoint_pair<R: EndpointResolver>(
    resolver: &R,
    left_arg: &str,
    right_arg: &str,
    profile_id: Option<&str>,
) -> Result<(EndpointPair<R::Fs>, PathBuf, PathBuf), CliError> {
    let left_url = Url::parse(left_arg)?;
    let right_url = Url::parse(right_arg)?;
    let pair = if endpoint_identity(&left_url) == endpoint_identity(&right_url)
    {
        let fs = resolver.resolve(&left_url, profile_id)?;
        EndpointPair::Shared(fs)
    } else {
        let left = resolver.resolve(&left_url, profile_id)?;
        let right = resolver.resolve(&right_url, profile_id)?;
        EndpointPair::Separate(left, right)
    };
    Ok((pair, left_url.path, right_url.path))
}
