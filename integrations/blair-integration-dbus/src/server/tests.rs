use super::*;
use blair_integration::events::{self, SendResult, EVENT_CAPACITY};
use blair_protocol::WindowId;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

#[tokio::test]
async fn stalled_emission_keeps_one_inflight_event_and_reports_backlog_loss() {
    let (sender, mut receiver) = events::channel();
    let opened = CompositorEvent::WindowOpened {
        id: WindowId(1),
        title: "first".into(),
        app_id: None,
    };
    sender.send(opened.clone());
    let (started, waiting) = oneshot::channel();
    let (release, blocked) = oneshot::channel();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let output = delivered.clone();
    let mut gate = Some((started, blocked));
    let forwarder = tokio::spawn(async move {
        forward_events(&mut receiver, move |item| {
            let gate = gate.take();
            let output = output.clone();
            async move {
                if let Some((started, blocked)) = gate {
                    started.send(()).unwrap();
                    blocked.await.unwrap();
                }
                output.lock().unwrap().push(item);
                Ok(())
            }
        })
        .await
        .unwrap();
    });
    waiting.await.unwrap();
    for id in 0..EVENT_CAPACITY {
        sender.send(CompositorEvent::WindowClosed {
            id: WindowId(id as u64),
        });
    }
    assert_eq!(
        sender.send(CompositorEvent::FocusCleared),
        SendResult::Reset
    );
    for _ in 0..1000 {
        assert_eq!(
            sender.send(CompositorEvent::ConfigurationChanged),
            SendResult::Suppressed
        );
    }
    release.send(()).unwrap();
    drop(sender);
    forwarder.await.unwrap();
    assert_eq!(
        *delivered.lock().unwrap(),
        [EventItem::Event(opened), EventItem::Reset]
    );
}

#[tokio::test]
async fn wire_signals_preserve_coalesced_lifecycles_and_expose_resets() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let bus = crate::test_support::Bus::default();
        let (commands, _receiver) = CommandSender::channel(Arc::new(|| {}));
        let server = bus
            .builder()
            .serve_at(
                OBJECT_PATH,
                CompositorInterface::new(commands, crate::auth::channel(Arc::new(|| {})).0),
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let client = bus.builder().build().await.unwrap();
        let proxy = crate::CompositorProxy::builder(&client)
            .destination(server.unique_name().unwrap().to_string())
            .unwrap()
            .build()
            .await
            .unwrap();
        let mut signals = proxy.inner().receive_all_signals().await.unwrap();
        let iface = server
            .object_server()
            .interface::<_, CompositorInterface>(OBJECT_PATH)
            .await
            .unwrap();
        let (sender, mut receiver) = events::channel();
        let open = CompositorEvent::WindowOpened {
            id: WindowId(1),
            title: "initial".into(),
            app_id: None,
        };
        sender.send(open);
        for title in ["old", "latest"] {
            sender.send(CompositorEvent::WindowTitleChanged {
                id: WindowId(1),
                title: title.into(),
            });
        }
        sender.send(CompositorEvent::WindowClosed { id: WindowId(1) });
        drop(sender);
        forward_events(&mut receiver, |item| emit_item(&iface, item))
            .await
            .unwrap();
        for member in ["WindowOpened", "WindowTitleChanged", "WindowClosed"] {
            let message = signals.next().await.unwrap();
            assert_eq!(message.header().member().unwrap().as_str(), member);
            if member == "WindowTitleChanged" {
                assert_eq!(
                    message.body().deserialize::<(u64, String)>().unwrap(),
                    (1, "latest".into())
                );
            }
        }
        let (sender, mut receiver) = events::channel();
        for _ in 0..=EVENT_CAPACITY {
            sender.send(CompositorEvent::FocusCleared);
        }
        drop(sender);
        forward_events(&mut receiver, |item| emit_item(&iface, item))
            .await
            .unwrap();
        let message = signals.next().await.unwrap();
        assert_eq!(message.header().member().unwrap().as_str(), "EventsReset");
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn real_client_disconnect_cancels_capture_but_forged_signals_do_not() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (bus, _portal, server, mut portal_events, _release) =
            crate::auth::tests::fixture_with(true, true).await;
        let (commands, mut queued) = CommandSender::channel(Arc::new(|| {}));
        let (_events, events_rx) = events::channel();
        let (auth, auth_rx) = crate::auth::channel(Arc::new(|| {}));
        let server_copy = server.clone();
        let service = tokio::spawn(run_connection(
            server_copy,
            commands,
            events_rx,
            Vec::new(),
            auth_rx,
            auth,
        ));
        let client = bus.builder().build().await.unwrap();
        let bus_proxy = zbus::fdo::DBusProxy::new(&client).await.unwrap();
        while !bus_proxy
            .name_has_owner(SERVICE_NAME.try_into().unwrap())
            .await
            .unwrap()
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let proxy = crate::CompositorProxy::new(&client).await.unwrap();
        let capture =
            tokio::spawn(async move { proxy.screenshot("screen", "/tmp/capture.png").await });
        let Command::ScreenCapturePolicy(reply) = queued.recv().await.unwrap().command else {
            panic!("missing policy check");
        };
        reply.send(blair_integration::CapturePolicy::Ask).unwrap();
        let subject = crate::capture_subject(std::process::id()).unwrap();
        assert_eq!(
            portal_events.recv().await.unwrap(),
            format!("begin:{subject}")
        );
        let attacker = bus.builder().build().await.unwrap();
        attacker
            .emit_signal(
                None::<&str>,
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameOwnerChanged",
                &(
                    client.unique_name().unwrap().as_str(),
                    client.unique_name().unwrap().as_str(),
                    "",
                ),
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), portal_events.recv())
                .await
                .is_err()
        );
        assert!(!capture.is_finished());
        client.close().await.unwrap();
        assert_eq!(
            portal_events.recv().await.unwrap(),
            format!("cancel:{subject}")
        );
        loop {
            match queued.recv().await.unwrap().command {
                Command::ClientDisconnected(_) => break,
                Command::Screenshot { .. } => panic!("disconnected caller was allowed to capture"),
                _ => {}
            }
        }
        let _ = capture.await;
        service.abort();
        let _ = service.await;
    })
    .await
    .unwrap();
}
