use super::*;
use crate::{
    command::{Command, CommandSender},
    test_support::Bus,
};
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

struct Portal {
    requests: Mutex<HashMap<String, String>>,
    seq: AtomicU64,
    events: mpsc::UnboundedSender<String>,
    release: Arc<Notify>,
    allowed: bool,
    hold: bool,
}

#[zbus::interface(name = "org.coconut.Permissions1")]
impl Portal {
    async fn begin_request(&self, subject: String, _permission: String) -> String {
        let id = self.seq.fetch_add(1, Ordering::Relaxed).to_string();
        self.requests
            .lock()
            .unwrap()
            .insert(id.clone(), subject.clone());
        let _ = self.events.send(format!("begin:{subject}"));
        id
    }
    async fn wait_request(&self, id: String) -> String {
        let subject = self.requests.lock().unwrap().get(&id).unwrap().clone();
        if subject == "slow" || self.hold {
            self.release.notified().await;
        }
        if subject == "denied" || !self.allowed {
            "deny"
        } else {
            "allow"
        }
        .into()
    }
    async fn cancel_request(&self, id: String) -> bool {
        if let Some(subject) = self.requests.lock().unwrap().remove(&id) {
            let _ = self.events.send(format!("cancel:{subject}"));
            true
        } else {
            false
        }
    }
}

async fn fixture() -> (
    Bus,
    zbus::Connection,
    zbus::Connection,
    mpsc::UnboundedReceiver<String>,
    Arc<Notify>,
) {
    fixture_with(true, false).await
}

pub(crate) async fn fixture_with(
    allowed: bool,
    hold: bool,
) -> (
    Bus,
    zbus::Connection,
    zbus::Connection,
    mpsc::UnboundedReceiver<String>,
    Arc<Notify>,
) {
    let bus = Bus::default();
    let (events, received) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let server = bus
        .builder()
        .name(SERVICE)
        .unwrap()
        .serve_at(
            PATH,
            Portal {
                requests: Mutex::new(HashMap::new()),
                seq: AtomicU64::new(0),
                events,
                release: release.clone(),
                allowed,
                hold,
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let connection = bus.builder().build().await.unwrap();
    (bus, server, connection, received, release)
}

async fn results(auth: &ScreencopyAuth, count: usize) -> Vec<(u64, bool)> {
    tokio::time::timeout(Duration::from_secs(3), async {
        let mut results = Vec::new();
        while results.len() < count {
            auth.drain(|token, allowed| results.push((token, allowed)));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        results
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn independent_requests_complete_while_another_prompt_is_pending() {
    let (_bus, _server, connection, mut events, _) = fixture().await;
    let wakes = Arc::new(AtomicUsize::new(0));
    let wake = wakes.clone();
    let (auth, rx) = channel(Arc::new(move || {
        wake.fetch_add(1, Ordering::Relaxed);
    }));
    let worker = tokio::spawn(run(connection, rx));
    let slow = auth.request("slow".into(), false).unwrap();
    let fast = auth.request("allowed".into(), false).unwrap();
    assert_eq!(results(&auth, 1).await, [(fast.token(), true)]);
    assert!(wakes.load(Ordering::Relaxed) > 0);
    let mut began = 0;
    while began < 2 {
        if events.recv().await.unwrap().starts_with("begin:") {
            began += 1;
        }
    }
    drop(slow);
    tokio::time::timeout(Duration::from_secs(3), async {
        while events.recv().await.unwrap() != "cancel:slow" {}
    })
    .await
    .unwrap();
    drop(fast);
    drop(auth);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn admission_is_bounded_and_closed_transport_is_reported() {
    let (auth, mut rx) = channel(Arc::new(|| {}));
    let tickets: Vec<_> = (0..CAPACITY)
        .map(|_| auth.request("app".into(), false).unwrap())
        .collect();
    assert!(matches!(
        auth.request("app".into(), false),
        Err(AdmissionError::Full)
    ));
    drop(tickets);
    while rx.receiver.try_recv().is_ok() {}
    let denied = auth.request("app".into(), false).unwrap();
    let allowed = auth.request("app".into(), true).unwrap();
    drop(rx);
    assert!(matches!(
        auth.request("app".into(), false),
        Err(AdmissionError::Unavailable)
    ));
    let mut ready = Vec::new();
    auth.drain(|token, verdict| ready.push((token, verdict)));
    ready.sort_unstable();
    assert_eq!(ready, [(denied.token(), false), (allowed.token(), true)]);
}

#[tokio::test]
async fn verdict_delivery_does_not_depend_on_space_in_the_command_queue() {
    let (_bus, _server, connection, _events, _) = fixture().await;
    let (commands, _full_queue) = CommandSender::channel(Arc::new(|| {}));
    while commands.send(Command::Quit).is_ok() {}
    let (auth, rx) = channel(Arc::new(|| {}));
    let worker = tokio::spawn(run(connection, rx));
    let ticket = auth.request("allowed".into(), false).unwrap();
    assert_eq!(results(&auth, 1).await, [(ticket.token(), true)]);
    drop(ticket);
    drop(auth);
    tokio::time::timeout(Duration::from_secs(3), worker)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn worker_shutdown_resolves_inflight_requests_using_their_policy() {
    let (_bus, _server, connection, mut events, _) = fixture().await;
    let (auth, rx) = channel(Arc::new(|| {}));
    let worker = tokio::spawn(run(connection, rx));
    let deny = auth.request("slow".into(), false).unwrap();
    let allow = auth.request("slow".into(), true).unwrap();
    while events.recv().await.unwrap() != "begin:slow" {}
    worker.abort();
    let _ = worker.await;
    let mut ready = results(&auth, 2).await;
    ready.sort_unstable();
    assert_eq!(ready, [(deny.token(), false), (allow.token(), true)]);
}

#[tokio::test]
async fn explicit_denial_survives_transport_shutdown_and_disconnected_callers_cancel() {
    let (_bus, _server, connection, mut events, _) = fixture().await;
    let (auth, rx) = channel(Arc::new(|| {}));
    let worker = tokio::spawn(run(connection, rx));
    let denied = auth.request("denied".into(), true).unwrap();
    let waiting = auth.clone();
    let caller = tokio::spawn(async move {
        waiting
            .authorize("slow".into(), false, ":1.50".into())
            .await
    });
    while events.recv().await.unwrap() != "cancel:denied" {}
    while auth
        .shared
        .entries
        .lock()
        .unwrap()
        .get(&denied.token())
        .unwrap()
        .result
        .is_none()
    {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    auth.disconnect(":1.50");
    assert!(!caller.await.unwrap().unwrap());
    worker.abort();
    let _ = worker.await;
    assert_eq!(results(&auth, 1).await, [(denied.token(), false)]);
}
