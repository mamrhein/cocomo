// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Provider enum and registry that unify all filesystem backends.
//!
//! The [`Provider`] enum wraps every concrete filesystem implementation
//! ([`LocalFs`], [`FtpFs`], [`SftpFs`], [`S3Fs`], [`WebDavFs`]) behind a
//! single type.
//! This enables the [`ProviderRegistry`] to store heterogeneous providers
//! and resolve them from connection profiles.
//!
//! # Unified ID types
//!
//! All built-in providers use `u64` for node identifiers and filesystem
//! identifiers. This allows [`Provider`] to implement [`NodeFileSystem`]
//! and [`WritableFileSystem`] with concrete associated types rather than
//! requiring type erasure.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fmt,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use thiserror::Error;

use crate::{
    error::{FsError, Result},
    file::FsFile,
    fs::{
        DirStream, FileSystem, NodeFileSystem, OpenMode, WritableFileSystem,
    },
    ftp::{FtpConfig, FtpFs},
    identity::{DirId, FileId, FileSystemId, NodeId},
    local::LocalFs,
    meta::Metadata,
    node::Node,
    profile::{Profile, ProfileError, ProfileStore, ProviderType},
    s3::{S3Config, S3Fs},
    secrets::{Prompter, Secrets},
    sftp::{SftpConfig, SftpFs},
    snapshot::ProviderId,
    url::Url,
    webdav::{WebDavConfig, WebDavFs},
};

// ---------------------------------------------------------------------------
// Provider enum
// ---------------------------------------------------------------------------

/// Unified wrapper around all filesystem providers.
///
/// Every built-in provider is a variant of this enum. The enum implements
/// [`NodeFileSystem`], [`WritableFileSystem`], and [`FileSystem`] by
/// delegating to the inner provider.
pub enum Provider {
    /// Local filesystem.
    Local(LocalFs),
    /// FTP / FTPS.
    Ftp(FtpFs),
    /// SFTP (SSH file transfer).
    Sftp(SftpFs),
    /// Amazon S3.
    S3(S3Fs),
    /// WebDAV.
    WebDav(WebDavFs),
}

// ---------------------------------------------------------------------------
// Credentials and resolution errors
// ---------------------------------------------------------------------------

/// Credentials for connecting to a remote endpoint.
///
/// Usernames and secrets are kept private and redacted in `Debug` output so
/// that they cannot leak through logging or error rendering by accident.
#[derive(Clone, Default)]
pub struct Credentials {
    user: Option<String>,
    secret: Option<String>,
}

impl Credentials {
    /// Create credentials from an optional user and an optional secret.
    pub fn new(user: Option<String>, secret: Option<String>) -> Self {
        Self { user, secret }
    }

    /// Return the username, if one was resolved.
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Return the secret (password or access key), if one was resolved.
    pub fn secret(&self) -> Option<&str> {
        self.secret.as_deref()
    }

    /// Return whether neither a user nor a secret was resolved.
    pub fn is_empty(&self) -> bool {
        self.user.is_none() && self.secret.is_none()
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never reveal the user name or the secret in debug output.
        f.debug_struct("Credentials")
            .field("user", &self.user.as_ref().map(|_| "[REDACTED]"))
            .field("secret", &self.secret.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Errors that can occur while resolving an endpoint into a provider.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// The backend for the URL's scheme is only scaffolding (every I/O
    /// method would panic), so the CLI must refuse such endpoints instead
    /// of constructing a provider that panics on first use.
    #[error("provider for scheme `{scheme}` is not implemented yet")]
    Unimplemented { scheme: String },

    /// A remote endpoint requires authentication but neither the profile
    /// store, nor the environment, nor the keychain, nor an interactive
    /// prompt could supply credentials (no implicit anonymous access).
    #[error(
        "authentication required for `{endpoint}`; use `--profile <ID>` to \
         supply credentials"
    )]
    AuthRequired { endpoint: String },

    /// The explicitly selected profile does not describe the endpoint that
    /// was addressed. Mismatches are refused instead of silently resolving
    /// against a different server.
    #[error("profile `{profile}` does not match endpoint `{endpoint}`")]
    ProfileMismatch { profile: String, endpoint: String },

    /// A profile-store operation failed.
    #[error(transparent)]
    Profile(#[from] ProfileError),
}

impl Provider {
    /// Return the provider type for this instance.
    pub fn provider_type(&self) -> ProviderType {
        match self {
            Self::Local(_) => ProviderType::Local,
            Self::Ftp(_) => ProviderType::Ftp,
            Self::Sftp(_) => ProviderType::Sftp,
            Self::S3(_) => ProviderType::S3,
            Self::WebDav(_) => ProviderType::WebDav,
        }
    }

    /// Return the provider identifier that references this instance.
    ///
    /// The label of the inner filesystem identifies the source the
    /// provider was built from: providers resolved from a URL or a bare
    /// path carry the scheme or the URL's lookup key (e.g.
    /// `ftp:ftp.example.com:21`) as their label, while providers built
    /// from a profile carry the profile id. A lookup key always contains
    /// a `:` and a profile id never does, so labels equal to the bare
    /// scheme or containing a `:` denote the default provider of their
    /// scheme (profile `None`).
    pub fn provider_id(&self) -> ProviderId {
        let provider_type = self.provider_type();
        let scheme = provider_type.scheme();
        let label = self.label_node();
        let profile =
            if label.is_empty() || label == scheme || label.contains(':') {
                None
            } else {
                Some(label.to_owned())
            };
        ProviderId::new(scheme, profile)
    }

    /// Construct a provider that addresses the endpoint given by `url`.
    ///
    /// `creds` are only consulted by the remote backends that need them
    /// (FTP/FTPS). The `s3` and `webdav`/`webdavs` schemes are only
    /// scaffolding backends right now, so they yield
    /// [`ProviderError::Unimplemented`] instead of a provider whose every
    /// I/O method would panic.
    ///
    /// # Errors
    ///
    /// Returns a [`ProviderError`] if the scheme has no working backend yet.
    pub fn from_url(
        url: &Url,
        creds: &Credentials,
    ) -> std::result::Result<Self, ProviderError> {
        Self::build(url, creds, url.uses_tls_by_default(), url.lookup_key())
    }

    /// Resolve the provider that services `url`, following the full
    /// credential resolution chain.
    ///
    /// For remote URLs the resolution order is:
    ///
    /// 1. `profile_id` given: load exactly that profile from `store` (an
    ///    unknown or endpoint-mismatching profile is an error, never a silent
    ///    fallback).
    /// 2. otherwise auto-match: the first profile in `store` whose provider
    ///    type and host (and explicit port) match the URL.
    /// 3. otherwise `secrets` (environment, then keychain) and finally an
    ///    interactive `prompter` prompt on a TTY; on a non-TTY (or without a
    ///    prompter) the resolution fails with [`ProviderError::AuthRequired`].
    ///
    /// A matching profile's `tls` key overrides the TLS default implied by
    /// the scheme (`ftps`/`webdavs`), and a profile without any secret
    /// never triggers an implicit anonymous login.
    ///
    /// # Errors
    ///
    /// Returns a [`ProviderError`] if no provider can be resolved for
    /// `url`.
    pub fn resolve(
        url: &Url,
        store: Option<&ProfileStore>,
        profile_id: Option<&str>,
        secrets: &Secrets,
        prompter: &dyn Prompter,
    ) -> std::result::Result<Self, ProviderError> {
        match url.scheme.as_str() {
            Url::LOCAL_SCHEME => Self::build(
                url,
                &Credentials::default(),
                false,
                String::from(Url::LOCAL_SCHEME),
            ),
            "ftp" | "ftps" => {
                Self::resolve_ftp(url, store, profile_id, secrets, prompter)
            }
            "sftp" => {
                Self::resolve_sftp(url, store, profile_id, secrets, prompter)
            }
            scheme => Err(ProviderError::Unimplemented {
                scheme: scheme.to_owned(),
            }),
        }
    }

    /// Resolve an FTP/FTPS URL: profile store first, then secrets, then
    /// prompt (see [`Provider::resolve`]).
    fn resolve_ftp(
        url: &Url,
        store: Option<&ProfileStore>,
        profile_id: Option<&str>,
        secrets: &Secrets,
        prompter: &dyn Prompter,
    ) -> std::result::Result<Self, ProviderError> {
        let endpoint = url.to_string();
        // Step 1: locate the profile that supplies the credentials. An
        // explicit profile that does not describe the endpoint is refused
        // outright instead of silently connecting elsewhere.
        let profile = match (store, profile_id) {
            (Some(store), Some(id)) => match store.get_decrypted(id)? {
                None => {
                    return Err(ProviderError::Profile(
                        ProfileError::NotFound(id.to_owned()),
                    ));
                }
                Some(profile) if profile_matches_endpoint(&profile, url) => {
                    Some(profile)
                }
                Some(_) => {
                    return Err(ProviderError::ProfileMismatch {
                        profile: id.to_owned(),
                        endpoint,
                    });
                }
            },
            (Some(store), None) => store
                .list_decrypted()?
                .into_iter()
                .find(|p| profile_matches_endpoint(p, url)),
            (None, _) => None,
        };
        // Step 2: collect credentials and the TLS flag.
        let mut user = None;
        let mut secret = None;
        let mut tls = url.uses_tls_by_default();
        if let Some(profile) = &profile {
            if let Some(tls_key) = profile.setting("tls") {
                // OQ2: the profile's `tls` key overrides the scheme default.
                tls = tls_key.eq_ignore_ascii_case("true");
            }
            user = profile.setting("username").map(str::to_owned);
            secret = profile.secrets.get("password").map(str::to_owned);
        }
        if profile.is_none() {
            // Step 3: environment/keychain fallback, then a one-time TTY
            // prompt. Without a secret there is no authentication to
            // attempt, and anonymous access would be implicit (OQ3).
            user = secrets.get(&url.scheme, "user");
            secret = secrets.get(&url.scheme, "password");
            if secret.is_none() && prompter.is_tty() {
                if user.is_none() {
                    user = prompter.prompt_user(&endpoint);
                }
                secret = prompter.prompt_secret(&endpoint);
            }
        }
        let Some(secret) = secret else {
            return Err(ProviderError::AuthRequired { endpoint });
        };
        let creds = Credentials::new(user, Some(secret));
        let label = profile
            .as_ref()
            .map_or_else(|| url.lookup_key(), |p| p.id.clone());
        Self::build(url, &creds, tls, label)
    }

    /// Resolve an SFTP URL: profile store first, then secrets, then prompt
    /// (see [`Provider::resolve`]).
    ///
    /// Authentication is key-based when a `key_file` setting (profile) or
    /// secret resolves, with a password fallback. Neither implicit
    /// anonymous access nor implicit ssh-agent/default-identity access is
    /// attempted (OQ3).
    fn resolve_sftp(
        url: &Url,
        store: Option<&ProfileStore>,
        profile_id: Option<&str>,
        secrets: &Secrets,
        prompter: &dyn Prompter,
    ) -> std::result::Result<Self, ProviderError> {
        let endpoint = url.to_string();
        // Step 1: locate the profile that supplies the credentials. An
        // explicit profile that does not describe the endpoint is refused
        // outright instead of silently connecting elsewhere.
        let profile = match (store, profile_id) {
            (Some(store), Some(id)) => match store.get_decrypted(id)? {
                None => {
                    return Err(ProviderError::Profile(
                        ProfileError::NotFound(id.to_owned()),
                    ));
                }
                Some(profile) if profile_matches_endpoint(&profile, url) => {
                    Some(profile)
                }
                Some(_) => {
                    return Err(ProviderError::ProfileMismatch {
                        profile: id.to_owned(),
                        endpoint,
                    });
                }
            },
            (Some(store), None) => store
                .list_decrypted()?
                .into_iter()
                .find(|p| profile_matches_endpoint(p, url)),
            (None, _) => None,
        };
        // Step 2: collect credentials and the key file. SSH encrypts its
        // own transport, so there is no TLS flag to resolve.
        let mut user = None;
        let mut secret = None;
        let mut key_file = None;
        if let Some(profile) = &profile {
            user = profile.setting("username").map(str::to_owned);
            secret = profile.secrets.get("password").map(str::to_owned);
            key_file = profile.setting("key_file").map(PathBuf::from);
        }
        if profile.is_none() {
            // Step 3: environment/keychain fallback, then a one-time TTY
            // prompt. Without a secret or key file there is no
            // authentication to attempt, and anonymous access would be
            // implicit (OQ3).
            user = secrets.get(&url.scheme, "user");
            secret = secrets.get(&url.scheme, "password");
            if secret.is_none() && prompter.is_tty() {
                if user.is_none() {
                    user = prompter.prompt_user(&endpoint);
                }
                secret = prompter.prompt_secret(&endpoint);
            }
        }
        if secret.is_none() && key_file.is_none() {
            return Err(ProviderError::AuthRequired { endpoint });
        }
        let label = profile
            .as_ref()
            .map_or_else(|| url.lookup_key(), |p| p.id.clone());
        Ok(Self::Sftp(SftpFs::new(
            label,
            Self::sftp_config_from_url(url, user.as_deref(), secret, key_file),
        )))
    }

    /// Build an [`SftpConfig`] from a URL and resolved credentials.
    fn sftp_config_from_url(
        url: &Url,
        user: Option<&str>,
        secret: Option<String>,
        key_file: Option<PathBuf>,
    ) -> SftpConfig {
        SftpConfig {
            host: url.host.clone().unwrap_or_default(),
            port: url.effective_port().unwrap_or(22),
            username: user.unwrap_or_default().to_owned(),
            password: secret,
            key_file,
            // The URL's path selects the scan root, so it must not also
            // become the provider's root prefix.
            root_path: None,
        }
    }

    /// Construct a provider for `url` with the given credentials and TLS
    /// flag; `label` identifies the provider instance.
    fn build(
        url: &Url,
        creds: &Credentials,
        tls: bool,
        label: String,
    ) -> std::result::Result<Self, ProviderError> {
        match url.scheme.as_str() {
            Url::LOCAL_SCHEME => Ok(Self::Local(LocalFs::new(label))),
            "ftp" | "ftps" => {
                let host = url.host.clone().unwrap_or_default();
                let port = url.effective_port().unwrap_or(21);
                Ok(Self::Ftp(FtpFs::new(
                    label,
                    FtpConfig {
                        host,
                        port,
                        username: creds.user().unwrap_or_default().to_owned(),
                        password: creds
                            .secret()
                            .unwrap_or_default()
                            .to_owned(),
                        tls,
                        // The URL's path selects the scan root, so it must
                        // not also become the provider's root prefix.
                        root_path: None,
                    },
                )))
            }
            "sftp" => Ok(Self::Sftp(SftpFs::new(
                label,
                Self::sftp_config_from_url(
                    url,
                    creds.user(),
                    creds.secret().map(str::to_owned),
                    None,
                ),
            ))),
            scheme => Err(ProviderError::Unimplemented {
                scheme: scheme.to_owned(),
            }),
        }
    }

    /// Create a new `Provider` from a profile.
    ///
    /// This is a factory method that reads the profile's settings and
    /// secrets to construct the appropriate provider. The profile should
    /// contain decrypted secrets.
    ///
    /// # Errors
    ///
    /// Returns an error if required settings are missing or invalid.
    pub fn from_profile(
        profile: &Profile,
    ) -> std::result::Result<Self, ProfileError> {
        let label = profile.id.clone();
        match profile.provider_type {
            ProviderType::Local => {
                // Local providers don't need a profile; the root_path
                // setting can override the working directory.
                Ok(Self::Local(LocalFs::new(label)))
            }
            ProviderType::Ftp => {
                let host = profile
                    .setting("host")
                    .ok_or_else(|| ProfileError::NotFound(profile.id.clone()))?
                    .to_string();
                let port: u16 = profile
                    .setting("port")
                    .unwrap_or("21")
                    .parse::<u16>()
                    .map_err(|e| ProfileError::Toml(e.to_string()))?;
                let username =
                    profile.setting("username").unwrap_or("").to_string();
                let password =
                    profile.secrets.get("password").unwrap_or("").to_string();
                let tls = profile
                    .setting("tls")
                    .map(|s| s == "true")
                    .unwrap_or(false);
                let root_path =
                    profile.setting("root_path").map(PathBuf::from);

                Ok(Self::Ftp(FtpFs::new(
                    label,
                    FtpConfig {
                        host,
                        port,
                        username,
                        password,
                        tls,
                        root_path,
                    },
                )))
            }
            ProviderType::Sftp => {
                let host = profile
                    .setting("host")
                    .ok_or_else(|| ProfileError::NotFound(profile.id.clone()))?
                    .to_string();
                let port: u16 = profile
                    .setting("port")
                    .unwrap_or("22")
                    .parse::<u16>()
                    .map_err(|e| ProfileError::Toml(e.to_string()))?;
                let username =
                    profile.setting("username").unwrap_or("").to_string();
                let password =
                    profile.secrets.get("password").map(String::from);
                let key_file = profile.setting("key_file").map(PathBuf::from);
                let root_path =
                    profile.setting("root_path").map(PathBuf::from);

                Ok(Self::Sftp(SftpFs::new(
                    label,
                    SftpConfig {
                        host,
                        port,
                        username,
                        password,
                        key_file,
                        root_path,
                    },
                )))
            }
            ProviderType::S3 => {
                let region = profile
                    .setting("region")
                    .ok_or_else(|| ProfileError::NotFound(profile.id.clone()))?
                    .to_string();
                let bucket = profile
                    .setting("bucket")
                    .ok_or_else(|| ProfileError::NotFound(profile.id.clone()))?
                    .to_string();
                let prefix = profile.setting("prefix").map(PathBuf::from);

                Ok(Self::S3(S3Fs::new(
                    label,
                    S3Config {
                        region,
                        bucket,
                        prefix,
                    },
                )))
            }
            ProviderType::WebDav => {
                let base_url = profile
                    .setting("base_url")
                    .ok_or_else(|| ProfileError::NotFound(profile.id.clone()))?
                    .to_string();
                let username = profile.setting("username").map(String::from);
                let password =
                    profile.secrets.get("password").map(String::from);
                let tls = profile
                    .setting("tls")
                    .map(|s| s == "true")
                    .unwrap_or(true);
                let root_path =
                    profile.setting("root_path").map(PathBuf::from);

                Ok(Self::WebDav(WebDavFs::new(
                    label,
                    WebDavConfig {
                        base_url,
                        username,
                        password,
                        tls,
                        root_path,
                    },
                )))
            }
        }
    }
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(_) => f.debug_tuple("Provider::Local").finish(),
            Self::Ftp(_) => f.debug_tuple("Provider::Ftp").finish(),
            Self::Sftp(_) => f.debug_tuple("Provider::Sftp").finish(),
            Self::S3(_) => f.debug_tuple("Provider::S3").finish(),
            Self::WebDav(_) => f.debug_tuple("Provider::WebDav").finish(),
        }
    }
}

/// Check whether `profile` describes the endpoint addressed by `url`.
///
/// The provider type and host must match; an explicit port in the URL must
/// equal an explicit `port` setting of the profile (a profile without a
/// `port` setting matches any port, since the port only distinguishes
/// endpoints on the same host).
fn profile_matches_endpoint(profile: &Profile, url: &Url) -> bool {
    let type_ok = match url.scheme.as_str() {
        "ftp" | "ftps" => matches!(profile.provider_type, ProviderType::Ftp),
        "sftp" => matches!(profile.provider_type, ProviderType::Sftp),
        "s3" => matches!(profile.provider_type, ProviderType::S3),
        "webdav" | "webdavs" => {
            matches!(profile.provider_type, ProviderType::WebDav)
        }
        _ => false,
    };
    if !type_ok {
        return false;
    }
    if profile.setting("host") != url.host.as_deref() {
        return false;
    }
    match (
        url.port,
        profile.setting("port").and_then(|p| p.parse::<u16>().ok()),
    ) {
        (Some(url_port), Some(profile_port)) => url_port == profile_port,
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// FileSystem implementation (path-based)
// ---------------------------------------------------------------------------

#[async_trait]
impl FileSystem for Provider {
    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        match self {
            Self::Local(p) => p.metadata(path).await,
            Self::Ftp(p) => p.metadata(path).await,
            Self::Sftp(p) => p.metadata(path).await,
            Self::S3(p) => p.metadata(path).await,
            Self::WebDav(p) => p.metadata(path).await,
        }
    }

    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>> {
        match self {
            Self::Local(p) => p.read_dir(path).await,
            Self::Ftp(p) => p.read_dir(path).await,
            Self::Sftp(p) => p.read_dir(path).await,
            Self::S3(p) => p.read_dir(path).await,
            Self::WebDav(p) => p.read_dir(path).await,
        }
    }

    async fn open(
        &self,
        path: &Path,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        match self {
            Self::Local(p) => p.open(path, mode).await,
            Self::Ftp(p) => p.open(path, mode).await,
            Self::Sftp(p) => p.open(path, mode).await,
            Self::S3(p) => p.open(path, mode).await,
            Self::WebDav(p) => p.open(path, mode).await,
        }
    }

    async fn read(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        match self {
            Self::Local(p) => p.read(path, range).await,
            Self::Ftp(p) => p.read(path, range).await,
            Self::Sftp(p) => p.read(path, range).await,
            Self::S3(p) => p.read(path, range).await,
            Self::WebDav(p) => p.read(path, range).await,
        }
    }

    async fn read_stream(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        match self {
            Self::Local(p) => p.read_stream(path, range).await,
            Self::Ftp(p) => p.read_stream(path, range).await,
            Self::Sftp(p) => p.read_stream(path, range).await,
            Self::S3(p) => p.read_stream(path, range).await,
            Self::WebDav(p) => p.read_stream(path, range).await,
        }
    }

    async fn write(&self, path: &Path, data: Bytes) -> Result<()> {
        match self {
            Self::Local(p) => p.write(path, data).await,
            Self::Ftp(p) => p.write(path, data).await,
            Self::Sftp(p) => p.write(path, data).await,
            Self::S3(p) => p.write(path, data).await,
            Self::WebDav(p) => p.write(path, data).await,
        }
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.create_dir(path).await,
            Self::Ftp(p) => p.create_dir(path).await,
            Self::Sftp(p) => p.create_dir(path).await,
            Self::S3(p) => p.create_dir(path).await,
            Self::WebDav(p) => p.create_dir(path).await,
        }
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.remove(path).await,
            Self::Ftp(p) => p.remove(path).await,
            Self::Sftp(p) => p.remove(path).await,
            Self::S3(p) => p.remove(path).await,
            Self::WebDav(p) => p.remove(path).await,
        }
    }

    async fn remove_all(&self, path: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.remove_all(path).await,
            Self::Ftp(p) => p.remove_all(path).await,
            Self::Sftp(p) => p.remove_all(path).await,
            Self::S3(p) => p.remove_all(path).await,
            Self::WebDav(p) => p.remove_all(path).await,
        }
    }

    async fn rename(&self, src: &Path, dst: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.rename(src, dst).await,
            Self::Ftp(p) => p.rename(src, dst).await,
            Self::Sftp(p) => p.rename(src, dst).await,
            Self::S3(p) => p.rename(src, dst).await,
            Self::WebDav(p) => p.rename(src, dst).await,
        }
    }

    async fn copy(&self, src: &Path, dst: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.copy(src, dst).await,
            Self::Ftp(p) => p.copy(src, dst).await,
            Self::Sftp(p) => p.copy(src, dst).await,
            Self::S3(p) => p.copy(src, dst).await,
            Self::WebDav(p) => p.copy(src, dst).await,
        }
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        match self {
            Self::Local(p) => p.read_link(path).await,
            Self::Ftp(p) => p.read_link(path).await,
            Self::Sftp(p) => p.read_link(path).await,
            Self::S3(p) => p.read_link(path).await,
            Self::WebDav(p) => p.read_link(path).await,
        }
    }

    async fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.symlink(target, link).await,
            Self::Ftp(p) => p.symlink(target, link).await,
            Self::Sftp(p) => p.symlink(target, link).await,
            Self::S3(p) => p.symlink(target, link).await,
            Self::WebDav(p) => p.symlink(target, link).await,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Local(p) => p.label(),
            Self::Ftp(p) => p.label(),
            Self::Sftp(p) => p.label(),
            Self::S3(p) => p.label(),
            Self::WebDav(p) => p.label(),
        }
    }
}

// ---------------------------------------------------------------------------
// NodeFileSystem implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl NodeFileSystem for Provider {
    type FsId = u64;
    type Nid = u64;
    type Error = FsError;

    fn id(&self) -> FileSystemId<Self::FsId> {
        match self {
            Self::Local(p) => p.id(),
            Self::Ftp(p) => p.id(),
            Self::Sftp(p) => p.id(),
            Self::S3(p) => p.id(),
            Self::WebDav(p) => p.id(),
        }
    }

    fn label_node(&self) -> &str {
        match self {
            Self::Local(p) => p.label_node(),
            Self::Ftp(p) => p.label_node(),
            Self::Sftp(p) => p.label_node(),
            Self::S3(p) => p.label_node(),
            Self::WebDav(p) => p.label_node(),
        }
    }

    async fn resolve_path(&self, path: &Path) -> Result<NodeId<Self::Nid>> {
        match self {
            Self::Local(p) => p.resolve_path(path).await,
            Self::Ftp(p) => p.resolve_path(path).await,
            Self::Sftp(p) => p.resolve_path(path).await,
            Self::S3(p) => p.resolve_path(path).await,
            Self::WebDav(p) => p.resolve_path(path).await,
        }
    }

    async fn resolve_symlink(
        &self,
        id: NodeId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        match self {
            Self::Local(p) => p.resolve_symlink(id).await,
            Self::Ftp(p) => p.resolve_symlink(id).await,
            Self::Sftp(p) => p.resolve_symlink(id).await,
            Self::S3(p) => p.resolve_symlink(id).await,
            Self::WebDav(p) => p.resolve_symlink(id).await,
        }
    }

    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>> {
        match self {
            Self::Local(p) => p.get_node(id),
            Self::Ftp(p) => p.get_node(id),
            Self::Sftp(p) => p.get_node(id),
            Self::S3(p) => p.get_node(id),
            Self::WebDav(p) => p.get_node(id),
        }
    }

    fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata> {
        match self {
            Self::Local(p) => p.node_metadata(id),
            Self::Ftp(p) => p.node_metadata(id),
            Self::Sftp(p) => p.node_metadata(id),
            Self::S3(p) => p.node_metadata(id),
            Self::WebDav(p) => p.node_metadata(id),
        }
    }

    fn set_node_hash(
        &self,
        id: NodeId<Self::Nid>,
        hash: String,
    ) -> Result<()> {
        match self {
            Self::Local(p) => p.set_node_hash(id, hash),
            Self::Ftp(p) => p.set_node_hash(id, hash),
            Self::Sftp(p) => p.set_node_hash(id, hash),
            Self::S3(p) => p.set_node_hash(id, hash),
            Self::WebDav(p) => p.set_node_hash(id, hash),
        }
    }

    async fn read_dir_node(&self, id: DirId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.read_dir_node(id).await,
            Self::Ftp(p) => p.read_dir_node(id).await,
            Self::Sftp(p) => p.read_dir_node(id).await,
            Self::S3(p) => p.read_dir_node(id).await,
            Self::WebDav(p) => p.read_dir_node(id).await,
        }
    }

    async fn open_node(
        &self,
        id: FileId<Self::Nid>,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        match self {
            Self::Local(p) => p.open_node(id, mode).await,
            Self::Ftp(p) => p.open_node(id, mode).await,
            Self::Sftp(p) => p.open_node(id, mode).await,
            Self::S3(p) => p.open_node(id, mode).await,
            Self::WebDav(p) => p.open_node(id, mode).await,
        }
    }

    async fn read_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        match self {
            Self::Local(p) => p.read_node(id, range).await,
            Self::Ftp(p) => p.read_node(id, range).await,
            Self::Sftp(p) => p.read_node(id, range).await,
            Self::S3(p) => p.read_node(id, range).await,
            Self::WebDav(p) => p.read_node(id, range).await,
        }
    }

    async fn read_stream_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        match self {
            Self::Local(p) => p.read_stream_node(id, range).await,
            Self::Ftp(p) => p.read_stream_node(id, range).await,
            Self::Sftp(p) => p.read_stream_node(id, range).await,
            Self::S3(p) => p.read_stream_node(id, range).await,
            Self::WebDav(p) => p.read_stream_node(id, range).await,
        }
    }
}

// ---------------------------------------------------------------------------
// WritableFileSystem implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl WritableFileSystem for Provider {
    async fn create_file(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<FileId<Self::Nid>> {
        match self {
            Self::Local(p) => p.create_file(parent, name).await,
            Self::Ftp(p) => p.create_file(parent, name).await,
            Self::Sftp(p) => p.create_file(parent, name).await,
            Self::S3(p) => p.create_file(parent, name).await,
            Self::WebDav(p) => p.create_file(parent, name).await,
        }
    }

    async fn create_dir_node(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<DirId<Self::Nid>> {
        match self {
            Self::Local(p) => p.create_dir_node(parent, name).await,
            Self::Ftp(p) => p.create_dir_node(parent, name).await,
            Self::Sftp(p) => p.create_dir_node(parent, name).await,
            Self::S3(p) => p.create_dir_node(parent, name).await,
            Self::WebDav(p) => p.create_dir_node(parent, name).await,
        }
    }

    async fn create_symlink(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
        target: &Path,
    ) -> Result<NodeId<Self::Nid>> {
        match self {
            Self::Local(p) => p.create_symlink(parent, name, target).await,
            Self::Ftp(p) => p.create_symlink(parent, name, target).await,
            Self::Sftp(p) => p.create_symlink(parent, name, target).await,
            Self::S3(p) => p.create_symlink(parent, name, target).await,
            Self::WebDav(p) => p.create_symlink(parent, name, target).await,
        }
    }

    async fn write_node(
        &self,
        id: FileId<Self::Nid>,
        data: Bytes,
    ) -> Result<()> {
        match self {
            Self::Local(p) => p.write_node(id, data).await,
            Self::Ftp(p) => p.write_node(id, data).await,
            Self::Sftp(p) => p.write_node(id, data).await,
            Self::S3(p) => p.write_node(id, data).await,
            Self::WebDav(p) => p.write_node(id, data).await,
        }
    }

    async fn flush_node(&self, id: FileId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.flush_node(id).await,
            Self::Ftp(p) => p.flush_node(id).await,
            Self::Sftp(p) => p.flush_node(id).await,
            Self::S3(p) => p.flush_node(id).await,
            Self::WebDav(p) => p.flush_node(id).await,
        }
    }

    async fn remove_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.remove_node(id).await,
            Self::Ftp(p) => p.remove_node(id).await,
            Self::Sftp(p) => p.remove_node(id).await,
            Self::S3(p) => p.remove_node(id).await,
            Self::WebDav(p) => p.remove_node(id).await,
        }
    }

    async fn remove_all_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.remove_all_node(id).await,
            Self::Ftp(p) => p.remove_all_node(id).await,
            Self::Sftp(p) => p.remove_all_node(id).await,
            Self::S3(p) => p.remove_all_node(id).await,
            Self::WebDav(p) => p.remove_all_node(id).await,
        }
    }

    async fn rename_node(
        &self,
        id: NodeId<Self::Nid>,
        new_name: &OsStr,
    ) -> Result<()> {
        match self {
            Self::Local(p) => p.rename_node(id, new_name).await,
            Self::Ftp(p) => p.rename_node(id, new_name).await,
            Self::Sftp(p) => p.rename_node(id, new_name).await,
            Self::S3(p) => p.rename_node(id, new_name).await,
            Self::WebDav(p) => p.rename_node(id, new_name).await,
        }
    }

    async fn copy_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        match self {
            Self::Local(p) => p.copy_node(src, dst).await,
            Self::Ftp(p) => p.copy_node(src, dst).await,
            Self::Sftp(p) => p.copy_node(src, dst).await,
            Self::S3(p) => p.copy_node(src, dst).await,
            Self::WebDav(p) => p.copy_node(src, dst).await,
        }
    }

    async fn move_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        match self {
            Self::Local(p) => p.move_node(src, dst).await,
            Self::Ftp(p) => p.move_node(src, dst).await,
            Self::Sftp(p) => p.move_node(src, dst).await,
            Self::S3(p) => p.move_node(src, dst).await,
            Self::WebDav(p) => p.move_node(src, dst).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider registry
// ---------------------------------------------------------------------------

/// Stores and manages filesystem provider instances.
///
/// Providers are stored as [`Arc<Provider>`] and can be looked up by their
/// filesystem ID. The registry also supports creating new providers from
/// connection profiles.
///
/// # Thread safety
///
/// This type is `Send + Sync` and can be shared across threads.
pub struct ProviderRegistry {
    /// Map from filesystem ID to provider instance.
    providers: parking_lot::RwLock<HashMap<u64, Arc<Provider>>>,
}

impl ProviderRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            providers: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Register a provider instance.
    ///
    /// If a provider with the same filesystem ID is already registered, it
    /// will be replaced.
    pub fn register(&self, provider: Arc<Provider>) {
        let id = *provider.id().get();
        self.providers.write().insert(id, provider);
    }

    /// Look up a provider by filesystem ID.
    ///
    /// Returns `None` if no provider with the given ID is registered.
    pub fn get(&self, id: &FileSystemId<u64>) -> Option<Arc<Provider>> {
        self.providers.read().get(id.get()).cloned()
    }

    /// Look up a provider by filesystem ID, creating one from the given
    /// profile if it doesn't exist yet.
    ///
    /// The created provider is cached in the registry so subsequent lookups
    /// return the same instance.
    pub async fn get_or_create(
        &self,
        id: &FileSystemId<u64>,
        profile: &Profile,
    ) -> std::result::Result<Arc<Provider>, ProfileError> {
        // Check cache first.
        if let Some(provider) = self.get(id) {
            return Ok(provider);
        }

        // Create from profile.
        let provider = Arc::new(Provider::from_profile(profile)?);
        let provider_id = *provider.id().get();

        // Only cache if the ID matches (it should, since providers derive
        // their ID from their config, not from the profile ID).
        self.providers.write().insert(provider_id, provider.clone());

        // If the requested ID doesn't match the created provider's ID,
        // also cache under the requested ID so lookups by profile-derived
        // ID still work.
        if provider_id != *id.get() {
            self.providers.write().insert(*id.get(), provider.clone());
        }

        Ok(provider)
    }

    /// Remove a provider from the registry by filesystem ID.
    pub fn unregister(&self, id: &FileSystemId<u64>) {
        self.providers.write().remove(id.get());
    }

    /// List all registered providers.
    pub fn list(&self) -> Vec<Arc<Provider>> {
        self.providers.read().values().cloned().collect()
    }

    /// Check if a provider is registered.
    pub fn contains(&self, id: &FileSystemId<u64>) -> bool {
        self.providers.read().contains_key(id.get())
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{
        profile::{Profile, ProfileStore},
        secrets::{NullPrompter, Secrets},
        url::Url,
    };

    fn local_profile() -> Profile {
        Profile::new("test-local", ProviderType::Local)
    }

    fn ftp_profile() -> Profile {
        let mut profile = Profile::new("test-ftp", ProviderType::Ftp);
        profile.set_setting("host".into(), "ftp.example.com".into());
        profile.set_setting("port".into(), "21".into());
        profile.set_setting("username".into(), "user".into());
        profile.secrets.set("password".into(), "pass".into());
        profile
    }

    fn sftp_profile() -> Profile {
        let mut profile = Profile::new("test-sftp", ProviderType::Sftp);
        profile.set_setting("host".into(), "sftp.example.com".into());
        profile.set_setting("port".into(), "22".into());
        profile.set_setting("username".into(), "user".into());
        profile.secrets.set("password".into(), "pass".into());
        profile
    }

    fn s3_profile() -> Profile {
        let mut profile = Profile::new("test-s3", ProviderType::S3);
        profile.set_setting("region".into(), "us-east-1".into());
        profile.set_setting("bucket".into(), "my-bucket".into());
        profile
    }

    fn webdav_profile() -> Profile {
        let mut profile = Profile::new("test-webdav", ProviderType::WebDav);
        profile
            .set_setting("base_url".into(), "https://dav.example.com/".into());
        profile.set_setting("tls".into(), "true".into());
        profile
    }

    #[test]
    fn provider_type_from_variant() {
        let local = Provider::Local(LocalFs::new("test"));
        assert_eq!(local.provider_type(), ProviderType::Local);

        let ftp = Provider::Ftp(FtpFs::new(
            "test",
            FtpConfig {
                host: "host".into(),
                port: 21,
                username: "u".into(),
                password: "p".into(),
                tls: false,
                root_path: None,
            },
        ));
        assert_eq!(ftp.provider_type(), ProviderType::Ftp);

        let sftp = Provider::Sftp(SftpFs::new(
            "test",
            SftpConfig {
                host: "host".into(),
                port: 22,
                username: "u".into(),
                password: Some("p".into()),
                key_file: None,
                root_path: None,
            },
        ));
        assert_eq!(sftp.provider_type(), ProviderType::Sftp);

        let s3 = Provider::S3(S3Fs::new(
            "test",
            S3Config {
                region: "us-east-1".into(),
                bucket: "b".into(),
                prefix: None,
            },
        ));
        assert_eq!(s3.provider_type(), ProviderType::S3);
    }

    #[test]
    fn provider_id_from_bare_path() {
        let url = Url::from_path(Path::new("./src"));
        let provider =
            Provider::from_url(&url, &Credentials::default()).unwrap();
        // A bare-path provider carries the bare scheme as its label, so
        // it identifies as the default local provider.
        assert_eq!(provider.provider_id(), ProviderId::local());
    }

    #[test]
    fn provider_id_from_url_without_profile() {
        let url = Url::parse("ftp://ftp.example.com:21/pub").unwrap();
        let creds =
            Credentials::new(Some("user".to_owned()), Some("pass".to_owned()));
        let provider = Provider::from_url(&url, &creds).unwrap();
        // The label is the URL's lookup key, which names the endpoint but
        // no profile, so the provider identifies as the default ftp one.
        assert_eq!(provider.provider_id(), ProviderId::new("ftp", None));
    }

    #[test]
    fn provider_id_from_profile_carries_profile_id() {
        let ftp = Provider::from_profile(&ftp_profile()).unwrap();
        assert_eq!(
            ftp.provider_id(),
            ProviderId::new("ftp", Some("test-ftp".to_owned()))
        );

        let sftp = Provider::from_profile(&sftp_profile()).unwrap();
        assert_eq!(
            sftp.provider_id(),
            ProviderId::new("sftp", Some("test-sftp".to_owned()))
        );

        let s3 = Provider::from_profile(&s3_profile()).unwrap();
        assert_eq!(
            s3.provider_id(),
            ProviderId::new("s3", Some("test-s3".to_owned()))
        );

        let webdav = Provider::from_profile(&webdav_profile()).unwrap();
        assert_eq!(
            webdav.provider_id(),
            ProviderId::new("webdav", Some("test-webdav".to_owned()))
        );
    }

    #[test]
    fn from_profile_local() {
        let profile = local_profile();
        let provider = Provider::from_profile(&profile).unwrap();
        assert_eq!(provider.provider_type(), ProviderType::Local);
        assert_eq!(provider.label(), "test-local");
    }

    #[test]
    fn from_profile_ftp() {
        let profile = ftp_profile();
        let provider = Provider::from_profile(&profile).unwrap();
        assert_eq!(provider.provider_type(), ProviderType::Ftp);
        assert_eq!(provider.label(), "test-ftp");
    }

    #[test]
    fn from_profile_sftp() {
        let profile = sftp_profile();
        let provider = Provider::from_profile(&profile).unwrap();
        assert_eq!(provider.provider_type(), ProviderType::Sftp);
        assert_eq!(provider.label(), "test-sftp");
    }

    #[test]
    fn from_profile_s3() {
        let profile = s3_profile();
        let provider = Provider::from_profile(&profile).unwrap();
        assert_eq!(provider.provider_type(), ProviderType::S3);
        assert_eq!(provider.label(), "test-s3");
    }

    #[test]
    fn from_profile_webdav() {
        let profile = webdav_profile();
        let provider = Provider::from_profile(&profile).unwrap();
        assert_eq!(provider.provider_type(), ProviderType::WebDav);
        assert_eq!(provider.label(), "test-webdav");
    }

    #[test]
    fn from_profile_ftp_missing_host_fails() {
        let profile = Profile::new("bad-ftp", ProviderType::Ftp);
        let result = Provider::from_profile(&profile);
        assert!(result.is_err());
    }

    #[test]
    fn from_profile_sftp_missing_host_fails() {
        let profile = Profile::new("bad-sftp", ProviderType::Sftp);
        let result = Provider::from_profile(&profile);
        assert!(result.is_err());
    }

    #[test]
    fn from_profile_s3_missing_bucket_fails() {
        let mut profile = Profile::new("bad-s3", ProviderType::S3);
        profile.set_setting("region".into(), "us-east-1".into());
        let result = Provider::from_profile(&profile);
        assert!(result.is_err());
    }

    #[test]
    fn from_profile_webdav_missing_url_fails() {
        let profile = Profile::new("bad-webdav", ProviderType::WebDav);
        let result = Provider::from_profile(&profile);
        assert!(result.is_err());
    }

    #[test]
    fn registry_register_and_get() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(Provider::Local(LocalFs::new("test")));
        let id = provider.id();
        registry.register(provider);

        assert!(registry.contains(&id));
        let found = registry.get(&id);
        assert!(found.is_some());
        assert_eq!(found.unwrap().label(), "test");
    }

    #[test]
    fn registry_unregister() {
        let registry = ProviderRegistry::new();
        let provider = Arc::new(Provider::Local(LocalFs::new("test")));
        let id = provider.id();
        registry.register(provider);
        registry.unregister(&id);

        assert!(!registry.contains(&id));
        assert!(registry.get(&id).is_none());
    }

    #[test]
    fn registry_list() {
        let registry = ProviderRegistry::new();
        // Use different provider types so they have different filesystem IDs.
        registry.register(Arc::new(Provider::Local(LocalFs::new("a"))));
        let s3 = Provider::from_profile(&s3_profile()).unwrap();
        registry.register(Arc::new(s3));

        let list = registry.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn registry_replaces_existing() {
        let registry = ProviderRegistry::new();
        let provider1 = Arc::new(Provider::Local(LocalFs::new("old")));
        let id = provider1.id();
        registry.register(provider1);

        let provider2 = Arc::new(Provider::Local(LocalFs::new("new")));
        // Force the same ID for testing by using the same underlying fs.
        // LocalFs always returns the same ID, so this works.
        registry.register(provider2);

        let found = registry.get(&id).unwrap();
        assert_eq!(found.label(), "new");
    }

    #[test]
    fn provider_from_profile_ftp_id_deterministic() {
        let p1 = Provider::from_profile(&ftp_profile()).unwrap();
        let p2 = Provider::from_profile(&ftp_profile()).unwrap();
        assert_eq!(p1.id(), p2.id());
    }

    #[test]
    fn provider_from_profile_s3_id_deterministic() {
        let p1 = Provider::from_profile(&s3_profile()).unwrap();
        let p2 = Provider::from_profile(&s3_profile()).unwrap();
        assert_eq!(p1.id(), p2.id());
    }

    #[tokio::test]
    async fn registry_get_or_create_caches_provider() {
        let registry = ProviderRegistry::new();
        let profile = s3_profile();

        // Create a fake filesystem ID that matches the S3 provider's
        // deterministic ID.
        let provider = Provider::from_profile(&profile).unwrap();
        let id = provider.id();

        let result = registry.get_or_create(&id, &profile).await;
        assert!(result.is_ok());

        // Second call should return the cached provider.
        let cached = registry.get(&id);
        assert!(cached.is_some());
    }

    #[tokio::test]
    async fn registry_get_or_create_missing_profile_fails() {
        let registry = ProviderRegistry::new();
        let bad_profile = Profile::new("bad-s3", ProviderType::S3); // missing required fields
        let fake_id = FileSystemId::new(999u64);

        let result = registry.get_or_create(&fake_id, &bad_profile).await;
        assert!(result.is_err());
    }

    #[test]
    fn provider_delegates_label() {
        let provider = Provider::Local(LocalFs::new("my-local"));
        assert_eq!(provider.label(), "my-local");
    }

    #[test]
    fn provider_delegates_id() {
        let local = Provider::Local(LocalFs::new("test"));
        let id = local.id();
        assert!(*id.get() > 0);
    }

    #[test]
    fn provider_default_registry() {
        let registry = ProviderRegistry::default();
        assert!(registry.list().is_empty());
    }

    // ── URL-based provider resolution ──

    fn ftp_url(input: &str) -> Url {
        Url::parse(input).expect("valid test URL")
    }

    fn temp_store(dir: &std::path::Path) -> ProfileStore {
        ProfileStore::new(dir.join("profiles.toml"), vec![0xA5u8; 32])
    }

    fn ftp_config(provider: &Provider) -> &FtpConfig {
        match provider {
            Provider::Ftp(fs) => fs.config(),
            other => panic!("expected an FTP provider, got {other:?}"),
        }
    }

    fn sftp_config(provider: &Provider) -> &SftpConfig {
        match provider {
            Provider::Sftp(fs) => fs.config(),
            other => panic!("expected an SFTP provider, got {other:?}"),
        }
    }

    #[test]
    fn from_url_local() {
        let provider =
            Provider::from_url(&ftp_url("./src"), &Credentials::default())
                .unwrap();
        assert_eq!(provider.provider_type(), ProviderType::Local);
    }

    #[test]
    fn from_url_ftp_keeps_url_details() {
        let provider = Provider::from_url(
            &ftp_url("ftp://ftp.example.com/pub/src"),
            &Credentials::new(Some("alice".into()), Some("s3cret".into())),
        )
        .unwrap();
        let config = ftp_config(&provider);
        assert_eq!(config.host, "ftp.example.com");
        assert_eq!(config.port, 21);
        assert_eq!(config.username, "alice");
        assert_eq!(config.password, "s3cret");
        assert!(!config.tls);
        // The URL path selects the scan root, so it must not also
        // prefix every resolved path a second time.
        assert!(config.root_path.is_none());
    }

    #[test]
    fn from_url_ftps_defaults_to_tls() {
        let provider = Provider::from_url(
            &ftp_url("ftps://secure.example.com:990/data"),
            &Credentials::default(),
        )
        .unwrap();
        let config = ftp_config(&provider);
        assert_eq!(config.port, 990);
        assert!(config.tls);
    }

    #[test]
    fn from_url_sftp_keeps_url_details() {
        let provider = Provider::from_url(
            &ftp_url("sftp://files.example.com:2222/pub/src"),
            &Credentials::new(Some("alice".into()), Some("s3cret".into())),
        )
        .unwrap();
        let config = sftp_config(&provider);
        assert_eq!(config.host, "files.example.com");
        assert_eq!(config.port, 2222);
        assert_eq!(config.username, "alice");
        assert_eq!(config.password.as_deref(), Some("s3cret"));
        assert!(config.key_file.is_none());
        // The URL path selects the scan root, so it must not also
        // prefix every resolved path a second time.
        assert!(config.root_path.is_none());
    }

    #[test]
    fn from_url_sftp_defaults_port() {
        let provider = Provider::from_url(
            &ftp_url("sftp://files.example.com/pub"),
            &Credentials::default(),
        )
        .unwrap();
        assert_eq!(sftp_config(&provider).port, 22);
    }

    #[test]
    fn from_url_gates_scaffolding_backends() {
        // S3 and WebDAV would panic on first I/O, so resolution must
        // fail with a clean error instead of constructing them.
        for input in [
            "s3://my-bucket/prefix",
            "webdav://dav.example.com/share",
            "webdavs://dav.example.com/share",
        ] {
            let error =
                Provider::from_url(&ftp_url(input), &Credentials::default())
                    .unwrap_err();
            assert!(
                matches!(error, ProviderError::Unimplemented { .. }),
                "unexpected error for {input}: {error}"
            );
        }
    }

    #[test]
    fn resolve_local_needs_no_credentials() {
        let provider = Provider::resolve(
            &ftp_url("file:///tmp/data"),
            None,
            None,
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap();
        assert_eq!(provider.provider_type(), ProviderType::Local);
    }

    #[test]
    fn resolve_explicit_profile_supplies_credentials() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        store.add(ftp_profile()).unwrap();
        let provider = Provider::resolve(
            &ftp_url("ftp://ftp.example.com/pub/src"),
            Some(&store),
            Some("test-ftp"),
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap();
        let config = ftp_config(&provider);
        assert_eq!(config.username, "user");
        assert_eq!(config.password, "pass");
        assert_eq!(provider.label(), "test-ftp");
    }

    #[test]
    fn resolve_auto_matches_profile() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        store.add(ftp_profile()).unwrap();
        let provider = Provider::resolve(
            &ftp_url("ftp://ftp.example.com/pub/src"),
            Some(&store),
            None,
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap();
        let config = ftp_config(&provider);
        assert_eq!(config.username, "user");
        assert_eq!(config.password, "pass");
    }

    #[test]
    fn resolve_missing_profile_fails() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        let error = Provider::resolve(
            &ftp_url("ftp://ftp.example.com/pub/src"),
            Some(&store),
            Some("nope"),
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ProviderError::Profile(ProfileError::NotFound(_))
        ));
    }

    #[test]
    fn resolve_profile_endpoint_mismatch_fails() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        // `test-ftp` addresses ftp.example.com, not other.example.com.
        store.add(ftp_profile()).unwrap();
        let error = Provider::resolve(
            &ftp_url("ftp://other.example.com/pub/src"),
            Some(&store),
            Some("test-ftp"),
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap_err();
        assert!(matches!(error, ProviderError::ProfileMismatch { .. }));
    }

    #[test]
    fn resolve_without_credentials_fails_on_non_tty() {
        // `with_vars` also guarantees the fallback variables are unset
        // for this assertion (the mutex serializes env mutations).
        temp_env::with_vars(
            vec![
                ("COCOMO_FTP_USER", None::<&str>),
                ("COCOMO_FTP_PASSWORD", None::<&str>),
            ],
            || {
                let error = Provider::resolve(
                    &ftp_url("ftp://ftp.example.com/pub/src"),
                    None,
                    None,
                    &Secrets::with_keychain(false),
                    &NullPrompter,
                )
                .unwrap_err();
                assert!(matches!(error, ProviderError::AuthRequired { .. }));
                assert!(error.to_string().contains("authentication required"));
            },
        );
    }

    #[test]
    fn resolve_falls_back_to_environment_credentials() {
        temp_env::with_vars(
            vec![
                ("COCOMO_FTP_USER", Some("bob")),
                ("COCOMO_FTP_PASSWORD", Some("envpass")),
            ],
            || {
                let provider = Provider::resolve(
                    &ftp_url("ftp://ftp.example.com/pub/src"),
                    None,
                    None,
                    &Secrets::with_keychain(false),
                    &NullPrompter,
                )
                .unwrap();
                let config = ftp_config(&provider);
                assert_eq!(config.username, "bob");
                assert_eq!(config.password, "envpass");
            },
        );
    }

    #[test]
    fn resolve_sftp_explicit_profile_supplies_credentials() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        store.add(sftp_profile()).unwrap();
        let provider = Provider::resolve(
            &ftp_url("sftp://sftp.example.com/pub/src"),
            Some(&store),
            Some("test-sftp"),
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap();
        let config = sftp_config(&provider);
        assert_eq!(config.username, "user");
        assert_eq!(config.password.as_deref(), Some("pass"));
        assert_eq!(provider.label(), "test-sftp");
    }

    #[test]
    fn resolve_sftp_profile_key_file_is_used() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        let mut profile = Profile::new("key-sftp", ProviderType::Sftp);
        profile.set_setting("host".into(), "sftp.example.com".into());
        profile.set_setting("username".into(), "user".into());
        profile.set_setting(
            "key_file".into(),
            "/home/user/.ssh/id_ed25519".to_string(),
        );
        store.add(profile).unwrap();
        // A key file alone satisfies the authentication requirement; no
        // password is needed.
        let provider = Provider::resolve(
            &ftp_url("sftp://sftp.example.com/pub/src"),
            Some(&store),
            None,
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap();
        let config = sftp_config(&provider);
        assert_eq!(
            config.key_file.as_deref(),
            Some(Path::new("/home/user/.ssh/id_ed25519")),
        );
        assert!(config.password.is_none());
    }

    #[test]
    fn resolve_sftp_without_credentials_fails_on_non_tty() {
        // `with_vars` also guarantees the fallback variables are unset
        // for this assertion (the mutex serializes env mutations).
        temp_env::with_vars(
            vec![
                ("COCOMO_SFTP_USER", None::<&str>),
                ("COCOMO_SFTP_PASSWORD", None::<&str>),
            ],
            || {
                let error = Provider::resolve(
                    &ftp_url("sftp://sftp.example.com/pub/src"),
                    None,
                    None,
                    &Secrets::with_keychain(false),
                    &NullPrompter,
                )
                .unwrap_err();
                assert!(matches!(error, ProviderError::AuthRequired { .. }));
            },
        );
    }

    #[test]
    fn resolve_sftp_falls_back_to_environment_credentials() {
        temp_env::with_vars(
            vec![
                ("COCOMO_SFTP_USER", Some("bob")),
                ("COCOMO_SFTP_PASSWORD", Some("envpass")),
            ],
            || {
                let provider = Provider::resolve(
                    &ftp_url("sftp://sftp.example.com/pub/src"),
                    None,
                    None,
                    &Secrets::with_keychain(false),
                    &NullPrompter,
                )
                .unwrap();
                let config = sftp_config(&provider);
                assert_eq!(config.username, "bob");
                assert_eq!(config.password.as_deref(), Some("envpass"));
            },
        );
    }

    #[test]
    fn resolve_sftp_profile_endpoint_mismatch_fails() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        // `test-sftp` addresses sftp.example.com, not other.example.com.
        store.add(sftp_profile()).unwrap();
        let error = Provider::resolve(
            &ftp_url("sftp://other.example.com/pub/src"),
            Some(&store),
            Some("test-sftp"),
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap_err();
        assert!(matches!(error, ProviderError::ProfileMismatch { .. }));
    }

    #[test]
    fn resolve_profile_tls_key_overrides_scheme_default() {
        let dir = tempdir().unwrap();
        let store = temp_store(dir.path());
        let mut profile = Profile::new("ftps-insecure", ProviderType::Ftp);
        profile.set_setting("host".into(), "ftp.example.com".into());
        profile.set_setting("username".into(), "user".into());
        profile.set_setting("tls".into(), "false".into());
        profile.secrets.set("password".into(), "pass".into());
        store.add(profile).unwrap();
        // An ftps URL defaults to TLS, but the profile overrides it.
        let provider = Provider::resolve(
            &ftp_url("ftps://ftp.example.com/data"),
            Some(&store),
            None,
            &Secrets::with_keychain(false),
            &NullPrompter,
        )
        .unwrap();
        assert!(!ftp_config(&provider).tls);
    }

    #[test]
    fn credentials_debug_redacts_values() {
        let creds =
            Credentials::new(Some("alice".into()), Some("s3cret".into()));
        let text = format!("{creds:?}");
        assert!(!text.contains("alice"));
        assert!(!text.contains("s3cret"));
    }
}
