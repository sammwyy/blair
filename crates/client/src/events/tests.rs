use super::*;

#[tokio::test]
async fn local_overflow_is_explicit_and_the_legacy_stream_ends() {
    let (sender, receiver) = events::channel();
    let mut events = Events {
        receiver,
        error: Arc::new(OnceLock::new()),
        ended: false,
    };
    sender.reset();
    assert_eq!(events.next_result().await, Err(EventStreamError::Overflow));
    assert_eq!(events.error(), Some(EventStreamError::Overflow));
    assert_eq!(events.next_result().await, Ok(None));
    assert!(events.next().await.is_none());
}

#[tokio::test]
async fn producer_shutdown_reports_disconnection() {
    let (sender, receiver) = events::channel();
    let mut events = Events {
        receiver,
        error: Arc::new(OnceLock::new()),
        ended: false,
    };
    drop(sender);
    assert_eq!(
        events.next_result().await,
        Err(EventStreamError::Disconnected)
    );
}

async fn fixture() -> (
    blair_integration_dbus::test_support::Bus,
    Connection,
    Connection,
) {
    let bus = blair_integration_dbus::test_support::Bus::default();
    let server = bus
        .builder()
        .name(blair_integration_dbus::SERVICE_NAME)
        .unwrap()
        .build()
        .await
        .unwrap();
    let client = bus.builder().build().await.unwrap();
    (bus, server, client)
}

#[tokio::test]
async fn server_reset_ends_the_subscription_and_a_new_subscription_receives_events() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (_bus, server, client) = fixture().await;
        let emitter =
            zbus::object_server::SignalEmitter::new(&server, blair_integration_dbus::OBJECT_PATH)
                .unwrap();
        let mut events = Events::subscribe(&client).await.unwrap();
        blair_integration_dbus::CompositorInterface::window_closed(&emitter, 7)
            .await
            .unwrap();
        assert_eq!(
            events.next_result().await.unwrap(),
            Some(CompositorEvent::WindowClosed { id: WindowId(7) })
        );
        blair_integration_dbus::CompositorInterface::events_reset(&emitter)
            .await
            .unwrap();
        assert_eq!(events.next_result().await, Err(EventStreamError::Reset));
        assert_eq!(events.error(), Some(EventStreamError::Reset));
        let mut events = Events::subscribe(&client).await.unwrap();
        blair_integration_dbus::CompositorInterface::window_opened(&emitter, 9, "fresh", "app")
            .await
            .unwrap();
        assert_eq!(
            events.next_result().await.unwrap(),
            Some(CompositorEvent::WindowOpened {
                id: WindowId(9),
                title: "fresh".into(),
                app_id: Some("app".into())
            })
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn slow_client_consumers_report_local_overflow() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (_bus, server, client) = fixture().await;
        let emitter =
            zbus::object_server::SignalEmitter::new(&server, blair_integration_dbus::OBJECT_PATH)
                .unwrap();
        let mut events = Events::subscribe(&client).await.unwrap();
        for id in 0..=blair_integration::events::EVENT_CAPACITY {
            blair_integration_dbus::CompositorInterface::window_closed(&emitter, id as u64)
                .await
                .unwrap();
        }
        while events.error().is_none() {
            tokio::task::yield_now().await;
        }
        assert_eq!(events.next_result().await, Err(EventStreamError::Overflow));
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn owner_loss_ends_the_stream_and_other_peers_cannot_forge_events() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (bus, server, client) = fixture().await;
        let mut events = Events::subscribe(&client).await.unwrap();
        let stranger = bus.builder().build().await.unwrap();
        let forged =
            zbus::object_server::SignalEmitter::new(&stranger, blair_integration_dbus::OBJECT_PATH)
                .unwrap();
        blair_integration_dbus::CompositorInterface::window_closed(&forged, 99)
            .await
            .unwrap();
        let emitter =
            zbus::object_server::SignalEmitter::new(&server, blair_integration_dbus::OBJECT_PATH)
                .unwrap();
        blair_integration_dbus::CompositorInterface::window_closed(&emitter, 1)
            .await
            .unwrap();
        assert_eq!(
            events.next_result().await.unwrap(),
            Some(CompositorEvent::WindowClosed { id: WindowId(1) })
        );
        server
            .release_name(blair_integration_dbus::SERVICE_NAME)
            .await
            .unwrap();
        assert_eq!(
            events.next_result().await,
            Err(EventStreamError::Disconnected)
        );
        stranger
            .request_name(blair_integration_dbus::SERVICE_NAME)
            .await
            .unwrap();
        let mut events = Events::subscribe(&client).await.unwrap();
        blair_integration_dbus::CompositorInterface::window_closed(&emitter, 98)
            .await
            .unwrap();
        blair_integration_dbus::CompositorInterface::window_closed(&forged, 2)
            .await
            .unwrap();
        assert_eq!(
            events.next_result().await.unwrap(),
            Some(CompositorEvent::WindowClosed { id: WindowId(2) })
        );
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn dropping_a_subscription_cancels_its_idle_forwarder() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let (_bus, _server, client) = fixture().await;
        for poll_first in [false, true] {
            let events = Events::subscribe(&client).await.unwrap();
            let status = events.error.clone();
            if poll_first {
                tokio::task::yield_now().await;
            }
            drop(events);
            while Arc::strong_count(&status) > 1 {
                tokio::task::yield_now().await;
            }
        }
    })
    .await
    .unwrap();
}
