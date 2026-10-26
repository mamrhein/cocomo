// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Directory comparison and synchronization command logic.

mod display;
mod filter;
mod render;
mod sort;
mod summary;

#[cfg(test)]
mod test_helpers;

use cocomo_lib::{
    compare::{
        CompareConfig, compare_directories_node, compare_directories_pair_node,
    },
    error::{FsOperation, wrap},
    report::{ReportConfig, ReportFormat, generate_report},
    sync::{
        SyncOperation, SyncRules, plan_sync, plan_sync_pair, sync_directories,
        sync_directories_pair,
    },
    transfer::FsPair,
};
pub(crate) use display::format_size;
use render::{
    print_comparison_csv, print_comparison_json, print_comparison_tree,
};
use sort::sort_entries;
use summary::{collect_and_count, print_summary};

use crate::{
    cli::{DirCommand, DirCompareArgs, DirSyncArgs, OutputFormat},
    error::{CliError, DiffResult, fs_error_to_cli},
    resolver::{EndpointPair, EndpointResolver, resolve_endpoint_pair},
};

// ---------------------------------------------------------------------------
// Command dispatch
// ---------------------------------------------------------------------------

pub(crate) async fn run_dir<R: EndpointResolver>(
    cmd: &DirCommand,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    match cmd {
        DirCommand::Compare(args) => dir_compare(args, resolver).await,
        DirCommand::Sync(args) => dir_sync(args, resolver).await,
    }
}

pub(crate) async fn dir_compare<R: EndpointResolver>(
    args: &DirCompareArgs,
    resolver: &R,
) -> Result<DiffResult, CliError> {
    let (pair, left, right) = resolve_endpoint_pair(
        resolver,
        &args.left,
        &args.right,
        args.profile.as_deref(),
    )?;
    let mut config = if args.content {
        CompareConfig::full()
    } else {
        CompareConfig::meta_data_only()
    };
    config.max_depth = args.max_depth;

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

    // Flatten, filter, and count visible entries in one pass so that both
    // the output and the summary reflect the same set of entries. Text
    // output is rendered as a tree, so the tree-aware filter must match
    // what the renderer shows.
    let use_tree_filter = args.format == OutputFormat::Text;
    let (mut visible_entries, summary) =
        collect_and_count(&comparison.entries, args, use_tree_filter);

    match args.format {
        OutputFormat::Text => {
            print_comparison_tree(&comparison, args);
        }
        OutputFormat::Csv => {
            if let Some(sort) = args.sort {
                sort_entries(&mut visible_entries, sort);
            }
            print_comparison_csv(&visible_entries);
        }
        OutputFormat::Json => {
            print_comparison_json(&visible_entries, &summary);
        }
    }

    // Print summary unless suppressed via --no-summary.
    if !args.no_summary {
        print_summary(&summary, args.summary);
    }

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

    if summary.different_count() > 0 || summary.orphan_count() > 0 {
        Ok(DiffResult::HasDiffs)
    } else {
        Ok(DiffResult::NoDiffs)
    }
}

pub(super) fn build_report_config(args: &DirCompareArgs) -> ReportConfig {
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

pub(crate) async fn dir_sync<R: EndpointResolver>(
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
        compare_content: args.compare_files,
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
