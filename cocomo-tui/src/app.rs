// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Application Module (`app`)
//!
//! This module contains the main application state and logic. It handles
//! events, manages views (tabs), and drives the main loop.

use std::convert::From;

use cocomo_core::FSItem;
use ratatui::{
    DefaultTerminal,
    buffer::Buffer,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::Text,
    widgets::{Tabs, Widget},
};

use crate::{
    dialog::SimpleConfirm,
    dirview::DirView,
    event::{AppEvent, Event, EventQueue, EventThread},
    keymap::{KeyHint, KeyMap, KeyMapItem, KeyMapper},
    pending_op::{Op, PendingOp},
    textview::TextView,
    view::NavigableView,
};

/// Container for items currently being compared.
#[derive(Debug, Default)]
pub(crate) struct CmpItems {
    /// Left side item.
    pub left: Option<FSItem>,
    /// Right side item.
    pub right: Option<FSItem>,
}

/// Views available in the application.
pub(crate) type AppView = Box<dyn NavigableView>;

/// Pre-built key map items for the `App`.
#[rustfmt::skip]
const APP_KEYMAP_ITEMS: [KeyMapItem; 6] = [
    KeyMapItem::new(
        KeyCode::Char('?'),
        None,
        "Show/hide hints",
        true,
        Event::App(AppEvent::ToggleHints),
    ),
    KeyMapItem::new(
        KeyCode::Char('q'),
        None,
        "Quit",
        true,
        Event::App(AppEvent::Quit),
    ),
    KeyMapItem::new(
        KeyCode::Enter,
        Some(KeyCode::Char('o')),
        "Open view",
        true,
        Event::App(AppEvent::OpenView),
    ),
    KeyMapItem::new(
        KeyCode::Char('x'),
        None,
        "Close tab",
        true,
        Event::App(AppEvent::CloseTab),
    ),
    KeyMapItem::new(
        KeyCode::Tab,
        None,
        "Next tab",
        true,
        Event::App(AppEvent::NextTab),
    ),
    KeyMapItem::new(
        KeyCode::BackTab,
        None,
        "Prev tab",
        true,
        Event::App(AppEvent::PrevTab),
    ),
];

/// Main application state.
#[derive(Debug)]
pub(crate) struct App {
    /// Flag indicating if the application is running.
    running: bool,
    /// Event queue for the application.
    events: EventQueue,
    /// App level key map
    keymap: KeyMap,
    /// Open views (tabs).
    views: Vec<AppView>,
    /// Index of the currently active view.
    active_view: usize,
    /// Flag to show key hints.
    show_key_hints: bool,
    /// Operation waiting for confirmation.
    pending_op: Option<PendingOp>,
}

impl App {
    /// Constructs a new instance of [`App`].
    pub(crate) async fn new(
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> color_eyre::Result<Self> {
        let mut app = Self {
            running: false,
            events: EventQueue::default(),
            keymap: KeyMap::from(APP_KEYMAP_ITEMS.as_slice()),
            views: vec![],
            active_view: 0,
            show_key_hints: false,
            pending_op: None,
        };
        app.new_view(left_item, right_item).await?;
        Ok(app)
    }

    /// Returns the active view.
    pub(crate) fn current_view(&self) -> &AppView {
        self.views.get(self.active_view).unwrap()
    }

    /// Returns a mutable reference to the active view.
    pub(crate) fn current_view_mut(&mut self) -> &mut AppView {
        self.views.get_mut(self.active_view).unwrap()
    }

    /// Creates a new app view.
    async fn new_view(
        &mut self,
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> color_eyre::Result<()> {
        debug_assert!(left_item.is_some() || right_item.is_some());
        let view: AppView = match (left_item, right_item) {
            (Some(left), _) => {
                if left.is_dir() {
                    Box::new(DirView::new(left_item, right_item).await?)
                } else {
                    Box::new(TextView::new(left_item, right_item).await?)
                }
            }
            (_, Some(right)) => {
                if right.is_dir() {
                    Box::new(DirView::new(left_item, right_item).await?)
                } else {
                    Box::new(TextView::new(left_item, right_item).await?)
                }
            }
            _ => unreachable!(),
        };
        self.views.push(view);
        self.active_view = self.views.len() - 1;
        Ok(())
    }

    /// Run the application's main loop.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal drawing or event handling fails.
    pub(crate) async fn run(
        &mut self,
        mut terminal: DefaultTerminal,
    ) -> color_eyre::Result<()> {
        self.running = true;
        while self.running {
            terminal
                .draw(|frame| frame.render_widget(&*self, frame.area()))?;
            match self.events.dequeue().await? {
                Event::Tick => self.tick(),
                Event::Crossterm(event) => match event {
                    crossterm::event::Event::Key(key_event)
                        if key_event.kind
                            == crossterm::event::KeyEventKind::Press =>
                    {
                        self.handle_key_event(key_event).await?;
                    }
                    _ => {}
                },
                Event::App(app_event) => {
                    self.handle_app_event(app_event).await?;
                }
                Event::Nav(nav_event) => {
                    self.current_view_mut().handle_nav_event(nav_event)?;
                }
                Event::Op(op_event) => {
                    self.current_view_mut().handle_op_event(op_event)?;
                }
            };
        }
        Ok(())
    }

    /// Handles the key events and updates the state of [`App`].
    ///
    /// # Errors
    ///
    /// Returns an error if an application event cannot be sent.
    async fn handle_key_event(
        &mut self,
        key_event: KeyEvent,
    ) -> color_eyre::Result<()> {
        if let Some(pending_op) = self.pending_op.as_mut() {
            return pending_op.dialog().handle_key_event(key_event);
        };
        if let Some(event) = self.keymap.map_key_code(key_event.code) {
            self.events.enqueue(event);
            return Ok(());
        }
        // Forward key events that are not handled by the keymap to the
        // current view
        self.current_view_mut().handle_key_event(key_event)
    }

    /// Handles the tick event of the terminal.
    ///
    /// The tick event is where you can update the state of your application
    /// with any logic that needs to be updated at a fixed frame rate. E.g.
    /// polling a server, updating an animation.
    #[allow(clippy::unused_self)]
    pub const fn tick(&self) {}

    /// Set running to false to quit the application.
    pub const fn quit(&mut self) {
        self.running = false;
    }

    /// Closes the current tab.
    pub fn close_tab(&mut self) {
        if self.views.len() == 1 {
            let msg = "Close last tab and quit?";
            let confirm = Box::new(SimpleConfirm::new(
                "",
                msg,
                EventThread::from(&self.events),
            ));
            self.pending_op = Some(PendingOp::new(Op::Quit, confirm));
            return;
        }
        self.views.remove(self.active_view);
        if self.active_view >= self.views.len() {
            self.active_view = self.views.len().saturating_sub(1);
        }
    }

    /// Handles application events from the event channel.
    async fn handle_app_event(
        &mut self,
        app_event: AppEvent,
    ) -> color_eyre::Result<()> {
        match app_event {
            AppEvent::OpenView => {
                if let Some(item) = self.current_view().current_diff_item() {
                    let left_item = item.left_item.clone();
                    let right_item = item.right_item.clone();
                    self.new_view(&left_item, &right_item).await?;
                };
            }
            AppEvent::CloseTab => self.close_tab(),
            AppEvent::NextTab => {
                if self.active_view < self.views.len() - 1 {
                    self.active_view += 1;
                }
            }
            AppEvent::PrevTab => {
                if self.active_view > 0 {
                    self.active_view -= 1;
                }
            }
            AppEvent::Confirmed => {
                if let Some(pending_op) = &self.pending_op {
                    match pending_op.op() {
                        Op::Quit => self.quit(),
                    }
                    self.pending_op = None;
                }
            }
            AppEvent::NotConfirmed => {
                self.pending_op = None;
            }
            AppEvent::ToggleHints => {
                self.show_key_hints = !self.show_key_hints;
            }
            AppEvent::Quit => self.quit(),
            _ => unreachable!(), // should never happen!
        }
        Ok(())
    }
}

impl KeyHint for App {
    #[inline(always)]
    fn key_hint(&self) -> Text<'_> {
        Text::from(&self.keymap)
    }
}

impl KeyMapper for App {
    #[inline(always)]
    fn keymapper(&self) -> &dyn KeyMapper {
        &self.keymap
    }
}

impl Widget for &App {
    /// Renders the user interface widgets.
    fn render(self, area: Rect, buf: &mut Buffer) {
        let current_view = self.current_view();
        // Create layout
        let vert_constraints = [
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(if self.show_key_hints { 3 } else { 0 }),
        ];
        let [tab_bar, main_view, key_bar] =
            Layout::vertical(vert_constraints).areas(area);

        // Render key hints
        if self.show_key_hints {
            let mut txt = self.key_hint();
            txt.extend(current_view.key_hint());
            txt.centered().render(key_bar, buf);
        }

        let titles: Vec<String> =
            self.views.iter().map(|view| view.title()).collect();

        Tabs::new(titles)
            .select(self.active_view)
            .highlight_style(Style::default().fg(Color::Yellow).bold())
            .divider("|")
            .render(tab_bar, buf);

        // Render current view
        current_view.render_ref(main_view, buf);

        if let Some(pending_op) = &self.pending_op {
            pending_op.dialog().render_ref(area, buf);
        }
    }
}
