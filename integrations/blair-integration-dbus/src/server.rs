use std::{sync::mpsc::Receiver, thread::JoinHandle};

use blair_protocol::CompositorEvent;
use futures_util::StreamExt;
use zbus::{fdo::NameOwnerChanged, message::Type as MessageType, MatchRule, MessageStream};

use crate::{
    command::{Command, CommandSender},
    interface::{CompositorInterface, CompositorInterfaceSignals},
    INTERFACE_NAME, OBJECT_PATH, SERVICE_NAME,
};

/// Starts the D-Bus service thread. `environment` is published to the bus so
/// D-Bus activated services inherit the session variables.
pub fn serve(
    commands: CommandSender,
    events: Receiver<CompositorEvent>,
    environment: Vec<(String, String)>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("blair-integration-dbus".into())
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
            if let Err(err) = runtime.block_on(run(commands, events, environment)) {
                tracing::error!(%err, "D-Bus service exited");
            }
        })
        .expect("failed to spawn D-Bus integration thread")
}

async fn run(
    commands: CommandSender,
    events: Receiver<CompositorEvent>,
    environment: Vec<(String, String)>,
) -> zbus::Result<()> {
    let interface = CompositorInterface::new(commands.clone());
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

    if !environment.is_empty() {
        update_activation_environment(&connection, &environment).await;
    }

    let (forward_tx, mut forward_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::task::spawn_blocking(move || {
        while let Ok(event) = events.recv() {
            if forward_tx.send(event).is_err() {
                break;
            }
        }
    });

    let disconnect_commands = commands.clone();
    let disconnect_connection = connection.clone();
    tokio::spawn(async move {
        let rule = match MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface("org.freedesktop.DBus")
            .and_then(|builder| builder.member("NameOwnerChanged"))
            .map(|builder| builder.build())
        {
            Ok(rule) => rule,
            Err(err) => {
                tracing::warn!(%err, "failed to subscribe to D-Bus client disconnects");
                return;
            }
        };
        let mut stream =
            match MessageStream::for_match_rule(rule, &disconnect_connection, None).await {
                Ok(stream) => stream,
                Err(err) => {
                    tracing::warn!(%err, "failed to monitor D-Bus client disconnects");
                    return;
                }
            };
        while let Some(Ok(message)) = stream.next().await {
            let Some(signal) = NameOwnerChanged::from_message(message) else {
                continue;
            };
            let Ok(args) = signal.args() else {
                continue;
            };
            if !args.name().as_str().starts_with(':') || args.new_owner().is_some() {
                continue;
            }
            disconnect_commands.send(Command::ClientDisconnected(args.name().to_string()));
        }
    });

    while let Some(event) = forward_rx.recv().await {
        if let Err(err) = emit(&iface_ref, event).await {
            tracing::warn!(%err, "failed to emit D-Bus signal");
        }
    }

    Ok(())
}

/// Publishes session variables so D-Bus activated services (portals, agents)
/// can reach this compositor.
async fn update_activation_environment(
    connection: &zbus::Connection,
    environment: &[(String, String)],
) {
    let variables: std::collections::HashMap<&str, &str> = environment
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let result = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "UpdateActivationEnvironment",
            &(variables,),
        )
        .await;
    match result {
        Ok(_) => tracing::info!(?environment, "published the D-Bus activation environment"),
        Err(err) => tracing::warn!(%err, "failed to publish the D-Bus activation environment"),
    }
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
        CompositorEvent::WindowAppIdChanged { id, app_id } => {
            iface_ref.window_app_id_changed(id.0, &app_id).await
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
        CompositorEvent::WorkspaceActivated { output, id } => {
            iface_ref.workspace_activated(&output, id).await
        }
        CompositorEvent::WorkspacesChanged => iface_ref.workspaces_changed().await,
        CompositorEvent::ConfigurationChanged => iface_ref.configuration_changed().await,
        CompositorEvent::ShortcutActivated { client, id } => {
            let emitter = iface_ref
                .signal_emitter()
                .clone()
                .set_destination(client.as_str().try_into()?);
            CompositorInterface::shortcut_activated(&emitter, &id).await
        }
    }
}
