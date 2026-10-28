use super::*;
use crate::command::{CommandSender, QueuedCommand};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command as ProcessCommand, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::sync::mpsc;

struct Bus(Child);

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn fixture() -> (
    Bus,
    zbus::Connection,
    zbus::Connection,
    CommandSender,
    mpsc::Receiver<QueuedCommand>,
) {
    let child = ProcessCommand::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut bus = Bus(child);
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let (sender, receiver) = CommandSender::channel(Arc::new(|| {}));
    let server = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .serve_at(crate::OBJECT_PATH, CompositorInterface::new(sender.clone()))
        .unwrap()
        .build()
        .await
        .unwrap();
    let client = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .build()
        .await
        .unwrap();
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
