use super::*;

fn title(id: u64, value: &str) -> CompositorEvent {
    CompositorEvent::WindowTitleChanged {
        id: WindowId(id),
        title: value.into(),
    }
}

#[tokio::test]
async fn replaceable_updates_keep_the_latest_value_in_emission_order() {
    let (sender, mut receiver) = channel();
    let geometry = CompositorEvent::WindowGeometryChanged {
        id: WindowId(1),
        geometry: Default::default(),
    };
    assert_eq!(sender.send(title(1, "old")), SendResult::Queued);
    sender.send(geometry.clone());
    assert_eq!(sender.send(title(1, "new")), SendResult::Coalesced);
    assert_eq!(receiver.recv().await, Some(EventItem::Event(geometry)));
    assert_eq!(
        receiver.recv().await,
        Some(EventItem::Event(title(1, "new")))
    );
    drop(sender);
    assert_eq!(receiver.recv().await, None);
}

#[tokio::test]
async fn lifecycle_events_are_barriers_even_when_window_ids_are_reused() {
    let (sender, mut receiver) = channel();
    let open = CompositorEvent::WindowOpened {
        id: WindowId(1),
        title: "initial".into(),
        app_id: None,
    };
    let close = CompositorEvent::WindowClosed { id: WindowId(1) };
    let minimized = CompositorEvent::WindowMinimized { id: WindowId(1) };
    let events = [
        open.clone(),
        title(1, "before"),
        minimized,
        title(1, "after"),
        close,
        open,
        title(1, "reused"),
    ];
    for event in &events {
        sender.send(event.clone());
    }
    drop(sender);
    for event in events {
        assert_eq!(receiver.recv().await, Some(EventItem::Event(event)));
    }
    assert_eq!(receiver.recv().await, None);
}

#[test]
fn update_floods_keep_storage_bounded_without_resetting() {
    let (sender, _receiver) = channel();
    for index in 0..10_000 {
        sender.send(title(1, &index.to_string()));
    }
    let buffer = sender.producer.shared.buffer.lock().unwrap();
    assert_eq!(buffer.events.len(), 1);
    assert!(buffer.bytes < 256);
    assert!(!buffer.reset_pending);
}

#[tokio::test]
async fn lifecycle_overflow_releases_events_and_requires_a_reset_before_resuming() {
    let (sender, mut receiver) = channel();
    for id in 0..EVENT_CAPACITY {
        assert_eq!(
            sender.send(CompositorEvent::WindowClosed {
                id: WindowId(id as u64)
            }),
            SendResult::Queued
        );
    }
    assert_eq!(
        sender.send(CompositorEvent::FocusCleared),
        SendResult::Reset
    );
    assert_eq!(sender.send(title(1, "suppressed")), SendResult::Suppressed);
    {
        let buffer = sender.producer.shared.buffer.lock().unwrap();
        assert!(buffer.events.is_empty());
        assert_eq!(buffer.bytes, 0);
    }
    assert_eq!(receiver.recv().await, Some(EventItem::Reset));
    assert_eq!(sender.send(title(1, "resumed")), SendResult::Queued);
    assert_eq!(
        receiver.recv().await,
        Some(EventItem::Event(title(1, "resumed")))
    );
}

#[tokio::test]
async fn retained_capacities_and_oversized_events_obey_byte_limits() {
    let (sender, mut receiver) = channel();
    for id in 0..EVENT_CAPACITY {
        let result = sender.send(CompositorEvent::WindowTitleChanged {
            id: WindowId(id as u64),
            title: String::with_capacity(MAX_EVENT_BYTES / 2),
        });
        if result == SendResult::Reset {
            break;
        }
    }
    assert_eq!(receiver.recv().await, Some(EventItem::Reset));
    assert_eq!(
        sender.send(CompositorEvent::WindowTitleChanged {
            id: WindowId(1),
            title: String::with_capacity(MAX_EVENT_BYTES)
        }),
        SendResult::Reset
    );
    assert_eq!(receiver.recv().await, Some(EventItem::Reset));
}

#[tokio::test]
async fn receiver_drop_wakes_every_producer_waiter_and_releases_storage() {
    let (sender, receiver) = channel();
    sender.send(title(1, "pending"));
    let first = sender.clone();
    let first = tokio::spawn(async move { first.closed().await });
    let second = sender.clone();
    let second = tokio::spawn(async move { second.closed().await });
    tokio::task::yield_now().await;
    drop(receiver);
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(sender.send(title(1, "closed")), SendResult::Closed);
    assert_eq!(sender.producer.shared.buffer.lock().unwrap().bytes, 0);
}
