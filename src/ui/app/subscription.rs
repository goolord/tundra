//! Events the app listens for outside any widget, and timers.

use super::App;
use crate::ui::message::{BulkAutoTagMsg, Message, WaveformMsg, WindowMsg};
use futures::StreamExt;
use iced::event::{self, Event};
use iced::{Subscription, keyboard, mouse, window};
use std::time::Duration;

/// A message every `millis` milliseconds.
fn every(millis: u64) -> Subscription<()> {
    Subscription::run_with(millis, |&millis| async_io::Timer::interval(Duration::from_millis(millis)).map(|_| ()))
}

fn global_event(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Window(window::Event::FileDropped(path)) => Some(Message::FileDropped(path)),
        Event::Window(window::Event::FileHovered(path)) => Some(Message::FileHovered(path)),
        Event::Window(window::Event::FilesHoveredLeft) => Some(Message::FilesHoverLeft),
        Event::Mouse(mouse::Event::CursorMoved { position }) => Some(Message::CursorMoved(position)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(Message::MouseReleased),
        Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => Some(Message::ModifiersChanged(modifiers)),
        // Shortcuts only see keys no focused widget took.
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, repeat: false, .. })
            if status == event::Status::Ignored =>
        {
            Some(Message::KeyPressed(key, modifiers))
        }
        _ => None,
    }
}

impl App {
    pub fn subscription(&self) -> Subscription<Message> {
        let waveform = self.player.waveform.as_ref();
        let springing =
            waveform.is_some_and(|waveform| waveform.view_state().overscroll_active() && !waveform.pan_active());
        let (dragging, bulk_running) = (self.native_drag.is_active(), self.bulk_auto_tag.job.is_some());
        let timers = [
            // The waveform animates its own playhead; this only refreshes the time label.
            (waveform.is_some() && self.player.is_playing()).then(|| every(250).map(|()| Message::PlaybackTick)),
            dragging.then(|| every(16).map(|()| Message::FileDragTick)),
            springing.then(|| every(16).map(|()| WaveformMsg::SpringTick.into())),
            bulk_running.then(|| every(100).map(|()| BulkAutoTagMsg::ProgressTick.into())),
        ];
        let events = [
            event::listen_with(global_event),
            window::close_requests().map(|_| Message::Quit),
            window::resize_events().map(|_| WindowMsg::SyncMaximized.into()),
        ];
        Subscription::batch(events.into_iter().chain(timers.into_iter().flatten()))
    }
}
