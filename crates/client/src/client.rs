use blair_dbus::CompositorProxy;
use blair_protocol::{Rect, WindowId, WindowInfo};
use zbus::Connection;

use crate::Events;

/// Client for the `org.blair.Compositor1` D-Bus interface.
pub struct BlairClient {
    connection: Connection,
    proxy: CompositorProxy<'static>,
}

impl BlairClient {
    pub async fn connect() -> zbus::Result<Self> {
        let connection = Connection::session().await?;
        let proxy = CompositorProxy::new(&connection).await?;
        Ok(Self { connection, proxy })
    }

    pub async fn windows(&self) -> zbus::Result<Vec<WindowInfo>> {
        Ok(self
            .proxy
            .list_windows()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn focus_window(&self, id: WindowId) -> zbus::Result<bool> {
        self.proxy.focus_window(id.0).await
    }

    pub async fn close_window(&self, id: WindowId) -> zbus::Result<bool> {
        self.proxy.close_window(id.0).await
    }

    pub async fn minimize_window(&self, id: WindowId) -> zbus::Result<bool> {
        self.proxy.minimize_window(id.0).await
    }

    pub async fn toggle_maximize_window(&self, id: WindowId) -> zbus::Result<bool> {
        self.proxy.toggle_maximize_window(id.0).await
    }

    pub async fn move_resize_window(&self, id: WindowId, geometry: Rect) -> zbus::Result<bool> {
        self.proxy
            .move_resize_window(
                id.0,
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
            )
            .await
    }

    /// Returns the work area; an empty name selects the first output.
    pub async fn work_area(&self, output: &str) -> zbus::Result<Rect> {
        let (x, y, width, height) = self.proxy.work_area(output).await?;
        Ok(Rect {
            x,
            y,
            width,
            height,
        })
    }

    pub async fn outputs(&self) -> zbus::Result<Vec<String>> {
        self.proxy.outputs().await
    }

    /// Registers `accelerator` and identifies activations with `id`.
    pub async fn bind_shortcut(&self, id: &str, accelerator: &str) -> zbus::Result<bool> {
        self.proxy.bind_shortcut(id, accelerator).await
    }

    pub async fn unbind_shortcut(&self, id: &str) -> zbus::Result<()> {
        self.proxy.unbind_shortcut(id).await
    }

    pub async fn quit(&self) -> zbus::Result<()> {
        self.proxy.quit().await
    }

    /// Subscribes to compositor events.
    pub async fn events(&self) -> zbus::Result<Events> {
        Events::subscribe(&self.connection).await
    }
}
