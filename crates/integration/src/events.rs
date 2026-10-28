use std::{
    collections::VecDeque,
    mem::size_of,
    sync::{Arc, Mutex},
};

use blair_protocol::{CompositorEvent, WindowId};
use tokio::sync::Notify;

pub const EVENT_CAPACITY: usize = 512;
pub const EVENT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResult {
    Queued,
    Coalesced,
    Reset,
    Suppressed,
    Closed,
}

#[derive(Debug, PartialEq)]
pub enum EventItem {
    Event(CompositorEvent),
    Reset,
}

#[derive(Default)]
struct Buffer {
    events: VecDeque<CompositorEvent>,
    bytes: usize,
    reset_pending: bool,
    producer_closed: bool,
    receiver_closed: bool,
}

#[derive(Default)]
struct Shared {
    buffer: Mutex<Buffer>,
    ready: Notify,
    closed: Notify,
}

struct Producer {
    shared: Arc<Shared>,
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.shared.buffer.lock().unwrap().producer_closed = true;
        self.shared.ready.notify_one();
    }
}

#[derive(Clone)]
pub struct EventSender {
    producer: Arc<Producer>,
}

pub struct EventReceiver {
    shared: Arc<Shared>,
}

pub fn channel() -> (EventSender, EventReceiver) {
    let shared = Arc::new(Shared::default());
    (
        EventSender {
            producer: Arc::new(Producer {
                shared: shared.clone(),
            }),
        },
        EventReceiver { shared },
    )
}

impl EventSender {
    /// Coalesces updates within lifecycle barriers; overflow discards the backlog and emits a reset.
    pub fn send(&self, event: CompositorEvent) -> SendResult {
        let shared = &self.producer.shared;
        let mut buffer = shared.buffer.lock().unwrap();
        if buffer.receiver_closed {
            return SendResult::Closed;
        }
        if buffer.reset_pending {
            return SendResult::Suppressed;
        }
        let bytes = retained_bytes(&event);
        let previous = key(&event).and_then(|key| {
            buffer
                .events
                .iter()
                .enumerate()
                .rev()
                .take_while(|(_, event)| self::key(event).is_some())
                .find(|(_, event)| self::key(event).as_ref() == Some(&key))
                .map(|(index, _)| index)
        });
        if let Some(index) = previous {
            let previous = buffer.events.remove(index).unwrap();
            buffer.bytes -= retained_bytes(&previous);
        }
        let result = if bytes > MAX_EVENT_BYTES
            || buffer.bytes.saturating_add(bytes) > EVENT_BYTES
            || buffer.events.len() == EVENT_CAPACITY
        {
            buffer.events.clear();
            buffer.bytes = 0;
            buffer.reset_pending = true;
            SendResult::Reset
        } else {
            buffer.events.push_back(event);
            buffer.bytes += bytes;
            if previous.is_some() {
                SendResult::Coalesced
            } else {
                SendResult::Queued
            }
        };
        drop(buffer);
        shared.ready.notify_one();
        result
    }

    pub fn reset(&self) {
        let shared = &self.producer.shared;
        let mut buffer = shared.buffer.lock().unwrap();
        if buffer.receiver_closed {
            return;
        }
        buffer.events.clear();
        buffer.bytes = 0;
        buffer.reset_pending = true;
        drop(buffer);
        shared.ready.notify_one();
    }

    pub async fn closed(&self) {
        let shared = &self.producer.shared;
        let notified = shared.closed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if shared.buffer.lock().unwrap().receiver_closed {
            return;
        }
        notified.await;
    }
}

impl EventReceiver {
    pub async fn recv(&mut self) -> Option<EventItem> {
        loop {
            let notified = self.shared.ready.notified();
            {
                let mut buffer = self.shared.buffer.lock().unwrap();
                if std::mem::take(&mut buffer.reset_pending) {
                    return Some(EventItem::Reset);
                }
                if let Some(event) = buffer.events.pop_front() {
                    buffer.bytes -= retained_bytes(&event);
                    return Some(EventItem::Event(event));
                }
                if buffer.producer_closed {
                    return None;
                }
            }
            notified.await;
        }
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        let mut buffer = self.shared.buffer.lock().unwrap();
        buffer.receiver_closed = true;
        buffer.events.clear();
        buffer.bytes = 0;
        buffer.reset_pending = false;
        drop(buffer);
        self.shared.closed.notify_waiters();
    }
}

#[derive(PartialEq, Eq)]
enum Key<'a> {
    Title(WindowId),
    AppId(WindowId),
    Geometry(WindowId),
    WorkArea(&'a str),
}

fn key(event: &CompositorEvent) -> Option<Key<'_>> {
    match event {
        CompositorEvent::WindowTitleChanged { id, .. } => Some(Key::Title(*id)),
        CompositorEvent::WindowAppIdChanged { id, .. } => Some(Key::AppId(*id)),
        CompositorEvent::WindowGeometryChanged { id, .. } => Some(Key::Geometry(*id)),
        CompositorEvent::WorkAreaChanged { output, .. } => Some(Key::WorkArea(output)),
        _ => None,
    }
}

fn retained_bytes(event: &CompositorEvent) -> usize {
    let payload = match event {
        CompositorEvent::WindowOpened { title, app_id, .. } => title
            .capacity()
            .saturating_add(app_id.as_ref().map_or(0, String::capacity)),
        CompositorEvent::WindowTitleChanged { title, .. } => title.capacity(),
        CompositorEvent::WindowAppIdChanged { app_id, .. } => app_id.capacity(),
        CompositorEvent::OutputAdded { name } | CompositorEvent::OutputRemoved { name } => {
            name.capacity()
        }
        CompositorEvent::WorkAreaChanged { output, .. }
        | CompositorEvent::WorkspaceActivated { output, .. } => output.capacity(),
        CompositorEvent::ShortcutActivated { client, id } => {
            client.capacity().saturating_add(id.capacity())
        }
        CompositorEvent::WindowClosed { .. }
        | CompositorEvent::WindowFocused { .. }
        | CompositorEvent::FocusCleared
        | CompositorEvent::WindowGeometryChanged { .. }
        | CompositorEvent::WindowMinimized { .. }
        | CompositorEvent::WindowRestored { .. }
        | CompositorEvent::WindowMaximized { .. }
        | CompositorEvent::WorkspacesChanged
        | CompositorEvent::ConfigurationChanged => 0,
    };
    size_of::<CompositorEvent>().saturating_add(payload)
}

#[cfg(test)]
mod tests;
