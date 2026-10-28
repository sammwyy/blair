use std::{
    fmt,
    sync::{Arc, OnceLock},
};

use blair_integration::events::{self, EventItem, EventReceiver, SendResult};
use blair_protocol::{CompositorEvent, Rect, WindowId};
use futures_util::StreamExt;
use zbus::{message::Type as MessageType, Connection, MatchRule, Message, MessageStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventStreamError {
    Overflow,
    Reset,
    Disconnected,
}

impl fmt::Display for EventStreamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Overflow => "client event queue overflowed",
            Self::Reset => "compositor event stream reset",
            Self::Disconnected => "compositor event stream disconnected",
        })
    }
}

impl std::error::Error for EventStreamError {}

/// A bounded event stream. After an error, resubscribe before refreshing snapshots.
pub struct Events {
    receiver: EventReceiver,
    error: Arc<OnceLock<EventStreamError>>,
    ended: bool,
}

impl Events {
    pub(crate) async fn subscribe(connection: &Connection) -> zbus::Result<Self> {
        let proxy = blair_integration_dbus::CompositorProxy::new(connection).await?;
        let mut owners = proxy.inner().receive_owner_changed().await?;
        let owner = zbus::fdo::DBusProxy::new(connection)
            .await?
            .get_name_owner(blair_integration_dbus::SERVICE_NAME.try_into()?)
            .await?;
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender(owner.as_str().to_owned())?
            .interface(blair_integration_dbus::INTERFACE_NAME)?
            .path(blair_integration_dbus::OBJECT_PATH)?
            .build();
        let mut stream = MessageStream::for_match_rule(rule, connection, Some(64)).await?;
        let (sender, receiver) = events::channel();
        let error = Arc::new(OnceLock::new());
        let status = error.clone();
        tokio::spawn(async move {
            let reason = loop {
                let message = tokio::select! {
                    biased;
                    _ = sender.closed() => return,
                    _ = owners.next() => break EventStreamError::Disconnected,
                    message = stream.next() => message,
                };
                let Some(Ok(message)) = message else {
                    break EventStreamError::Disconnected;
                };
                if message
                    .header()
                    .member()
                    .is_some_and(|member| member.as_str() == "EventsReset")
                {
                    break EventStreamError::Reset;
                }
                if let Some(event) = decode(&message) {
                    match sender.send(event) {
                        SendResult::Reset => break EventStreamError::Overflow,
                        SendResult::Closed => return,
                        SendResult::Queued | SendResult::Coalesced | SendResult::Suppressed => {}
                    }
                }
            };
            tracing::warn!(%reason, "compositor event subscription ended");
            let _ = status.set(reason);
            sender.reset();
        });
        Ok(Self {
            receiver,
            error,
            ended: false,
        })
    }

    /// Returns `None` after an error; inspect `error()` or use `next_result()`.
    pub async fn next(&mut self) -> Option<CompositorEvent> {
        self.next_result().await.ok().flatten()
    }

    pub fn error(&self) -> Option<EventStreamError> {
        self.error.get().copied()
    }

    pub async fn next_result(&mut self) -> Result<Option<CompositorEvent>, EventStreamError> {
        if self.ended {
            return Ok(None);
        }
        let fallback = match self.receiver.recv().await {
            Some(EventItem::Event(event)) => return Ok(Some(event)),
            Some(EventItem::Reset) => EventStreamError::Overflow,
            None => EventStreamError::Disconnected,
        };
        self.ended = true;
        Err(*self.error.get_or_init(|| fallback))
    }
}

fn decode(message: &Message) -> Option<CompositorEvent> {
    let header = message.header();
    let member = header.member()?.as_str();
    let body = message.body();
    match member {
        "WindowOpened" => {
            let (id, title, app_id) = body.deserialize::<(u64, String, String)>().ok()?;
            Some(CompositorEvent::WindowOpened {
                id: WindowId(id),
                title,
                app_id: (!app_id.is_empty()).then_some(app_id),
            })
        }
        "WindowClosed" => body
            .deserialize::<u64>()
            .ok()
            .map(|id| CompositorEvent::WindowClosed { id: WindowId(id) }),
        "WindowFocused" => body
            .deserialize::<u64>()
            .ok()
            .map(|id| CompositorEvent::WindowFocused { id: WindowId(id) }),
        "FocusCleared" => Some(CompositorEvent::FocusCleared),
        "WindowTitleChanged" => {
            let (id, title) = body.deserialize::<(u64, String)>().ok()?;
            Some(CompositorEvent::WindowTitleChanged {
                id: WindowId(id),
                title,
            })
        }
        "WindowAppIdChanged" => {
            let (id, app_id) = body.deserialize::<(u64, String)>().ok()?;
            Some(CompositorEvent::WindowAppIdChanged {
                id: WindowId(id),
                app_id,
            })
        }
        "WindowGeometryChanged" => {
            let (id, x, y, width, height) = body.deserialize::<(u64, i32, i32, i32, i32)>().ok()?;
            Some(CompositorEvent::WindowGeometryChanged {
                id: WindowId(id),
                geometry: Rect {
                    x,
                    y,
                    width,
                    height,
                },
            })
        }
        "WindowMinimized" => body
            .deserialize::<u64>()
            .ok()
            .map(|id| CompositorEvent::WindowMinimized { id: WindowId(id) }),
        "WindowRestored" => body
            .deserialize::<u64>()
            .ok()
            .map(|id| CompositorEvent::WindowRestored { id: WindowId(id) }),
        "WindowMaximized" => {
            let (id, maximized) = body.deserialize::<(u64, bool)>().ok()?;
            Some(CompositorEvent::WindowMaximized {
                id: WindowId(id),
                maximized,
            })
        }
        "OutputAdded" => body
            .deserialize::<String>()
            .ok()
            .map(|name| CompositorEvent::OutputAdded { name }),
        "OutputRemoved" => body
            .deserialize::<String>()
            .ok()
            .map(|name| CompositorEvent::OutputRemoved { name }),
        "WorkAreaChanged" => {
            let (output, x, y, width, height) =
                body.deserialize::<(String, i32, i32, i32, i32)>().ok()?;
            Some(CompositorEvent::WorkAreaChanged {
                output,
                area: Rect {
                    x,
                    y,
                    width,
                    height,
                },
            })
        }
        "WorkspaceActivated" => {
            let (output, id) = body.deserialize::<(String, u64)>().ok()?;
            Some(CompositorEvent::WorkspaceActivated { output, id })
        }
        "WorkspacesChanged" => Some(CompositorEvent::WorkspacesChanged),
        "ConfigurationChanged" => Some(CompositorEvent::ConfigurationChanged),
        "ShortcutActivated" => {
            body.deserialize::<String>()
                .ok()
                .map(|id| CompositorEvent::ShortcutActivated {
                    client: String::new(),
                    id,
                })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
