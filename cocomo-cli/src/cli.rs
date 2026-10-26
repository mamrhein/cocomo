// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use cocomo_lib::{
    grammar::Grammar, report::ReportFormat, text::WhitespaceMode,
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
    #[command(visible_alias = "d")]
    Dir(DirArgs),
    /// File text comparison.
    #[command(visible_alias = "t")]
    Text(TextArgs),
    /// Point-in-time directory snapshots.
    #[command(visible_alias = "s")]
    Snapshot(SnapshotArgs),
}

#[derive(clap::Args)]
pub struct DirArgs {
    #[command(subcommand)]
    pub(crate) command: DirCommand,
}

#[derive(Subcommand)]
pub(crate) enum DirCommand {
    /// Compare two directory trees.
    #[command(visible_aliases = ["c", "cmp"])]
    Compare(DirCompareArgs),
    /// Synchronize two directory trees.
    #[command(visible_aliases = ["s", "syn"])]
    Sync(DirSyncArgs),
}

#[derive(clap::Args)]
pub struct TextArgs {
    #[command(subcommand)]
    pub(crate) command: TextCommand,
}

#[derive(Subcommand)]
pub(crate) enum TextCommand {
    /// Compare two text files and show differences.
    #[command(visible_aliases = ["c", "cmp"])]
    Compare(TextCompareArgs),
    /// Compare two text files in unified diff format.
    #[command(visible_aliases = ["d", "df"])]
    Diff(TextDiffArgs),
}

#[derive(clap::Args)]
pub struct SnapshotArgs {
    #[command(subcommand)]
    pub(crate) command: SnapshotCommand,
}

#[derive(Subcommand)]
pub(crate) enum SnapshotCommand {
    /// Capture a snapshot of a directory tree.
    #[command(visible_aliases = ["c", "cap"])]
    Capture(SnapshotCaptureArgs),
    /// List saved snapshots in a directory.
    #[command(visible_aliases = ["l", "ls"])]
    List(SnapshotListArgs),
    /// Compare two snapshot files.
    #[command(visible_aliases = ["d", "df"])]
    Diff(SnapshotDiffArgs),
}

// ---------------------------------------------------------------------------
// Argument structs
// ---------------------------------------------------------------------------

#[derive(Parser)]
pub struct DirCompareArgs {
    /// Left directory path or URL (e.g. `ftp://host/pub/src`).
    pub(crate) left: String,
    /// Right directory path or URL (e.g. `ftp://host/pub/src`).
    pub(crate) right: String,
    /// Compare file contents (activate content hashing).
    #[arg(short = 'c', long)]
    pub(crate) content: bool,
    /// Maximum scan depth (default: 1). The value must be assigned with
    /// `=` (e.g. `-m=2`, `--max-depth=2`); pass the flag without a value
    /// to scan without depth limit.
    #[arg(
        short = 'm',
        long,
        num_args = 0..=1,
        default_value = "1",
        require_equals = true
    )]
    pub(crate) max_depth: Option<usize>,
    /// Output format for console display.
    #[arg(short = 'f', long, default_value = "text")]
    pub(crate) format: OutputFormat,
    /// Show only entries that differ between left and right.
    #[arg(short = 'd', long)]
    pub(crate) show_different: bool,
    /// Show only orphan entries (left-only or right-only).
    #[arg(short = 'o', long)]
    pub(crate) show_orphans: bool,
    /// Write a report file in the specified format.
    #[arg(short = 'r', long)]
    pub(crate) report: Option<PathBuf>,
    /// Format for the report file (used with --report).
    #[arg(long, default_value = "text")]
    pub(crate) report_format: ReportFormatArg,
    /// Control ANSI color output.
    #[arg(long, default_value = "auto")]
    pub(crate) color: ColorMode,
    /// Hide modification timestamp columns in text output.
    #[arg(long)]
    pub(crate) no_mtime: bool,
    /// Suppress the summary line(s) at the end of output.
    #[arg(long)]
    pub(crate) no_summary: bool,
    /// Summary output mode: brief (default) or full per-status breakdown.
    #[arg(long, default_value = "brief")]
    pub(crate) summary: SummaryMode,
    /// Sort entries by the specified field (default: scan order).
    #[arg(short = 's', long)]
    pub(crate) sort: Option<SortField>,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(short = 'P', long)]
    pub(crate) profile: Option<String>,
    /// Exclude hidden files and directories (names starting with `.`).
    #[arg(short = 'x', long = "excl-hidden")]
    pub(crate) excl_hidden: bool,
    /// Show only directories in the output.
    #[arg(short = 'D', long = "dirs-only")]
    pub(crate) dirs_only: bool,
    /// Show only entries whose name matches this glob pattern (e.g.
    /// `*.txt`, `config*`).
    #[arg(short = 'F', long, value_name = "expr")]
    pub(crate) filter: Option<String>,
}

#[derive(Parser)]
pub struct DirSyncArgs {
    /// Left directory path or URL (e.g. `ftp://host/pub/mirror`).
    pub(crate) left: String,
    /// Right directory path or URL (e.g. `ftp://host/pub/mirror`).
    pub(crate) right: String,
    /// Make right match left (copy left-only/different to right, delete
    /// right-only).
    #[arg(long, default_value_t = false)]
    pub(crate) mirror_left: bool,
    /// Make left match right (copy right-only/different to left, delete
    /// left-only).
    #[arg(long, default_value_t = false)]
    pub(crate) mirror_right: bool,
    /// Update newer files only.
    #[arg(long, default_value_t = false)]
    pub(crate) update_newer: bool,
    /// Update newer files in both directions.
    #[arg(long, default_value_t = false)]
    pub(crate) update_both: bool,
    /// Copy left-only files to right (no deletions).
    #[arg(long, default_value_t = false)]
    pub(crate) copy_left: bool,
    /// Copy right-only files to left (no deletions).
    #[arg(long, default_value_t = false)]
    pub(crate) copy_right: bool,
    /// Copy only newer files.
    #[arg(long, default_value_t = false)]
    pub(crate) copy_newer: bool,
    /// Delete orphan files that exist on only one side.
    #[arg(short = 'd', long, default_value_t = false)]
    pub(crate) delete_orphans: bool,
    /// Plan transfers without executing them.
    #[arg(short = 'n', long, default_value_t = false)]
    pub(crate) dry_run: bool,
    /// Compare file contents. If absent, uses size/mtime only.
    #[arg(short = 'c', long, default_value_t = true)]
    pub(crate) compare_files: bool,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(short = 'P', long)]
    pub(crate) profile: Option<String>,
}

#[derive(Parser)]
pub struct TextCompareArgs {
    /// Left file path or URL (e.g. `ftp://host/pub/file.txt`).
    pub(crate) left: String,
    /// Right file path or URL (e.g. `ftp://host/pub/file.txt`).
    pub(crate) right: String,
    /// Ignore case when comparing lines.
    #[arg(short = 'i', long, default_value_t = false)]
    pub(crate) ignore_case: bool,
    /// Ignore whitespace differences. Accepts: sensitive, trim, insensitive.
    #[arg(long, default_value = "sensitive")]
    pub(crate) ignore_whitespace: WhitespaceArg,
    /// Skip blank lines during comparison.
    #[arg(long, default_value_t = false)]
    pub(crate) ignore_blank_lines: bool,
    /// Skip comment lines. Requires a grammar.
    #[arg(long, default_value_t = false)]
    pub(crate) ignore_comments: bool,
    /// Grammar for syntax-aware classification.
    #[arg(short = 'g', long)]
    pub(crate) grammar: Option<GrammarArg>,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(short = 'P', long)]
    pub(crate) profile: Option<String>,
}

#[derive(Parser)]
pub struct TextDiffArgs {
    /// Left file path or URL (e.g. `ftp://host/pub/file.txt`).
    pub(crate) left: String,
    /// Right file path or URL (e.g. `ftp://host/pub/file.txt`).
    pub(crate) right: String,
    /// Ignore case when comparing lines.
    #[arg(short = 'i', long, default_value_t = false)]
    pub(crate) ignore_case: bool,
    /// Ignore whitespace differences. Accepts: sensitive, trim, insensitive.
    #[arg(long, default_value = "sensitive")]
    pub(crate) ignore_whitespace: WhitespaceArg,
    /// Skip blank lines during comparison.
    #[arg(long, default_value_t = false)]
    pub(crate) ignore_blank_lines: bool,
    /// Skip comment lines. Requires a grammar.
    #[arg(long, default_value_t = false)]
    pub(crate) ignore_comments: bool,
    /// Grammar for syntax-aware classification.
    #[arg(short = 'g', long)]
    pub(crate) grammar: Option<GrammarArg>,
    /// Profile id to authenticate the addressed provider(s) with.
    #[arg(short = 'P', long)]
    pub(crate) profile: Option<String>,
}

#[derive(Parser)]
pub struct SnapshotCaptureArgs {
    /// Directory path or URL to snapshot.
    pub(crate) path: String,
    /// Output file path (default: <dirname>.snap in current directory).
    pub(crate) output: Option<PathBuf>,
    /// Profile id to authenticate the addressed provider with.
    #[arg(short = 'P', long)]
    pub(crate) profile: Option<String>,
}

#[derive(Parser)]
pub struct SnapshotListArgs {
    /// Directory containing .snap files (default: current directory).
    #[arg(default_value = ".")]
    pub(crate) directory: PathBuf,
}

#[derive(Parser)]
pub struct SnapshotDiffArgs {
    /// First snapshot file.
    pub(crate) left: PathBuf,
    /// Second snapshot file.
    pub(crate) right: PathBuf,
}

// ---------------------------------------------------------------------------
// Enum arguments
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, ValueEnum)]
pub(crate) enum OutputFormat {
    Text,
    Csv,
    Json,
}

/// Color output mode for terminal display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum ColorMode {
    /// Enable colors only when stdout is a TTY.
    Auto,
    /// Always emit ANSI color codes.
    Always,
    /// Never emit ANSI color codes.
    Never,
}

/// Summary output mode for directory comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SummaryMode {
    /// Single-line summary with aggregate counts (total, same, different,
    /// orphans).
    Brief,
    /// Per-status breakdown table with counts and percentages for every
    /// status.
    Full,
}

/// Sort field for directory comparison entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SortField {
    /// Sort by entry name (alphabetical).
    Name,
    /// Sort by file size (largest first).
    Size,
    /// Sort by modification time (newest first).
    Modified,
    /// Sort by status display name.
    Status,
}

#[derive(Debug, Clone, ValueEnum)]
pub(crate) enum WhitespaceArg {
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
pub(crate) enum GrammarArg {
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
pub(crate) enum ReportFormatArg {
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
// Test helpers
// ---------------------------------------------------------------------------

/// Build a `DirCompareArgs` with all flags set to their defaults, suitable
/// for unit tests that need an args instance.
#[cfg(test)]
impl DirCompareArgs {
    pub(crate) fn test_defaults() -> Self {
        Self {
            left: "./left".to_string(),
            right: "./right".to_string(),
            content: false,
            max_depth: Some(1),
            format: OutputFormat::Text,
            show_different: false,
            show_orphans: false,
            report: None,
            report_format: ReportFormatArg::Text,
            color: ColorMode::Never,
            no_mtime: false,
            no_summary: false,
            summary: SummaryMode::Brief,
            sort: None,
            profile: None,
            excl_hidden: false,
            dirs_only: false,
            filter: None,
        }
    }
}
