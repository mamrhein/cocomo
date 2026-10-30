// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Application asset source.
//!
//! GPUI resolves every path-based icon through the registered
//! [`AssetSource`]; without one all icons — and therefore all icon-only
//! buttons — stay blank. This module composes the three sources the app
//! needs into a single one:
//!
//! 1. the app's own SVGs from `resources/icons` (the display-filter preset
//!    glyphs),
//! 2. the Lucide catalog icons the UI uses beyond the default component
//!    bundle, selected at compile time via `icon_assets!`,
//! 3. the default component bundle for everything the framework itself
//!    renders.
//!
//! Lookup order matters: the built-in bundle reports an unknown path as
//! an error rather than `None`, so the app-owned sources must be
//! consulted before it.

use std::borrow::Cow;

use gpui::{
    AssetSource, Result, SharedString,
    assets::{Assets as ComponentAssets, icon_assets},
};
use rust_embed::RustEmbed;

/// The app's own icons, embedded from `resources/` at build time.
///
/// Keys are relative to the embedded folder root, e.g.
/// `icons/left-newer.svg`.
#[derive(RustEmbed)]
#[folder = "./resources"]
#[include = "icons/**/*.svg"]
struct AppIcons;

// Lucide catalog icons used by the UI that the default component bundle
// does not carry.
//
// The macro resolves each identifier against the catalog at compile
// time, so a stale or misspelled name fails the build instead of
// rendering blank at runtime.
icon_assets!(
    ExtraIcons,
    [
        ArrowLeftRight,
        ArrowUpToLine,
        Binary,
        CircleQuestionMark,
        CircleStop,
        Database,
        EyeClosed,
        FileDiff,
        FolderSync,
        Funnel,
        Gavel,
        House,
        Image,
        Merge,
        PanelBottomClose,
        Pencil,
        Save,
        Table,
        TextWrap,
        ChevronsDown,
        ChevronsUp,
    ]
);

/// Composite source registered with `with_assets` at startup.
#[derive(Clone, Copy, Debug)]
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }
        if let Some(file) = AppIcons::get(path) {
            return Ok(Some(file.data));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        ComponentAssets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = ComponentAssets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(
            AppIcons::iter()
                .filter_map(|p| p.starts_with(path).then(|| p.into())),
        );
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset glyph shipped in `resources/icons`.
    const APP_ICON_PATHS: [&str; 11] = [
        "icons/diff-no-orphans.svg",
        "icons/left-newer-and-orphans.svg",
        "icons/left-newer.svg",
        "icons/left-orphans.svg",
        "icons/no-orphans.svg",
        "icons/no-same.svg",
        "icons/orphans-only.svg",
        "icons/right-newer-and-orphans.svg",
        "icons/right-newer.svg",
        "icons/right-orphans.svg",
        "icons/same-only.svg",
    ];

    #[test]
    fn app_icons_are_served() {
        for path in APP_ICON_PATHS {
            let bytes = AppAssets
                .load(path)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(bytes.is_some(), "{path} is not embedded");
        }
    }

    #[test]
    fn extra_catalog_icons_are_served() {
        for path in [
            "icons/house.svg",
            "icons/gavel.svg",
            "icons/funnel.svg",
            "icons/panel-bottom-close.svg",
            "icons/circle-question-mark.svg",
        ] {
            let bytes = AppAssets
                .load(path)
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(bytes.is_some(), "{path} is not selected");
        }
    }

    #[test]
    fn default_component_icons_still_resolve() {
        assert!(AppAssets.load("icons/eye.svg").unwrap().is_some());
        assert!(AppAssets.load("icons/close.svg").unwrap().is_some());
    }

    #[test]
    fn empty_path_is_none_and_unknown_path_errors() {
        assert!(AppAssets.load("").unwrap().is_none());
        assert!(AppAssets.load("icons/no-such-icon.svg").is_err());
    }

    #[test]
    fn list_exposes_app_extra_and_default_icons() {
        let paths = AppAssets.list("icons/").unwrap();
        for path in APP_ICON_PATHS {
            assert!(paths.iter().any(|p| p == path), "{path} missing");
        }
        assert!(paths.iter().any(|p| p == "icons/house.svg"));
        assert!(paths.iter().any(|p| p == "icons/eye.svg"));
    }
}
