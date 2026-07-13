// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Report generation for directory comparisons.
//!
//! Serializes a [`DirComparison`] into various output formats (text, CSV,
//! JSON) based on [`ReportConfig`] filters.

use std::fmt::Write;

use crate::compare::{DirComparison, DirEntry, DirEntryStatus};

/// Supported report output formats.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReportFormat {
    /// Plain text table matching the TUI column layout.
    #[default]
    Text,
    /// Comma-separated values with a header row.
    Csv,
    /// JSON-serialized comparison result.
    Json,
}

impl std::fmt::Display for ReportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportFormat::Text => write!(f, "text"),
            ReportFormat::Csv => write!(f, "csv"),
            ReportFormat::Json => write!(f, "json"),
        }
    }
}

/// Configuration that controls which entries appear in the report.
#[derive(Clone, Debug, Default)]
pub struct ReportConfig {
    /// Include entries that are identical on both sides.
    pub include_same: bool,
    /// Include entries that differ between left and right.
    pub include_different: bool,
    /// Include orphan entries (left-only or right-only).
    pub include_orphans: bool,
    /// Recurse into subdirectory entries.
    pub include_subdirectories: bool,
    /// Include detailed file metadata (size, modified, hash).
    pub include_file_details: bool,
}

impl ReportConfig {
    /// Create a config that includes all entries and details.
    pub fn all() -> Self {
        Self {
            include_same: true,
            include_different: true,
            include_orphans: true,
            include_subdirectories: true,
            include_file_details: true,
        }
    }

    /// Create a config that shows only differences.
    pub fn differences_only() -> Self {
        Self {
            include_same: false,
            include_different: true,
            include_orphans: true,
            include_subdirectories: true,
            include_file_details: true,
        }
    }
}

/// Check whether an entry should be included based on the config.
fn entry_matches(entry: &DirEntry, config: &ReportConfig) -> bool {
    match entry.status {
        DirEntryStatus::Same | DirEntryStatus::SameBinary => {
            config.include_same
        }
        DirEntryStatus::Different | DirEntryStatus::Similar => {
            config.include_different
        }
        DirEntryStatus::LeftOnly
        | DirEntryStatus::RightOnly
        | DirEntryStatus::CenterOnly => config.include_orphans,
        // Always include merge/conflict/mismatch entries.
        DirEntryStatus::Mergeable
        | DirEntryStatus::Conflict
        | DirEntryStatus::IdenticalNameDifferentType => true,
    }
}

/// Generate a human-readable report string from a comparison result.
///
/// The format and filtering are controlled by [`ReportFormat`] and
/// [`ReportConfig`].
pub fn generate_report(
    comparison: &DirComparison,
    format: ReportFormat,
    config: &ReportConfig,
) -> String {
    match format {
        ReportFormat::Text => generate_text_report(comparison, config),
        ReportFormat::Csv => generate_csv_report(comparison, config),
        ReportFormat::Json => generate_json_report(comparison, config),
    }
}

// ---------------------------------------------------------------------------
// Text report
// ---------------------------------------------------------------------------

fn generate_text_report(
    comparison: &DirComparison,
    config: &ReportConfig,
) -> String {
    let mut buf = String::new();

    // Collect flat entries.
    let mut entries: Vec<&DirEntry> = Vec::new();
    collect_matching_entries(&comparison.entries, config, &mut entries);

    if entries.is_empty() && !config.include_same {
        // No diffs found — still useful to report that.
        write!(buf, "No differences found.\n").ok();
        write_summary(&mut buf, comparison);
        return buf;
    }

    if entries.is_empty() {
        write!(buf, "Directories are identical.\n").ok();
        write_summary(&mut buf, comparison);
        return buf;
    }

    // Print header.
    if config.include_file_details {
        write!(
            buf,
            "{:<5} {:<40} {:>12} {:>12}\n",
            "Stat", "Name", "Left Size", "Right Size"
        )
        .ok();
        write!(
            buf,
            "{:<5} {:<40} {:>12} {:>12}\n",
            "----",
            "----------------------------------------",
            "------------",
            "------------"
        )
        .ok();
    } else {
        write!(buf, "{:<5} {:<40}\n", "Stat", "Name").ok();
        write!(
            buf,
            "{:<5} {:<40}\n",
            "----", "----------------------------------------"
        )
        .ok();
    }

    for entry in &entries {
        writeln_entry_text(&mut buf, entry, config);
    }

    write_summary(&mut buf, comparison);
    buf
}

fn writeln_entry_text(
    buf: &mut String,
    entry: &DirEntry,
    config: &ReportConfig,
) {
    let status_sym = entry.status.symbol();
    let name = &entry.name;

    if config.include_file_details {
        let left_size = entry
            .left
            .as_ref()
            .map(|l| format_size(l.size))
            .unwrap_or_else(|| "-".to_string());
        let right_size = entry
            .right
            .as_ref()
            .map(|r| format_size(r.size))
            .unwrap_or_else(|| "-".to_string());
        write!(
            buf,
            "{:<5} {:<40} {:>12} {:>12}\n",
            status_sym, name, left_size, right_size
        )
        .ok();
    } else {
        write!(buf, "{:<5} {:<40}\n", status_sym, name).ok();
    }
}

fn write_summary(buf: &mut String, comparison: &DirComparison) {
    writeln!(buf).ok();
    writeln!(
        buf,
        "Summary: {} total, {} same, {} different, {} orphans",
        comparison.total(),
        comparison.same_count(),
        comparison.different_count(),
        comparison.orphan_count(),
    )
    .ok();

    if !comparison.errors.is_empty() {
        writeln!(buf, "Errors: {}", comparison.errors.len()).ok();
    }
}

// ---------------------------------------------------------------------------
// CSV report
// ---------------------------------------------------------------------------

fn generate_csv_report(
    comparison: &DirComparison,
    config: &ReportConfig,
) -> String {
    let mut buf = String::new();

    let mut entries: Vec<&DirEntry> = Vec::new();
    collect_matching_entries(&comparison.entries, config, &mut entries);

    // Header row.
    if config.include_file_details {
        writeln!(
            buf,
            "status,name,is_dir,left_size,right_size,left_modified,\
             right_modified"
        )
        .ok();
    } else {
        writeln!(buf, "status,name,is_dir").ok();
    }

    for &entry in &entries {
        writeln_entry_csv(&mut buf, entry, config);
    }

    buf
}

fn writeln_entry_csv(
    buf: &mut String,
    entry: &DirEntry,
    config: &ReportConfig,
) {
    let is_dir = entry
        .left
        .as_ref()
        .map(|l| l.is_dir)
        .or_else(|| entry.right.as_ref().map(|r| r.is_dir))
        .unwrap_or(false);

    if config.include_file_details {
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

        writeln!(
            buf,
            "{},{},{},{},{},{},{}",
            entry.status,
            escape_csv_field(&entry.name),
            is_dir,
            left_size,
            right_size,
            escape_csv_field(&left_modified),
            escape_csv_field(&right_modified),
        )
        .ok();
    } else {
        writeln!(
            buf,
            "{},{},{}",
            entry.status,
            escape_csv_field(&entry.name),
            is_dir,
        )
        .ok();
    }
}

// ---------------------------------------------------------------------------
// JSON report
// ---------------------------------------------------------------------------

fn generate_json_report(
    comparison: &DirComparison,
    config: &ReportConfig,
) -> String {
    let mut entries: Vec<&DirEntry> = Vec::new();
    collect_matching_entries(&comparison.entries, config, &mut entries);

    let json_entries: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| entry_to_json(*entry, config))
        .collect();

    let output = serde_json::json!({
        "summary": {
            "total": comparison.total(),
            "same": comparison.same_count(),
            "different": comparison.different_count(),
            "orphans": comparison.orphan_count(),
        },
        "entries": json_entries,
    });

    serde_json::to_string_pretty(&output).unwrap_or_default()
}

fn entry_to_json(
    entry: &DirEntry,
    config: &ReportConfig,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "status".into(),
        serde_json::Value::String(entry.status.to_string()),
    );
    obj.insert("name".into(), serde_json::json!(entry.name));
    obj.insert(
        "is_dir".into(),
        serde_json::json!(
            entry
                .left
                .as_ref()
                .map(|l| l.is_dir)
                .or_else(|| entry.right.as_ref().map(|r| r.is_dir))
                .unwrap_or(false)
        ),
    );

    if config.include_file_details {
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
            ro.insert("modified".into(), serde_json::json!(right.modified));
            ro.insert("path".into(), serde_json::json!(right.path));
            if let Some(ref h) = right.hash {
                ro.insert("hash".into(), serde_json::json!(h));
            }
            obj.insert("right".into(), serde_json::Value::Object(ro));
        } else {
            obj.insert("right".into(), serde_json::Value::Null);
        }
    }

    serde_json::Value::Object(obj)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Recursively collect entries that match the config into a flat list.
fn collect_matching_entries<'a>(
    entries: &'a [DirEntry],
    config: &ReportConfig,
    out: &mut Vec<&'a DirEntry>,
) {
    for entry in entries {
        if entry_matches(entry, config) {
            out.push(entry);
        }
        // Recurse into subdirectories if configured.
        if config.include_subdirectories {
            if let Some(ref sub) = entry.sub_entries {
                collect_matching_entries(&sub.entries, config, out);
            }
        }
    }
}

fn escape_csv_field(field: &str) -> String {
    if field.contains(',')
        || field.contains('"')
        || field.contains('\n')
        || field.contains('\r')
    {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(
        name: &str,
        status: DirEntryStatus,
        left_size: u64,
        right_size: Option<u64>,
    ) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            status,
            left: Some(crate::compare::EntryInfo {
                name: name.to_string(),
                path: format!("/left/{}", name),
                size: left_size,
                modified: "2026-01-01T00:00:00Z".to_string(),
                is_dir: false,
                hash: None,
            }),
            right: right_size.map(|sz| crate::compare::EntryInfo {
                name: name.to_string(),
                path: format!("/right/{}", name),
                size: sz,
                modified: "2026-01-01T00:00:00Z".to_string(),
                is_dir: false,
                hash: None,
            }),
            center: None,
            sub_entries: None,
        }
    }

    fn make_comparison(entries: Vec<DirEntry>) -> DirComparison {
        DirComparison {
            entries,
            counts: Default::default(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn config_all_includes_everything() {
        let config = ReportConfig::all();
        let comparison = make_comparison(vec![
            make_entry("a.txt", DirEntryStatus::Same, 100, Some(100)),
            make_entry("b.txt", DirEntryStatus::Different, 100, Some(200)),
            make_entry("c.txt", DirEntryStatus::LeftOnly, 50, None),
        ]);

        let report = generate_report(&comparison, ReportFormat::Csv, &config);
        assert!(report.contains("a.txt"));
        assert!(report.contains("b.txt"));
        assert!(report.contains("c.txt"));
    }

    #[test]
    fn config_differences_only_excludes_same() {
        let config = ReportConfig::differences_only();
        let comparison = make_comparison(vec![
            make_entry("a.txt", DirEntryStatus::Same, 100, Some(100)),
            make_entry("b.txt", DirEntryStatus::Different, 100, Some(200)),
            make_entry("c.txt", DirEntryStatus::RightOnly, 0, Some(50)),
        ]);

        let report = generate_report(&comparison, ReportFormat::Csv, &config);
        assert!(!report.contains("a.txt"));
        assert!(report.contains("b.txt"));
        assert!(report.contains("c.txt"));
    }

    #[test]
    fn json_report_contains_summary() {
        let comparison = make_comparison(vec![
            make_entry("a.txt", DirEntryStatus::Same, 100, Some(100)),
            make_entry("b.txt", DirEntryStatus::Different, 100, Some(200)),
        ]);
        let config = ReportConfig::all();

        let report = generate_report(&comparison, ReportFormat::Json, &config);
        assert!(report.contains("\"total\": 2"));
        assert!(report.contains("\"same\": 0"));
        // Note: DirComparison.counts is empty by default in this test,
        // so same_count/different_count use .counts which is empty.
        // The summary values come from DirComparison methods that read
        // .counts.
    }

    #[test]
    fn csv_escapes_comma_in_name() {
        let mut entry = make_entry(
            "file,with,commas.txt",
            DirEntryStatus::Different,
            100,
            Some(200),
        );
        // Fix paths to match the actual name.
        entry.left.as_mut().unwrap().path =
            "/left/file,with,commas.txt".to_string();
        entry.right.as_mut().unwrap().path =
            "/right/file,with,commas.txt".to_string();

        let comparison = make_comparison(vec![entry]);
        let config = ReportConfig::all();

        let report = generate_report(&comparison, ReportFormat::Csv, &config);
        assert!(report.contains("\"file,with,commas.txt\""));
    }

    #[test]
    fn format_size_human_readable() {
        assert_eq!(format_size(512), "512B");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1_048_576), "1.0M");
        assert_eq!(format_size(1_073_741_824), "1.0G");
    }
}
