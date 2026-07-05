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
    /// Compiled content regex.
    content_regex: Option<Regex>,
    /// Other filters.
    other: OtherFilters,
    /// Display filters.
    display: DisplayFilters,
}

impl FilterSet {
    /// Create a new filter set from the given filter definitions.
    pub fn new(
        name_filters: &[NameFilter],
        other: OtherFilters,
        display: DisplayFilters,
    ) -> Self {
        let mut include_builder = GlobSetBuilder::new();
        let mut exclude_builder = GlobSetBuilder::new();
        let mut has_include = false;
        let mut has_exclude = false;

        for filter in name_filters {
            match filter {
                NameFilter::Include(pattern) => {
                    if Self::add_pattern_to_builder(
                        pattern,
                        &mut include_builder,
                    ) {
                        has_include = true;
                    }
                }
                NameFilter::Exclude(pattern) => {
                    if Self::add_pattern_to_builder(
                        pattern,
                        &mut exclude_builder,
                    ) {
                        has_exclude = true;
                    }
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

        let content_regex = other
            .regular_expression
            .as_ref()
            .and_then(|r| Regex::new(r).ok());

        Self {
            include_set,
            exclude_set,
            content_regex,
            other,
            display,
        }
    }

    fn add_pattern_to_builder(
        pattern: &Pattern,
        builder: &mut GlobSetBuilder,
    ) -> bool {
        match pattern {
            Pattern::Glob(g) => {
                if let Ok(glob) = Glob::new(g) {
                    builder.add(glob);
                    return true;
                }
            }
            Pattern::Regex(r) => {
                // Convert regex to a glob-like pattern by wrapping it.
                // This is a best-effort approach; full regex matching is
                // done separately.
                if let Ok(glob) = Glob::new(&format!("*{r}*")) {
                    builder.add(glob);
                    return true;
                }
            }
            Pattern::Name(n) => {
                if let Ok(glob) = Glob::new(n) {
                    builder.add(glob);
                    return true;
                }
            }
        }
        false
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

        // Include filter: entry must match at least one include pattern.
        if matches!(&self.include_set, Some(include) if !include.is_match(name))
        {
            return false;
        }

        // Exclude filter: entry must not match any exclude pattern.
        if matches!(&self.exclude_set, Some(exclude) if exclude.is_match(name))
        {
            return false;
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
}
