// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Operations waiting for confirmation.

use crate::dialog::Dialog;

/// The operation to be confirmed.
#[derive(Debug)]
pub(crate) enum Op {
    Quit,
}

/// A pending operation waiting for confirmation.
#[derive(Debug)]
pub(crate) struct PendingOp {
    /// The operation to be confirmed.
    op: Op,
    /// The dialog to display the confirmation.
    dialog: Box<dyn Dialog>,
}

impl PendingOp {
    /// Create a new `PendingOp` with the given operation and dialog.
    #[inline(always)]
    pub(crate) fn new(op: Op, dialog: Box<dyn Dialog>) -> Self {
        Self { op, dialog }
    }

    /// Return a reference to the operation to be confirmed.
    #[inline(always)]
    pub(crate) const fn op(&self) -> &Op {
        &self.op
    }

    /// Return a reference to the dialog to display the confirmation.
    #[inline(always)]
    pub(crate) fn dialog(&self) -> &dyn Dialog {
        self.dialog.as_ref()
    }
}
