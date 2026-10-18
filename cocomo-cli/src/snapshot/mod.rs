// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::path::PathBuf;

use cocomo_lib::{
    error::{FsOperation, wrap},
    snapshot::{
        Snapshot, SnapshotEntry, SnapshotEntryStatus, capture_snapshot_node,
    },
};

use crate::{
    cli::{
        SnapshotCaptureArgs, SnapshotCommand, SnapshotDiffArgs,
        SnapshotListArgs,
    },
    dir::format_size,
    error::{CliError, DiffResult, fs_error_to_cli},
    resolver::{EndpointResolver, resolve_endpoint},
};

// ---------------------------------------------------------------------------
// Snapshot commands
// ---------------------------------------------------------------------------

pub(crate) async fn run_snapshot<R: EndpointResolver>(
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

pub(crate) async fn snapshot_capture<R: EndpointResolver>(
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

pub(crate) async fn snapshot_list(
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

pub(crate) async fn snapshot_diff(
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
