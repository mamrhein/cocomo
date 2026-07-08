# Requirements: cocomo

A file/folder comparison and synchronization tool inspired by Beyond Compare, written in Rust with four components:

- **`cocomo-lib`** — async filesystem abstraction library
- **`cocomo-tui`** — terminal UI (`ratatui`)
- **`cocomo-gui`** — desktop GUI (`gpui`)
- **`cocomo-cli`** — CLI / scripting front-end

---

## 1. Architecture

### 1.1 Crate Structure

```

cocomo/
├── cocomo_lib/ # core library
├── cocomo_tui/ # ratatui front-end
├── cocomo_gui/ # gpui front-end
└── cocomo_cli/ # CLI / scripting front-end

```

### 1.2 Design Principles

- The lib owns all business logic. Front-ends are thin view layers.
- All filesystem I/O is async behind a trait; front-ends await on the same abstractions.
- Session state is serializable to a single file format so TUI, GUI, and CLI can open the same session.
- The comparison engine (diffing) is CPU-bound and runs in `rayon` threads to avoid blocking the async runtime or the UI event loop.

### 1.3 Key Dependencies

| Layer          | Crate                   | Purpose                                               |
| -------------- | ----------------------- | ----------------------------------------------------- |
| Async runtime  | `tokio`                 | I/O event loop                                        |
| Parallelism    | `rayon`                 | CPU-bound diffing                                     |
| Filesystem I/O | `fs_err` + `walkdir`    | Rich error context; battle-tested directory traversal |
| TUI            | `ratatui` + `crossterm` | Terminal rendering                                    |
| GUI            | `gpui`                  | Desktop windowing (via Zed's toolkit)                 |
| CLI            | `clap`                  | Argument parsing                                      |
| Compression    | `flate2`, `zip`, `tar`  | Archive support                                       |
| Crypto         | `sha2`, `blake3`        | Content hashing for fast equality checks              |
| Image          | `image`                 | Picture comparison                                    |
| CSV/TSV        | `csv`, `serde`          | Table comparison                                      |
| Regex          | `regex`                 | Filters, grammars                                     |
| Config         | `serde_json` / `toml`   | Settings and sessions                                 |
| HTML report    | `askama`                | Templated output                                      |

---

## 2. cocomo_lib — Unified Async Filesystem Library

### 2.1 Core Traits: `FileSystem`, `NodeFileSystem`, `WritableFileSystem`

The library provides two parallel APIs for filesystem access. The legacy
path-based [`FileSystem`] trait remains for backward compatibility. The new
node-based traits, [`NodeFileSystem`] and [`WritableFileSystem`], operate on
opaque [`NodeId`] identifiers to eliminate TOCTOU races.

Backends implement both API surfaces. Downstream modules migrate gradually
from path-based to node-based operations.

#### 2.1.1 Opaque Identifiers

Every filesystem backend assigns its own node identifiers internally. IDs are
opaque to callers — not paths, not inodes, not memory addresses.

```rust
/// Opaque identifier for a filesystem instance.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileSystemId<F>;

/// Opaque identifier for a node within a filesystem.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId<N>;

/// Type-safe wrapper for a directory node. Conversion from `NodeId` is
/// unchecked — callers must verify the node kind first.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct DirId<N>(NodeId<N>);

/// Type-safe wrapper for a file node. Conversion from `NodeId` is unchecked.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId<N>(NodeId<N>);
```

For `LocalFs`, the concrete types are `NodeId<u64>`, `DirId<u64>`, and
`FileId<u64>`, with IDs allocated from a monotonically increasing counter.

#### 2.1.2 Node Model

Nodes are cached by the provider. Each node carries metadata, a kind
classification, the absolute filesystem path (for I/O anchoring), and a
parent reference (for navigation).

```rust
/// Access rights evaluated for the calling user/context.
bitflags::bitflags! {
    pub struct UserPermissions: u8 {
        const READ  = 0x04;
        const WRITE = 0x02;
        const EXEC  = 0x01;
    }
}

/// File or directory metadata captured at stat time.
pub struct Metadata {
    pub size: u64,
    pub created: Option<DateTime<Utc>>,
    pub modified: DateTime<Utc>,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub inode: Option<u64>,
    pub device_id: Option<u64>,
    pub permissions: UserPermissions,
}

/// Classification of a filesystem node. File content is never stored in
/// the node — it is always read via the `FileSystem` trait.
#[derive(Clone, Debug)]
pub enum NodeKind {
    Directory { children: Option<Vec<String>> },
    File,
    Symlink { target: SymlinkTarget },
    Special,
}

/// Target of a symbolic link. The raw path is always present; the resolved
/// node ID is populated lazily.
#[derive(Clone, Debug)]
pub struct SymlinkTarget {
    path: PathBuf,
    node: Option<NodeId<u64>>,
}

/// A node in the filesystem graph.
///
/// Directory children are resolved lazily. The `parent` field is a strong
/// index (not an `Arc`) into the provider cache, enabling O(1) "go up"
/// navigation. The `deleted` flag implements a tombstone: tombstoned nodes
/// are still addressable but return `FsError::StaleNode` on access.
pub struct Node {
    name: OsString,
    path: PathBuf,           // absolute path, used for OS I/O
    metadata: Metadata,
    kind: NodeKind,
    parent: Option<u64>,     // parent node ID, None for root
    deleted: bool,           // tombstone flag
}
```

#### 2.1.3 Node Cache

Each provider maintains its own in-memory node cache:

- **Node map**: `HashMap<NodeId, Arc<Node>>` — the primary cache of resolved
  nodes. Nodes are shared via `Arc` so that `get_node` returns a stable
  reference without holding a lock.
- **Path index**: `HashMap<PathBuf, NodeId>` — reverse lookup from absolute
  path to node ID, enabling O(1) deduplication on repeated resolutions.
- **ID allocator**: Monotonically increasing `AtomicU64` counter. IDs are
  never reused; deleted nodes are tombstoned instead.

The cache uses `parking_lot::RwLock` for interior mutability, enabling the
provider to satisfy `Send + Sync` trait bounds required by `async_trait`.

#### 2.1.4 Path-Based FileSystem Trait (legacy)

The original [`FileSystem`] trait uses path-based operations. It remains
unchanged for backward compatibility with existing downstream modules.

```rust
#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn metadata(&self, path: &Path) -> Result<Metadata>;
    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>>;
    async fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn FsFile>>;
    async fn read(&self, path: &Path, range: Option<Range<u64>>) -> Result<Bytes>;
    async fn read_stream(&self, path: &Path, range: Option<Range<u64>>) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>>;
    async fn write(&self, path: &Path, data: Bytes) -> Result<()>;
    async fn create_dir(&self, path: &Path) -> Result<()>;
    async fn remove(&self, path: &Path) -> Result<()>;
    async fn remove_all(&self, path: &Path) -> Result<()>;
    async fn rename(&self, src: &Path, dst: &Path) -> Result<()>;
    async fn copy(&self, src: &Path, dst: &Path) -> Result<()>;
    async fn read_link(&self, path: &Path) -> Result<PathBuf>;
    async fn symlink(&self, target: &Path, link: &Path) -> Result<()>;
    fn label(&self) -> &str;
}
```

#### 2.1.5 NodeFileSystem Trait (new, read-only)

The [`NodeFileSystem`] trait operates on opaque node identifiers. Paths are
resolved exactly once at entry time via `resolve_path()`, and all subsequent
I/O uses node identifiers.

```rust
#[async_trait]
pub trait NodeFileSystem: Send + Sync {
    type FsId: Copy + Eq + Hash + Debug + Default;
    type Nid: Copy + Eq + Hash + Debug + Default;
    type Error: Into<FsError>;

    fn id(&self) -> FileSystemId<Self::FsId>;
    fn label_node(&self) -> &str;

    // ── Resolution (path -> node, the entry point) ──
    async fn resolve_path(&self, path: &Path) -> Result<NodeId<Self::Nid>>;
    async fn resolve_symlink(&self, id: NodeId<Self::Nid>) -> Result<NodeId<Self::Nid>>;

    // ── Node access ──
    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>>;
    fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata>;

    // ── Directory traversal ──
    async fn read_dir_node(&self, id: DirId<Self::Nid>) -> Result<()>;

    // ── File I/O ──
    async fn open_node(&self, id: FileId<Self::Nid>, mode: OpenMode) -> Result<Box<dyn FsFile>>;
    async fn read_node(&self, id: FileId<Self::Nid>, range: Option<Range<u64>>) -> Result<Bytes>;
    async fn read_stream_node(&self, id: FileId<Self::Nid>, range: Option<Range<u64>>) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>>;
}
```

Note that `get_node` returns `Arc<Node>` rather than `&Node`. This is
necessary because the underlying cache uses `RwLock` for interior
mutability, and returning a reference tied to `&self` would require
holding the lock for the lifetime of the borrow, which is incompatible
with the `Send + Sync` requirement.

#### 2.1.6 WritableFileSystem Trait (new, write operations)

The [`WritableFileSystem`] trait extends [`NodeFileSystem`] with mutation
operations.

```rust
#[async_trait]
pub trait WritableFileSystem: NodeFileSystem {
    // ── Node creation ──
    async fn create_file(&self, parent: DirId<Self::Nid>, name: &OsStr) -> Result<FileId<Self::Nid>>;
    async fn create_dir_node(&self, parent: DirId<Self::Nid>, name: &OsStr) -> Result<DirId<Self::Nid>>;
    async fn create_symlink(&self, parent: DirId<Self::Nid>, name: &OsStr, target: &Path) -> Result<NodeId<Self::Nid>>;

    // ── Write I/O ──
    async fn write_node(&self, id: FileId<Self::Nid>, data: Bytes) -> Result<()>;
    async fn flush_node(&self, id: FileId<Self::Nid>) -> Result<()>;

    // ── High-level operations ──
    async fn remove_node(&self, id: NodeId<Self::Nid>) -> Result<()>;
    async fn remove_all_node(&self, id: NodeId<Self::Nid>) -> Result<()>;
    async fn rename_node(&self, id: NodeId<Self::Nid>, new_name: &OsStr) -> Result<()>;
    async fn copy_node(&self, src: NodeId<Self::Nid>, dst: DirId<Self::Nid>) -> Result<NodeId<Self::Nid>>;
    async fn move_node(&self, src: NodeId<Self::Nid>, dst: DirId<Self::Nid>) -> Result<NodeId<Self::Nid>>;
}
```

The split between [`NodeFileSystem`] (read) and [`WritableFileSystem`] (read

- write) allows read-only providers (archives, Git commits, S3 in some modes)
  to implement only the read trait.

### 2.2 Built-in Providers

| Provider    | Scheme                                          | Notes                                                                                                                    |
| ----------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `LocalFs`   | `file://` or bare paths                         | Uses `fs_err::tokio` for rich error context; `walkdir` for directory traversal; inode-based hardlink and cycle detection |
| `FtpFs`     | `ftp://`, `ftps://`                             | Async FTP via `async-ftp`; profile-based auth                                                                            |
| `S3Fs`      | `s3://bucket/path`                              | `aws-sdk-s3`; credential profiles                                                                                        |
| `WebDavFs`  | `davs://`                                       | `webdav` crate                                                                                                           |
| `ArchiveFs` | `zip://file.zip!/inner/`, `tar://`, `tar.gz://` | Virtual FS over read-only compressed archives                                                                            |
| `SshFs`     | `sftp://` (future)                              | SFTP over SSH keys/passwords                                                                                             |
| `GitFs`     | `git://repo@rev/path` (future)                  | Virtual FS over a specific commit                                                                                        |

### 2.3 Provider Registry

A central registry maps URI schemes to provider constructors. Users and plugins can register additional providers at runtime.

### 2.4 Profile Management

A `ProfileStore` manages named connection profiles with encrypted secrets:

```rust
pub struct Profile {
    pub id: String,
    pub provider_type: ProviderType,  // "ftp", "s3", "webdav", ...
    pub settings: BTreeMap<String, String>, // host, port, bucket, etc.
    pub secrets: EncryptedMap,          // password, access_key, etc.
}
```

Secrets are encrypted at rest using the OS keyring (Secret Service / macOS Keychain / Windows Credential Manager).

### 2.5 Content Hashing & Caching

- Files are identified for equality by `(size, mtime, hash)` triple.
- Hash is computed with `blake3` (fast, parallel). Streaming `Hasher::update()` is used for large files to avoid loading entire files into memory.
- Two hashing functions are provided: `hash_file()` uses the path-based
  [`FileSystem`] API, while `hash_file_node()` uses the node-based
  [`NodeFileSystem`] API and reads content via `FileId` identifiers.
- An in-memory `ContentCache` stores recent hashes so repeated scans avoid re-reading.
- Cache uses LRU eviction with a configurable max size and TTL to bound memory and open handles.
- Cache entries are invalidated on write operations and by a TTL.

### 2.5.1 Structured Errors

All `FileSystem` methods return `Result<T, FsError>` where `FsError` carries
rich context — the operation that failed, the path or node that was attempted,
and the underlying cause — so that callers can produce actionable diagnostics.

```rust
pub enum FsOperation {
    Open, Read, Write, Flush, Remove, Rename, Move, Copy, CreateDir, CreateFile,
    CreateSymlink, ReadDir, ReadLink, Symlink, Resolve,
}

pub enum FsError {
    Io { operation: FsOperation, path: PathBuf, message: String },
    PermissionDenied { operation: FsOperation, path: PathBuf },
    NotFound { path: PathBuf },
    InvalidArgument { operation: FsOperation, path: PathBuf, message: String },
    StaleNode,               // node was deleted or evicted
    NotResolved { field: &'static str },  // lazy field not yet populated
    WrongKind { expected: &'static str, actual: &'static str },
}
```

### 2.6 Transfer Operations Between Providers

The `transfer` module plans and executes file transfers (copy, move, delete)
between filesystem providers. Operations work on a `DirComparison` result from
the comparison engine and use the node-based [`WritableFileSystem`] API.

#### Planning

[`plan_transfers`] walks a `DirComparison` tree and collects all entries whose
status matches the given [`TransferAction`]. For example,
[`TransferAction::CopyRight`] collects all `LeftOnly` and `Different` entries.
Directories with resolved sub-entries are recursed into rather than collected
as single items, so the executor sees leaf-level operations.

```rust
pub enum TransferAction {
    CopyLeft,     // left -> right
    CopyRight,    // right -> left
    CopyCenter,   // center -> left & right (3-way merge)
    DeleteLeft,
    DeleteRight,
    MoveLeft,
    MoveRight,
}

pub struct TransferItem {
    pub action: TransferAction,
    pub name: String,
    pub is_dir: bool,
    pub rel_path: String,
}
```

#### Execution

[`execute_transfers`] runs a batch of [`TransferItem`] operations sequentially.
Each item is resolved to a node identifier via [`NodeFileSystem::resolve_path`],
and all I/O goes through the node-based API (`copy_node`, `write_node`,
`remove_all_node`, etc.). Non-fatal errors are collected rather than aborting
the entire batch.

```rust
pub struct TransferResult {
    pub succeeded: usize,
    pub failed: usize,
    pub errors: Vec<String>,
}
```

File copies stream content from source to destination via
[`NodeFileSystem::read_stream_node`] and [`WritableFileSystem::write_node`],
avoiding loading entire files into memory.

---

## 3. Comparison Engine

### 3.1 Directory Comparison

Produces a `DirComparison` result:

```rust
pub enum DirEntryStatus {
    Same,                    // equal by hash
    SameBinary,              // binary files, equal by hash
    Similar,                 // same name, size within tolerance
    Different,               // same name, different content
    LeftOnly,                // orphan in left
    RightOnly,               // orphan in right
    CenterOnly,              // orphan in center (3-way)
    Mergeable,               // can be auto-merged (text, no conflicts)
    Conflict,                // conflicting changes in 3-way
    IdenticalNameDifferentType, // e.g. file vs directory
}

pub struct DirEntry {
    pub name: String,
    pub status: DirEntryStatus,
    pub left: Option<EntryInfo>,
    pub right: Option<EntryInfo>,
    pub center: Option<EntryInfo>,  // 3-way merge
    pub sub_entries: Option<Arc<DirComparison>>,  // for directories
}
```

**Scan modes** (per session settings):

| Mode                      | Behavior                              |
| ------------------------- | ------------------------------------- |
| Compare files only        | Compare contents of matching names    |
| Compare structure only    | Compare names and dates, skip content |
| Compare files & structure | Full comparison                       |

**Recursive traversal**: directory scans delegate to `walkdir` for the local
filesystem. The walk is configured to:

- skip symlinks by default (configurable opt-in to follow symlinks),
- track visited inodes per device ID to detect hard-link and symlink cycles,
- detect filesystem boundary crossings (device ID changes) so inode tracking
  is scoped correctly per mount.

Per-entry traversal errors (e.g., permission denied) are reported individually
rather than aborting the entire scan.

**Node-based scanning and comparison**: the library also provides
`scan_directory_node()` and `compare_directories_node()` functions that
operate on the [`NodeFileSystem`] API. Unlike the path-based variants, these
resolve paths to node identifiers exactly once and perform all subsequent
I/O via opaque `NodeId` references, eliminating TOCTOU races. The
`compare_directories_node()` function uses `hash_file_node()` for content
hashing, which reads file content through the node-based stream API.
Downstream modules migrate gradually to these node-based functions.

**Sorting**: by name, size, modified date, type, status. Multi-column secondary sort.

**3-way merge**: a `DirComparison` can include a third "center" provider. Entry statuses include `Mergeable` and `Conflict` to indicate whether the two sides can be auto-merged against the center.

### 3.2 Text Comparison

Uses a line-based diff engine:

```rust
pub enum TextDifference {
    LineDifferent(LineInfo, LineInfo),
    LinesAdded(usize, Vec<LineInfo>),     // start line, added lines
    LinesRemoved(usize, Vec<LineInfo>),   // start line, removed lines
    LinesChanged(usize, Vec<LineInfo>, Vec<LineInfo>),
    BlankLine,
}

pub struct LineInfo {
    pub number: usize,
    pub content: String,
    pub tokens: Vec<Token>,  // for syntax-aware diffing
}
```

**Settings per session** (`TextCompareSettings`):

| Setting                      | Description                                                    |
| ---------------------------- | -------------------------------------------------------------- |
| `format`                     | File format (text, hex, grammar-based)                         |
| `encoding`                   | UTF-8, UTF-16, Latin-1, auto-detect                            |
| `line_ending`                | CRLF, LF, CR, ignore                                           |
| `ignore_case`                | Case-insensitive comparison                                    |
| `ignore_whitespace`          | Trim/ignore leading, trailing, or all whitespace               |
| `ignore_blank_lines`         | Skip blank lines                                               |
| `ignore_comments`            | Skip comment lines (grammar-dependent)                         |
| `ignore_numeric_changes`     | Treat numeric differences as equal within tolerance            |
| `ignore_regular_expressions` | List of regexes to ignore during comparison                    |
| `alignment_mode`             | `Line`, `Word`, or `Character` granularity                     |
| `importance_rules`           | Grammar rules that classify lines as code, data, comment, etc. |

**Importance classification** (drives "Next Difference" navigation):

- **Code** — high importance, always shown
- **Data** — medium importance
- **Comment** — low importance, can be hidden
- **Ignored** — not shown unless filters are suppressed

### 3.3 Text Merge (3-Way)

```rust
pub enum MergeResult {
    AutoMerged(Vec<TextDifference>),    // clean merge, list of accepted changes
    Conflict(TextConflict),             // manual resolution needed
}

pub struct TextConflict {
    pub base_lines: Range<usize>,
    pub left_lines: Vec<LineInfo>,
    pub right_lines: Vec<LineInfo>,
    pub resolved: Option<ResolvedConflict>,
}

pub enum ResolvedConflict {
    AcceptLeft,
    AcceptRight,
    AcceptBoth,
    EditManually(String),
}
```

### 3.4 Text Patch

Apply a unified diff patch file to a target:

```rust
pub enum PatchResult {
    Applied,
    HunkFailed { hunk: usize, reason: String },
    FuzzApplied { hunks_with_fuzz: usize },
}
```

### 3.5 Table Comparison

Parses structured data (CSV, TSV, JSON Lines, fixed-width, Excel via `calamine`):

```rust
pub enum TableDifference {
    RowAdded(Row),
    RowRemoved(Row),
    RowChanged(Row, Row),
    ColumnAdded(ColumnDef),
    ColumnRemoved(ColumnDef),
}

pub struct TableCompareSettings {
    pub key_columns: Vec<usize>,       // columns that identify a row
    pub comparison_columns: Vec<usize>, // columns to diff
    pub column_types: Vec<ColumnType>, // String, Integer, Float, Date
    pub ignore_case: bool,
    pub numeric_tolerance: f64,
    pub date_tolerance: Duration,
    pub match_mode: MatchMode,         // Exact, Fuzzy, Soundex
}

pub enum MatchMode {
    Exact,
    Fuzzy { threshold: f64 },
    Soundex,
}
```

**Multi-sheet support**: compare workbooks with multiple sheets, including sheet-level add/remove detection.

### 3.6 Hex Comparison

Byte-level comparison with configurable view:

```rust
pub struct HexCompareSettings {
    pub display_format: HexFormat,  // HexOnly, HexAscii, HexBinary, etc.
    pub byte_order: ByteOrder,      // LittleEndian, BigEndian
    pub comparison_unit: HexUnit,   // Byte, Word, DoubleWord, QuadWord
    pub ignore_offsets: Vec<Range<u64>>,
}
```

Highlights differences at byte granularity; shows ASCII interpretation alongside hex.

### 3.7 Picture Comparison

```rust
pub struct PixCompareSettings {
    pub comparison_mode: PixMode,
    pub blend_alpha: f32,           // 0.0..1.0 for overlay blend
    pub color_tolerance: u8,        // per-channel tolerance (0-255)
    pub size_tolerance: f32,        // percentage size difference to ignore
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

pub enum PixMode {
    SideBySide,
    Blend,              // alpha-blend overlay
    Difference,         // pixel-by-pixel difference image
    Ranges,             // per-channel range comparison
}
```

Uses the `image` crate to decode and compare pixel data. Supports JPEG, PNG, GIF, BMP, TIFF, WebP.

### 3.8 Media Comparison (Metadata)

Compare media files by extracting and comparing metadata (EXIF, IPTC, XMP, ID3):

```rust
pub struct MediaCompareSettings {
    pub importance_rules: Vec<MetadataRule>,  // which tags matter
}
```

### 3.9 Version Comparison (3-Way File Tree)

Compare three directory trees (typically: common ancestor, left branch, right branch). Reuses `DirComparison` with a `center` provider and `TextMerge` for file-level resolution.

---

## 4. Filtering System

### 4.1 Name Filters

Patterns applied to file/directory names before comparison:

```rust
pub enum NameFilter {
    Include(Pattern),   // show only matching
    Exclude(Pattern),   // hide matching
}

pub enum Pattern {
    Glob(String),           // *.txt, **/src/**
    Regex(Regex),           // full regex
    Name(String),           // exact match
}
```

Filters cascade: include filters are ANDed; exclude filters are ORed. Predefined filter sets can be saved.

### 4.2 Other Filters

Additional criteria beyond name:

```rust
pub struct OtherFilters {
    pub size_range: Option<Range<u64>>,
    pub date_range: Option<Range<DateTime>>,
    pub file_type: Option<FileTypeFilter>,  // files only, dirs only, symlinks, etc.
    pub status: Vec<DirEntryStatus>,         // show only differing, only orphans, etc.
    pub regular_expression: Option<Regex>,   // content-based filter
    pub ignore_hidden: bool,
    pub ignore_system: bool,
}
```

### 4.3 Display Filters

Toggle visibility of matched items without affecting comparison results:

```rust
pub struct DisplayFilters {
    pub show_same: bool,
    pub show_different: bool,
    pub show_orphans: bool,
    pub show_mergeable: bool,
    pub show_conflicts: bool,
    pub suppress_content_filters: bool,  // show ignored lines
}
```

### 4.4 Regular Expression Engine

Full PCRE-compatible regex support via the `regex` crate:

- Named capture groups for replacement rules
- Case-insensitive, multiline, dotall modes
- Used in: name filters, content filters, text replacements, grammar rules

---

## 5. Session System

### 5.1 Session Types

```rust
pub enum SessionType {
    Home,                    // dashboard / session browser
    DirCompare,              // 2-way folder compare
    DirMerge,                // 3-way folder merge
    DirSync,                 // folder synchronization
    TextCompare,             // 2-way text diff
    TextMerge,               // 3-way text merge
    TextEdit,                // single-file editor
    TextPatch,               // apply patch
    TableCompare,            // structured data comparison
    HexCompare,              // binary comparison
    PixCompare,              // image comparison
}
```

### 5.2 Session Object

The runtime [`Session`](cocomo-lib/src/session.rs) holds live provider
references and resolved node IDs. Paths are stored for display and
serialization; node IDs are used for I/O operations.

```rust
pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub session_type: SessionType,
    pub left_provider: Arc<dyn NodeFileSystem>,
    pub right_provider: Arc<dyn NodeFileSystem>,
    pub center_provider: Option<Arc<dyn NodeFileSystem>>,
    pub left_path: PathBuf,
    pub right_path: PathBuf,
    pub center_path: Option<PathBuf>,
    /// Resolved node IDs (valid while nodes exist in the provider cache).
    pub left_node_id: Option<NodeId<u64>>,
    pub right_node_id: Option<NodeId<u64>>,
    pub center_node_id: Option<NodeId<u64>>,
    pub settings: SessionSettings,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
}
```

### 5.3 Session Serialization

Sessions save to `.bcs` files (TOML format). The serializable form is
[`SessionConfig`](cocomo-lib/src/session.rs), which stores paths and
provider labels. On load, paths are resolved to node IDs via
`resolve_path()`.

```toml
name = "Project Alpha vs Beta"
type = "DirCompare"
[left]
  provider = "local"
  path = "/home/user/alpha"
[right]
  provider = "local"
  path = "/home/user/beta"
[settings]
  compare_files = true
  compare_structure = true
  name_filter = ["*.rs", "*.toml"]
  ignore_whitespace = true
```

### 5.4 Session Management

- Create, open, save, save-as sessions
- Session list / recent sessions
- Clone a session (swap paths, keep settings)
- Share sessions (export/import files)
- Auto-save session state on comparison completion

---

## 6. Synchronization

The `sync` module provides directory synchronization by combining the comparison
engine with the transfer executor. A [`SyncOperation`] defines the strategy,
and [`SyncRules`] configure the behavior.

### 6.1 Sync Operations

```rust
pub enum SyncOperation {
    MirrorLeft,        // make right match left
    MirrorRight,       // make left match right
    UpdateNewer,       // copy newer over older
    UpdateBoth,        // copy newer in both directions
    CopyLeft,          // copy left-only and different to right
    CopyRight,         // copy right-only and different to left
    CopyNewer,         // copy only newer files
    DeleteOrphans,     // delete files that exist on one side only
}
```

Mirror operations copy matching entries and delete orphans. Copy operations
transfer entries without deleting anything. Update operations compare
modification times and copy only newer files.

### 6.2 Sync Safety

| Feature       | Description                          |
| ------------- | ------------------------------------ |
| Dry run       | Preview operations without executing |
| Atomic writes | Write to temp file, then rename      |

Dry run mode is configured via [`SyncRules::dry_run`]. When enabled, the
sync engine plans all transfers but does not execute them. The returned
[`SyncResult`] contains the planned actions for review.

Atomic writes are handled by the [`WritableFileSystem`] implementation
(`LocalFs`), which writes to a temporary file and renames on success.

### 6.3 Sync Rules

Configuration for a synchronization operation:

```rust
pub struct SyncRules {
    pub operation: SyncOperation,
    pub dry_run: bool,
    pub max_depth: Option<usize>,
    pub compare_files: bool,
}
```

`max_depth` limits the recursive comparison depth. `compare_files` controls
whether file contents are compared or only size/metadata.

### 6.4 Sync API

Two entry points are provided:

- [`plan_sync`] — compares directories and plans transfers, returns a
  [`SyncResult`] without executing any I/O.
- [`sync_directories`] — plans and executes transfers (unless `dry_run`
  is `true`). Returns a [`SyncResult`] with execution results.

```rust
pub struct SyncResult {
    pub transfer: Option<TransferResult>,
    pub planned: Vec<TransferItem>,
    pub errors: Vec<String>,
}
```

---

## 7. Reports

Generate comparison reports in multiple formats:

```rust
pub enum ReportFormat {
    Html,
    Text,
    Csv,
    Json,
}

pub struct ReportConfig {
    pub format: ReportFormat,
    pub include_same: bool,
    pub include_different: bool,
    pub include_orphans: bool,
    pub include_subdirectories: bool,
    pub include_file_details: bool,   // size, date, hash
    pub include_diff_content: bool,   // inline text diff in report
    pub page_size: Option<usize>,     // max entries per page
}
```

---

## 8. Snapshots

Capture a point-in-time view of a directory tree.

Snapshots are stored as serializable TOML files (`.snap` extension). The
[`capture_snapshot`](cocomo-lib/src/snapshot.rs) function scans a directory
and records paths, sizes, modification times, and hashes. Content hashes
are computed on demand when comparing against a live filesystem.

```rust
/// A serializable identifier that references a provider. Resolved lazily
/// back to a live `NodeFileSystem` when the snapshot is loaded for comparison.
pub struct ProviderId {
    pub scheme: String,  // "file", "s3", "ftp", ...
    pub profile: Option<String>, // optional named profile
}

pub struct Snapshot {
    pub id: Uuid,
    pub provider_id: ProviderId,  // serializable; resolved on load
    pub path: PathBuf,
    pub entries: Vec<SnapshotEntry>,
    pub created_at: DateTime<Utc>,
    pub labels: Vec<String>,
}

pub struct SnapshotEntry {
    pub path: PathBuf,  // relative to snapshot root
    pub size: u64,
    pub modified: DateTime,
    pub hash: String,
    pub is_dir: bool,
}
```

Snapshots are compared against live filesystems or other snapshots, avoiding re-reading files. The `provider_id` is resolved back to a live `Arc<dyn FileSystem>` via the provider registry when a snapshot is loaded for comparison.

---

## 9. File Formats & Grammars

### 9.1 File Format Registry

```rust
pub struct FileFormat {
    pub id: String,
    pub name: String,
    pub extensions: Vec<String>,       // "*.rs", "*.toml"
    pub mime_types: Vec<String>,       // "text/x-rust"
    pub format_type: FormatType,       // Text, Table, Hex, Picture, External
    pub grammar: Option<Grammar>,      // syntax rules
    pub settings: FormatSettings,      // encoding, line ending, etc.
}

pub enum FormatType {
    Text,
    Table { parser: TableParser },
    Hex,
    Picture,
    External { command: String },     // external diff tool
}
```

### 9.2 Grammar System

Grammars define syntax-aware rules for text comparison:

```rust
pub struct Grammar {
    pub name: String,
    pub rules: Vec<GrammarRule>,
}

pub struct GrammarRule {
    pub name: String,            // "comment", "string", "keyword"
    pub pattern: Regex,          // regex to match the rule
    pub importance: Importance,  // Code, Data, Comment, Ignored
    pub replacement: Option<String>,  // normalize matching text
}

pub enum Importance {
    Code,
    Data,
    Comment,
    Ignored,
}
```

Grammars enable:

- **Syntax highlighting** in the UI
- **Smart diffing**: ignore comments, strings, whitespace-only changes
- **Text replacement**: normalize variations (e.g., trim trailing whitespace)
- **Line classification**: drive "Next Difference" to skip unimportant changes

### 9.3 Conversion Rules

Per-format conversion settings:

| Type    | Settings                                                                                                   |
| ------- | ---------------------------------------------------------------------------------------------------------- |
| Text    | Encoding, line ending, BOM handling, Unicode normalization                                                 |
| Table   | Delimiter, quote character, escape character, column types, date format, numeric format, regional settings |
| Hex     | Byte order, display format (hex, ASCII, binary), word size                                                 |
| Picture | Color space, resize mode, quality                                                                          |

---

## 10. Scripting & CLI

### 10.1 Script Language

A simple line-oriented scripting language (BCS — Beyond Compare Script):

```
# Example script: sync two directories
arg 1, left_path
arg 2, right_path

lb "Starting sync: ${left_path} -> ${right_path}"

set filetype text, encoding utf-8
set ignorecase yes
set trimtrailing yes

dir sync "${left_path}", "${right_path}"
    mirror left
    prompt no
    backup "/tmp/backup"
end

if errorlevel() > 0
    lb "ERROR: Sync failed"
    exit 1
end

lb "Sync complete"
```

**Built-in commands**:

| Command                    | Description                |
| -------------------------- | -------------------------- |
| `arg`                      | Access script arguments    |
| `lb`                       | Log / output message       |
| `set`                      | Set session options        |
| `dir compare`              | Run folder comparison      |
| `dir merge`                | Run 3-way folder merge     |
| `dir sync`                 | Run folder synchronization |
| `text compare`             | Compare text files         |
| `text merge`               | 3-way text merge           |
| `text patch`               | Apply a patch              |
| `table compare`            | Compare structured data    |
| `hex compare`              | Compare binary files       |
| `pix compare`              | Compare images             |
| `copy`                     | Copy files                 |
| `delete`                   | Delete files               |
| `rename`                   | Rename files               |
| `export`                   | Generate report            |
| `snapshot`                 | Create a snapshot          |
| `if / elif / else / endif` | Conditional branching      |
| `while / endwhile`         | Looping                    |
| `call`                     | Call another script        |
| `exit`                     | Terminate with exit code   |

**Variables**: `${variable}`, `${errorlevel()}`, `${date()}`, `${fileexists(path)}`

### 10.2 CLI Interface

```bash
# Compare two directories
cocomo dir compare /src/alpha /src/beta

# Sync with mirror
cocomo dir sync --mirror-left /src /dst

# Run a script
cocomo run sync.bcs /src /dst

# Text diff
cocomo text compare file1.rs file2.rs

# Generate HTML report
cocomo dir compare /a /b --report /tmp/report.html

# Apply a patch
cocomo text patch original.txt changes.patch
```

Exit codes: `0` = no differences / success, `1` = differences found (compare mode) or error, `2` = script error.

---

## 11. cocomo_tui — Terminal UI

### 11.1 Layout

```
┌─────────────────────────────────────────────────────────┐
│  [Sessions] [Compare] [Sync] [View] [Filter] [Help]    │  ← Menu bar
├──────────────┬──────────────────────┬───────────────────┤
│  Left:/src   │  Right:/dst          │  Status panel     │
│              │                      │                   │
│  [=] file.rs │  [=] file.rs        │  Filter: *.rs     │
│  [>] mod.rs  │  [ ] mod.rs         │  Status: 42 diff  │
│  [<] util.rs │  [=] util.rs        │  Same: 15        │
│  ...         │  ...                 │  Left-only: 3    │
│              │                      │  Right-only: 24   │
├──────────────┴──────────────────────┴───────────────────┤
│  >  [L/R] [Copy] [Delete] [Sync] [Open Diff] [Next]    │  ← Action bar
├─────────────────────────────────────────────────────────┤
│  file.rs  │  Same  │  1.2KB  │  2024-01-15 14:32:01   │  ← Detail row
└─────────────────────────────────────────────────────────┘
```

### 11.2 Text Diff View (Modal)

```
┌─────────────────────────────────────────────────────────┐
│  file.rs: Left vs Right              [ESC] close        │
├──────────────────────┬──────────────────────────────────┤
│  7 │- fn old_name()  │  9 │+ fn new_name()             │
│  8 │  let x = 42;    │ 10 │  let x = 42;               │
│  9 │-  return x;     │ 11 │+  x                         │
│ 10 │+  x             │ 12 │ }                           │
│ 11 │ }               │                                  │
└──────────────────────┴──────────────────────────────────┘
   [n]ext diff  [p]rev  [c]opy left  [v]copy right  [e]dit
```

### 11.3 Keybindings

| Key           | Action                                  |
| ------------- | --------------------------------------- |
| `j` / `k`     | Navigate down / up                      |
| `l`           | Open file diff / descend into directory |
| `h`           | Go back / ascend                        |
| `n` / `N`     | Next / previous difference              |
| `C-l` / `C-r` | Copy left → right / right → left        |
| `D-l` / `D-r` | Delete from left / right                |
| `S`           | Run sync (with confirmation modal)      |
| `f`           | Toggle filter panel                     |
| `/`           | Search in files                         |
| `:`           | Enter command mode (run script command) |
| `q`           | Quit session / app                      |
| `ESC`         | Close modal / panel                     |
| `Tab`         | Switch between panes                    |

### 11.4 Modal Panels

- **Filter panel**: regex input, name filter toggle, status filter checkboxes
- **Settings modal**: per-session settings editor
- **Sync preview**: list of pending operations with approve/reject per item
- **Report modal**: choose format, destination, options
- **Session browser**: list saved sessions, create new

### 11.5 Terminal Features

- Fullscreen mode (alt-screen)
- Mouse support (click to select, drag to multi-select)
- Paste paths from clipboard (OSC 52 or `paste` crate)
- Theme support (256 color / true color)

---

## 12. cocomo_gui — Desktop GUI

### 12.1 Layout (gpui)

```
┌─────────────────────────────────────────────────────────────┐
│  File  Edit  View  Session  Tools  Help                     │  ← Menu bar
│  ┌─────────┐  ┌──────────────────────┐  ┌────────────────┐ │
│  │ Toolbar │  │  Tab bar: [file.rs]  │  │ Toolbar Right  │ │
│  │ [icons] │  │           [+ mod.rs] │  │ [icons]        │ │
│  ├─────────┤  ├──────────────────────┤  ├────────────────┤ │
│  │         │  │                      │  │                │ │
│  │  Left   │  │   Content / Diff     │  │    Right       │ │
│  │ /src    │  │   view               │  │   /dst         │ │
│  │         │  │                      │  │                │ │
│  │ [=]     │  │                      │  │ [=]            │ │
│  │ file.rs │  │                      │  │ file.rs        │ │
│  │ [>]     │  │                      │  │ [ ]            │ │
│  │ mod.rs  │  │                      │  │ mod.rs         │ │
│  │         │  │                      │  │                │ │
│  ├─────────┤  ├──────────────────────┤  ├────────────────┤ │
│  │ Detail  │  │  Status bar          │  │ Detail         │ │
│  │ panel   │  │  42 diff, 15 same    │  │ panel          │ │
│  └─────────┘  └──────────────────────┘  └────────────────┘ │
│  ┌──────────────────────────────────────────────────────────┐│
│  │ Output / Script log                                      ││
│  └──────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
```

### 12.2 View Modes

| Mode                | Layout                                                    |
| ------------------- | --------------------------------------------------------- |
| **Folder Compare**  | 2-panel directory tree + detail bar                       |
| **Folder Merge**    | 3-panel directory tree (left, center, right)              |
| **Folder Sync**     | 2-panel with sync queue overlay                           |
| **Text Compare**    | 2-pane text editor with diff highlights                   |
| **Text Merge**      | 3-pane text editor with conflict markers                  |
| **Text Edit**       | Single-pane syntax-highlighted editor                     |
| **Text Patch**      | Side-by-side: patch file + target file                    |
| **Table Compare**   | 2-grid data viewer with key column alignment              |
| **Hex Compare**     | 2-pane hex dump with byte-level highlights                |
| **Picture Compare** | Image viewer with side-by-side / blend / difference modes |

### 12.3 GUI-Specific Features

- Drag-and-drop files/folders onto window to create sessions
- Copy-paste file paths from clipboard
- Context menus on right-click
- Toolbar with icons for all common operations
- Resizable panes with draggable splitters
- Session tabs (multiple open comparisons)
- Home screen with recent sessions, quick compare buttons
- Keyboard shortcuts (configurable)
- High DPI / retina support
- Dark / light theme toggle
- Fullscreen mode

### 12.4 gpui Integration

- Use gpui's `View`/`Model` architecture for reactive UI
- Diff highlighting via custom `Element` with colored spans
- Syntax highlighting via grammar-driven token streams
- Virtual scrolling for large file lists and long files
- Image rendering for picture compare via gpui image support

---

## 13. Settings System

### 13.1 Program Options

```rust
pub struct ProgramOptions {
    // Startup
    pub start_in: StartIn,          // Last session, home view, specific path
    pub restore_sessions: bool,

    // Interface
    pub theme: Theme,               // Dark, Light, System
    pub font_family: String,
    pub font_size: f32,
    pub colors: ColorScheme,

    // Tabs
    pub tab_position: TabPosition,  // Top, Bottom
    pub show_tab_close: bool,

    // Text editing
    pub tab_width: usize,
    pub show_whitespace: bool,
    pub show_line_numbers: bool,
    pub word_wrap: bool,
    pub auto_indent: bool,

    // Next difference
    pub skip_blank_lines: bool,
    pub skip_comments: bool,

    // Backups
    pub backup_enabled: bool,
    pub backup_path: Option<PathBuf>,
    pub backup_on_overwrite: bool,

    // File operations
    pub copy_to_clipboard_format: ClipboardFormat,
    pub delete_to_trash: bool,

    // Archive types
    pub archive_associations: Vec<ArchiveType>,

    // Commands
    pub external_commands: BTreeMap<String, ExternalCommand>,

    // Open with
    pub open_with_apps: Vec<ExternalApp>,
}
```

### 13.2 Per-Session Settings

Each session type has its own settings struct (see Sections 3.2, 3.5, 3.7). Settings are:

- Per-session (saved with the session file)
- Per-type defaults (saved in global options)
- Factory defaults (compiled in)

Settings merge: session > type default > factory default.

### 13.3 Storage Locations

| Platform | Path                                    |
| -------- | --------------------------------------- |
| Linux    | `~/.config/cocomo/`                     |
| macOS    | `~/Library/Application Support/cocomo/` |
| Windows  | `%APPDATA%\cocomo\`                     |

---

## 14. Additional Features

### 14.1 Clipboard Compare

Capture text from clipboard and compare with a file or another clipboard capture:

```rust
pub struct ClipboardSession {
    pub left: ClipboardCapture,
    pub right: ClipboardCapture,
}

pub struct ClipboardCapture {
    pub content: String,
    pub captured_at: DateTime,
    pub source_label: String,
}
```

### 14.2 File Masks

Glob patterns that classify files into comparison types:

```rust
pub struct FileMask {
    pub pattern: String,       // "*.jpg", "*.png"
    pub format: FormatType,    // Picture, Text, etc.
    pub priority: u8,          // higher overrides lower
}
```

### 14.3 Display Columns (Folder View)

Configurable columns:

| Column        | Data                          |
| ------------- | ----------------------------- |
| Status        | Difference indicator icon     |
| Name          | File/folder name              |
| Size          | File size (formatted)         |
| Date modified | Last modification time        |
| Date created  | Creation time                 |
| Type          | File extension / MIME type    |
| Line count    | Number of lines (text files)  |
| Hash          | Content hash                  |
| Favorite      | User-assigned favorite marker |

### 14.4 Display Filters (Folder View)

Toggle buttons to show/hide:

- Same files
- Different files
- Left-only (orphan) files
- Right-only (orphan) files
- Mergeable files (3-way)
- Conflict files (3-way)
- Archive contents (expand/collapse)

### 14.5 Alignment Details (Table Compare)

For key-matched tables, show alignment information:

```rust
pub struct AlignmentInfo {
    pub left_key: String,
    pub right_key: String,
    pub match_score: f64,       // for fuzzy matches
    pub unmatched_fields: Vec<String>,
}
```

### 14.6 Context Menus

| Context          | Actions                                                                                           |
| ---------------- | ------------------------------------------------------------------------------------------------- |
| Folder view item | Compare, Sync, Copy, Delete, Rename, New Folder, Open With, Show in Explorer, Copy Path, Favorite |
| Text diff line   | Copy Line, Take Left, Take Right, Take Both, Accept to Here, Accept All                           |
| Table cell       | Copy Cell, Copy Column, Sort by Column, Filter by Value                                           |
| Hex byte         | Copy Hex, Copy ASCII, Go to Offset                                                                |
| Image view       | Zoom In/Out/Fit/Actual, Flip, Rotate, Show Difference                                             |

### 14.7 Session Sharing

- Export session as `.bcs` file
- Import session from `.bcs` file
- Share via URL (for cloud-backed sessions, future)

---

## 15. Non-Functional Requirements

### 15.1 Performance

| Metric                       | Target                                             |
| ---------------------------- | -------------------------------------------------- |
| Directory scan (10K files)   | < 2 seconds (hash-based, parallel)                 |
| Concurrent open file handles | Bounded by a semaphore (configurable, default 256) |
| Text diff (100K lines)       | < 500 ms                                           |
| Hex compare (10 MB)          | < 1 second                                         |
| Image compare (4K JPEG)      | < 500 ms                                           |
| TUI render refresh           | < 16 ms (60 FPS)                                   |
| GUI frame render             | < 16 ms (60 FPS)                                   |
| Memory per session           | < 50 MB for typical use                            |

Directory scans use a semaphore to limit the number of concurrently open file handles, preventing exhaustion of the OS file descriptor limit.

### 15.2 Reliability

- No data loss during sync: write-to-temp + rename pattern
- Crash-safe: session state auto-saved before destructive operations
- Graceful error handling: network failures, permission errors, locked files
- Transaction log for sync operations

### 15.3 Portability

- Runs on Linux, macOS, Windows
- Single binary distribution (TUI + CLI)
- Separate binary for GUI (requires display server)
- Cross-compile via `cargo xbuild`

### 15.4 Security

- Secrets encrypted in OS keyring
- No telemetry by default
- Sandboxed script execution (no arbitrary code execution)
- File path validation to prevent path traversal in archives
- Symlinks are not followed outside the scan root by default on the local filesystem. Following symlinks is an explicit opt-in via provider configuration.

### 15.5 Accessibility

- TUI: keyboard-only operation, screen reader friendly (braille display compatible)
- GUI: keyboard navigation, high contrast mode, font scaling, focus indicators

---

## 16. Out of Scope (v1)

The following Beyond Compare features are deferred to later releases:

| Feature                                | Reason                                              |
| -------------------------------------- | --------------------------------------------------- |
| Registry Compare                       | Windows-specific                                    |
| Source Control Integration             | Complex plugin system; use external tools           |
| Dropbox / OneDrive native profiles     | Use FTP/WebDAV/S3 equivalents; SDKs are restrictive |
| Touch UI                               | Desktop-focused                                     |
| Explorer / Finder integration          | Requires native shell extensions                    |
| Admin policies / Group Policy          | Enterprise feature                                  |
| Clipboard Compare (GUI)                | Platform-dependent clipboard API                    |
| Media Compare (ID3/EXIF deep metadata) | Low priority; basic metadata is enough              |

---

## 17. Milestones

| Phase  | Deliverable                                                                  |
| ------ | ---------------------------------------------------------------------------- |
| **M1** | `cocomo_lib`: `FileSystem` trait, `LocalFs`, content hashing, directory scan |
| **M2** | `cocomo_lib`: Text diff engine, `TextCompareSettings`, grammar system        |
| **M3** | `cocomo_tui`: Folder compare view, navigation, basic filtering               |
| **M4** | `cocomo_lib`: `FtpFs`, `S3Fs`, profile management                            |
| **M5** | `cocomo_tui`: Text diff modal, sync operations, session save/load            |
| **M6** | `cocomo_gui`: gpui scaffold, folder compare view                             |
| **M7** | `cocomo_gui`: Text diff, hex compare, picture compare views                  |
| **M8** | `cocomo_cli`: Script engine, CLI commands, report generation                 |
| **M9** | Polish: themes, keyboard shortcuts, performance tuning, documentation        |
