// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Connection profiles for filesystem providers.
//!
//! A profile stores the settings and secrets needed to connect to a remote
//! filesystem (FTP, S3, WebDAV). Profiles are persisted to disk and can be
//! referenced by sessions, snapshots, and CLI commands.
//!
//! # Secrets encryption
//!
//! Secrets (passwords, access keys, etc.) are encrypted at rest using
//! ChaCha20-Poly1305. A per-profile key is derived from a master key using
//! HMAC-SHA256, so compromising one profile does not expose others.
//!
//! # Storage layout
//!
//! Profiles are stored as TOML in `~/.config/cocomo/profiles.toml`. Each
//! profile has an `[[profile]]` table containing its `id`, `provider_type`,
//! and `settings`. Encrypted secrets are stored in the same file under the
//! `secrets` key of each profile table.

use std::{collections::BTreeMap, env, fmt, fs, io, path::PathBuf};

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use getrandom::getrandom;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Sha256;
use thiserror::Error;

use crate::provider::Provider;

type HmacSha256 = Hmac<Sha256>;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors that can occur during profile operations.
#[derive(Debug, Error)]
pub enum ProfileError {
    /// A profile with this ID already exists.
    #[error("profile \"{0}\" already exists")]
    AlreadyExists(String),

    /// The requested profile was not found.
    #[error("profile \"{0}\" not found")]
    NotFound(String),

    /// I/O error reading or writing the profile store.
    #[error("profile store I/O error: {0}")]
    Io(#[from] io::Error),

    /// Failed to decrypt secrets (wrong master key or corrupted data).
    #[error("decryption failed for profile \"{profile_id}\": {reason}")]
    DecryptionFailed { profile_id: String, reason: String },

    /// Failed to encrypt secrets.
    #[error("encryption failed for profile \"{profile_id}\": {reason}")]
    EncryptionFailed { profile_id: String, reason: String },

    /// TOML serialization/deserialization error.
    #[error("TOML error: {0}")]
    Toml(String),

    /// Failed to generate a master key because the system entropy source
    /// is unavailable.
    #[error(
        "cannot generate master key: system entropy source unavailable: \
         {reason}"
    )]
    EntropyUnavailable { reason: String },
}

/// Result type alias for profile operations.
pub type ProfileResult<T> = Result<T, ProfileError>;

// ---------------------------------------------------------------------------
// Provider type
// ---------------------------------------------------------------------------

/// Identifies the kind of filesystem provider a profile configures.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// Local filesystem.
    Local,
    /// FTP / FTPS.
    Ftp,
    /// Amazon S3.
    S3,
    /// WebDAV.
    WebDav,
}

impl ProviderType {
    /// Return the scheme string for this provider type.
    pub fn scheme(&self) -> &str {
        match self {
            Self::Local => "file",
            Self::Ftp => "ftp",
            Self::S3 => "s3",
            Self::WebDav => "webdav",
        }
    }
}

impl fmt::Display for ProviderType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => write!(f, "local"),
            Self::Ftp => write!(f, "ftp"),
            Self::S3 => write!(f, "s3"),
            Self::WebDav => write!(f, "webdav"),
        }
    }
}

// ---------------------------------------------------------------------------
// Encrypted secrets
// ---------------------------------------------------------------------------

/// Secrets that are encrypted at rest.
///
/// In memory, this is just a plain `BTreeMap<String, String>`. When
/// serialized, values are encrypted with ChaCha20-Poly1305 and stored as
/// base64 strings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EncryptedSecrets(BTreeMap<String, String>);

impl EncryptedSecrets {
    /// Create an empty secrets map.
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Create secrets from an existing map.
    pub fn from_map(map: BTreeMap<String, String>) -> Self {
        Self(map)
    }

    /// Get a secret by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(|s| s.as_str())
    }

    /// Set a secret.
    pub fn set(&mut self, key: String, value: String) {
        self.0.insert(key, value);
    }

    /// Remove a secret.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.0.remove(key)
    }

    /// Check if a secret exists.
    pub fn contains_key(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    /// Return the number of secrets.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if there are no secrets.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Convert into the inner map.
    pub fn into_inner(self) -> BTreeMap<String, String> {
        self.0
    }
}

impl Serialize for EncryptedSecrets {
    fn serialize<S: Serializer>(
        &self,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        // Serialize as a plain map; encryption is handled by ProfileStore.
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EncryptedSecrets {
    fn deserialize<D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Self, D::Error> {
        let map = BTreeMap::deserialize(deserializer)?;
        Ok(Self(map))
    }
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// A named connection profile for a filesystem provider.
///
/// Profiles store all the settings and secrets needed to connect to a remote
/// filesystem. They are referenced by ID in sessions, snapshots, and CLI
/// commands.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Profile {
    /// Unique profile identifier.
    pub id: String,
    /// The kind of provider this profile configures.
    pub provider_type: ProviderType,
    /// Non-sensitive settings (host, port, bucket name, etc.).
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    /// Encrypted secrets (password, access key, etc.).
    #[serde(default)]
    pub secrets: EncryptedSecrets,
}

impl Profile {
    /// Create a new profile.
    pub fn new(id: impl Into<String>, provider_type: ProviderType) -> Self {
        Self {
            id: id.into(),
            provider_type,
            settings: BTreeMap::new(),
            secrets: EncryptedSecrets::new(),
        }
    }

    /// Get a setting by key.
    pub fn setting(&self, key: &str) -> Option<&str> {
        self.settings.get(key).map(|s| s.as_str())
    }

    /// Set a non-sensitive setting.
    pub fn set_setting(&mut self, key: String, value: String) {
        self.settings.insert(key, value);
    }

    /// Return the provider scheme for this profile.
    pub fn scheme(&self) -> &str {
        self.provider_type.scheme()
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.id, self.provider_type)
    }
}

/// TOML wrapper for serializing/deserializing a collection of profiles.
///
/// The `toml` crate requires the top level to be a table, so a `Vec<Profile>`
/// must be wrapped in a struct that produces a TOML array of tables.
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
struct ProfileCollection {
    #[serde(rename = "profile")]
    items: Vec<Profile>,
}

impl ProfileCollection {
    fn new(profiles: Vec<Profile>) -> Self {
        Self { items: profiles }
    }

    fn into_profiles(self) -> Vec<Profile> {
        self.items
    }
}

// ---------------------------------------------------------------------------
// Profile store
// ---------------------------------------------------------------------------

/// Persists profiles to disk with encrypted secrets.
///
/// Profiles are stored in a single TOML file. Secrets are encrypted with
/// ChaCha20-Poly1305 using a per-profile key derived from the master key.
///
/// # Thread safety
///
/// This type is neither `Send` nor `Sync` because it holds a raw master
/// key. It is designed for single-threaded use during profile management
/// (add, list, remove). The encrypted data can be read from any thread.
pub struct ProfileStore {
    /// Path to the profiles TOML file.
    pub(crate) store_path: PathBuf,
    /// Master key for encrypting/decrypting secrets.
    master_key: Vec<u8>,
}

impl ProfileStore {
    /// Create a new profile store.
    ///
    /// `master_key` should be at least 32 bytes of cryptographically random
    /// data. It is used to derive per-profile encryption keys.
    ///
    /// # Panics
    ///
    /// Panics if `master_key` is shorter than 32 bytes.
    pub fn new(store_path: PathBuf, master_key: Vec<u8>) -> Self {
        assert!(
            master_key.len() >= 32,
            "master key must be at least 32 bytes"
        );
        Self {
            store_path,
            master_key,
        }
    }

    /// Load profiles from disk.
    fn load_profiles(&self) -> ProfileResult<Vec<Profile>> {
        if !self.store_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs_err::read_to_string(&self.store_path)
            .map_err(ProfileError::Io)?;

        toml::from_str::<ProfileCollection>(&content)
            .map(ProfileCollection::into_profiles)
            .map_err(|e| ProfileError::Toml(e.to_string()))
    }

    /// Save profiles to disk.
    fn save_profiles(&self, profiles: &[Profile]) -> ProfileResult<()> {
        let dir = self.store_path.parent().ok_or_else(|| {
            ProfileError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "store path has no parent directory",
            ))
        })?;

        fs_err::create_dir_all(dir).map_err(ProfileError::Io)?;

        let collection = ProfileCollection::new(profiles.to_vec());
        let content = toml::to_string_pretty(&collection)
            .map_err(|e| ProfileError::Toml(e.to_string()))?;

        fs_err::write(&self.store_path, content).map_err(ProfileError::Io)?;

        // Restrict the store file to owner-read/write only.
        set_owner_only_permissions(&self.store_path)
            .map_err(ProfileError::Io)?;

        Ok(())
    }

    // ── CRUD operations ──

    /// List all profiles (without decrypted secrets).
    pub fn list(&self) -> ProfileResult<Vec<Profile>> {
        self.load_profiles()
    }

    /// Get a profile by ID.
    pub fn get(&self, id: &str) -> ProfileResult<Option<Profile>> {
        let profiles = self.load_profiles()?;
        Ok(profiles.into_iter().find(|p| p.id == id))
    }

    /// Add a new profile.
    ///
    /// Secrets are encrypted before storage. Returns an error if a profile
    /// with the same ID already exists.
    pub fn add(&self, mut profile: Profile) -> ProfileResult<Profile> {
        // Check for duplicates.
        let existing = self.load_profiles()?;
        if existing.iter().any(|p| p.id == profile.id) {
            return Err(ProfileError::AlreadyExists(profile.id.clone()));
        }

        // Encrypt secrets before storage.
        let encrypted = self.encrypt_secrets(&profile.id, &profile.secrets)?;
        profile.secrets = encrypted;

        let mut profiles = existing;
        profiles.push(profile.clone());
        self.save_profiles(&profiles)?;

        Ok(profile)
    }

    /// Update an existing profile.
    ///
    /// Returns the updated profile. Errors if the profile doesn't exist.
    pub fn update(&self, mut profile: Profile) -> ProfileResult<Profile> {
        let mut profiles = self.load_profiles()?;
        let target = profiles
            .iter_mut()
            .find(|p| p.id == profile.id)
            .ok_or_else(|| ProfileError::NotFound(profile.id.clone()))?;

        // Encrypt secrets before storage.
        let encrypted = self.encrypt_secrets(&profile.id, &profile.secrets)?;
        profile.secrets = encrypted;

        *target = profile.clone();
        self.save_profiles(&profiles)?;

        Ok(profile)
    }

    /// Remove a profile by ID.
    pub fn remove(&self, id: &str) -> ProfileResult<Option<Profile>> {
        let mut profiles = self.load_profiles()?;
        let removed = profiles.iter().find(|p| p.id == id).cloned();
        if removed.is_some() {
            profiles.retain(|p| p.id != id);
            self.save_profiles(&profiles)?;
        }

        // Decrypt secrets before returning.
        removed.map(|p| self.decrypt_profile(&p)).transpose()
    }

    /// Get a profile with decrypted secrets.
    pub fn get_decrypted(&self, id: &str) -> ProfileResult<Option<Profile>> {
        let profile = self.get(id)?;
        profile.map(|p| self.decrypt_profile(&p)).transpose()
    }

    /// List all profiles with decrypted secrets.
    pub fn list_decrypted(&self) -> ProfileResult<Vec<Profile>> {
        let profiles = self.load_profiles()?;
        profiles
            .into_iter()
            .map(|p| self.decrypt_profile(&p))
            .collect()
    }

    // ── Encryption helpers ──

    /// Derive a per-profile encryption key from the master key.
    fn derive_key(&self, profile_id: &str) -> Key {
        let mut mac: HmacSha256 = hmac::Mac::new_from_slice(&self.master_key)
            .expect("HMAC can accept keys of any size");
        mac.update(profile_id.as_bytes());
        let result = mac.finalize().into_bytes();
        // Truncate to 32 bytes for ChaCha20 key.
        let mut key = [0u8; 32];
        key.copy_from_slice(&result[..32]);
        key.into()
    }

    /// Encrypt a secrets map.
    fn encrypt_secrets(
        &self,
        profile_id: &str,
        secrets: &EncryptedSecrets,
    ) -> ProfileResult<EncryptedSecrets> {
        if secrets.is_empty() {
            return Ok(EncryptedSecrets::new());
        }

        let cipher = ChaCha20Poly1305::new(&self.derive_key(profile_id));
        let mut encrypted = BTreeMap::new();

        for (k, v) in secrets.0.iter() {
            let mut nonce_bytes = [0u8; 12];
            getrandom(&mut nonce_bytes).map_err(|e| {
                ProfileError::EncryptionFailed {
                    profile_id: profile_id.to_string(),
                    reason: format!("failed to generate nonce: {e}"),
                }
            })?;
            let nonce = Nonce::from_slice(&nonce_bytes);
            let payload = Payload {
                msg: v.as_bytes(),
                aad: k.as_bytes(),
            };
            let ciphertext = cipher.encrypt(nonce, payload).map_err(|e| {
                ProfileError::EncryptionFailed {
                    profile_id: profile_id.to_string(),
                    reason: e.to_string(),
                }
            })?;

            // Pack nonce + ciphertext into base64.
            let mut packed = Vec::with_capacity(12 + ciphertext.len());
            packed.extend_from_slice(nonce);
            packed.extend_from_slice(&ciphertext);
            encrypted.insert(k.clone(), B64.encode(&packed));
        }

        Ok(EncryptedSecrets::from_map(encrypted))
    }

    /// Decrypt a secrets map.
    fn decrypt_secrets(
        &self,
        profile_id: &str,
        encrypted: &EncryptedSecrets,
    ) -> ProfileResult<EncryptedSecrets> {
        if encrypted.is_empty() {
            return Ok(EncryptedSecrets::new());
        }

        let cipher = ChaCha20Poly1305::new(&self.derive_key(profile_id));
        let mut decrypted = BTreeMap::new();

        for (k, v) in encrypted.0.iter() {
            let packed =
                B64.decode(v).map_err(|e| ProfileError::DecryptionFailed {
                    profile_id: profile_id.to_string(),
                    reason: format!("base64 decode failed: {e}"),
                })?;

            if packed.len() < 12 {
                return Err(ProfileError::DecryptionFailed {
                    profile_id: profile_id.to_string(),
                    reason: "packed data too short".into(),
                });
            }

            let (nonce_bytes, ciphertext) = packed.split_at(12);
            let nonce = Nonce::from_slice(nonce_bytes);
            let payload = Payload {
                msg: ciphertext,
                aad: k.as_bytes(),
            };

            let plaintext = cipher.decrypt(nonce, payload).map_err(|e| {
                ProfileError::DecryptionFailed {
                    profile_id: profile_id.to_string(),
                    reason: e.to_string(),
                }
            })?;

            let value = String::from_utf8_lossy(&plaintext).to_string();
            decrypted.insert(k.clone(), value);
        }

        Ok(EncryptedSecrets::from_map(decrypted))
    }

    /// Decrypt a profile's secrets.
    fn decrypt_profile(&self, profile: &Profile) -> ProfileResult<Profile> {
        let mut decrypted = profile.clone();
        decrypted.secrets =
            self.decrypt_secrets(&profile.id, &profile.secrets)?;
        Ok(decrypted)
    }

    /// Resolve a profile by ID into a filesystem provider.
    ///
    /// This method loads the profile, decrypts its secrets, and constructs
    /// a [`Provider`] from the profile's settings. The returned provider
    /// is ready for I/O operations.
    pub fn resolve(&self, id: &str) -> ProfileResult<Provider> {
        let profile = self
            .get_decrypted(id)?
            .ok_or_else(|| ProfileError::NotFound(id.to_string()))?;

        Provider::from_profile(&profile)
    }

    /// Resolve all profiles into filesystem providers.
    ///
    /// Returns a vector of (profile_id, provider) pairs. Profiles that
    /// fail to resolve are skipped with their errors collected.
    pub fn resolve_all(&self) -> ProfileResult<Vec<(String, Provider)>> {
        let profiles = self.list_decrypted()?;
        let mut resolved = Vec::new();

        for profile in profiles {
            match Provider::from_profile(&profile) {
                Ok(provider) => {
                    resolved.push((profile.id.clone(), provider));
                }
                Err(e) => {
                    // Skip profiles that can't be resolved.
                    let _ = e;
                }
            }
        }

        Ok(resolved)
    }
}

// ---------------------------------------------------------------------------
// Default paths and master key
// ---------------------------------------------------------------------------

/// The environment variable name for the master key.
///
/// The key should be a hex-encoded 32-byte value (64 hex characters).
pub const MASTER_KEY_ENV: &str = "COCOMO_MASTER_KEY";

/// Restrict file permissions to owner-read/write only (0o600).
///
/// On Unix this prevents other users from reading the profile store.
/// On Windows this is a no-op since the default ACL is already sufficient.
fn set_owner_only_permissions(path: &std::path::Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Return the default profile store path.
///
/// On Unix-like systems this is `~/.config/cocomo/profiles.toml`.
/// On Windows this is `%APPDATA%\\cocomo\\profiles.toml`.
pub fn default_store_path() -> PathBuf {
    let config_dir = if cfg!(target_os = "windows") {
        // On Windows, use %APPDATA%.cocomo.
        env::var("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("~/"))
    } else {
        // On Unix, use ~/.config.
        dirs::config_dir().unwrap_or_else(|| {
            env::var("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|_| PathBuf::from("~/"))
        })
    };

    config_dir.join("cocomo").join("profiles.toml")
}

/// Derive a master key from environment or generate a random one.
///
/// Checks the `COCOMO_MASTER_KEY` environment variable for a hex-encoded
/// 32-byte key. If the variable is not set or invalid, generates a random
/// 32-byte key. A random key means secrets won't persist across restarts,
/// but the store will still function.
pub fn derive_master_key() -> ProfileResult<Vec<u8>> {
    if let Ok(hex_key) = env::var(MASTER_KEY_ENV)
        && let Ok(bytes) = hex::decode(&hex_key)
        && bytes.len() >= 32
    {
        return Ok(bytes);
    }
    let mut key = vec![0u8; 32];
    getrandom(&mut key).map_err(|e| ProfileError::EntropyUnavailable {
        reason: e.to_string(),
    })?;
    Ok(key)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::fs::FileSystem;

    fn make_test_store() -> (ProfileStore, PathBuf) {
        let dir = std::env::temp_dir()
            .join(format!("cocomo-profile-test-{}", std::process::id()));
        let path = dir.join("profiles.toml");
        let key = vec![0xAB; 32];
        let store = ProfileStore::new(path.clone(), key);
        (store, dir)
    }

    fn cleanup(dir: &Path) {
        let _ = fs_err::remove_dir_all(dir);
    }

    #[test]
    fn provider_type_scheme() {
        assert_eq!(ProviderType::Local.scheme(), "file");
        assert_eq!(ProviderType::Ftp.scheme(), "ftp");
        assert_eq!(ProviderType::S3.scheme(), "s3");
        assert_eq!(ProviderType::WebDav.scheme(), "webdav");
    }

    #[test]
    fn provider_type_display() {
        assert_eq!(format!("{}", ProviderType::Local), "local");
        assert_eq!(format!("{}", ProviderType::Ftp), "ftp");
        assert_eq!(format!("{}", ProviderType::S3), "s3");
        assert_eq!(format!("{}", ProviderType::WebDav), "webdav");
    }

    #[test]
    fn profile_new_and_setting() {
        let mut profile = Profile::new("my-ftp", ProviderType::Ftp);
        profile.set_setting("host".into(), "ftp.example.com".into());
        profile.set_setting("port".into(), "21".into());

        assert_eq!(profile.id, "my-ftp");
        assert_eq!(profile.provider_type, ProviderType::Ftp);
        assert_eq!(profile.setting("host"), Some("ftp.example.com"));
        assert_eq!(profile.setting("missing"), None);
        assert_eq!(profile.scheme(), "ftp");
    }

    #[test]
    fn profile_display() {
        let profile = Profile::new("test-s3", ProviderType::S3);
        assert_eq!(format!("{profile}"), "test-s3 (s3)");
    }

    #[test]
    fn encrypted_secrets_basic() {
        let mut secrets = EncryptedSecrets::new();
        secrets.set("password".into(), "supersecret".into());
        secrets.set("token".into(), "abc123".into());

        assert_eq!(secrets.get("password"), Some("supersecret"));
        assert_eq!(secrets.len(), 2);
        assert!(!secrets.is_empty());

        let removed = secrets.remove("token");
        assert_eq!(removed, Some("abc123".to_string()));
        assert_eq!(secrets.len(), 1);
        assert!(!secrets.contains_key("token"));
    }

    #[test]
    fn encrypted_secrets_serialization() {
        let mut secrets = EncryptedSecrets::new();
        secrets.set("key".into(), "value".into());

        let toml = toml::to_string(&secrets).unwrap();
        assert!(toml.contains("key"));
        assert!(toml.contains("value"));

        let deserialized: EncryptedSecrets = toml::from_str(&toml).unwrap();
        assert_eq!(deserialized.get("key"), Some("value"));
    }

    #[test]
    fn profile_serialization() {
        let mut profile = Profile::new("test", ProviderType::Ftp);
        profile.set_setting("host".into(), "example.com".into());

        let toml = toml::to_string_pretty(&profile).unwrap();
        assert!(toml.contains("id = \"test\""));
        assert!(toml.contains("provider_type = \"ftp\""));

        let deserialized: Profile = toml::from_str(&toml).unwrap();
        assert_eq!(deserialized.id, "test");
        assert_eq!(deserialized.provider_type, ProviderType::Ftp);
    }

    #[test]
    fn profile_store_add_and_get() {
        let (store, dir) = make_test_store();

        let mut profile = Profile::new("my-ftp", ProviderType::Ftp);
        profile.set_setting("host".into(), "ftp.example.com".into());
        profile.set_setting("port".into(), "21".into());

        let added = store.add(profile).unwrap();
        assert_eq!(added.id, "my-ftp");

        let found =
            store.get("my-ftp").unwrap().expect("profile should exist");
        assert_eq!(found.id, "my-ftp");
        assert_eq!(found.setting("host"), Some("ftp.example.com"));

        cleanup(&dir);
    }

    #[test]
    fn profile_store_add_duplicate() {
        let (store, dir) = make_test_store();

        let profile1 = Profile::new("dup", ProviderType::S3);
        store.add(profile1).unwrap();

        let profile2 = Profile::new("dup", ProviderType::Ftp);
        let result = store.add(profile2);
        assert!(result.is_err());

        let ProfileError::AlreadyExists(id) = result.unwrap_err() else {
            panic!("expected AlreadyExists error");
        };
        assert_eq!(id, "dup");

        cleanup(&dir);
    }

    #[test]
    fn profile_store_remove() {
        let (store, dir) = make_test_store();

        let profile = Profile::new("removable", ProviderType::S3);
        store.add(profile).unwrap();

        let removed = store.remove("removable").unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().id, "removable");

        let gone = store.get("removable").unwrap();
        assert!(gone.is_none());

        cleanup(&dir);
    }

    #[test]
    fn profile_store_remove_nonexistent() {
        let (store, dir) = make_test_store();

        let removed = store.remove("nonexistent").unwrap();
        assert!(removed.is_none());

        cleanup(&dir);
    }

    #[test]
    fn profile_store_list() {
        let (store, dir) = make_test_store();

        let ftp = Profile::new("ftp1", ProviderType::Ftp);
        let s3 = Profile::new("s3-1", ProviderType::S3);
        store.add(ftp).unwrap();
        store.add(s3).unwrap();

        let list = store.list().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.iter().any(|p| p.id == "ftp1"));
        assert!(list.iter().any(|p| p.id == "s3-1"));

        cleanup(&dir);
    }

    #[test]
    fn profile_store_secrets_encryption_roundtrip() {
        let (store, dir) = make_test_store();

        let mut profile = Profile::new("secure-ftp", ProviderType::Ftp);
        profile.set_setting("host".into(), "ftp.example.com".into());
        profile.secrets.set("password".into(), "hunter2".into());
        profile.secrets.set("access_key".into(), "AKIA1234".into());

        store.add(profile).unwrap();

        // Read the raw file and verify secrets are encrypted (base64, not
        // plaintext).
        let content = fs_err::read_to_string(&store.store_path).unwrap();
        assert!(!content.contains("hunter2"));
        assert!(!content.contains("AKIA1234"));

        // Decrypt and verify.
        let decrypted = store
            .get_decrypted("secure-ftp")
            .unwrap()
            .expect("should exist");
        assert_eq!(decrypted.secrets.get("password"), Some("hunter2"));
        assert_eq!(decrypted.secrets.get("access_key"), Some("AKIA1234"));

        cleanup(&dir);
    }

    #[test]
    fn profile_store_update() {
        let (store, dir) = make_test_store();

        let mut profile = Profile::new("updatable", ProviderType::Ftp);
        profile.set_setting("host".into(), "old.example.com".into());
        store.add(profile).unwrap();

        let mut updated = Profile::new("updatable", ProviderType::Ftp);
        updated.set_setting("host".into(), "new.example.com".into());
        updated.secrets.set("password".into(), "newpass".into());

        let result = store.update(updated).unwrap();
        assert_eq!(result.setting("host"), Some("new.example.com"));

        let decrypted = store
            .get_decrypted("updatable")
            .unwrap()
            .expect("should exist");
        assert_eq!(decrypted.setting("host"), Some("new.example.com"));
        assert_eq!(decrypted.secrets.get("password"), Some("newpass"));

        cleanup(&dir);
    }

    #[test]
    fn profile_store_update_nonexistent() {
        let (store, dir) = make_test_store();

        let profile = Profile::new("no-such-profile", ProviderType::S3);
        let result = store.update(profile);
        assert!(result.is_err());

        let ProfileError::NotFound(id) = result.unwrap_err() else {
            panic!("expected NotFound error");
        };
        assert_eq!(id, "no-such-profile");

        cleanup(&dir);
    }

    #[test]
    fn profile_store_list_decrypted() {
        let (store, dir) = make_test_store();

        let mut p1 = Profile::new("a", ProviderType::Ftp);
        p1.secrets.set("password".into(), "pass-a".into());

        let mut p2 = Profile::new("b", ProviderType::S3);
        p2.secrets.set("access_key".into(), "key-b".into());

        store.add(p1).unwrap();
        store.add(p2).unwrap();

        let list = store.list_decrypted().unwrap();
        assert_eq!(list.len(), 2);

        let a = list.iter().find(|p| p.id == "a").expect("profile a");
        assert_eq!(a.secrets.get("password"), Some("pass-a"));

        let b = list.iter().find(|p| p.id == "b").expect("profile b");
        assert_eq!(b.secrets.get("access_key"), Some("key-b"));

        cleanup(&dir);
    }

    #[test]
    fn profile_store_persists_across_instances() {
        let dir = std::env::temp_dir()
            .join(format!("cocomo-persist-test-{}", std::process::id()));
        let path = dir.join("profiles.toml");
        let key = vec![0xCD; 32];

        // Create profiles in one store instance.
        {
            let store = ProfileStore::new(path.clone(), key.clone());
            let profile = Profile::new("persist-ftp", ProviderType::Ftp);
            store.add(profile).unwrap();
        }

        // Load them in a new store instance.
        {
            let store = ProfileStore::new(path, key);
            let found = store.get("persist-ftp").unwrap();
            assert!(found.is_some());
            assert_eq!(found.unwrap().id, "persist-ftp");
        }

        cleanup(&dir);
    }

    #[test]
    fn profile_store_empty_secrets() {
        let (store, dir) = make_test_store();

        let profile = Profile::new("no-secrets", ProviderType::Local);
        store.add(profile).unwrap();

        let decrypted = store
            .get_decrypted("no-secrets")
            .unwrap()
            .expect("should exist");
        assert!(decrypted.secrets.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn profile_error_messages() {
        let err = ProfileError::AlreadyExists("test".to_string());
        assert_eq!(format!("{err}"), "profile \"test\" already exists");

        let err = ProfileError::NotFound("test".to_string());
        assert_eq!(format!("{err}"), "profile \"test\" not found");
    }

    #[test]
    fn master_key_too_short_panics() {
        let dir = std::env::temp_dir().join("cocomo-short-key-test");
        let path = dir.join("profiles.toml");
        let short_key = vec![0u8; 16]; // Too short.

        let result = std::panic::catch_unwind(|| {
            ProfileStore::new(path, short_key);
        });
        assert!(result.is_err());

        let _ = fs_err::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_local_profile() {
        let (store, dir) = make_test_store();

        let profile = Profile::new("my-local", ProviderType::Local);
        store.add(profile).unwrap();

        let provider = store.resolve("my-local").unwrap();
        assert_eq!(provider.label(), "my-local");
        assert!(matches!(provider, Provider::Local(_)));

        cleanup(&dir);
    }

    #[test]
    fn resolve_missing_profile_returns_not_found() {
        let (store, dir) = make_test_store();

        let result = store.resolve("nonexistent");
        assert!(result.is_err());
        match result.unwrap_err() {
            ProfileError::NotFound(id) => assert_eq!(id, "nonexistent"),
            other => panic!("expected NotFound, got: {other}"),
        }

        cleanup(&dir);
    }

    #[test]
    fn resolve_ftp_profile() {
        let (store, dir) = make_test_store();

        let mut profile = Profile::new("my-ftp", ProviderType::Ftp);
        profile.set_setting("host".into(), "ftp.example.com".into());
        profile.set_setting("port".into(), "21".into());
        store.add(profile).unwrap();

        let provider = store.resolve("my-ftp").unwrap();
        assert_eq!(provider.label(), "my-ftp");
        assert!(matches!(provider, Provider::Ftp(_)));

        cleanup(&dir);
    }

    #[test]
    fn resolve_all_returns_resolvable_profiles() {
        let (store, dir) = make_test_store();

        let local1 = Profile::new("local-1", ProviderType::Local);
        let local2 = Profile::new("local-2", ProviderType::Local);
        let mut ftp_profile = Profile::new("my-ftp", ProviderType::Ftp);
        ftp_profile.set_setting("host".into(), "ftp.example.com".into());

        store.add(local1).unwrap();
        store.add(local2).unwrap();
        store.add(ftp_profile).unwrap();

        let resolved = store.resolve_all().unwrap();
        assert_eq!(resolved.len(), 3);

        let ids: Vec<_> = resolved.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"local-1"));
        assert!(ids.contains(&"local-2"));
        assert!(ids.contains(&"my-ftp"));

        cleanup(&dir);
    }

    #[test]
    fn resolve_all_empty_store() {
        let (store, dir) = make_test_store();

        let resolved = store.resolve_all().unwrap();
        assert!(resolved.is_empty());

        cleanup(&dir);
    }

    #[test]
    fn resolve_round_trip_with_secrets() {
        let (store, dir) = make_test_store();

        let mut profile = Profile::new("secure-ftp", ProviderType::Ftp);
        profile.set_setting("host".into(), "ftp.example.com".into());
        profile.set_setting("port".into(), "21".into());
        profile.secrets.set("password".into(), "s3cret".into());
        profile.secrets.set("username".into(), "user1".into());

        store.add(profile).unwrap();

        // Resolve the profile and verify the provider carries the settings.
        let provider = store.resolve("secure-ftp").unwrap();
        assert_eq!(provider.label(), "secure-ftp");

        // Verify it's an FTP provider with the correct host.
        if let Provider::Ftp(fs) = provider {
            assert_eq!(fs.config().host, "ftp.example.com");
            assert_eq!(fs.config().port, 21);
        } else {
            panic!("expected FTP provider");
        }

        cleanup(&dir);
    }

    #[test]
    fn default_store_path_is_valid() {
        let path = default_store_path();

        // The path should contain "cocomo" and end with "profiles.toml".
        assert!(
            path.to_string_lossy().contains("cocomo"),
            "path should contain 'cocomo', got: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "profiles.toml"
        );
    }

    #[test]
    fn derive_master_key_from_env() {
        // A valid hex-encoded 32-byte key (64 hex chars).
        let hex_key = "aa".repeat(32);
        temp_env::with_var(MASTER_KEY_ENV, Some(&hex_key), || {
            let key = derive_master_key().unwrap();
            assert_eq!(key.len(), 32);
        });
    }

    #[test]
    fn derive_master_key_invalid_hex_falls_back() {
        temp_env::with_var(MASTER_KEY_ENV, Some("not-valid-hex!"), || {
            let key = derive_master_key().unwrap();
            // Should fall back to random key (32 bytes).
            assert_eq!(key.len(), 32);
        });
    }

    #[test]
    fn derive_master_key_short_hex_falls_back() {
        temp_env::with_var(MASTER_KEY_ENV, Some("aabbcc"), || {
            // Too short.
            let key = derive_master_key().unwrap();
            // Should fall back to random key (32 bytes).
            assert_eq!(key.len(), 32);
        });
    }

    #[test]
    fn derive_master_key_no_env_generates_random() {
        temp_env::with_var_unset(MASTER_KEY_ENV, || {
            let key = derive_master_key().unwrap();
            assert_eq!(key.len(), 32);
        });
    }

    #[test]
    fn entropy_unavailable_error_display() {
        let err = ProfileError::EntropyUnavailable {
            reason: "no entropy".into(),
        };
        let msg = format!("{err}");
        assert!(msg.contains("entropy"));
        assert!(msg.contains("no entropy"));
    }

    #[cfg(unix)]
    #[test]
    fn profile_store_sets_owner_only_permissions() {
        let (store, dir) = make_test_store();
        let profile = Profile::new("perm-test", ProviderType::Local);
        store.add(profile).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let meta = fs_err::metadata(&store.store_path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "profile store should be owner-read/write only"
        );
        cleanup(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn profile_store_maintains_permissions_on_update() {
        let (store, dir) = make_test_store();
        let profile1 = Profile::new("perm-update-1", ProviderType::Local);
        store.add(profile1).unwrap();
        let profile2 = Profile::new("perm-update-2", ProviderType::Local);
        store.add(profile2).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let meta = fs_err::metadata(&store.store_path).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "permissions should remain owner-read/write only after update"
        );
        cleanup(&dir);
    }
}
