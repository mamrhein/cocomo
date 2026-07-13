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
//! # Exit codes
//!
//! - `0` — success, no differences found
//! - `1` — differences found (comparison commands only)
//! - `2` — error occurred

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand, ValueEnum};
use cocomo_lib::{
    DirEntry, LocalFs, TextDifference,
    compare::{
        CompareConfig, DirComparison, DirEntryStatus, compare_directories_node,
    },
    error::{FsError, FsOperation, wrap},
    grammar::Grammar,
    snapshot::{
        ProviderId, Snapshot, SnapshotEntry, SnapshotEntryStatus,
        capture_snapshot,
    },
    sync::{SyncOperation, SyncRules, plan_sync, sync_directories},
    text::{TextCompareSettings, TextDiff, WhitespaceMode, compare_texts},
};

// ---------------------------------------------------------------------------
// CLI definitions
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "cocomo")]
#[command(version = "0.0.1")]
#[command(about = "Compare, copy & move directories and files.")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Directory comparison and synchronization.
    Dir(DirArgs),
    /// File text comparison.
    Text(TextArgs),
    /// Point-in-time directory snapshots.
    Snapshot(SnapshotArgs),
}

#[derive(clap::Args)]
struct DirArgs {
    #[command(subcommand)]
    command: DirCommand,
}

#[derive(Subcommand)]
enum DirCommand {
    /// Compare two directory trees.
    Compare(DirCompareArgs),
    /// Synchronize two directory trees.
    Sync(DirSyncArgs),
}

#[derive(clap::Args)]
struct TextArgs {
    #[command(subcommand)]
    command: TextCommand,
}

#[derive(Subcommand)]
enum TextCommand {
    /// Compare two text files and show differences.
    Compare(TextCompareArgs),
    /// Compare two text files in unified diff format.
    Diff(TextDiffArgs),
}

#[derive(clap::Args)]
struct SnapshotArgs {
    #[command(subcommand)]
    command: SnapshotCommand,
}

#[derive(Subcommand)]
enum SnapshotCommand {
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
struct DirCompareArgs {
    /// Left directory path.
    left: PathBuf,
    /// Right directory path.
    right: PathBuf,
    /// Compare directory structure only (skip content hashing).
    #[arg(long)]
    structure_only: bool,
    /// Output format.
    #[arg(long, default_value = "text")]
    format: OutputFormat,
    /// Show only entries that differ between left and right.
    #[arg(long)]
    show_different: bool,
    /// Show only orphan entries (left-only or right-only).
    #[arg(long)]
    show_orphans: bool,
}

#[derive(Parser)]
struct DirSyncArgs {
    /// Left directory path.
    left: PathBuf,
    /// Right directory path.
    right: PathBuf,
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
}

#[derive(Parser)]
struct TextCompareArgs {
    /// Left file path.
    left: PathBuf,
    /// Right file path.
    right: PathBuf,
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
}

#[derive(Parser)]
struct TextDiffArgs {
    /// Left file path.
    left: PathBuf,
    /// Right file path.
    right: PathBuf,
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
}

#[derive(Parser)]
struct SnapshotCaptureArgs {
    /// Directory path to snapshot.
    path: PathBuf,
    /// Output file path (default: <dirname>.snap in current directory).
    output: Option<PathBuf>,
}

#[derive(Parser)]
struct SnapshotListArgs {
    /// Directory containing .snap files (default: current directory).
    #[arg(default_value = ".")]
    directory: PathBuf,
}

#[derive(Parser)]
struct SnapshotDiffArgs {
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

// ---------------------------------------------------------------------------
// Main dispatch
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(&cli.command).await {
        Ok(DiffResult::NoDiffs) => ExitCode::SUCCESS,
        Ok(DiffResult::HasDiffs) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

/// Result of a command that may or may not find differences.
enum DiffResult {
    NoDiffs,
    HasDiffs,
}

async fn run(command: &Commands) -> Result<DiffResult, FsError> {
    match command {
        Commands::Dir(dir_args) => run_dir(&dir_args.command).await,
        Commands::Text(text_args) => run_text(&text_args.command).await,
        Commands::Snapshot(snap_args) => {
            run_snapshot(&snap_args.command).await
        }
    }
}

// ---------------------------------------------------------------------------
// Directory commands
// ---------------------------------------------------------------------------

async fn run_dir(cmd: &DirCommand) -> Result<DiffResult, FsError> {
    match cmd {
        DirCommand::Compare(args) => dir_compare(args).await,
        DirCommand::Sync(args) => dir_sync(args).await,
    }
}

async fn dir_compare(args: &DirCompareArgs) -> Result<DiffResult, FsError> {
    let fs = LocalFs::new("local");
    let config = if args.structure_only {
        CompareConfig::structure_only()
    } else {
        CompareConfig::full()
    };

    let comparison =
        compare_directories_node(&fs, &args.left, &args.right, &config, None)
            .await?;

    match args.format {
        OutputFormat::Text => print_comparison_text(&comparison, args),
        OutputFormat::Csv => print_comparison_csv(&comparison, args),
        OutputFormat::Json => print_comparison_json(&comparison, args),
    }

    // Print summary.
    print_summary(&comparison);

    if comparison.different_count() > 0 || comparison.orphan_count() > 0 {
        Ok(DiffResult::HasDiffs)
    } else {
        Ok(DiffResult::NoDiffs)
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

    if entries.is_empty() {
        println!("Directories are identical.");
        return;
    }

    // Print header.
    println!(
        "{:<5} {:<40} {:>12} {:>12}",
        "Stat", "Name", "Left Size", "Right Size"
    );
    println!(
        "{:<5} {:<40} {:>12} {:>12}",
        "----",
        "----------------------------------------",
        "------------",
        "------------"
    );

    for entry in &entries {
        if !should_show_entry(entry, args) {
            continue;
        }

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
            "{:<5} {:<40} {:>12} {:>12}",
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

async fn dir_sync(args: &DirSyncArgs) -> Result<DiffResult, FsError> {
    let fs = LocalFs::new("local");

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

    let result = if args.dry_run {
        plan_sync(&fs, &args.left, &args.right, &rules).await?
    } else {
        sync_directories(&fs, &args.left, &args.right, &rules).await?
    };

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
    }

    Ok(DiffResult::HasDiffs)
}

// ---------------------------------------------------------------------------
// Text commands
// ---------------------------------------------------------------------------

async fn run_text(cmd: &TextCommand) -> Result<DiffResult, FsError> {
    match cmd {
        TextCommand::Compare(args) => text_compare(args).await,
        TextCommand::Diff(args) => text_diff(args).await,
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

async fn read_file_content(path: &PathBuf) -> Result<String, FsError> {
    tokio::fs::read_to_string(path)
        .await
        .map_err(|e| wrap(e, FsOperation::Read, path.clone()))
}

async fn text_compare(args: &TextCompareArgs) -> Result<DiffResult, FsError> {
    let left_content = read_file_content(&args.left).await?;
    let right_content = read_file_content(&args.right).await?;

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
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

async fn text_diff(args: &TextDiffArgs) -> Result<DiffResult, FsError> {
    let left_content = read_file_content(&args.left).await?;
    let right_content = read_file_content(&args.right).await?;

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
        &args.left,
        &args.right,
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

async fn run_snapshot(cmd: &SnapshotCommand) -> Result<DiffResult, FsError> {
    match cmd {
        SnapshotCommand::Capture(args) => snapshot_capture(args).await,
        SnapshotCommand::List(args) => snapshot_list(args).await,
        SnapshotCommand::Diff(args) => snapshot_diff(args).await,
    }
}

async fn snapshot_capture(
    args: &SnapshotCaptureArgs,
) -> Result<DiffResult, FsError> {
    let fs: std::sync::Arc<dyn cocomo_lib::FileSystem> =
        std::sync::Arc::new(LocalFs::new("local"));
    let provider_id = ProviderId::local();

    let snapshot = capture_snapshot(&fs, provider_id, &args.path).await?;

    let output_path = args.output.clone().unwrap_or_else(|| {
        let dir_name = args
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        PathBuf::from(format!("{}.snap", dir_name))
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
) -> Result<DiffResult, FsError> {
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
) -> Result<DiffResult, FsError> {
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
