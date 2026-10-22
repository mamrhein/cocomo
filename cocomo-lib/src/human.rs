// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Human-readable formatting shared by the frontends.
//!
//! The TUI, GUI, and CLI all render byte counts and modification timestamps
//! the same way; keeping the helpers here removes the per-frontend copies and
//! guarantees the three surfaces never drift apart.

/// Format a byte count as a human-readable size string using binary units.
///
/// Values below 1 KiB are shown verbatim with a `B` suffix; larger values are
/// scaled to `K`, `M`, or `G` with one fractional digit.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Truncate an ISO-8601 date string to `"YYYY-MM-DD HH:MM"`.
///
/// Strings at or below the `YYYY-MM-DDTHH:MM` length pass through unchanged
/// (only the `T` separator is replaced with a space when the string is long
/// enough to carry a time component).
pub fn format_date(date_str: &str) -> String {
    if date_str.len() > 16 {
        date_str[..16].replace('T', " ")
    } else {
        date_str.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_uses_binary_units() {
        assert_eq!(format_size(0), "0B");
        assert_eq!(format_size(999), "999B");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1536), "1.5K");
        assert_eq!(format_size(3 * 1024 * 1024), "3.0M");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.0G");
    }

    #[test]
    fn format_date_truncates_to_minutes() {
        assert_eq!(
            format_date("2026-10-22T09:47:08+02:00"),
            "2026-10-22 09:47"
        );
        // Strings at or below the cut length pass through unchanged.
        assert_eq!(format_date("2026-10-22T09:47"), "2026-10-22T09:47");
        assert_eq!(format_date(""), "");
    }
}
