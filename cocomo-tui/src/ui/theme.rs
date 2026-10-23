// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Central color scheme of the TUI.
//!
//! This is the only module in the crate that spells out `Color::` literals:
//! [`cocomo_palette`] is injected at the root with a `PaletteProvider` and
//! drives the framework's built-in components (borders, table headers,
//! selection, input fields), while [`CocomoTheme`] is the app-specific
//! [`ComponentTheme`] carrying the semantic styles no `Palette` field covers
//! (entry status colors, diff colors, transfer-action colors). A new view
//! inherits the whole scheme by reading the theme instead of hardcoding
//! styles.

use cocomo_lib::{DirEntryStatus, Importance};
use ratatui_kit::{
    prelude::*,
    ratatui::style::{Color, Modifier, Style},
};

/// The app palette: gray borders, yellow accents for titles and table
/// headers, and a dark-gray selection background.
pub(crate) fn cocomo_palette() -> Palette {
    let mut palette = Palette::default();
    palette.border = Color::Gray;
    palette.accent = Color::Yellow;
    palette.selection = Color::DarkGray;
    palette
}

/// App-specific component theme: the semantic styles the built-in
/// [`Palette`] fields do not cover.
///
/// Read it in any component with `hooks.use_component_theme::<CocomoTheme>()`;
/// it derives from the ambient [`Palette`], so retuning the palette at the
/// root recolors these styles too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CocomoTheme {
    /// Border style for panels, tables, and bars.
    pub border: Style,
    /// Yellow bold style for panel titles and table headers.
    pub title: Style,
    /// Reverse-video style for the selected table row.
    pub highlight: Style,
    /// Cyan bold style for the application name.
    pub brand: Style,
    /// Dark-gray style for hints, line numbers, and other secondary text.
    pub dim: Style,
    /// Yellow style for grammar-classified data tokens (not bold: unlike
    /// titles, data is content, not chrome).
    pub data: Style,
    /// Yellow bold style for the sync status line.
    pub sync_status: Style,
    /// Red bold style for removed (left-side) diff content.
    pub diff_removed: Style,
    /// Green bold style for added (right-side) diff content.
    pub diff_added: Style,
    /// Cyan bold style for the left file's column header.
    pub left_name: Style,
    /// Magenta bold style for the right file's column header.
    pub right_name: Style,
    /// Color of copy transfer actions.
    pub transfer_copy: Color,
    /// Color of center-copy transfer actions.
    pub transfer_copy_center: Color,
    /// Color of delete transfer actions.
    pub transfer_delete: Color,
    /// Color of move transfer actions.
    pub transfer_move: Color,
}

impl ComponentTheme for CocomoTheme {
    fn from_palette(palette: &Palette) -> Self {
        Self {
            border: Style::new().fg(palette.border),
            title: Style::new()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
            highlight: Style::new()
                .bg(palette.selection)
                .add_modifier(Modifier::REVERSED),
            brand: Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            dim: Style::new().fg(palette.fg_dim),
            data: Style::new().fg(palette.warning),
            sync_status: Style::new()
                .fg(palette.warning)
                .add_modifier(Modifier::BOLD),
            diff_removed: Style::new()
                .fg(palette.error)
                .add_modifier(Modifier::BOLD),
            diff_added: Style::new()
                .fg(palette.success)
                .add_modifier(Modifier::BOLD),
            left_name: Style::new()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            right_name: Style::new()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
            transfer_copy: palette.success,
            transfer_copy_center: palette.warning,
            transfer_delete: palette.error,
            transfer_move: Color::Magenta,
        }
    }
}

impl Default for CocomoTheme {
    fn default() -> Self {
        Self::from_palette(&Palette::default())
    }
}

impl CocomoTheme {
    /// Return the bold status style for a directory entry status.
    ///
    /// Every status is visually distinct except `Same` and `SameBinary`,
    /// which intentionally share a color.
    pub fn status_style(&self, status: DirEntryStatus) -> Style {
        let color = match status {
            DirEntryStatus::Same | DirEntryStatus::SameBinary => Color::Gray,
            DirEntryStatus::Similar => Color::Yellow,
            DirEntryStatus::Different => Color::Red,
            DirEntryStatus::LeftOnly => Color::Cyan,
            DirEntryStatus::RightOnly => Color::Magenta,
            DirEntryStatus::CenterOnly => Color::White,
            DirEntryStatus::Mergeable => Color::Green,
            DirEntryStatus::Conflict => Color::Rgb(255, 0, 255),
            DirEntryStatus::IdenticalNameDifferentType => {
                Color::Rgb(255, 255, 0)
            }
        };
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    }

    /// Return the text style for a line of the given grammar importance.
    pub fn grammar_style(&self, importance: Importance) -> Style {
        match importance {
            Importance::Code => Style::default(),
            Importance::Data => self.data,
            Importance::Comment | Importance::Ignored => self.dim,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_style_distinguishes_all_statuses() {
        let theme = CocomoTheme::default();
        let statuses = [
            DirEntryStatus::Same,
            DirEntryStatus::SameBinary,
            DirEntryStatus::Similar,
            DirEntryStatus::Different,
            DirEntryStatus::LeftOnly,
            DirEntryStatus::RightOnly,
            DirEntryStatus::CenterOnly,
            DirEntryStatus::Mergeable,
            DirEntryStatus::Conflict,
            DirEntryStatus::IdenticalNameDifferentType,
        ];
        let mut styles: Vec<Style> = statuses
            .iter()
            .copied()
            .map(|s| theme.status_style(s))
            .collect();
        // `Same` and `SameBinary` intentionally share a color; every other
        // status must be visually distinct.
        styles.dedup();
        assert_eq!(styles.len(), statuses.len() - 1);
    }

    #[test]
    fn theme_derives_from_the_app_palette() {
        let theme = CocomoTheme::from_palette(&cocomo_palette());
        assert_eq!(theme.title.fg, Some(Color::Yellow));
        assert_eq!(theme.border.fg, Some(Color::Gray));
        assert_eq!(theme.highlight.bg, Some(Color::DarkGray));
    }
}
