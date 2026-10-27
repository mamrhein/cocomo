// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Change summary shared by the approval dialogs.
//!
//! [`change_summary`] turns a list of planned [`TransferItem`]s into the
//! lines that describe the change before it is approved: one totals line
//! per distinct action (`move left → right: 3 files, 1 dir`) followed by
//! one row per item (colored action label and root-relative path). The
//! transfer dialog feeds it the preview of the option under the cursor and
//! the sync approval modal the whole planned batch, so both answer the same
//! question — "what will change?" — with the same rendering. Action labels
//! carry [`CocomoTheme::transfer_action_style`], the same colors the
//! sync-preview table uses.

use cocomo_lib::{TransferAction, TransferItem};
use ratatui_kit::ratatui::text::{Line, Span};

use super::theme::CocomoTheme;

/// Maximum number of per-item rows a summary lists before collapsing the
/// rest into a dim `+N more` line.
const MAX_ITEM_LINES: usize = 5;

/// The actions the totals are grouped by, in display order: copies, moves,
/// then deletes, each pointing right-to-left-last so the dominant
/// left → right direction reads first.
const ACTION_ORDER: [TransferAction; 7] = [
    TransferAction::CopyRight,
    TransferAction::CopyLeft,
    TransferAction::CopyCenter,
    TransferAction::MoveRight,
    TransferAction::MoveLeft,
    TransferAction::DeleteRight,
    TransferAction::DeleteLeft,
];

/// Render the changes `items` would apply.
///
/// Empty for an empty item list, so a caller can render the summary
/// unconditionally and simply gets nothing when the highlighted option
/// would not change anything.
pub(super) fn change_summary(
    items: &[TransferItem],
    theme: &CocomoTheme,
) -> Vec<Line<'static>> {
    if items.is_empty() {
        return Vec::new();
    }

    // One totals line per action that actually occurs, in the fixed
    // display order, so the summary reads the same for the same plan.
    let mut lines: Vec<Line<'static>> = ACTION_ORDER
        .iter()
        .filter_map(|&action| {
            let group =
                items.iter().filter(|item| item.action == action).count();
            if group == 0 {
                return None;
            }
            let files = items
                .iter()
                .filter(|i| i.action == action && !i.is_dir)
                .count();
            Some(Line::from(Span::styled(
                format!(
                    "{}: {}",
                    action.label(),
                    count_totals(files, group - files)
                ),
                theme.transfer_action_style(action),
            )))
        })
        .collect();

    // Then the individual rows, labels padded to a common width so the
    // paths form a column, with the tail collapsed.
    let label_width = items
        .iter()
        .map(|item| item.action.label().chars().count())
        .max()
        .unwrap_or(0);
    lines.extend(items.iter().take(MAX_ITEM_LINES).map(|item| {
        let label = item.action.label();
        let pad = " ".repeat(label_width - label.chars().count());
        let dir = if item.is_dir { "\u{1F4C4} " } else { "" };
        Line::from(vec![
            Span::styled(
                format!("{label}{pad}"),
                theme.transfer_action_style(item.action),
            ),
            Span::raw("  "),
            Span::raw(format!("{dir}{}", item.rel_path)),
        ])
    }));
    if let Some(more) = items.len().checked_sub(MAX_ITEM_LINES) {
        lines.push(Line::from(Span::styled(
            format!("+{more} more"),
            theme.dim,
        )));
    }
    lines
}

/// Format `N files, M dirs` totals, omitting the zero parts.
pub(super) fn count_totals(files: usize, dirs: usize) -> String {
    let plural = |count: usize, noun: &str| {
        format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
    };
    let mut parts = Vec::new();
    if files > 0 {
        parts.push(plural(files, "file"));
    }
    if dirs > 0 {
        parts.push(plural(dirs, "dir"));
    }
    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use cocomo_lib::TransferAction::*;

    use super::*;

    fn item(action: TransferAction, name: &str, is_dir: bool) -> TransferItem {
        TransferItem::new(action, name.into(), is_dir, format!("dir/{name}"))
    }

    #[test]
    fn empty_plan_renders_nothing() {
        assert!(change_summary(&[], &CocomoTheme::default()).is_empty());
    }

    #[test]
    fn totals_group_by_action_with_file_and_dir_counts() {
        let items = vec![
            item(CopyRight, "a.txt", false),
            item(CopyRight, "b.txt", false),
            item(CopyRight, "sub", true),
            item(DeleteLeft, "gone.txt", false),
        ];
        let lines = change_summary(&items, &CocomoTheme::default());
        let texts: Vec<String> =
            lines.iter().map(ToString::to_string).collect();
        assert!(texts[0].contains("copy left \u{2192} right: 2 files, 1 dir"));
        assert!(texts[1].contains("delete left: 1 file"));
        // After the two totals lines come the per-item rows.
        assert!(texts[2].contains("dir/a.txt"));
        assert!(texts[4].contains("\u{1F4C4} dir/sub"));
    }

    #[test]
    fn single_counts_are_singular() {
        let lines = change_summary(
            &[item(MoveRight, "a.txt", false)],
            &CocomoTheme::default(),
        );
        assert!(
            lines[0]
                .to_string()
                .contains("move left \u{2192} right: 1 file")
        );
    }

    #[test]
    fn item_rows_collapse_beyond_the_cap() {
        let items: Vec<TransferItem> = (0..MAX_ITEM_LINES + 3)
            .map(|i| item(CopyRight, &format!("f{i}.txt"), false))
            .collect();
        let lines = change_summary(&items, &CocomoTheme::default());
        // 1 totals line + the capped item rows + the collapse line.
        assert_eq!(lines.len(), 1 + MAX_ITEM_LINES + 1);
        assert!(lines.last().unwrap().to_string().contains("+3 more"));
    }
}
