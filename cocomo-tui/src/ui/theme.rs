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

use cocomo_lib::{DirEntryStatus, Importance, StatusColor, TransferAction};
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
    /// Selection-background style for the selected table row. Style it
    /// with the background only: the native table paints this style *over*
    /// the already-rendered row, so a foreground (even `Reset`) would
    /// overwrite the colors the cells bring themselves.
    pub highlight: Style,
    /// Selection-background style for the rows a drag selection spans
    /// between its anchor and the cursor (same background as `highlight`,
    /// so a dragged range reads as one continuous selection).
    pub range: Style,
    /// Bold style for the row under the mouse pointer.
    pub hover: Style,
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
    /// Color of delete transfer actions.
    pub transfer_delete: Color,
    /// Color of move transfer actions.
    pub transfer_move: Color,
    /// Status colors for directory entries.
    pub status: StatusColors,
}

/// Status colors for directory entries.
///
/// The three roles mirror [`StatusColor`] in `cocomo-lib`: every status
/// bitmap resolves to exactly one of them through `DirEntryStatus::symbol`
/// (the color component), so a cell never has to know which bits it carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct StatusColors {
    /// Gray for identical or equal entries.
    pub equal: Color,
    /// Red for entries that differ between the sides.
    pub different: Color,
    /// Blue for entries that only exist on one side.
    pub orphan: Color,
}

impl StatusColors {
    fn from_palette(palette: &Palette) -> Self {
        Self {
            equal: Color::Gray,
            different: palette.error,
            // Orphans are one-sided endpoints, not an error state, so they
            // get a fixed blue instead of a palette role.
            orphan: Color::Blue,
        }
    }
}

impl ComponentTheme for CocomoTheme {
    fn from_palette(palette: &Palette) -> Self {
        Self {
            border: Style::new().fg(palette.border),
            title: Style::new()
                .fg(palette.accent)
                .add_modifier(Modifier::BOLD),
            highlight: Style::new().bg(palette.selection),
            range: Style::new().bg(palette.selection),
            hover: Style::new().add_modifier(Modifier::BOLD),
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
            transfer_delete: palette.error,
            transfer_move: Color::Magenta,
            status: StatusColors::from_palette(palette),
        }
    }
}

impl Default for CocomoTheme {
    fn default() -> Self {
        Self::from_palette(&Palette::default())
    }
}

impl CocomoTheme {
    /// Return the bold status style for a directory entry status bitmap.
    ///
    /// The color follows the role the library resolves via
    /// [`DirEntryStatus::symbol`], so every bitmap (including aggregated
    /// directory ones) maps to exactly one of the three [`StatusColors`].
    pub fn status_style(&self, status: DirEntryStatus) -> Style {
        let color = match status.symbol().1 {
            StatusColor::Equal => self.status.equal,
            StatusColor::Different => self.status.different,
            StatusColor::Orphan => self.status.orphan,
        };
        Style::new().fg(color).add_modifier(Modifier::BOLD)
    }

    /// Return the bold style for a transfer action label.
    ///
    /// Shared by the sync-preview table and the approval-dialog change
    /// summaries, so an action reads in the same color everywhere.
    pub fn transfer_action_style(&self, action: TransferAction) -> Style {
        let color = match action {
            TransferAction::CopyLeft | TransferAction::CopyRight => {
                self.transfer_copy
            }
            TransferAction::DeleteLeft | TransferAction::DeleteRight => {
                self.transfer_delete
            }
            TransferAction::MoveLeft | TransferAction::MoveRight => {
                self.transfer_move
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
    fn status_style_maps_the_three_color_roles() {
        let theme = CocomoTheme::default();
        for status in [DirEntryStatus::Same, DirEntryStatus::Equal] {
            assert_eq!(
                theme.status_style(status).fg,
                Some(theme.status.equal)
            );
        }
        for status in [
            DirEntryStatus::LeftNewer,
            DirEntryStatus::RightNewer,
            DirEntryStatus::DIFFERENT,
            DirEntryStatus::Equal.union(DirEntryStatus::LeftOrphan),
        ] {
            assert_eq!(
                theme.status_style(status).fg,
                Some(theme.status.different)
            );
        }
        for status in [DirEntryStatus::LeftOrphan, DirEntryStatus::RightOrphan]
        {
            assert_eq!(
                theme.status_style(status).fg,
                Some(theme.status.orphan)
            );
        }
        // The three roles are visually distinct from each other.
        assert_ne!(theme.status.equal, theme.status.different);
        assert_ne!(theme.status.different, theme.status.orphan);
        assert_ne!(theme.status.equal, theme.status.orphan);
    }

    #[test]
    fn theme_derives_from_the_app_palette() {
        let palette = cocomo_palette();
        let theme = CocomoTheme::from_palette(&palette);
        assert_eq!(theme.title.fg, Some(Color::Yellow));
        assert_eq!(theme.border.fg, Some(Color::Gray));
        assert_eq!(theme.highlight.bg, Some(Color::DarkGray));
        // The selection marks the row by background only: the table paints
        // the highlight over the rendered row, so a foreground there would
        // overwrite each cell's own color.
        assert_eq!(theme.highlight.fg, None);
        assert!(!theme.highlight.add_modifier.contains(Modifier::REVERSED));
        // The Different role follows the palette's error color.
        assert_eq!(
            theme.status_style(DirEntryStatus::LeftNewer).fg,
            Some(palette.error)
        );
    }
}
