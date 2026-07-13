// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! cocomo_lib — Unified async filesystem library for file/folder comparison,
//! copying, and moving.
//!
//! # Architecture
//!
//! All filesystem I/O is async behind the [`FileSystem`] trait. Built-in
//! providers include [`LocalFs`] for the local filesystem, [`S3Fs`] for
//! Amazon S3, [`FtpFs`] for FTP/FTPS, and [`WebDavFs`] for WebDAV. Front-ends
//! (TUI, GUI, CLI) await on the same abstractions, keeping the library as the
//! single source of business logic.
//!
//! # Key modules
//!
//! - [`fs`] — Core [`FileSystem`] trait and related types.
//! - [`identity`] — Opaque identifiers for filesystems and nodes.
//! - [`node`] — Filesystem node model, classification, and symlink targets.
//! - [`local`] — [`LocalFs`], the local filesystem provider.
//! - [`s3`] — [`S3Fs`], the Amazon S3 provider.
//! - [`ftp`] — [`FtpFs`], the FTP/FTPS provider.
//! - [`webdav`] — [`WebDavFs`], the WebDAV provider.
//! - [`provider`] — [`Provider`] enum and [`ProviderRegistry`] for unified
//!   access to all backends.
//! - [`hash`] — Content hashing (blake3) and LRU [`ContentCache`].
//! - [`scan`] — Directory scanning that produces a tree of [`ScanEntry`]
//!   nodes.
//! - [`compare`] — Directory comparison engine that merges two scanned trees.
//! - [`filter`] — Name, display, and content filters for comparison results.
//! - [`text`] — Line-based text comparison engine.
//! - [`grammar`] — Syntax-aware grammar rules for smart diffing.
//! - [`format`] — File format registry and format-specific settings.
//! - [`error`] — Structured [`FsError`] with operation context.
//! - [`meta`] — [`Metadata`] for files and directories.
//! - [`file`] — [`FsFile`] trait for opened file handles.
//! - [`profile`] — Connection profiles with encrypted secrets.
//! - [`session`] — Session management for workspaces.
//! - [`snapshot`] — Point-in-time snapshots of directory trees.

#![forbid(unsafe_code)]

pub mod compare;
pub mod error;
pub mod file;
pub mod filter;
pub mod format;
pub mod fs;
pub mod ftp;
pub mod grammar;
pub mod hash;
pub mod identity;
pub mod local;
pub mod meta;
pub mod node;
pub mod profile;
pub mod provider;
pub mod report;
pub mod s3;
pub mod scan;
pub mod session;
pub mod snapshot;
pub mod sync;
pub mod text;
pub mod transfer;
pub mod webdav;

// Re-exports for convenience.
pub use compare::{
    CompareConfig, DirComparison, DirEntry, DirEntryStatus, EntryInfo,
    compare_directories, compare_directories_node,
};
pub use error::{FsError, FsOperation, Result};
pub use file::FsFile;
pub use format::{
    FileFormat, FormatRegistry, FormatSettings, FormatType, LineEnding,
    TableParser, TextEncoding,
};
pub use fs::{
    DirEntryMeta, FileSystem, NodeFileSystem, OpenMode, WritableFileSystem,
};
pub use ftp::{FtpConfig, FtpDirId, FtpFileId, FtpFs, FtpNodeId};
pub use grammar::{Grammar, GrammarRule, Importance};
pub use hash::{
    ContentCache, ContentCacheConfig, ContentId, hash_and_cache_node,
    hash_bytes, hash_file, hash_file_node,
};
pub use identity::{DirId, FileId, FileSystemId, NodeId};
pub use local::LocalFs;
pub use meta::Metadata;
pub use node::{Node, NodeKind, SymlinkTarget, UserPermissions};
pub use profile::{
    EncryptedSecrets, Profile, ProfileError, ProfileStore, ProviderType,
    default_store_path, derive_master_key,
};
pub use provider::{Provider, ProviderRegistry};
pub use s3::{S3Config, S3DirId, S3FileId, S3Fs, S3NodeId};
pub use scan::{
    ScanConfig, ScanEntry, ScanResult, scan_directory, scan_directory_node,
};
pub use session::{
    ProviderRef, Session, SessionConfig, SessionManager, SessionSettings,
    SessionType,
};
pub use snapshot::{
    ProviderId, Snapshot, SnapshotEntry, SnapshotEntryStatus, SnapshotManager,
    capture_snapshot, entry_matches_node,
};
pub use sync::{
    SyncOperation, SyncResult, SyncRules, plan_sync, sync_directories,
};
pub use text::{
    AlignmentMode, LineInfo, TextCompareSettings, TextDiff, TextDifference,
    Token, WhitespaceMode, compare_texts, lines_equal,
};
pub use transfer::{
    TransferAction, TransferItem, TransferResult, execute_transfers,
    plan_transfers,
};
pub use webdav::{
    WebDavConfig, WebDavDirId, WebDavFileId, WebDavFs, WebDavNodeId,
};
