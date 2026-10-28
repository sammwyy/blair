use super::*;
use crate::command::{CommandSender, QueuedCommand};
use crate::test_support::Bus;
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;

async fn fixture() -> (
    Bus,
    zbus::Connection,
    zbus::Connection,
    CommandSender,
    mpsc::Receiver<QueuedCommand>,
) {
    let bus = Bus::default();
    let (sender, receiver) = CommandSender::channel(Arc::new(|| {}));
    let server = bus
        .builder()
        .serve_at(crate::OBJECT_PATH, CompositorInterface::new(sender.clone()))
        .unwrap()
        .build()
        .await
        .unwrap();
    let client = bus.builder().build().await.unwrap();
    (bus, server, client, sender, receiver)
}

#[tokio::test]
async fn dbus_reports_saturation_oversized_requests_and_shutdown_as_errors() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (_bus, server, client, sender, mut receiver) = fixture().await;
        let proxy = CompositorProxy::builder(&client)
            .destination(server.unique_name().unwrap().to_string())
            .unwrap()
            .build()
            .await
            .unwrap();
        while sender.send(Command::Quit).is_ok() {}
        assert!(matches!(
            proxy.list_windows().await,
            Err(zbus::fdo::Error::LimitsExceeded(_))
        ));
        while receiver.try_recv().is_ok() {}
        assert!(matches!(
            proxy.set_configuration(&"x".repeat(1024 * 1024)).await,
            Err(zbus::fdo::Error::LimitsExceeded(_))
        ));
        drop(receiver);
        assert!(matches!(
            proxy.list_windows().await,
            Err(zbus::fdo::Error::Failed(_))
        ));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn dbus_dispatch_keeps_waiting_method_tasks_bounded_and_preserves_replies() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (_bus, server, client, _sender, mut receiver) = fixture().await;
        let proxy = CompositorProxy::builder(&client)
            .destination(server.unique_name().unwrap().to_string())
            .unwrap()
            .build()
            .await
            .unwrap();
        let first = proxy.clone();
        let first = tokio::spawn(async move { first.focus_window(1).await });
        let queued = receiver.recv().await.unwrap();
        let Command::FocusWindow(1, reply) = queued.command else {
            panic!("unexpected request");
        };
        let second = tokio::spawn(async move { proxy.focus_window(2).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(25), receiver.recv())
                .await
                .is_err()
        );
        reply.send(true).unwrap();
        assert!(first.await.unwrap().unwrap());
        let queued = receiver.recv().await.unwrap();
        let Command::FocusWindow(2, reply) = queued.command else {
            panic!("unexpected request");
        };
        drop(reply);
        assert!(matches!(
            second.await.unwrap(),
            Err(zbus::fdo::Error::Failed(_))
        ));
    })
    .await
    .unwrap();
}
