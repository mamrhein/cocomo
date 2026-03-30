// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Shared behavior for interactive views.

use core::fmt::Debug;

use cocomo_core::DiffItem;
use ratatui::widgets::WidgetRef;

use crate::appevent::AppEvent;

/// Common trait for all views
pub(crate) trait View: Debug + WidgetRef {
    /// Returns the title of the view.
    fn title(&self) -> String;

    /// Returns `true` if the view is a directory view.
    fn is_dir_view(&self) -> bool {
        // There will only be one directory view but several file views.
        false
    }

    /// Returns `true` if the view is a file view.
    fn is_file_view(&self) -> bool {
        // There will only be one directory view but several file views.
        true
    }

    /// Returns the current diff item, if any.
    fn current_diff_item(&self) -> Option<&DiffItem> {
        None
    }

    /// Handles an application event.
    fn handle_app_event(
        &mut self,
        app_event: AppEvent,
    ) -> color_eyre::Result<()>;
}

/// Trait for views that support cursor-style navigation.
pub(crate) trait NavigableView: View {
    /// Makes the previous logical item the current item.
    fn prev(&mut self);

    /// Makes the next logical item the current item.
    fn next(&mut self);

    /// Makes the first logical item the current item.
    fn home(&mut self);

    /// Makes the last logical item the current item.
    fn end(&mut self);
}
