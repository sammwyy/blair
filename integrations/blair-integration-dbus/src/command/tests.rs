use super::*;
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering};

fn channel() -> (
    CommandSender,
    mpsc::Receiver<QueuedCommand>,
    Arc<AtomicUsize>,
) {
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = wakes.clone();
    let (sender, receiver) = CommandSender::channel(Arc::new(move || {
        counter.fetch_add(1, Ordering::Relaxed);
    }));
    (sender, receiver, wakes)
}

#[test]
fn admission_is_bounded_and_rejected_requests_do_not_wake_the_compositor() {
    let (sender, mut receiver, wakes) = channel();
    for _ in 0..COMMAND_CAPACITY {
        sender.send(Command::Quit).unwrap();
    }
    assert!(matches!(
        sender.send(Command::Quit),
        Err(fdo::Error::LimitsExceeded(_))
    ));
    assert_eq!(wakes.load(Ordering::Relaxed), COMMAND_CAPACITY);
    drop(receiver.try_recv().unwrap());
    sender.send(Command::Quit).unwrap();
    drop(receiver);
    assert!(matches!(
        sender.send(Command::Quit),
        Err(fdo::Error::Failed(_))
    ));
    assert_eq!(sender.memory.available_permits(), COMMAND_BYTES);
}

#[test]
fn payload_capacity_counts_toward_the_memory_budget_and_is_released_on_dequeue() {
    let (sender, mut receiver, _) = channel();
    let large = || Command::ClientDisconnected(String::with_capacity(MAX_COMMAND_BYTES / 2));
    while sender.send(large()).is_ok() {}
    assert!(receiver.len() < COMMAND_CAPACITY);
    let available = sender.memory.available_permits();
    assert!(available < MAX_COMMAND_BYTES / 2);
    drop(receiver.try_recv().unwrap());
    assert!(sender.memory.available_permits() > available);
    sender.send(large()).unwrap();
    assert!(matches!(
        sender.send(Command::ClientDisconnected(String::with_capacity(
            MAX_COMMAND_BYTES
        ))),
        Err(fdo::Error::LimitsExceeded(_))
    ));
    drop(receiver);
    assert_eq!(sender.memory.available_permits(), COMMAND_BYTES);
}

#[test]
fn batches_preserve_fifo_and_yield_even_when_producers_keep_refilling() {
    let (sender, mut receiver, wakes) = channel();
    let enqueue = |id| {
        let (reply, _) = oneshot::channel();
        sender.send(Command::SwitchWorkspace(id, reply)).unwrap();
    };
    for id in 0..32 {
        enqueue(id);
    }
    let mut ids = Vec::new();
    dispatch_batch(
        &mut receiver,
        &|| {
            wakes.fetch_add(1, Ordering::Relaxed);
        },
        |command| {
            let Command::SwitchWorkspace(id, _) = command else {
                panic!("unexpected command");
            };
            ids.push(id);
            enqueue(32 + id);
        },
    );
    assert!(!ids.is_empty() && ids.len() <= COMMAND_BATCH);
    assert_eq!(ids, (0..ids.len() as u64).collect::<Vec<_>>());
    assert_eq!(receiver.len(), 32);
    assert_eq!(wakes.load(Ordering::Relaxed), 32 + ids.len() + 1);
    let mut remaining = Vec::new();
    while !receiver.is_empty() {
        dispatch_batch(&mut receiver, &|| {}, |command| {
            let Command::SwitchWorkspace(id, _) = command else {
                panic!("unexpected command");
            };
            remaining.push(id);
        });
    }
    assert_eq!(
        remaining,
        (ids.len() as u64..32 + ids.len() as u64).collect::<Vec<_>>()
    );
}

#[test]
fn slow_commands_end_the_batch_after_the_time_slice() {
    let (sender, mut receiver, _) = channel();
    for _ in 0..3 {
        sender.send(Command::Quit).unwrap();
    }
    let mut count = 0;
    let wakes = AtomicUsize::new(0);
    dispatch_batch(
        &mut receiver,
        &|| {
            wakes.fetch_add(1, Ordering::Relaxed);
        },
        |_| {
            count += 1;
            std::thread::sleep(COMMAND_TIME_SLICE);
        },
    );
    assert_eq!(count, 1);
    assert_eq!(receiver.len(), 2);
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn control_commands_wait_for_capacity_and_stop_when_the_receiver_closes() {
    let (sender, mut receiver, _) = channel();
    for _ in 0..COMMAND_CAPACITY {
        sender.send(Command::Quit).unwrap();
    }
    let future = sender.send_control(Command::ThemeChanged);
    tokio::pin!(future);
    assert!(future.as_mut().now_or_never().is_none());
    drop(receiver.try_recv().unwrap());
    future.await.unwrap();
    let future = sender.send_control(Command::PrepareForSleep);
    tokio::pin!(future);
    assert!(future.as_mut().now_or_never().is_none());
    drop(receiver);
    assert!(matches!(future.await, Err(fdo::Error::Failed(_))));
    assert_eq!(sender.memory.available_permits(), COMMAND_BYTES);
}

#[test]
fn partial_batches_rewake_calloop_and_allow_other_sources_to_dispatch() {
    let mut event_loop = calloop::EventLoop::<(usize, bool)>::try_new().unwrap();
    let (ping, source) = calloop::ping::make_ping().unwrap();
    let wake = Arc::new(move || ping.ping());
    let (sender, mut receiver) = CommandSender::channel(wake.clone());
    for _ in 0..COMMAND_CAPACITY {
        sender.send(Command::Quit).unwrap();
    }
    event_loop
        .handle()
        .insert_source(source, move |_, _, state| {
            dispatch_batch(&mut receiver, wake.as_ref(), |_| state.0 += 1);
        })
        .unwrap();
    event_loop
        .handle()
        .insert_source(calloop::timer::Timer::immediate(), |_, _, state| {
            state.1 = true;
            calloop::timer::TimeoutAction::Drop
        })
        .unwrap();
    let mut counts = (0, false);
    event_loop.dispatch(Duration::ZERO, &mut counts).unwrap();
    assert!(counts.0 > 0 && counts.0 <= COMMAND_BATCH);
    assert!(counts.1);
    for _ in 0..COMMAND_CAPACITY {
        if counts.0 == COMMAND_CAPACITY {
            break;
        }
        event_loop.dispatch(Duration::ZERO, &mut counts).unwrap();
    }
    assert_eq!(counts.0, COMMAND_CAPACITY);
}
