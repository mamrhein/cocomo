// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Filtering system for directory comparison results.
//!
//! Provides name-based filters (include/exclude patterns), display filters
//! for controlling what statuses are visible, and content-based filters.

use std::ops::Range;

use chrono::{DateTime, Utc};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;

use crate::compare::DirEntryStatus;

/// Maximum allowed length for a name-matching pattern string.
const MAX_NAME_PATTERN_LEN: usize = 1024;

/// Maximum allowed length for a content-matching regex pattern string.
const MAX_CONTENT_PATTERN_LEN: usize = 4096;

/// A pattern used for name-based filtering.
#[derive(Clone, Debug)]
pub enum Pattern {
    /// Glob pattern (e.g., `*.txt`, `**/src/**`).
    Glob(String),
    /// Full regular expression.
    Regex(String),
    /// Exact name match.
    Name(String),
}

impl Pattern {
    /// Get the display string for this pattern.
    pub fn display(&self) -> &str {
        match self {
            Pattern::Glob(s) => s,
            Pattern::Regex(s) => s,
            Pattern::Name(s) => s,
        }
    }
}

/// Whether a name filter includes or excludes matching entries.
#[derive(Clone, Debug)]
pub enum NameFilter {
    /// Show only entries matching the pattern.
    Include(Pattern),
    /// Hide entries matching the pattern.
    Exclude(Pattern),
}

/// Type-based file filter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FileTypeFilter {
    /// Show everything.
    #[default]
    All,
    /// Files only.
    Files,
    /// Directories only.
    Dirs,
    /// Symbolic links only.
    Symlinks,
}

/// Additional filter criteria beyond name matching.
#[derive(Clone, Debug, Default)]
pub struct OtherFilters {
    /// Filter by file size range (bytes).
    pub size_range: Option<Range<u64>>,
    /// Filter by modification date range.
    pub date_range: Option<Range<DateTime<Utc>>>,
    /// Filter by file type.
    pub file_type: FileTypeFilter,
    /// Show only entries with these statuses.
    pub status: Vec<DirEntryStatus>,
    /// Content-based regex filter (matches file content).
    pub regular_expression: Option<String>,
    /// Ignore hidden files (names starting with `.`).
    pub ignore_hidden: bool,
    /// Ignore system files.
    pub ignore_system: bool,
}

/// Display filters that toggle visibility without affecting comparison
/// results.
#[derive(Clone, Debug, Default)]
pub struct DisplayFilters {
    /// Show entries that are the same on both sides.
    pub show_same: bool,
    /// Show entries that differ between sides.
    pub show_different: bool,
    /// Show orphan entries (present on only one side).
    pub show_orphans: bool,
    /// Show mergeable entries.
    pub show_mergeable: bool,
    /// Show conflicting entries.
    pub show_conflicts: bool,
    /// Show ignored lines in text diff (suppress content filters).
    pub suppress_content_filters: bool,
}

impl DisplayFilters {
    /// Create default display filters that show all items.
    pub fn all() -> Self {
        Self {
            show_same: true,
            show_different: true,
            show_orphans: true,
            show_mergeable: true,
            show_conflicts: true,
            suppress_content_filters: false,
        }
    }

    /// Create display filters that hide identical entries.
    pub fn differences_only() -> Self {
        Self {
            show_same: false,
            show_different: true,
            show_orphans: true,
            show_mergeable: true,
            show_conflicts: true,
            suppress_content_filters: false,
        }
    }

    /// Return `true` if an entry with the given status should be visible.
    pub fn should_show(&self, status: &DirEntryStatus) -> bool {
        match status {
            DirEntryStatus::Same | DirEntryStatus::SameBinary => {
                self.show_same
            }
            DirEntryStatus::Different | DirEntryStatus::Similar => {
                self.show_different
            }
            DirEntryStatus::LeftOnly
            | DirEntryStatus::RightOnly
            | DirEntryStatus::CenterOnly => self.show_orphans,
            DirEntryStatus::Mergeable => self.show_mergeable,
            DirEntryStatus::Conflict => self.show_conflicts,
            DirEntryStatus::IdenticalNameDifferentType => self.show_different,
        }
    }
}

/// Compiled filter state that can be efficiently applied to entries.
#[derive(Clone, Debug)]
pub struct FilterSet {
    /// Compiled include glob set. `None` means no include filter.
    include_set: Option<GlobSet>,
    /// Compiled exclude glob set. `None` means no exclude filter.
    exclude_set: Option<GlobSet>,
    /// Compiled include regex patterns.
    include_regexes: Vec<Regex>,
    /// Compiled exclude regex patterns.
    exclude_regexes: Vec<Regex>,
    /// Whether an include filter was requested (even if all patterns
    /// failed to compile).
    has_include: bool,
    /// Whether an exclude filter was requested (even if all patterns
    /// failed to compile).
    has_exclude: bool,
    /// Compiled content regex.
    content_regex: Option<Regex>,
    /// Other filters.
    other: OtherFilters,
    /// Display filters.
    display: DisplayFilters,
    /// Errors encountered during pattern compilation. Non-empty when at
    /// least one pattern could not be compiled; callers should inspect
    /// this to detect misconfigurations.
    compilation_errors: Vec<String>,
}

impl FilterSet {
    /// Create a new filter set from the given filter definitions.
    ///
    /// Patterns that fail to compile are recorded in the
    /// [`compilation_errors`](Self::compilation_errors) field rather than
    /// aborting the entire filter set, so that valid patterns still take
    /// effect.
    pub fn new(
        name_filters: &[NameFilter],
        other: OtherFilters,
        display: DisplayFilters,
    ) -> Self {
        let mut include_builder = GlobSetBuilder::new();
        let mut exclude_builder = GlobSetBuilder::new();
        let mut include_regexes = Vec::new();
        let mut exclude_regexes = Vec::new();
        let mut has_include = false;
        let mut has_exclude = false;
        let mut compilation_errors = Vec::new();

        for filter in name_filters {
            match filter {
                NameFilter::Include(pattern) => {
                    has_include = true;
                    Self::add_pattern(
                        pattern,
                        &mut include_builder,
                        &mut include_regexes,
                        &mut compilation_errors,
                    );
                }
                NameFilter::Exclude(pattern) => {
                    has_exclude = true;
                    Self::add_pattern(
                        pattern,
                        &mut exclude_builder,
                        &mut exclude_regexes,
                        &mut compilation_errors,
                    );
                }
            }
        }

        let include_set = if has_include {
            Some(include_builder.build().unwrap_or_default())
        } else {
            None
        };

        let exclude_set = if has_exclude {
            Some(exclude_builder.build().unwrap_or_default())
        } else {
            None
        };

        let content_regex = match other.regular_expression.as_ref() {
            Some(r) => {
                // Reject overly long patterns to prevent compilation DoS.
                if r.len() > MAX_CONTENT_PATTERN_LEN {
                    compilation_errors.push(format!(
                        "content regex too long ({} chars, max {})",
                        r.len(),
                        MAX_CONTENT_PATTERN_LEN
                    ));
                    None
                } else {
                    match Regex::new(r) {
                        Ok(re) => Some(re),
                        Err(e) => {
                            compilation_errors.push(format!(
                                "invalid content regex {:?}: {e}",
                                r
                            ));
                            None
                        }
                    }
                }
            }
            None => None,
        };

        Self {
            include_set,
            exclude_set,
            include_regexes,
            exclude_regexes,
            has_include,
            has_exclude,
            content_regex,
            other,
            display,
            compilation_errors,
        }
    }

    /// Try to compile a pattern into either a glob or a regex.
    ///
    /// `Pattern::Regex` is compiled as an actual [`Regex`] rather than being
    /// mangled into a glob. `Pattern::Glob` and `Pattern::Name` are compiled
    /// as globs. Compilation failures are recorded in `errors`.
    fn add_pattern(
        pattern: &Pattern,
        glob_builder: &mut GlobSetBuilder,
        regexes: &mut Vec<Regex>,
        errors: &mut Vec<String>,
    ) {
        match pattern {
            Pattern::Glob(g) => {
                // Reject overly long patterns to prevent compilation DoS.
                if g.len() > MAX_NAME_PATTERN_LEN {
                    errors.push(format!(
                        "glob pattern too long ({} chars, max {}): {:?}",
                        g.len(),
                        MAX_NAME_PATTERN_LEN,
                        &g[..g.len().min(80)]
                    ));
                    return;
                }
                if let Err(e) = Glob::new(g) {
                    errors.push(format!("invalid glob pattern {:?}: {e}", g));
                } else {
                    glob_builder.add(Glob::new(g).unwrap());
                }
            }
            Pattern::Regex(r) => {
                // Reject overly long patterns to prevent compilation DoS.
                if r.len() > MAX_NAME_PATTERN_LEN {
                    errors.push(format!(
                        "regex pattern too long ({} chars, max {}): {:?}",
                        r.len(),
                        MAX_NAME_PATTERN_LEN,
                        &r[..r.len().min(80)]
                    ));
                    return;
                }
                // Compile the regex directly; do not convert to a glob
                // because that would change the matching semantics.
                match Regex::new(r) {
                    Ok(re) => regexes.push(re),
                    Err(e) => {
                        errors.push(format!(
                            "invalid regex pattern {:?}: {e}",
                            r
                        ));
                    }
                }
            }
            Pattern::Name(n) => {
                // Reject overly long patterns to prevent compilation DoS.
                if n.len() > MAX_NAME_PATTERN_LEN {
                    errors.push(format!(
                        "name pattern too long ({} chars, max {}): {:?}",
                        n.len(),
                        MAX_NAME_PATTERN_LEN,
                        &n[..n.len().min(80)]
                    ));
                    return;
                }
                if let Err(e) = Glob::new(n) {
                    errors.push(format!("invalid name pattern {:?}: {e}", n));
                } else {
                    glob_builder.add(Glob::new(n).unwrap());
                }
            }
        }
    }

    /// Return `true` if the entry passes all non-content filters.
    ///
    /// This checks display filters, name filters, and other metadata
    /// filters. Content-based filtering requires file I/O and is
    /// handled separately via [`Self::passes_content`].
    pub fn passes(&self, name: &str, status: &DirEntryStatus) -> bool {
        // Display filter check.
        if !self.display.should_show(status) {
            return false;
        }

        // Hidden file check.
        if self.other.ignore_hidden && name.starts_with('.') {
            return false;
        }

        // Include filter: entry must match at least one include glob
        // or include regex when an include filter is active.
        if self.has_include {
            let glob_match =
                matches!(&self.include_set, Some(s) if s.is_match(name));
            let regex_match =
                self.include_regexes.iter().any(|r| r.is_match(name));
            if !(glob_match || regex_match) {
                return false;
            }
        }

        // Exclude filter: entry must not match any exclude glob
        // or exclude regex when an exclude filter is active.
        if self.has_exclude {
            let glob_match =
                matches!(&self.exclude_set, Some(s) if s.is_match(name));
            let regex_match =
                self.exclude_regexes.iter().any(|r| r.is_match(name));
            if glob_match || regex_match {
                return false;
            }
        }

        true
    }

    /// Return `true` if the given file content passes the content
    /// regex filter.
    ///
    /// Returns `true` by default when no content regex is configured.
    pub fn passes_content(&self, content: &str) -> bool {
        if let Some(ref re) = self.content_regex {
            re.is_match(content)
        } else {
            true
        }
    }

    /// Return the display filters.
    pub fn display(&self) -> &DisplayFilters {
        &self.display
    }

    /// Return the other filters.
    pub fn other(&self) -> &OtherFilters {
        &self.other
    }

    /// Return errors that occurred during pattern compilation.
    ///
    /// Non-empty when at least one pattern could not be compiled.
    /// Callers should surface these to users so that misconfigured
    /// filters are detected rather than silently ignored.
    pub fn compilation_errors(&self) -> &[String] {
        &self.compilation_errors
    }
}

/// Convenience: build a filter set with default display filters.
pub fn build_filter(
    name_filters: &[NameFilter],
    other: OtherFilters,
) -> FilterSet {
    FilterSet::new(name_filters, other, DisplayFilters::all())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_filters_show_all() {
        let df = DisplayFilters::all();
        assert!(df.should_show(&DirEntryStatus::Same));
        assert!(df.should_show(&DirEntryStatus::Different));
        assert!(df.should_show(&DirEntryStatus::LeftOnly));
        assert!(df.should_show(&DirEntryStatus::RightOnly));
        assert!(df.should_show(&DirEntryStatus::Conflict));
    }

    #[test]
    fn display_filters_differences_only() {
        let df = DisplayFilters::differences_only();
        assert!(!df.should_show(&DirEntryStatus::Same));
        assert!(df.should_show(&DirEntryStatus::Different));
        assert!(df.should_show(&DirEntryStatus::LeftOnly));
        assert!(df.should_show(&DirEntryStatus::RightOnly));
    }

    #[test]
    fn name_filter_include() {
        let filters = vec![NameFilter::Include(Pattern::Glob("*.rs".into()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert!(fs.passes("main.rs", &DirEntryStatus::Same));
        assert!(!fs.passes("main.txt", &DirEntryStatus::Same));
    }

    #[test]
    fn name_filter_exclude() {
        let filters = vec![NameFilter::Exclude(Pattern::Glob("*.log".into()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert!(fs.passes("main.rs", &DirEntryStatus::Same));
        assert!(!fs.passes("debug.log", &DirEntryStatus::Same));
    }

    #[test]
    fn ignore_hidden_files() {
        let other = OtherFilters {
            ignore_hidden: true,
            ..Default::default()
        };
        let fs = FilterSet::new(&[], other, DisplayFilters::all());

        assert!(fs.passes("visible.txt", &DirEntryStatus::Same));
        assert!(!fs.passes(".hidden", &DirEntryStatus::Same));
        assert!(!fs.passes(".gitignore", &DirEntryStatus::Same));
    }

    #[test]
    fn exact_name_filter() {
        let filters =
            vec![NameFilter::Include(Pattern::Name("Cargo.toml".into()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert!(fs.passes("Cargo.toml", &DirEntryStatus::Same));
        assert!(!fs.passes("Cargo.lock", &DirEntryStatus::Same));
    }

    #[test]
    fn display_filter_hides_same() {
        let display = DisplayFilters::differences_only();
        let fs = FilterSet::new(&[], OtherFilters::default(), display);

        assert!(!fs.passes("same.txt", &DirEntryStatus::Same));
        assert!(fs.passes("diff.txt", &DirEntryStatus::Different));
    }

    #[test]
    fn content_regex_filter() {
        let other = OtherFilters {
            regular_expression: Some("TODO".into()),
            ..Default::default()
        };
        let fs = FilterSet::new(&[], other, DisplayFilters::all());

        assert!(fs.passes_content("fix this TODO item"));
        assert!(!fs.passes_content("this is fine"));
    }

    #[test]
    fn content_regex_none_passes_all() {
        let fs = FilterSet::new(
            &[],
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert!(fs.passes_content("anything goes"));
    }

    #[test]
    fn regex_include_filter() {
        let filters =
            vec![NameFilter::Include(Pattern::Regex(r"^.*\.rs$".into()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert!(fs.passes("main.rs", &DirEntryStatus::Same));
        assert!(!fs.passes("main.txt", &DirEntryStatus::Same));
        assert!(fs.compilation_errors().is_empty());
    }

    #[test]
    fn regex_exclude_filter() {
        let filters =
            vec![NameFilter::Exclude(Pattern::Regex(r"^\..*".into()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert!(fs.passes("visible.txt", &DirEntryStatus::Same));
        assert!(!fs.passes(".hidden", &DirEntryStatus::Same));
        assert!(fs.compilation_errors().is_empty());
    }

    #[test]
    fn mixed_glob_and_regex_filters() {
        let filters = vec![
            NameFilter::Include(Pattern::Glob("*.rs".into())),
            NameFilter::Include(Pattern::Regex(r"^config\..*$".into())),
        ];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert!(fs.passes("main.rs", &DirEntryStatus::Same));
        assert!(fs.passes("config.yaml", &DirEntryStatus::Same));
        assert!(!fs.passes("data.csv", &DirEntryStatus::Same));
        assert!(fs.compilation_errors().is_empty());
    }

    #[test]
    fn invalid_regex_records_error() {
        let filters =
            vec![NameFilter::Include(Pattern::Regex(r"[invalid".into()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.compilation_errors()[0].contains("[invalid"));
        // With no valid patterns, all entries fail the include filter.
        assert!(!fs.passes("anything.txt", &DirEntryStatus::Same));
    }

    #[test]
    fn invalid_glob_records_error() {
        let filters =
            vec![NameFilter::Include(Pattern::Glob(r"[[[bad".into()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.compilation_errors()[0].contains("[[[bad"));
    }

    #[test]
    fn valid_pattern_survives_invalid_glob() {
        // A valid regex should work even when a glob fails to compile.
        let filters = vec![
            NameFilter::Include(Pattern::Glob(r"[[[bad".into())),
            NameFilter::Include(Pattern::Regex(r"^ok\.txt$".into())),
        ];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.passes("ok.txt", &DirEntryStatus::Same));
        assert!(!fs.passes("bad.txt", &DirEntryStatus::Same));
    }

    #[test]
    fn invalid_content_regex_records_error() {
        let other = OtherFilters {
            regular_expression: Some(r"[broken".into()),
            ..Default::default()
        };
        let fs = FilterSet::new(&[], other, DisplayFilters::all());

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.compilation_errors()[0].contains("content regex"));
        // With no valid content regex, all content passes.
        assert!(fs.passes_content("anything"));
    }

    #[test]
    fn glob_pattern_too_long_records_error() {
        let long_glob = "a".repeat(MAX_NAME_PATTERN_LEN + 1);
        let filters =
            vec![NameFilter::Include(Pattern::Glob(long_glob.clone()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.compilation_errors()[0].contains("glob pattern too long"));
        assert!(
            fs.compilation_errors()[0]
                .contains(&MAX_NAME_PATTERN_LEN.to_string())
        );
        // With no valid include patterns, nothing passes.
        assert!(!fs.passes("anything.txt", &DirEntryStatus::Same));
    }

    #[test]
    fn regex_pattern_too_long_records_error() {
        let long_regex = "a".repeat(MAX_NAME_PATTERN_LEN + 1);
        let filters =
            vec![NameFilter::Include(Pattern::Regex(long_regex.clone()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.compilation_errors()[0].contains("regex pattern too long"));
        // With no valid include patterns, nothing passes.
        assert!(!fs.passes("anything.txt", &DirEntryStatus::Same));
    }

    #[test]
    fn name_pattern_too_long_records_error() {
        let long_name = "a".repeat(MAX_NAME_PATTERN_LEN + 1);
        let filters =
            vec![NameFilter::Include(Pattern::Name(long_name.clone()))];
        let fs = FilterSet::new(
            &filters,
            OtherFilters::default(),
            DisplayFilters::all(),
        );

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.compilation_errors()[0].contains("name pattern too long"));
        // With no valid include patterns, nothing passes.
        assert!(!fs.passes("anything.txt", &DirEntryStatus::Same));
    }

    #[test]
    fn content_regex_too_long_records_error() {
        let long_regex = "a".repeat(MAX_CONTENT_PATTERN_LEN + 1);
        let other = OtherFilters {
            regular_expression: Some(long_regex),
            ..Default::default()
        };
        let fs = FilterSet::new(&[], other, DisplayFilters::all());

        assert_eq!(fs.compilation_errors().len(), 1);
        assert!(fs.compilation_errors()[0].contains("content regex too long"));
        // With no valid content regex, all content passes.
        assert!(fs.passes_content("anything"));
    }
}
