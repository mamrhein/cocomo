// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Event Handling Module (`event`)
//!
//! This module provides the infrastructure for handling terminal events
//! (via `crossterm`) and custom application events using an asynchronous
//! event loop.

use std::time::Duration;

use color_eyre::eyre::OptionExt;
use futures::{FutureExt, StreamExt};
use ratatui::crossterm::event::Event as CrosstermEvent;
use tokio::sync::mpsc;

/// Handles the terminal events (key press, mouse click, resize, etc.).
/// The frequency at which tick events are emitted.
const TICK_FPS: f64 = 30.0;

/// Representation of all possible events.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    /// An event that is emitted on a regular schedule.
    ///
    /// Use this event to run any code which has to run outside of being a
    /// direct response to a user event. e.g. polling exernal systems,
    /// updating animations, or rendering the UI based on a fixed frame
    /// rate.
    Tick,
    /// Crossterm events.
    ///
    /// These events are emitted by the terminal.
    Crossterm(CrosstermEvent),
    /// Application level events.
    ///
    /// Use this to emit events that are to be handled by the application.
    App(AppEvent),
    /// Navigation events.
    ///
    /// Use this to emit events that are to be handled by the app's current view's
    /// navigation system.
    Nav(NavEvent),
    /// Operation triggers.
    ///
    /// Use this to emit events that are to be handled by the app's current view
    /// to trigger operations.
    Op(OpEvent),
}

/// Application level events.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum AppEvent {
    /// Toggle the visibility of the key hints.
    ToggleHints,
    /// Quit the application.
    Quit,
    /// Open a new comparison view.
    OpenView,
    /// Close the current tab.
    CloseTab,
    /// Switch to the next tab.
    NextTab,
    /// Switch to the previous tab.
    PrevTab,
    /// Refresh the current view.
    Refresh,
    /// Confirm an action.
    Confirmed,
    /// Cancel an action.
    NotConfirmed,
}

/// Navigation events.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum NavEvent {
    /// Navigate the current item of the current view to the previous item.
    Prev,
    /// Navigate the current item of the current view to the next item.
    Next,
    /// Navigate the current item of the current view to the first item.
    First,
    /// Navigate the current item of the current view to the last item.
    Last,
}

/// Operation triggers.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(crate) enum OpEvent {
    /// Copy the current item to the other side.
    Copy,
    /// Move the current item to the other side.
    Move,
    /// Delete the current item.
    Delete,
    /// Rename the current item.
    Rename,
    /// Refresh the view.
    Refresh,
}

/// Terminal event handler.
#[derive(Debug)]
pub struct EventQueue {
    /// Event sender channel.
    sender: mpsc::UnboundedSender<Event>,
    /// Event receiver channel.
    receiver: mpsc::UnboundedReceiver<Event>,
}

impl EventQueue {
    /// Constructs a new instance of [`EventQueue`] and spawns a new thread
    /// to handle the queued events.
    #[must_use]
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        let actor = EventThread::new(sender.clone());
        tokio::spawn(async { actor.run().await });
        Self { sender, receiver }
    }

    /// Dequeues an event from the queue.
    ///
    /// This function blocks until an event is available.
    ///
    /// # Errors
    ///
    /// This function returns an error if the sender channel is disconnected.
    /// This can happen if an error occurs in the event thread. In
    /// practice, this should not happen unless there is a problem with the
    /// underlying terminal.
    pub(crate) async fn dequeue(&mut self) -> color_eyre::Result<Event> {
        self.receiver
            .recv()
            .await
            .ok_or_eyre("Failed to receive event")
    }

    /// Enqueues an event to the queue.
    ///
    /// This is useful for sending events to the event handler which will be
    /// processed by the next iteration of the application's event loop.
    pub(crate) fn enqueue(&mut self, event: Event) {
        // Ignore the result as the reciever cannot be dropped while this
        // struct still has a reference to it
        let _ = self.sender.send(event);
    }
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// A thread that handles reading crossterm events and emitting tick events on
/// a regular schedule.
#[derive(Debug)]
pub(crate) struct EventThread {
    /// Event sender channel.
    sender: mpsc::UnboundedSender<Event>,
}

impl EventThread {
    /// Constructs a new instance of [`EventThread`].
    pub(crate) const fn new(sender: mpsc::UnboundedSender<Event>) -> Self {
        Self { sender }
    }

    /// Runs the event thread.
    ///
    /// This function emits tick events at a fixed rate and polls for crossterm
    /// events in between.
    pub(crate) async fn run(self) -> color_eyre::Result<()> {
        let tick_rate = Duration::from_secs_f64(1.0 / TICK_FPS);
        let mut reader = crossterm::event::EventStream::new();
        let mut tick = tokio::time::interval(tick_rate);
        loop {
            let tick_delay = tick.tick();
            let crossterm_event = reader.next().fuse();
            tokio::select! {
              _ = self.sender.closed() => {
                break;
              }
              _ = tick_delay => {
                self.send(Event::Tick);
              }
              Some(Ok(evt)) = crossterm_event => {
                self.send(Event::Crossterm(evt));
              }
            }
        }
        Ok(())
    }

    /// Sends an event to the receiver.
    pub(crate) fn send(&self, event: Event) {
        // Ignores the result because shutting down the app drops the receiver,
        // which causes the send operation to fail. This is expected
        // behavior and should not panic.
        let _ = self.sender.send(event);
    }
}
