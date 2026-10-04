// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! COCOMO CLI — command-line interface for directory and file comparison,
//! synchronization, and snapshot management.
//!
//! The command logic is generic over an [`EndpointResolver`], which maps a
//! CLI endpoint argument (path or URL) to a filesystem instance. The binary
//! uses [`ProductionResolver`] to resolve endpoints to live [`Provider`]
//! instances; tests inject a mock resolver backed by `cocomo_lib::MockFs`
//! to exercise error handling without touching the real filesystem.
//!
//! # Exit codes
//!
//! - `0` — success, no differences found
//! - `1` — differences found (comparison commands only)
//! - `2` — error occurred

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::{Parser, Subcommand, ValueEnum};
use cocomo_lib::{
    DirEntry, FileSystem, ProviderId, TextDifference, WritableFileSystem,
    compare::{
        CompareConfig, DirComparison, DirEntryStatus,
        compare_directories_node, compare_directories_pair_node,
    },
    error::{FsError, FsOperation, wrap},
    grammar::Grammar,
    profile::{ProfileError, ProfileStore},
    provider::{Provider, ProviderError},
    report::{ReportConfig, ReportFormat, generate_report},
    secrets::{Secrets, TtyPrompter},
    snapshot::{
        Snapshot, SnapshotEntry, SnapshotEntryStatus, capture_snapshot_node,
    },
    sync::{
        SyncOperation, SyncRules, plan_sync, plan_sync_pair, sync_directories,
        sync_directories_pair,
    },
    text::{TextCompareSettings, TextDiff, WhitespaceMode, compare_texts},
    transfer::FsPair,
    url::{Url, UrlError},
};

// ---------------------------------------------------------------------------
// CLI definitions
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "cocomo")]
#[command(version = "0.0.1")]
#[command(about = "Compare, copy & move directories and files.")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Directory comparison and synchronization.
    Dir(DirArgs),
    /// File text comparison.
    Text(TextArgs),
    /// Point-in-time directory snapshots.
    Snapshot(SnapshotArgs),
}

#[derive(clap::Args)]
pub struct DirArgs {
    #[command(subcommand)]
    command: DirCommand,
}

#[derive(Subcommand)]
pub enum DirCommand {
    /// Compare two directory trees.
    Compare(DirCompareArgs),
    /// Synchronize two directory trees.
    Sync(DirSyncArgs),
}

#[derive(clap::Args)]
pub struct TextArgs {
    #[command(subcommand)]
    command: TextCommand,
}

#[derive(Subcommand)]
pub enum TextCommand {
    /// Compare two text files and show differences.
    Compare(TextCompareArgs),
    /// Compare two text files in unified diff format.
    Diff(TextDiffArgs),
}

#[derive(clap::Args)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    command: SnapshotCommand,
}

#[derive(Subcommand)]
pub enum SnapshotCommand {
    /// Capture a snapshot of a directory tree.
    Capture(SnapshotCaptureArgs),
    /// List saved snapshots in a directory.
    List(SnapshotListArgs),
    /// Compare two snapshot files.
    Diff(SnapshotDiffArgs),
}

// ---------------------------------------------------------------------------
// Argument structs
// ---------------------------------------------------------------------------

#[derive(Parser)]
pub struct DirCompareArgs {
    /// Left directory path or URL (e.g. `ftp://host/pub/src`).
    left: String,
    /// Right directory path or URL (e.g. `ftp://host/pub/src`).
    right: String,
    /// Compare directory structure only (skip content hashing).
    #[arg(long)]
    structure_only: bool,
    /// Output format for console display.
    #[arg(long, default_value = "text")]
    format: OutputFormat,
    /// Show only entries that differ between left and right.
    #[arg(long)]
    show_different: bool,
    /// Show only orphan entries (left-only or right-only).
    #[arg(long)]
    show_orphans: bool,
    /// Write a report file in the specified format.
    #[arg(long)]
    report: Option<PathBuf>,
    /// Format for the report file (used with --report).
    #[arg(long, default_value = "text")]
    report_format: ReportFormatArg,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(long)]
    profile: Option<String>,
}

#[derive(Parser)]
pub struct DirSyncArgs {
    /// Left directory path or URL (e.g. `ftp://host/pub/mirror`).
    left: String,
    /// Right directory path or URL (e.g. `ftp://host/pub/mirror`).
    right: String,
    /// Make right match left (copy left-only/different to right, delete
    /// right-only).
    #[arg(long, default_value_t = false)]
    mirror_left: bool,
    /// Make left match right (copy right-only/different to left, delete
    /// left-only).
    #[arg(long, default_value_t = false)]
    mirror_right: bool,
    /// Update newer files only.
    #[arg(long, default_value_t = false)]
    update_newer: bool,
    /// Update newer files in both directions.
    #[arg(long, default_value_t = false)]
    update_both: bool,
    /// Copy left-only files to right (no deletions).
    #[arg(long, default_value_t = false)]
    copy_left: bool,
    /// Copy right-only files to left (no deletions).
    #[arg(long, default_value_t = false)]
    copy_right: bool,
    /// Copy only newer files.
    #[arg(long, default_value_t = false)]
    copy_newer: bool,
    /// Delete orphan files that exist on only one side.
    #[arg(long, default_value_t = false)]
    delete_orphans: bool,
    /// Plan transfers without executing them.
    #[arg(long, default_value_t = false)]
    dry_run: bool,
    /// Compare file contents. If absent, uses size/mtime only.
    #[arg(long, default_value_t = true)]
    compare_files: bool,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(long)]
    profile: Option<String>,
}

#[derive(Parser)]
pub struct TextCompareArgs {
    /// Left file path or URL (e.g. `ftp://host/pub/file.txt`).
    left: String,
    /// Right file path or URL (e.g. `ftp://host/pub/file.txt`).
    right: String,
    /// Ignore case when comparing lines.
    #[arg(long, default_value_t = false)]
    ignore_case: bool,
    /// Ignore whitespace differences. Accepts: sensitive, trim, insensitive.
    #[arg(long, default_value = "sensitive")]
    ignore_whitespace: WhitespaceArg,
    /// Skip blank lines during comparison.
    #[arg(long, default_value_t = false)]
    ignore_blank_lines: bool,
    /// Skip comment lines. Requires a grammar.
    #[arg(long, default_value_t = false)]
    ignore_comments: bool,
    /// Grammar for syntax-aware classification.
    #[arg(long)]
    grammar: Option<GrammarArg>,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(long)]
    profile: Option<String>,
}

#[derive(Parser)]
pub struct TextDiffArgs {
    /// Left file path or URL (e.g. `ftp://host/pub/file.txt`).
    left: String,
    /// Right file path or URL (e.g. `ftp://host/pub/file.txt`).
    right: String,
    /// Ignore case when comparing lines.
    #[arg(long, default_value_t = false)]
    ignore_case: bool,
    /// Ignore whitespace differences. Accepts: sensitive, trim, insensitive.
    #[arg(long, default_value = "sensitive")]
    ignore_whitespace: WhitespaceArg,
    /// Skip blank lines during comparison.
    #[arg(long, default_value_t = false)]
    ignore_blank_lines: bool,
    /// Skip comment lines. Requires a grammar.
    #[arg(long, default_value_t = false)]
    ignore_comments: bool,
    /// Grammar for syntax-aware classification.
    #[arg(long)]
    grammar: Option<GrammarArg>,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(long)]
    profile: Option<String>,
}

#[derive(Parser)]
pub struct SnapshotCaptureArgs {
    /// Directory path or URL to snapshot.
    path: String,
    /// Output file path (default: <dirname>.snap in current directory).
    output: Option<PathBuf>,
    /// Profile id to authenticate the addressed provider with.
    #[arg(long)]
    profile: Option<String>,
}

#[derive(Parser)]
pub struct SnapshotListArgs {
    /// Directory containing .snap files (default: current directory).
    #[arg(default_value = ".")]
    directory: PathBuf,
}

#[derive(Parser)]
pub struct SnapshotDiffArgs {
    /// First snapshot file.
    left: PathBuf,
    /// Second snapshot file.
    right: PathBuf,
}

// ---------------------------------------------------------------------------
// Enum arguments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Text,
    Csv,
    Json,
}

#[derive(Debug, Clone, ValueEnum)]
enum WhitespaceArg {
    Sensitive,
    Trim,
    Insensitive,
}

impl From<WhitespaceArg> for WhitespaceMode {
    fn from(arg: WhitespaceArg) -> Self {
        match arg {
            WhitespaceArg::Sensitive => WhitespaceMode::Sensitive,
            WhitespaceArg::Trim => WhitespaceMode::Trim,
            WhitespaceArg::Insensitive => WhitespaceMode::Insensitive,
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum GrammarArg {
    Rust,
    Python,
    C,
    PlainText,
}

impl From<GrammarArg> for Grammar {
    fn from(arg: GrammarArg) -> Self {
        match arg {
            GrammarArg::Rust => Grammar::rust(),
            GrammarArg::Python => Grammar::python(),
            GrammarArg::C => Grammar::c(),
            GrammarArg::PlainText => Grammar::plain_text(),
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
enum ReportFormatArg {
    Text,
    Csv,
    Json,
}

impl From<ReportFormatArg> for ReportFormat {
    fn from(arg: ReportFormatArg) -> Self {
        match arg {
            ReportFormatArg::Text => ReportFormat::Text,
            ReportFormatArg::Csv => ReportFormat::Csv,
            ReportFormatArg::Json => ReportFormat::Json,
        }
    }
}

// ---------------------------------------------------------------------------
// Endpoint resolution
// ---------------------------------------------------------------------------

/// Result of a command that may or may not find differences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffResult {
    NoDiffs,
    HasDiffs,
}

/// Errors that can occur while resolving an endpoint or reading from a
/// resolved provider.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Fs(#[from] FsError),
    #[error(transparent)]
    Url(#[from] UrlError),
    #[error(transparent)]
    Provider(#[from] ProviderError),
    #[error(transparent)]
    Profile(#[from] ProfileError),
    /// Non-fatal filesystem errors collected while walking the trees. The
    /// result may be incomplete; each error is printed to stderr before
    /// this variant is returned.
    #[error("operation incomplete due to filesystem errors")]
    FsErrors(Vec<FsError>),
}

/// Convert a filesystem error into a [`CliError`], printing the individual
/// errors of an incomplete operation to stderr before returning them.
fn fs_error_to_cli(e: FsError) -> CliError {
    match e {
        FsError::Incomplete { errors } => {
            for err in &errors {
                eprintln!("error: {err}");
            }
            CliError::FsErrors(errors)
        }
        other => CliError::Fs(other),
    }
}

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
fn endpoint_identity(url: &Url) -> (String, Option<String>, Option<u16>) {
    (url.scheme.clone(), url.host.clone(), url.effective_port())
}

/// Resolve the provider that services `url`, letting `Provider::resolve`
/// consult the default profile store, the environment, and the keychain.
/// The store is only opened for remote endpoints, so local-only runs
/// neither read nor create the profile configuration files.
fn resolve_provider(
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
fn resolve_endpoint<R: EndpointResolver>(
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
enum EndpointPair<N> {
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
fn resolve_endpoint_pair<R: EndpointResolver>(
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

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

/// Run the parsed `command` against the filesystems provided by `resolver`.
pub async fn run<R: EndpointResolver>(
    command: &Commands,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    match command {
        Commands::Dir(dir_args) => run_dir(&dir_args.command, resolver).await,
        Commands::Text(text_args) => {
            run_text(&text_args.command, resolver).await
        }
        Commands::Snapshot(snap_args) => {
            run_snapshot(&snap_args.command, resolver).await
        }
    }
}

// ---------------------------------------------------------------------------
// Directory commands
// ---------------------------------------------------------------------------

async fn run_dir<R: EndpointResolver>(
    cmd: &DirCommand,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    match cmd {
        DirCommand::Compare(args) => dir_compare(args, resolver).await,
        DirCommand::Sync(args) => dir_sync(args, resolver).await,
    }
}

async fn dir_compare<R: EndpointResolver>(
    args: &DirCompareArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (pair, left, right) = resolve_endpoint_pair(
        resolver,
        &args.left,
        &args.right,
        args.profile.as_deref(),
    )?;
    let config = if args.structure_only {
        CompareConfig::structure_only()
    } else {
        CompareConfig::full()
    };

    let comparison = match pair {
        EndpointPair::Shared(fs) => {
            compare_directories_node(&*fs, &left, &right, &config, None)
                .await?
        }
        EndpointPair::Separate(left_fs, right_fs) => {
            compare_directories_pair_node(
                &*left_fs, &*right_fs, &left, &right, &config, None,
            )
            .await?
        }
    };

    match args.format {
        OutputFormat::Text => print_comparison_text(&comparison, args),
        OutputFormat::Csv => print_comparison_csv(&comparison, args),
        OutputFormat::Json => print_comparison_json(&comparison, args),
    }

    // Print summary.
    print_summary(&comparison);

    // Write report file if requested.
    if let Some(ref report_path) = args.report {
        let report_format: ReportFormat = args.report_format.clone().into();
        let report_config = build_report_config(args);
        let report =
            generate_report(&comparison, report_format, &report_config);
        tokio::fs::write(report_path, &report)
            .await
            .map_err(|e| wrap(e, FsOperation::Write, report_path.clone()))?;
        println!("Report written to {}.", report_path.display());
    }

    // Surface non-fatal scan errors instead of reporting a possibly
    // incomplete comparison as authoritative.
    if !comparison.errors.is_empty() {
        for err in &comparison.errors {
            eprintln!("error: {err}");
        }
        return Err(CliError::FsErrors(comparison.errors));
    }

    if comparison.different_count() > 0 || comparison.orphan_count() > 0 {
        Ok(DiffResult::HasDiffs)
    } else {
        Ok(DiffResult::NoDiffs)
    }
}

fn build_report_config(args: &DirCompareArgs) -> ReportConfig {
    // Derive report config from the CLI filter flags.
    let (include_same, include_different, include_orphans) =
        if args.show_different && args.show_orphans {
            (true, true, true)
        } else if args.show_different {
            (false, true, false)
        } else if args.show_orphans {
            (false, false, true)
        } else {
            (true, true, true)
        };

    ReportConfig {
        include_same,
        include_different,
        include_orphans,
        include_subdirectories: true,
        include_file_details: true,
    }
}

fn should_show_entry(entry: &DirEntry, args: &DirCompareArgs) -> bool {
    if args.show_different && args.show_orphans {
        return true;
    }

    if args.show_different {
        matches!(
            entry.status,
            DirEntryStatus::Different | DirEntryStatus::Similar
        )
    } else if args.show_orphans {
        matches!(
            entry.status,
            DirEntryStatus::LeftOnly | DirEntryStatus::RightOnly
        )
    } else {
        true
    }
}

fn print_comparison_text(comparison: &DirComparison, args: &DirCompareArgs) {
    let mut entries = Vec::new();
    collect_entries(&comparison.entries, &mut entries);

    // Filter entries according to display flags.
    let visible: Vec<&DirEntry> = entries
        .iter()
        .filter(|e| should_show_entry(e, args))
        .collect();

    if visible.is_empty() {
        if entries.is_empty() {
            println!("Directories are identical.");
        } else {
            println!("No matching entries.");
        }
        return;
    }

    // Print header.
    println!(
        "{:5} {:<40} {:>12} {:>12}",
        "Stat", "Name", "Left Size", "Right Size"
    );
    println!(
        "{:5} {:<40} {:>12} {:>12}",
        "----",
        "----------------------------------------",
        "------------",
        "------------"
    );

    for entry in &visible {
        let status_sym = entry.status.symbol();
        let name = &entry.name;
        let left_size = entry
            .left
            .as_ref()
            .map(|l| format_size(l.size))
            .unwrap_or("-".to_string());
        let right_size = entry
            .right
            .as_ref()
            .map(|r| format_size(r.size))
            .unwrap_or("-".to_string());

        println!(
            "{:5} {:<40} {:>12} {:>12}",
            status_sym, name, left_size, right_size
        );
    }
}

fn collect_entries(entries: &[DirEntry], out: &mut Vec<DirEntry>) {
    for entry in entries {
        out.push(entry.clone());
        if let Some(ref sub) = entry.sub_entries {
            collect_entries(&sub.entries, out);
        }
    }
}

fn print_comparison_csv(comparison: &DirComparison, args: &DirCompareArgs) {
    let mut entries = Vec::new();
    collect_entries(&comparison.entries, &mut entries);

    println!("status,name,left_size,right_size,left_modified,right_modified");
    for entry in &entries {
        if !should_show_entry(entry, args) {
            continue;
        }

        let left_size = entry
            .left
            .as_ref()
            .map(|l| l.size.to_string())
            .unwrap_or_default();
        let right_size = entry
            .right
            .as_ref()
            .map(|r| r.size.to_string())
            .unwrap_or_default();
        let left_modified = entry
            .left
            .as_ref()
            .map(|l| l.modified.clone())
            .unwrap_or_default();
        let right_modified = entry
            .right
            .as_ref()
            .map(|r| r.modified.clone())
            .unwrap_or_default();

        // Escape fields that may contain commas.
        let name = escape_csv_field(&entry.name);
        println!(
            "{},{},{},{},{},{}",
            entry.status,
            name,
            left_size,
            right_size,
            left_modified,
            right_modified
        );
    }
}

fn escape_csv_field(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn print_comparison_json(comparison: &DirComparison, args: &DirCompareArgs) {
    let mut entries = Vec::new();
    collect_entries(&comparison.entries, &mut entries);

    let json_entries: Vec<serde_json::Value> = entries
        .iter()
        .filter(|e| should_show_entry(e, args))
        .map(|entry| {
            let mut obj = serde_json::Map::new();
            obj.insert(
                "status".into(),
                serde_json::Value::String(entry.status.to_string()),
            );
            obj.insert("name".into(), serde_json::json!(entry.name));
            obj.insert(
                "is_dir".into(),
                serde_json::json!(
                    entry.left.as_ref().map(|l| l.is_dir).unwrap_or(false)
                ),
            );

            if let Some(ref left) = entry.left {
                let mut lo = serde_json::Map::new();
                lo.insert("size".into(), serde_json::json!(left.size));
                lo.insert("modified".into(), serde_json::json!(left.modified));
                lo.insert("path".into(), serde_json::json!(left.path));
                if let Some(ref h) = left.hash {
                    lo.insert("hash".into(), serde_json::json!(h));
                }
                obj.insert("left".into(), serde_json::Value::Object(lo));
            } else {
                obj.insert("left".into(), serde_json::Value::Null);
            }

            if let Some(ref right) = entry.right {
                let mut ro = serde_json::Map::new();
                ro.insert("size".into(), serde_json::json!(right.size));
                ro.insert(
                    "modified".into(),
                    serde_json::json!(right.modified),
                );
                ro.insert("path".into(), serde_json::json!(right.path));
                if let Some(ref h) = right.hash {
                    ro.insert("hash".into(), serde_json::json!(h));
                }
                obj.insert("right".into(), serde_json::Value::Object(ro));
            } else {
                obj.insert("right".into(), serde_json::Value::Null);
            }

            serde_json::Value::Object(obj)
        })
        .collect();

    let summary = serde_json::json!({
        "total": comparison.total(),
        "same": comparison.same_count(),
        "different": comparison.different_count(),
        "orphans": comparison.orphan_count(),
    });

    let output = serde_json::json!({
        "summary": summary,
        "entries": json_entries,
    });

    println!(
        "{}",
        serde_json::to_string_pretty(&output).unwrap_or_default()
    );
}

fn print_summary(comparison: &DirComparison) {
    println!();
    println!(
        "Summary: {} total, {} same, {} different, {} orphans",
        comparison.total(),
        comparison.same_count(),
        comparison.different_count(),
        comparison.orphan_count(),
    );
}

fn format_size(size: u64) -> String {
    if size >= 1_073_741_824 {
        format!("{:.1}G", size as f64 / 1_073_741_824.0)
    } else if size >= 1_048_576 {
        format!("{:.1}M", size as f64 / 1_048_576.0)
    } else if size >= 1024 {
        format!("{:.1}K", size as f64 / 1024.0)
    } else {
        format!("{}B", size)
    }
}

async fn dir_sync<R: EndpointResolver>(
    args: &DirSyncArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (pair, left, right) = resolve_endpoint_pair(
        resolver,
        &args.left,
        &args.right,
        args.profile.as_deref(),
    )?;

    // Determine sync operation from flags. Default to MirrorLeft.
    let operation = if args.mirror_left {
        SyncOperation::MirrorLeft
    } else if args.mirror_right {
        SyncOperation::MirrorRight
    } else if args.update_newer {
        SyncOperation::UpdateNewer
    } else if args.update_both {
        SyncOperation::UpdateBoth
    } else if args.copy_left {
        SyncOperation::CopyLeft
    } else if args.copy_right {
        SyncOperation::CopyRight
    } else if args.copy_newer {
        SyncOperation::CopyNewer
    } else if args.delete_orphans {
        SyncOperation::DeleteOrphans
    } else {
        SyncOperation::MirrorLeft
    };

    let rules = SyncRules {
        operation,
        dry_run: args.dry_run,
        max_depth: None,
        compare_files: args.compare_files,
    };

    let result = match pair {
        EndpointPair::Shared(fs) => {
            if args.dry_run {
                plan_sync(&*fs, &left, &right, &rules)
                    .await
                    .map_err(fs_error_to_cli)?
            } else {
                sync_directories(&*fs, &left, &right, &rules)
                    .await
                    .map_err(fs_error_to_cli)?
            }
        }
        EndpointPair::Separate(left_fs, right_fs) => {
            let pair = FsPair::Separate(&*left_fs, &*right_fs);
            if args.dry_run {
                plan_sync_pair(pair, &left, &right, &rules)
                    .await
                    .map_err(fs_error_to_cli)?
            } else {
                sync_directories_pair(pair, &left, &right, &rules)
                    .await
                    .map_err(fs_error_to_cli)?
            }
        }
    };

    // Surface non-fatal scan errors before reporting the plan, which may
    // be incomplete.
    if !result.errors.is_empty() {
        for err in &result.errors {
            eprintln!("error: {err}");
        }
        return Err(CliError::FsErrors(result.errors));
    }

    // Print planned items.
    if result.planned.is_empty() {
        println!("Directories are already in sync. No actions needed.");
        return Ok(DiffResult::NoDiffs);
    }

    let mode = if args.dry_run { "[DRY RUN] " } else { "" };

    println!(
        "{}Planned {} transfers ({}):",
        mode,
        result.planned.len(),
        operation.label(),
    );

    for item in &result.planned {
        println!("  {} {}", item.action.label(), item.rel_path);
    }

    // Print transfer results if executed.
    if let Some(ref transfer) = result.transfer {
        println!(
            "\nTransfer complete: {} succeeded, {} failed",
            transfer.succeeded, transfer.failed,
        );
        // A failed transfer aborts the operation: print each error and
        // surface them as a hard failure (exit code 2).
        if transfer.failed > 0 {
            for err in &transfer.errors {
                eprintln!("error: {err}");
            }
            return Err(CliError::FsErrors(transfer.errors.clone()));
        }
    }

    Ok(DiffResult::HasDiffs)
}

// ---------------------------------------------------------------------------
// Text commands
// ---------------------------------------------------------------------------

async fn run_text<R: EndpointResolver>(
    cmd: &TextCommand,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    match cmd {
        TextCommand::Compare(args) => text_compare(args, resolver).await,
        TextCommand::Diff(args) => text_diff(args, resolver).await,
    }
}

fn build_text_settings(
    ignore_case: bool,
    whitespace: WhitespaceArg,
    ignore_blank_lines: bool,
    ignore_comments: bool,
    grammar: Option<GrammarArg>,
) -> TextCompareSettings {
    let mut settings = TextCompareSettings::new();
    settings.ignore_case = ignore_case;
    settings.whitespace_mode = whitespace.clone().into();
    settings.ignore_blank_lines = ignore_blank_lines;
    settings.ignore_comments = ignore_comments;
    if let Some(g) = grammar {
        settings.grammar = Some(g.into());
    }
    settings
}

async fn text_compare<R: EndpointResolver>(
    args: &TextCompareArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (left_fs, left_path) =
        resolve_endpoint(resolver, &args.left, args.profile.as_deref())?;
    let (right_fs, right_path) =
        resolve_endpoint(resolver, &args.right, args.profile.as_deref())?;
    let left_bytes = left_fs.read(&left_path, None).await?;
    let right_bytes = right_fs.read(&right_path, None).await?;
    let left_content = String::from_utf8_lossy(&left_bytes);
    let right_content = String::from_utf8_lossy(&right_bytes);

    let settings = build_text_settings(
        args.ignore_case,
        args.ignore_whitespace.clone(),
        args.ignore_blank_lines,
        args.ignore_comments,
        args.grammar.clone(),
    );

    let diff = compare_texts(&left_content, &right_content, &settings);

    if diff.is_empty() {
        println!("Files are identical.");
        return Ok(DiffResult::NoDiffs);
    }

    print_text_diff_table(&diff);

    println!(
        "\n{} differences found ({} lines left, {} lines right)",
        diff.differences.len(),
        diff.left_line_count,
        diff.right_line_count,
    );

    Ok(DiffResult::HasDiffs)
}

fn print_text_diff_table(diff: &TextDiff) {
    println!(
        "{:<6} {:<6}   {:<6} {:<6}   Content",
        "L-No", "R-No", "L-No", "R-No"
    );
    println!(
        "{:<6} {:<6}   {:<6} {:<6}   -------",
        "----", "----", "----", "----"
    );

    for difference in &diff.differences {
        match difference {
            TextDifference::LineDifferent(left, right) => {
                println!(
                    "{:<6} {:<6}   {:<6} {:<6}   | {} | {}",
                    left.number,
                    right.number,
                    "-",
                    "-",
                    truncate(&left.content, 60),
                    truncate(&right.content, 60),
                );
            }
            TextDifference::LinesAdded(_start, lines) => {
                for line in lines {
                    println!(
                        "{:<6} {:<6}   {:<6} {:<6}   |     | {}",
                        "-",
                        line.number,
                        "-",
                        "-",
                        truncate(&line.content, 60),
                    );
                }
            }
            TextDifference::LinesRemoved(_start, lines) => {
                for line in lines {
                    println!(
                        "{:<6} {:<6}   {:<6} {:<6}   | {} |     |",
                        line.number,
                        "-",
                        "-",
                        "-",
                        truncate(&line.content, 60),
                    );
                }
            }
            TextDifference::LinesChanged(_start, removed, added) => {
                for line in removed {
                    println!(
                        "{:<6} {:<6}   {:<6} {:<6}   | {} |     |",
                        line.number,
                        "-",
                        "-",
                        "-",
                        truncate(&line.content, 60),
                    );
                }
                for line in added {
                    println!(
                        "{:<6} {:<6}   {:<6} {:<6}   |     | {}",
                        "-",
                        line.number,
                        "-",
                        "-",
                        truncate(&line.content, 60),
                    );
                }
            }
            TextDifference::BlankLine => {
                println!("      {:<6}   {:<6} {:<6}   (blank)", "-", "-", "-");
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    // The cut point may fall inside a multi-byte character, so snap down to
    // the nearest char boundary before slicing.
    let end = s.floor_char_boundary(max.saturating_sub(3));
    format!("{}...", &s[..end])
}

async fn text_diff<R: EndpointResolver>(
    args: &TextDiffArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (left_fs, left_path) =
        resolve_endpoint(resolver, &args.left, args.profile.as_deref())?;
    let (right_fs, right_path) =
        resolve_endpoint(resolver, &args.right, args.profile.as_deref())?;
    let left_bytes = left_fs.read(&left_path, None).await?;
    let right_bytes = right_fs.read(&right_path, None).await?;
    let left_content = String::from_utf8_lossy(&left_bytes);
    let right_content = String::from_utf8_lossy(&right_bytes);

    let settings = build_text_settings(
        args.ignore_case,
        args.ignore_whitespace.clone(),
        args.ignore_blank_lines,
        args.ignore_comments,
        args.grammar.clone(),
    );

    let diff = compare_texts(&left_content, &right_content, &settings);

    if diff.is_empty() {
        return Ok(DiffResult::NoDiffs);
    }

    // Generate unified diff from the TextDiff result.
    let unified = generate_unified_diff(
        Path::new(&args.left),
        Path::new(&args.right),
        &left_content,
        &right_content,
        &diff,
    );

    print!("{}", unified);

    Ok(DiffResult::HasDiffs)
}

fn generate_unified_diff(
    left_path: &Path,
    right_path: &Path,
    left_content: &str,
    right_content: &str,
    diff: &TextDiff,
) -> String {
    use std::fmt::Write;

    let left_lines: Vec<&str> = left_content.lines().collect();
    let right_lines: Vec<&str> = right_content.lines().collect();

    let mut output = String::new();
    writeln!(
        output,
        "--- {}\n+++ {}",
        left_path.display(),
        right_path.display(),
    )
    .ok();

    // Walk the differences and emit hunks. We need context lines around
    // each change group.
    let context_lines = 3;

    // Build a mapping of which left/right lines are part of changes.
    let mut changed_left: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    let mut changed_right: std::collections::HashSet<usize> =
        std::collections::HashSet::new();

    for difference in &diff.differences {
        match difference {
            TextDifference::LineDifferent(left, right) => {
                changed_left.insert(left.number - 1);
                changed_right.insert(right.number - 1);
            }
            TextDifference::LinesAdded(_start, lines) => {
                for line in lines {
                    changed_right.insert(line.number - 1);
                }
            }
            TextDifference::LinesRemoved(_start, lines) => {
                for line in lines {
                    changed_left.insert(line.number - 1);
                }
            }
            TextDifference::LinesChanged(_start, removed, added) => {
                for line in removed {
                    changed_left.insert(line.number - 1);
                }
                for line in added {
                    changed_right.insert(line.number - 1);
                }
            }
            TextDifference::BlankLine => {
                // Blank lines between changes are context.
            }
        }
    }

    // Group into hunks based on left-side changes.
    let mut sorted_left: Vec<usize> = changed_left.iter().copied().collect();
    sorted_left.sort();

    let mut hunks: Vec<(usize, usize)> = Vec::new();
    let mut current_start: Option<usize> = None;
    let mut current_end: Option<usize> = None;

    for idx in sorted_left {
        let ctx_start = idx.saturating_sub(context_lines);
        let ctx_end = idx + context_lines;

        match (&mut current_start, &mut current_end) {
            (Some(s), Some(e)) if ctx_start <= *e => {
                *e = ctx_end.min(left_lines.len());
            }
            _ => {
                if let (Some(s), Some(e)) =
                    (current_start.take(), current_end.take())
                {
                    hunks.push((s, e));
                }
                current_start = Some(ctx_start);
                current_end = Some(ctx_end.min(left_lines.len()));
            }
        }
    }

    if let (Some(s), Some(e)) = (current_start, current_end) {
        hunks.push((s, e));
    }

    // If no left-side changes (only additions), emit a single hunk at
    // the end.
    if hunks.is_empty() && !changed_right.is_empty() {
        hunks.push((left_lines.len(), left_lines.len()));
    }

    for (hunk_start, hunk_end) in &hunks {
        let left_start = *hunk_start;
        let left_count = *hunk_end - *hunk_start + (context_lines * 2);

        // Approximate the right range.
        let mut right_start = *hunk_start;
        let mut right_count = *hunk_end - *hunk_start + (context_lines * 2);

        // Adjust for pure additions at the end.
        if changed_left.is_empty() {
            right_start = right_lines.len().saturating_sub(context_lines);
            right_count = context_lines + changed_right.len();
        }

        writeln!(output, "@@ -{},+{} @@", left_start + 1, right_start + 1,)
            .ok();

        // Emit context and changed lines from the original text.
        for (i, line) in left_lines.iter().enumerate() {
            if i >= left_count {
                break;
            }
            if changed_left.contains(&i) {
                writeln!(output, "-{}", line).ok();
            } else if i >= hunk_start.saturating_sub(context_lines)
                && i < hunk_end + context_lines
            {
                writeln!(output, " {}", line).ok();
            }
        }

        for (i, line) in right_lines.iter().enumerate() {
            if changed_right.contains(&i) {
                writeln!(output, "+{}", line).ok();
            } else if i >= right_start.saturating_sub(context_lines)
                && i < right_start + right_count
            {
                // Only emit context that wasn't already emitted as left
                // context.
                if !changed_left.contains(&i) {
                    // Avoid duplicate context lines.
                }
            }
        }
    }

    output
}

// ---------------------------------------------------------------------------
// Snapshot commands
// ---------------------------------------------------------------------------

async fn run_snapshot<R: EndpointResolver>(
    cmd: &SnapshotCommand,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    match cmd {
        SnapshotCommand::Capture(args) => {
            snapshot_capture(args, resolver).await
        }
        SnapshotCommand::List(args) => snapshot_list(args).await,
        SnapshotCommand::Diff(args) => snapshot_diff(args).await,
    }
}

async fn snapshot_capture<R: EndpointResolver>(
    args: &SnapshotCaptureArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (fs, in_path) =
        resolve_endpoint(resolver, &args.path, args.profile.as_deref())?;
    let snapshot =
        capture_snapshot_node(&*fs, resolver.provider_id(&*fs), &in_path)
            .await
            .map_err(fs_error_to_cli)?;

    let output_path = args.output.clone().unwrap_or_else(|| {
        let dir_name = in_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        PathBuf::from(format!("{dir_name}.snap"))
    });

    snapshot.save_to_file(&output_path).await?;

    println!(
        "Snapshot captured: {} entries -> {}",
        snapshot.entry_count(),
        output_path.display(),
    );

    Ok(DiffResult::NoDiffs)
}

async fn snapshot_list(
    args: &SnapshotListArgs,
) -> Result<DiffResult, CliError> {
    let entries = tokio::fs::read_dir(&args.directory)
        .await
        .map_err(|e| wrap(e, FsOperation::ReadDir, args.directory.clone()))?;

    let mut snapshots = Vec::new();
    let mut reader = entries;
    while let Ok(Some(entry)) = reader.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("snap")
            && let Ok(snap) = Snapshot::load_from_file(&path).await
        {
            snapshots.push((path, snap));
        }
    }

    if snapshots.is_empty() {
        println!("No snapshots found in {}.", args.directory.display());
        return Ok(DiffResult::NoDiffs);
    }

    snapshots.sort_by(|a, b| a.0.file_name().cmp(&b.0.file_name()));

    println!("{:<40} {:>12}  {:>20}", "File", "Entries", "Created");
    println!(
        "{:<40} {:>12}  {:>20}",
        "----------------------------------------",
        "------------",
        "--------------------",
    );

    for (path, snap) in &snapshots {
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let created = snap.created_at.format("%Y-%m-%d %H:%M:%S");
        println!("{:<40} {:>12}  {}", file_name, snap.entry_count(), created,);
    }

    Ok(DiffResult::NoDiffs)
}

async fn snapshot_diff(
    args: &SnapshotDiffArgs,
) -> Result<DiffResult, CliError> {
    let left_snap = Snapshot::load_from_file(&args.left).await?;
    let right_snap = Snapshot::load_from_file(&args.right).await?;

    // Build path-indexed maps for both snapshots.
    let left_map: std::collections::HashMap<&PathBuf, &SnapshotEntry> =
        left_snap.entries.iter().map(|e| (&e.path, e)).collect();
    let right_map: std::collections::HashMap<&PathBuf, &SnapshotEntry> =
        right_snap.entries.iter().map(|e| (&e.path, e)).collect();

    let mut all_paths: std::collections::BTreeSet<&PathBuf> =
        std::collections::BTreeSet::new();
    for path in left_map.keys() {
        all_paths.insert(path);
    }
    for path in right_map.keys() {
        all_paths.insert(path);
    }

    let mut modified = 0;
    let mut added = 0;
    let mut deleted = 0;

    println!(
        "{:<6} {:<40} {:>12} {:>12}",
        "Stat", "Path", "Left Size", "Right Size"
    );
    println!(
        "{:<6} {:<40} {:>12} {:>12}",
        "----",
        "----------------------------------------",
        "------------",
        "------------"
    );

    for path in &all_paths {
        let (left_entry, right_entry) =
            (left_map.get(path), right_map.get(path));

        let (status, _status_str) = match (left_entry, right_entry) {
            (Some(l), Some(r)) => {
                if l.size != r.size || l.modified != r.modified {
                    modified += 1;
                    ("M", SnapshotEntryStatus::Modified)
                } else {
                    continue; // unchanged
                }
            }
            (None, Some(_)) => {
                added += 1;
                ("A", SnapshotEntryStatus::Added)
            }
            (Some(_), None) => {
                deleted += 1;
                ("D", SnapshotEntryStatus::Deleted)
            }
            _ => unreachable!(),
        };

        let left_size = left_entry
            .map(|e| format_size(e.size))
            .unwrap_or("-".to_string());
        let right_size = right_entry
            .map(|e| format_size(e.size))
            .unwrap_or("-".to_string());

        println!(
            "{:<6} {:<40} {:>12} {:>12}",
            status,
            path.display(),
            left_size,
            right_size
        );
    }

    let total_changes = modified + added + deleted;
    if total_changes == 0 {
        println!("\nSnapshots are identical.");
        return Ok(DiffResult::NoDiffs);
    }

    println!(
        "\nSnapshot diff: {} modified, {} added, {} deleted ({} total)",
        modified, added, deleted, total_changes,
    );

    Ok(DiffResult::HasDiffs)
}

#[cfg(test)]
mod tests {
    use super::truncate;

    #[test]
    fn short_string_is_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn long_string_is_truncated_with_ellipsis() {
        assert_eq!(truncate("abcdefghij", 8), "abcde...");
    }

    #[test]
    fn cut_point_inside_multibyte_char_does_not_panic() {
        // '✅' occupies bytes 56..59, so a naive cut at byte 57 would panic.
        let line = format!("{}✅tail", "x".repeat(56));
        assert_eq!(truncate(&line, 60), format!("{}...", "x".repeat(56)));
    }

    #[test]
    fn max_smaller_than_ellipsis_returns_only_ellipsis() {
        assert_eq!(truncate("abcdef", 2), "...");
    }
}
