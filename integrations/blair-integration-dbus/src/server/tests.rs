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
            .serve_at(OBJECT_PATH, CompositorInterface::new(commands))
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
