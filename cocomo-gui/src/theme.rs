// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Light/dark theme application.
//!
//! gpui-component knows only the `Light` and `Dark` [`ThemeMode`]s;
//! "follow the OS" is an application-level concern. This module resolves
//! the persisted [`ThemePreference`] against the appearance GPUI reports
//! for the platform and keeps it applied:
//!
//! - once at startup and on every Options apply ([`apply`]),
//! - per window on creation ([`sync_if_system`], the Linux-safe source per
//!   gpui-kit issue #104), and
//! - on every OS light/dark switch, via the window appearance observer
//!   installed in `WindowRoot` that calls [`sync_if_system`].
//!
//! A fixed `Light`/`Dark` choice also forces the native window chrome
//! (macOS) so the titlebar matches the theme; while forced, GPUI stops
//! reporting OS switches for that app, which is exactly the semantics a
//! manual override wants. `System` clears the force and resumes tracking.

use gpui::{
    App, Window, WindowAppearance,
    component::{Theme, ThemeMode},
};

use crate::options::{AppOptions, ThemePreference};

/// Apply the persisted theme preference to the component theme.
pub fn apply(cx: &mut App) {
    apply_preference(cx.global::<AppOptions>().theme, cx);
}

/// Apply one theme preference.
pub fn apply_preference(preference: ThemePreference, cx: &mut App) {
    match preference {
        ThemePreference::System => {
            // Resume following the OS for the native chrome, then adopt
            // the platform appearance for the component theme.
            cx.set_window_appearance(None);
            Theme::sync_system_appearance(None, cx);
        }
        ThemePreference::Light => {
            Theme::change(ThemeMode::Light, None, cx);
            cx.set_window_appearance(Some(WindowAppearance::Light));
        }
        ThemePreference::Dark => {
            Theme::change(ThemeMode::Dark, None, cx);
            cx.set_window_appearance(Some(WindowAppearance::Dark));
        }
    }
}

/// Re-sync the theme with the OS appearance reported by `window`.
///
/// Only acts while the preference tracks the system; a manual choice
/// must survive the OS flipping light/dark underneath it.
pub fn sync_if_system(window: &mut Window, cx: &mut App) {
    if cx.global::<AppOptions>().theme == ThemePreference::System {
        Theme::sync_system_appearance(Some(window), cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, component::ActiveTheme as _};

    use super::*;

    #[gpui::test]
    fn manual_preferences_set_the_theme_mode(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui::init(cx);
            cx.set_global(AppOptions::default());

            apply_preference(ThemePreference::Dark, cx);
            assert!(cx.theme().is_dark());

            apply_preference(ThemePreference::Light, cx);
            assert!(!cx.theme().is_dark());
        });
    }

    #[gpui::test]
    fn applying_the_global_preference_uses_the_stored_choice(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui::init(cx);
            cx.set_global(AppOptions {
                theme: ThemePreference::Dark,
                ..AppOptions::default()
            });

            apply(cx);
            assert!(cx.theme().is_dark());
        });
    }
}
