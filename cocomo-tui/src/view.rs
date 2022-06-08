// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use tui::{
    backend::Backend,
    layout::{Constraint, Rect},
    Frame,
};

pub(crate) trait View {
    fn area(&self) -> Rect;
    fn set_area(&mut self, area: Rect) -> &Self;
    fn want_layout(&self) -> Constraint;
    fn draw<B: Backend>(&self, frame: &mut Frame<B>);
}
