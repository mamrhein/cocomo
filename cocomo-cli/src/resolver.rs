// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{path::PathBuf, sync::Arc};

pub use cocomo_lib::resolver::EndpointPair;
use cocomo_lib::{
    ProviderId, WritableFileSystem,
    provider::Provider,
    resolver::{endpoint_identity, resolve_provider},
    secrets::TtyPrompter,
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
    /// The filesystem type this resolver produces. It must support the
    /// node-based scan, compare, and sync pipelines and the path
    /// conveniences of the text commands.
    type Fs: WritableFileSystem<Nid = u64>;

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
/// [`Provider`] instances, prompting on the terminal for credentials that
/// the profile store, the environment, and the keychain do not supply.
pub struct ProductionResolver;

impl EndpointResolver for ProductionResolver {
    type Fs = Provider;

    fn resolve(
        &self,
        url: &Url,
        profile_id: Option<&str>,
    ) -> Result<Arc<Provider>, CliError> {
        Ok(Arc::new(resolve_provider(
            url,
            profile_id,
            &TtyPrompter::new(),
        )?))
    }

    fn provider_id(&self, fs: &Provider) -> ProviderId {
        fs.provider_id()
    }
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
