use blair_protocol::{CompositorEvent, Rect, WindowId};
use futures_util::StreamExt;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use zbus::{message::Type as MessageType, Connection, MatchRule, Message, MessageStream};

/// Typed stream of compositor events.
pub struct Events {
    receiver: UnboundedReceiver<CompositorEvent>,
}

impl Events {
    pub(crate) async fn subscribe(connection: &Connection) -> zbus::Result<Self> {
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface(blair_dbus::INTERFACE_NAME)?
            .path(blair_dbus::OBJECT_PATH)?
            .build();
        let mut stream = MessageStream::for_match_rule(rule, connection, None).await?;

        let (sender, receiver) = unbounded_channel();
        tokio::spawn(async move {
            while let Some(message) = stream.next().await {
                let Ok(message) = message else {
                    continue;
                };
                if let Some(event) = decode(&message) {
                    if sender.send(event).is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Self { receiver })
    }

    pub async fn next(&mut self) -> Option<CompositorEvent> {
        self.receiver.recv().await
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
        "ShortcutActivated" => body
            .deserialize::<String>()
            .ok()
            .map(|id| CompositorEvent::ShortcutActivated { id }),
        _ => None,
    }
}
