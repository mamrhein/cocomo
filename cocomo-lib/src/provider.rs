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
//! ([`LocalFs`], [`FtpFs`], [`S3Fs`], [`WebDavFs`]) behind a single type.
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
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

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
    profile::{Profile, ProfileError, ProviderType},
    s3::{S3Config, S3Fs},
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
    /// Amazon S3.
    S3(S3Fs),
    /// WebDAV.
    WebDav(WebDavFs),
}

impl Provider {
    /// Return the provider type for this instance.
    pub fn provider_type(&self) -> ProviderType {
        match self {
            Self::Local(_) => ProviderType::Local,
            Self::Ftp(_) => ProviderType::Ftp,
            Self::S3(_) => ProviderType::S3,
            Self::WebDav(_) => ProviderType::WebDav,
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
            Self::S3(_) => f.debug_tuple("Provider::S3").finish(),
            Self::WebDav(_) => f.debug_tuple("Provider::WebDav").finish(),
        }
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
            Self::S3(p) => p.metadata(path).await,
            Self::WebDav(p) => p.metadata(path).await,
        }
    }

    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>> {
        match self {
            Self::Local(p) => p.read_dir(path).await,
            Self::Ftp(p) => p.read_dir(path).await,
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
            Self::S3(p) => p.read_stream(path, range).await,
            Self::WebDav(p) => p.read_stream(path, range).await,
        }
    }

    async fn write(&self, path: &Path, data: Bytes) -> Result<()> {
        match self {
            Self::Local(p) => p.write(path, data).await,
            Self::Ftp(p) => p.write(path, data).await,
            Self::S3(p) => p.write(path, data).await,
            Self::WebDav(p) => p.write(path, data).await,
        }
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.create_dir(path).await,
            Self::Ftp(p) => p.create_dir(path).await,
            Self::S3(p) => p.create_dir(path).await,
            Self::WebDav(p) => p.create_dir(path).await,
        }
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.remove(path).await,
            Self::Ftp(p) => p.remove(path).await,
            Self::S3(p) => p.remove(path).await,
            Self::WebDav(p) => p.remove(path).await,
        }
    }

    async fn remove_all(&self, path: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.remove_all(path).await,
            Self::Ftp(p) => p.remove_all(path).await,
            Self::S3(p) => p.remove_all(path).await,
            Self::WebDav(p) => p.remove_all(path).await,
        }
    }

    async fn rename(&self, src: &Path, dst: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.rename(src, dst).await,
            Self::Ftp(p) => p.rename(src, dst).await,
            Self::S3(p) => p.rename(src, dst).await,
            Self::WebDav(p) => p.rename(src, dst).await,
        }
    }

    async fn copy(&self, src: &Path, dst: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.copy(src, dst).await,
            Self::Ftp(p) => p.copy(src, dst).await,
            Self::S3(p) => p.copy(src, dst).await,
            Self::WebDav(p) => p.copy(src, dst).await,
        }
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        match self {
            Self::Local(p) => p.read_link(path).await,
            Self::Ftp(p) => p.read_link(path).await,
            Self::S3(p) => p.read_link(path).await,
            Self::WebDav(p) => p.read_link(path).await,
        }
    }

    async fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
        match self {
            Self::Local(p) => p.symlink(target, link).await,
            Self::Ftp(p) => p.symlink(target, link).await,
            Self::S3(p) => p.symlink(target, link).await,
            Self::WebDav(p) => p.symlink(target, link).await,
        }
    }

    fn label(&self) -> &str {
        match self {
            Self::Local(p) => p.label(),
            Self::Ftp(p) => p.label(),
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
            Self::S3(p) => p.id(),
            Self::WebDav(p) => p.id(),
        }
    }

    fn label_node(&self) -> &str {
        match self {
            Self::Local(p) => p.label_node(),
            Self::Ftp(p) => p.label_node(),
            Self::S3(p) => p.label_node(),
            Self::WebDav(p) => p.label_node(),
        }
    }

    async fn resolve_path(&self, path: &Path) -> Result<NodeId<Self::Nid>> {
        match self {
            Self::Local(p) => p.resolve_path(path).await,
            Self::Ftp(p) => p.resolve_path(path).await,
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
            Self::S3(p) => p.resolve_symlink(id).await,
            Self::WebDav(p) => p.resolve_symlink(id).await,
        }
    }

    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>> {
        match self {
            Self::Local(p) => p.get_node(id),
            Self::Ftp(p) => p.get_node(id),
            Self::S3(p) => p.get_node(id),
            Self::WebDav(p) => p.get_node(id),
        }
    }

    fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata> {
        match self {
            Self::Local(p) => p.node_metadata(id),
            Self::Ftp(p) => p.node_metadata(id),
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
            Self::S3(p) => p.set_node_hash(id, hash),
            Self::WebDav(p) => p.set_node_hash(id, hash),
        }
    }

    async fn read_dir_node(&self, id: DirId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.read_dir_node(id).await,
            Self::Ftp(p) => p.read_dir_node(id).await,
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
            Self::S3(p) => p.write_node(id, data).await,
            Self::WebDav(p) => p.write_node(id, data).await,
        }
    }

    async fn flush_node(&self, id: FileId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.flush_node(id).await,
            Self::Ftp(p) => p.flush_node(id).await,
            Self::S3(p) => p.flush_node(id).await,
            Self::WebDav(p) => p.flush_node(id).await,
        }
    }

    async fn remove_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.remove_node(id).await,
            Self::Ftp(p) => p.remove_node(id).await,
            Self::S3(p) => p.remove_node(id).await,
            Self::WebDav(p) => p.remove_node(id).await,
        }
    }

    async fn remove_all_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        match self {
            Self::Local(p) => p.remove_all_node(id).await,
            Self::Ftp(p) => p.remove_all_node(id).await,
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
    use super::*;
    use crate::profile::Profile;

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
}
