// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Content hashing and caching.
//!
//! Files are identified for equality by a `(size, mtime, hash)` triple.
//! Hashes are computed with `blake3` (fast, parallel). Streaming
//! `Hasher::update()` is used for large files to avoid loading entire files
//! into memory.
//!
//! An in-memory `ContentCache` stores recent hashes so repeated scans avoid
//! re-reading. The cache uses LRU eviction with a configurable max size and
//! TTL to bound memory usage.

use std::{
    collections::HashMap,
    hash::{Hash, Hasher as StdHasher},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use blake3::Hash as Blake3Hash;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;

use crate::{
    error::FsOperation,
    fs::{FileSystem, NodeFileSystem},
    identity::FileId,
    meta::Metadata,
    node::Node,
};

/// The triple used to identify file content for equality checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContentId {
    /// File size in bytes.
    pub size: u64,
    /// Last modification time.
    pub modified: DateTime<Utc>,
    /// Blake3 content hash (hex-encoded).
    pub hash: String,
}

impl ContentId {
    /// Build a `ContentId` from metadata and a pre-computed hash string.
    pub fn new(size: u64, modified: DateTime<Utc>, hash: String) -> Self {
        Self {
            size,
            modified,
            hash,
        }
    }

    /// Build a `ContentId` from metadata and a raw blake3 hash.
    pub fn from_blake3(meta: &Metadata, hash: &Blake3Hash) -> Self {
        Self {
            size: meta.size,
            modified: meta.modified,
            hash: hash.to_hex().to_string(),
        }
    }
}

impl Hash for ContentId {
    fn hash<H: StdHasher>(&self, state: &mut H) {
        self.size.hash(state);
        self.modified.hash(state);
        self.hash.hash(state);
    }
}

/// Compute the blake3 hash of file content by reading it through a
/// `FileSystem` provider in a streaming fashion.
pub async fn hash_file(
    fs: &dyn FileSystem,
    path: &Path,
) -> crate::error::Result<Blake3Hash> {
    let mut hasher = blake3::Hasher::new();
    let mut stream = fs.read_stream(path, None).await?;
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| crate::error::FsError::Io {
            operation: crate::error::FsOperation::Read,
            path: path.to_path_buf(),
            message: format!("stream error: {e}"),
        })?;
        hasher.update(&chunk);
    }
    Ok(hasher.finalize())
}

/// Compute the blake3 hash of file content by reading it through a
/// node-based `NodeFileSystem` provider in a streaming fashion.
///
/// Unlike [`hash_file`], this uses a [`FileId`] instead of a path, avoiding
/// TOCTOU races.
pub async fn hash_file_node<N>(
    fs: &N,
    file_id: FileId<N::Nid>,
    node: &Node,
) -> crate::error::Result<Blake3Hash>
where
    N: NodeFileSystem,
{
    let mut hasher = blake3::Hasher::new();
    let mut stream = fs.read_stream_node(file_id, None).await?;
    use futures::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| crate::error::FsError::Io {
            operation: FsOperation::Read,
            path: node.path().to_path_buf(),
            message: format!("stream error: {e}"),
        })?;
        hasher.update(&chunk);
    }
    Ok(hasher.finalize())
}

/// Compute the blake3 hash of in-memory data.
pub fn hash_bytes(data: &[u8]) -> Blake3Hash {
    blake3::hash(data)
}

/// A cached `ContentId` entry with metadata for LRU eviction and TTL.
struct CacheEntry {
    content_id: ContentId,
    last_access: Instant,
}

/// Configuration for the content cache.
#[derive(Clone, Debug)]
pub struct ContentCacheConfig {
    /// Maximum number of entries before LRU eviction kicks in.
    pub max_entries: usize,
    /// Time-to-live for cache entries. Entries older than this are evicted.
    pub ttl_secs: u64,
}

impl Default for ContentCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 4096,
            ttl_secs: 300, // 5 minutes
        }
    }
}

/// A unique cache key combining a provider label with a file path.
///
/// Uses `(String, PathBuf)` rather than a string concatenation to avoid
/// collisions from `Path::display()` lossy encoding and separator ambiguity.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    label: String,
    path: PathBuf,
}

impl CacheKey {
    fn new(label: &str, path: &Path) -> Self {
        Self {
            label: label.to_string(),
            path: path.to_path_buf(),
        }
    }
}

/// In-memory LRU cache for file content hashes.
///
/// Stores recent `(size, mtime, hash)` triples keyed by `(path,
/// provider_label)` so repeated scans avoid re-reading files. Cache entries
/// are invalidated on write operations and by a configurable TTL.
pub struct ContentCache {
    inner: Mutex<ContentCacheInner>,
    config: ContentCacheConfig,
}

struct ContentCacheInner {
    entries: HashMap<CacheKey, CacheEntry>,
    /// Access order for LRU eviction. Most recent at the end.
    access_order: Vec<CacheKey>,
}

impl ContentCache {
    /// Create a new cache with the given configuration.
    pub fn new(config: ContentCacheConfig) -> Self {
        Self {
            inner: Mutex::new(ContentCacheInner {
                entries: HashMap::new(),
                access_order: Vec::new(),
            }),
            config,
        }
    }

    /// Create a new cache with default configuration.
    pub fn default_config() -> Self {
        Self::new(ContentCacheConfig::default())
    }

    /// Build the cache key from provider label and file path.
    fn cache_key(label: &str, path: &Path) -> CacheKey {
        CacheKey::new(label, path)
    }

    /// Look up a cached `ContentId` for the given provider and path.
    /// Returns `None` if the entry is missing or expired.
    pub fn get(&self, label: &str, path: &Path) -> Option<ContentId> {
        let key = Self::cache_key(label, path);
        let now = Instant::now();
        let ttl = Duration::from_secs(self.config.ttl_secs);

        let mut inner = self.inner.lock();

        // Check existence and expiry first, cloning the data we need.
        let (expired, content_id) =
            if let Some(entry) = inner.entries.get(&key) {
                let expired = now.duration_since(entry.last_access) > ttl;
                (expired, entry.content_id.clone())
            } else {
                return None;
            };

        if expired {
            // Entry expired — evict it.
            inner.entries.remove(&key);
            inner.access_order.retain(|k| k != &key);
            return None;
        }

        // Update access time and move to end (most recent).
        if let Some(entry) = inner.entries.get_mut(&key) {
            entry.last_access = now;
        }
        inner.access_order.retain(|k| k != &key);
        inner.access_order.push(key);
        Some(content_id)
    }

    /// Insert a `ContentId` into the cache, evicting LRU entries if full.
    pub fn insert(&self, label: &str, path: &Path, content_id: ContentId) {
        let key = Self::cache_key(label, path);
        let now = Instant::now();

        let mut inner = self.inner.lock();

        // Evict expired entries first.
        let ttl = Duration::from_secs(self.config.ttl_secs);
        let expired_keys: Vec<CacheKey> = inner
            .entries
            .iter()
            .filter(|(_k, v)| now.duration_since(v.last_access) > ttl)
            .map(|(k, _v)| k.clone())
            .collect();
        for ek in &expired_keys {
            inner.entries.remove(ek);
        }
        inner.access_order.retain(|k| !expired_keys.contains(k));

        // If still full, evict LRU entries.
        while inner.entries.len() >= self.config.max_entries {
            if let Some(lru_key) = inner.access_order.first().cloned() {
                inner.entries.remove(&lru_key);
                inner.access_order.remove(0);
            } else {
                break;
            }
        }

        // Insert or update.
        inner.entries.insert(
            key.clone(),
            CacheEntry {
                content_id,
                last_access: now,
            },
        );
        inner.access_order.retain(|k| k != &key);
        inner.access_order.push(key);
    }

    /// Remove a specific entry from the cache. Used to invalidate after
    /// write operations.
    pub fn invalidate(&self, label: &str, path: &Path) {
        let key = Self::cache_key(label, path);
        let mut inner = self.inner.lock();
        inner.entries.remove(&key);
        inner.access_order.retain(|k| k != &key);
    }

    /// Clear all cached entries.
    pub fn clear(&self) {
        let mut inner = self.inner.lock();
        inner.entries.clear();
        inner.access_order.clear();
    }

    /// Return the number of entries currently in the cache.
    pub fn len(&self) -> usize {
        self.inner.lock().entries.len()
    }

    /// Return `true` if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;

    use super::*;

    #[test]
    fn hash_bytes_deterministic() {
        let h1 = hash_bytes(b"hello");
        let h2 = hash_bytes(b"hello");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_bytes_different_content() {
        let h1 = hash_bytes(b"hello");
        let h2 = hash_bytes(b"world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn content_id_equality() {
        let now = Utc::now();
        let a = ContentId::new(100, now, "abc123".to_string());
        let b = ContentId::new(100, now, "abc123".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn content_id_hash_trait() {
        let now = Utc::now();
        let a = ContentId::new(100, now, "abc123".to_string());
        let b = ContentId::new(100, now, "abc123".to_string());
        assert_eq!(hash_for(&a), hash_for(&b));
    }

    fn hash_for<T: Hash>(t: &T) -> u64 {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    }

    #[test]
    fn cache_insert_and_get() {
        let cache = ContentCache::default_config();
        let path = Path::new("/tmp/test.txt");
        let cid = ContentId::new(42, Utc::now(), "xyz".to_string());
        cache.insert("local", path, cid.clone());
        assert_eq!(cache.get("local", path), Some(cid));
    }

    #[test]
    fn cache_miss_different_provider() {
        let cache = ContentCache::default_config();
        let path = Path::new("/tmp/test.txt");
        let cid = ContentId::new(42, Utc::now(), "xyz".to_string());
        cache.insert("local", path, cid);
        assert!(cache.get("remote", path).is_none());
    }

    #[test]
    fn cache_invalidate() {
        let cache = ContentCache::default_config();
        let path = Path::new("/tmp/test.txt");
        let cid = ContentId::new(42, Utc::now(), "xyz".to_string());
        cache.insert("local", path, cid);
        cache.invalidate("local", path);
        assert!(cache.get("local", path).is_none());
    }

    #[test]
    fn cache_clear() {
        let cache = ContentCache::default_config();
        cache.insert(
            "local",
            Path::new("/a"),
            ContentId::new(1, Utc::now(), "h".to_string()),
        );
        cache.insert(
            "local",
            Path::new("/b"),
            ContentId::new(2, Utc::now(), "h".to_string()),
        );
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn cache_lru_eviction() {
        let config = ContentCacheConfig {
            max_entries: 2,
            ttl_secs: 300,
        };
        let cache = ContentCache::new(config);
        cache.insert(
            "l",
            Path::new("/a"),
            ContentId::new(1, Utc::now(), "h".to_string()),
        );
        cache.insert(
            "l",
            Path::new("/b"),
            ContentId::new(2, Utc::now(), "h".to_string()),
        );
        // Inserting a third entry should evict /a (LRU).
        cache.insert(
            "l",
            Path::new("/c"),
            ContentId::new(3, Utc::now(), "h".to_string()),
        );
        assert!(cache.get("l", Path::new("/a")).is_none());
        assert!(cache.get("l", Path::new("/b")).is_some());
        assert!(cache.get("l", Path::new("/c")).is_some());
    }

    #[test]
    fn from_blake3() {
        let meta = crate::meta::Metadata::file(100, Utc::now());
        let h = hash_bytes(b"test data");
        let cid = ContentId::from_blake3(&meta, &h);
        assert_eq!(cid.size, 100);
        assert_eq!(cid.modified, meta.modified);
    }

    #[test]
    fn cache_key_no_collision_with_separator_in_path() {
        // A path containing "::" should not collide with the label separator.
        let cache = ContentCache::default_config();
        let cid_a = ContentId::new(1, Utc::now(), "a".to_string());
        let cid_b = ContentId::new(2, Utc::now(), "b".to_string());

        cache.insert("local", Path::new("a/b::c"), cid_a.clone());
        cache.insert("local", Path::new("a/b::c/d"), cid_b.clone());

        assert_eq!(cache.get("local", Path::new("a/b::c")), Some(cid_a));
        assert_eq!(cache.get("local", Path::new("a/b::c/d")), Some(cid_b));
    }
}
