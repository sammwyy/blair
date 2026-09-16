use std::{
    sync::mpsc::{Receiver, Sender},
    thread::JoinHandle,
};

use blair_protocol::CompositorEvent;

use crate::{
    command::Command,
    interface::{CompositorInterface, CompositorInterfaceSignals},
    INTERFACE_NAME, OBJECT_PATH, SERVICE_NAME,
};

/// Starts the D-Bus service thread.
pub fn serve(commands: Sender<Command>, events: Receiver<CompositorEvent>) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("blair-dbus".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    tracing::error!(%err, "failed to start D-Bus runtime");
                    return;
                }
            };
            if let Err(err) = runtime.block_on(run(commands, events)) {
                tracing::error!(%err, "D-Bus service exited");
            }
        })
        .expect("failed to spawn blair-dbus thread")
}

async fn run(commands: Sender<Command>, events: Receiver<CompositorEvent>) -> zbus::Result<()> {
    let interface = CompositorInterface::new(commands);
    let connection = zbus::connection::Builder::session()?
        .name(SERVICE_NAME)?
        .serve_at(OBJECT_PATH, interface)?
        .build()
        .await?;

    let iface_ref = connection
        .object_server()
        .interface::<_, CompositorInterface>(OBJECT_PATH)
        .await?;

    tracing::info!(
        service = SERVICE_NAME,
        path = OBJECT_PATH,
        interface = INTERFACE_NAME,
        "D-Bus service ready"
    );

    let (forward_tx, mut forward_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::task::spawn_blocking(move || {
        while let Ok(event) = events.recv() {
            if forward_tx.send(event).is_err() {
                break;
            }
        }
    });

    while let Some(event) = forward_rx.recv().await {
        if let Err(err) = emit(&iface_ref, event).await {
            tracing::warn!(%err, "failed to emit D-Bus signal");
        }
    }

    Ok(())
}

async fn emit(
    iface_ref: &zbus::object_server::InterfaceRef<CompositorInterface>,
    event: CompositorEvent,
) -> zbus::Result<()> {
    match event {
        CompositorEvent::WindowOpened { id, title, app_id } => {
            iface_ref
                .window_opened(id.0, &title, app_id.as_deref().unwrap_or(""))
                .await
        }
        CompositorEvent::WindowClosed { id } => iface_ref.window_closed(id.0).await,
        CompositorEvent::WindowFocused { id } => iface_ref.window_focused(id.0).await,
        CompositorEvent::FocusCleared => iface_ref.focus_cleared().await,
        CompositorEvent::WindowTitleChanged { id, title } => {
            iface_ref.window_title_changed(id.0, &title).await
        }
        CompositorEvent::WindowGeometryChanged { id, geometry } => {
            iface_ref
                .window_geometry_changed(
                    id.0,
                    geometry.x,
                    geometry.y,
                    geometry.width,
                    geometry.height,
                )
                .await
        }
        CompositorEvent::WindowMinimized { id } => iface_ref.window_minimized(id.0).await,
        CompositorEvent::WindowRestored { id } => iface_ref.window_restored(id.0).await,
        CompositorEvent::WindowMaximized { id, maximized } => {
            iface_ref.window_maximized(id.0, maximized).await
        }
        CompositorEvent::OutputAdded { name } => iface_ref.output_added(&name).await,
        CompositorEvent::OutputRemoved { name } => iface_ref.output_removed(&name).await,
        CompositorEvent::WorkAreaChanged { output, area } => {
            iface_ref
                .work_area_changed(&output, area.x, area.y, area.width, area.height)
                .await
        }
        CompositorEvent::ShortcutActivated { id } => iface_ref.shortcut_activated(&id).await,
    }
}
