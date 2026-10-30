// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Semantic row coloring for the comparison views.
//!
//! The UI spec (section 4.3) colors rows by *meaning*, not by raw status:
//! the newer side of an important content difference is red, the older
//! side gray; a minor difference uses yellow; orphans are blue; identical
//! or ignored rows stay neutral. [`ColorRole`] centralizes that mapping so
//! the folder panes and every later view (text, table, hex) resolve colors
//! through one place. Roles resolve to theme tokens, with per-session hex
//! overrides from [`ColorOverrides`] taking precedence (the personalization
//! hook of spec section 7.1).

use chrono::{DateTime, FixedOffset};
use cocomo_lib::{ColorOverrides, DirEntry, DirEntryStatus};
use gpui::{Hsla, Rgba};

use crate::state::status_category;

/// One side of a comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The left pane's side.
    Left,
    /// The right pane's side.
    Right,
}

/// Semantic color role for one side of a row.
///
/// Statuses map onto a small set of roles instead of a fixed RGB palette,
/// so colors follow the active theme unless the session overrides them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorRole {
    /// Newer side of an important content difference (spec: red).
    Important,
    /// Newer side of a minor difference (spec: yellow).
    Minor,
    /// Entry that exists on only one side (spec: blue).
    Orphan,
    /// Older side of a difference (spec: gray).
    OtherSide,
    /// Identical, ignored, or otherwise neutral row content.
    Neutral,
}

impl ColorRole {
    /// Resolve this role to a color: the session override when set and
    /// parseable, otherwise the matching theme token.
    pub fn resolve(
        self,
        theme: &gpui::component::Theme,
        overrides: &ColorOverrides,
    ) -> Hsla {
        let override_hex = match self {
            Self::Important => overrides.important.as_deref(),
            Self::Minor => overrides.minor.as_deref(),
            Self::Orphan => overrides.orphan.as_deref(),
            Self::OtherSide => overrides.other_side.as_deref(),
            Self::Neutral => None,
        };
        if let Some(hex) = override_hex
            && let Ok(rgba) = Rgba::try_from(hex)
        {
            return Hsla::from(rgba);
        }
        match self {
            Self::Important => theme.danger,
            Self::Minor => theme.warning,
            Self::Orphan => theme.info,
            Self::OtherSide | Self::Neutral => theme.muted_foreground,
        }
    }
}

/// Parse an RFC 3339 modification stamp into a comparable timestamp.
///
/// Provider metadata stores `modified` as an RFC 3339 string; offsets can
/// differ between sides, so the stamps must be parsed rather than
/// compared lexically. Anything unparseable yields `None`, which callers
/// treat as "no newer side".
pub fn parse_modified(modified: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(modified).ok()
}

/// The side whose modification time is strictly newer, if any.
///
/// Equal or unparseable stamps yield `None`: the coloring then treats both
/// sides alike instead of guessing a newer one.
pub fn newer_side(entry: &DirEntry) -> Option<Side> {
    let left = entry.left.as_ref()?.modified.as_str();
    let right = entry.right.as_ref()?.modified.as_str();
    let left = parse_modified(left)?;
    let right = parse_modified(right)?;
    match left.cmp(&right) {
        std::cmp::Ordering::Greater => Some(Side::Left),
        std::cmp::Ordering::Less => Some(Side::Right),
        std::cmp::Ordering::Equal => None,
    }
}

/// Color role for `side` of `entry`.
///
/// Ignored entries are neutral on both sides. Orphans color the existing
/// side blue. Differences color the newer side by severity and the older
/// side gray; without a determinable newer side both sides take the
/// severity color.
pub fn side_role(entry: &DirEntry, side: Side, ignored: bool) -> ColorRole {
    if ignored {
        return ColorRole::Neutral;
    }
    match status_category(&entry.status) {
        crate::state::StatusCategory::Same => ColorRole::Neutral,
        crate::state::StatusCategory::Orphan => {
            let present = match side {
                Side::Left => entry.left.is_some(),
                Side::Right => entry.right.is_some(),
            };
            if present {
                ColorRole::Orphan
            } else {
                ColorRole::Neutral
            }
        }
        crate::state::StatusCategory::Different => {
            let severity = match entry.status {
                DirEntryStatus::Similar => ColorRole::Minor,
                _ => ColorRole::Important,
            };
            match newer_side(entry) {
                Some(newer) if newer == side => severity,
                Some(_) => ColorRole::OtherSide,
                None => severity,
            }
        }
    }
}

/// Gutter icon for a row, per spec section 4.2.
///
/// The gutter icon is coarser than the row coloring: it marks *that*
/// something differs (`~` content, `≠` name/type attributes, `=` same,
/// `|` orphan, `*` filtered/ignored), leaving the important-vs-minor
/// distinction to the colors.
pub fn gutter_icon(entry: &DirEntry, ignored: bool) -> &'static str {
    if ignored {
        return "*";
    }
    if entry.name_conflict {
        return "≠";
    }
    match status_category(&entry.status) {
        crate::state::StatusCategory::Same => "=",
        crate::state::StatusCategory::Orphan => "|",
        crate::state::StatusCategory::Different => "~",
    }
}

#[cfg(test)]
mod tests {
    use cocomo_lib::EntryInfo;

    use super::*;

    fn info(path: &str, modified: &str) -> EntryInfo {
        EntryInfo {
            name: path.to_string(),
            path: path.to_string(),
            size: 1,
            modified: modified.to_string(),
            is_dir: false,
            hash: None,
        }
    }

    fn make_entry(
        status: DirEntryStatus,
        left: Option<EntryInfo>,
        right: Option<EntryInfo>,
    ) -> DirEntry {
        DirEntry {
            name: "a.txt".to_string(),
            status,
            left,
            right,
            center: None,
            sub_entries: None,
            name_conflict: false,
        }
    }

    #[test]
    fn newer_side_compares_parsed_timestamps() {
        let entry = make_entry(
            DirEntryStatus::Different,
            Some(info("a.txt", "2026-10-27T21:50:51+02:00")),
            Some(info("a.txt", "2026-10-27T20:50:51+01:00")),
        );
        // The same instant expressed with different offsets is not newer.
        assert_eq!(newer_side(&entry), None);

        let entry = make_entry(
            DirEntryStatus::Different,
            Some(info("a.txt", "2026-10-27T22:00:00+02:00")),
            Some(info("a.txt", "2026-10-27T21:00:00+01:00")),
        );
        // 22:00+02:00 (20:00 UTC) is older than 21:00+01:00 (20:00 UTC).
        assert_eq!(newer_side(&entry), None);

        let entry = make_entry(
            DirEntryStatus::Different,
            Some(info("a.txt", "2026-10-28T00:00:00+02:00")),
            Some(info("a.txt", "2026-10-27T23:30:00+01:00")),
        );
        // 22:00 UTC vs 22:30 UTC: the right side is newer.
        assert_eq!(newer_side(&entry), Some(Side::Right));
    }

    #[test]
    fn newer_side_is_none_for_missing_or_unparseable_stamps() {
        let entry = make_entry(
            DirEntryStatus::Different,
            Some(info("a.txt", "")),
            Some(info("a.txt", "2026-10-27T21:00:00+01:00")),
        );
        assert_eq!(newer_side(&entry), None);

        let entry = make_entry(
            DirEntryStatus::LeftOnly,
            Some(info("a.txt", "2026-10-27T21:00:00+01:00")),
            None,
        );
        assert_eq!(newer_side(&entry), None);
    }

    #[test]
    fn side_role_colors_newer_side_by_severity() {
        let different = make_entry(
            DirEntryStatus::Different,
            Some(info("a.txt", "2026-10-28T21:00:00+01:00")),
            Some(info("a.txt", "2026-10-27T21:00:00+01:00")),
        );
        assert_eq!(
            side_role(&different, Side::Left, false),
            ColorRole::Important
        );
        assert_eq!(
            side_role(&different, Side::Right, false),
            ColorRole::OtherSide
        );

        let similar = DirEntry {
            status: DirEntryStatus::Similar,
            ..different.clone()
        };
        assert_eq!(side_role(&similar, Side::Left, false), ColorRole::Minor);
        assert_eq!(
            side_role(&similar, Side::Right, false),
            ColorRole::OtherSide
        );
    }

    #[test]
    fn side_role_marks_orphans_and_neutral_rows() {
        let orphan = make_entry(
            DirEntryStatus::LeftOnly,
            Some(info("a.txt", "2026-10-27T21:00:00+01:00")),
            None,
        );
        assert_eq!(side_role(&orphan, Side::Left, false), ColorRole::Orphan);
        assert_eq!(side_role(&orphan, Side::Right, false), ColorRole::Neutral);

        let same = make_entry(
            DirEntryStatus::Same,
            Some(info("a.txt", "2026-10-27T21:00:00+01:00")),
            Some(info("a.txt", "2026-10-27T21:00:00+01:00")),
        );
        assert_eq!(side_role(&same, Side::Left, false), ColorRole::Neutral);
        // Ignoring overrides every other role.
        assert_eq!(side_role(&orphan, Side::Left, true), ColorRole::Neutral);
    }

    #[test]
    fn gutter_icon_covers_every_gutter_state() {
        let different = make_entry(
            DirEntryStatus::Different,
            Some(info("a.txt", "")),
            Some(info("a.txt", "")),
        );
        assert_eq!(gutter_icon(&different, false), "~");
        assert_eq!(gutter_icon(&different, true), "*");

        let same = DirEntry {
            status: DirEntryStatus::Same,
            ..different.clone()
        };
        assert_eq!(gutter_icon(&same, false), "=");

        let orphan = DirEntry {
            status: DirEntryStatus::RightOnly,
            left: None,
            ..different.clone()
        };
        assert_eq!(gutter_icon(&orphan, false), "|");

        let clash = DirEntry {
            name_conflict: true,
            ..different
        };
        assert_eq!(gutter_icon(&clash, false), "≠");
    }

    #[test]
    fn overrides_parse_hex_before_theme_fallback() {
        let overrides = ColorOverrides {
            important: Some("#ff0000".to_string()),
            orphan: Some("not-a-color".to_string()),
            ..ColorOverrides::default()
        };

        let theme = gpui::component::Theme::default();
        let important = ColorRole::Important.resolve(&theme, &overrides);
        let expected = Hsla::from(Rgba::try_from("#ff0000").unwrap());
        assert_eq!(important, expected);

        // An unparseable override falls back to the theme token.
        let orphan = ColorRole::Orphan.resolve(&theme, &overrides);
        assert_eq!(orphan, theme.info);
    }
}
