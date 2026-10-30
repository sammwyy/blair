use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};

const SERVICE: &str = "org.coconut.Permissions";
const PATH: &str = "/org/coconut/Permissions1";
const INTERFACE: &str = "org.coconut.Permissions1";
const CAPACITY: usize = 64;
const ASK_TIMEOUT: Duration = Duration::from_secs(190);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Unavailable,
    Full,
}

struct Entry {
    fallback: bool,
    result: Option<bool>,
    _cancel: oneshot::Sender<()>,
    reply: Option<oneshot::Sender<bool>>,
    owner: Option<String>,
}

struct Shared {
    entries: Mutex<HashMap<u64, Entry>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    next: AtomicU64,
}

impl Shared {
    fn complete(&self, token: u64, allowed: Option<bool>) {
        let wake = {
            let mut entries = self.entries.lock().unwrap();
            let Some(entry) = entries.get_mut(&token) else {
                return;
            };
            if entry.result.is_some() {
                return;
            }
            let allowed = allowed.unwrap_or(entry.fallback);
            if entry.owner.is_some() {
                let entry = entries.remove(&token).unwrap();
                if let Some(reply) = entry.reply {
                    let _ = reply.send(allowed);
                }
                false
            } else {
                entry.result = Some(allowed);
                true
            }
        };
        if wake {
            (self.wake)();
        }
    }
}

struct Completion {
    token: u64,
    shared: Arc<Shared>,
}
impl Drop for Completion {
    fn drop(&mut self) {
        self.shared.complete(self.token, None);
    }
}

pub struct AuthTicket {
    token: u64,
    shared: Arc<Shared>,
    reply: Option<oneshot::Receiver<bool>>,
}

impl AuthTicket {
    pub fn token(&self) -> u64 {
        self.token
    }
    async fn verdict(&mut self) -> bool {
        self.reply.take().unwrap().await.unwrap_or(false)
    }
}

impl Drop for AuthTicket {
    fn drop(&mut self) {
        self.shared.entries.lock().unwrap().remove(&self.token);
    }
}

struct AuthRequest {
    token: u64,
    subject: String,
    cancel: oneshot::Receiver<()>,
}

pub struct AuthReceiver {
    receiver: mpsc::Receiver<AuthRequest>,
    shared: Arc<Shared>,
}

impl Drop for AuthReceiver {
    fn drop(&mut self) {
        self.receiver.close();
        let tokens: Vec<_> = self
            .shared
            .entries
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect();
        for token in tokens {
            self.shared.complete(token, None);
        }
    }
}

#[derive(Clone)]
pub struct ScreencopyAuth {
    requests: mpsc::Sender<AuthRequest>,
    shared: Arc<Shared>,
}

impl ScreencopyAuth {
    pub fn request(&self, subject: String, fallback: bool) -> Result<AuthTicket, AdmissionError> {
        self.register(subject, fallback, None)
    }

    fn register(
        &self,
        subject: String,
        fallback: bool,
        owner: Option<String>,
    ) -> Result<AuthTicket, AdmissionError> {
        let mut entries = self.shared.entries.lock().unwrap();
        if entries.len() >= CAPACITY || subject.len() > 4096 {
            return Err(AdmissionError::Full);
        }
        let token = self.shared.next.fetch_add(1, Ordering::Relaxed);
        let (cancel, cancelled) = oneshot::channel();
        let (reply, verdict) = oneshot::channel();
        entries.insert(
            token,
            Entry {
                fallback,
                result: None,
                _cancel: cancel,
                reply: Some(reply),
                owner,
            },
        );
        if let Err(error) = self.requests.try_send(AuthRequest {
            token,
            subject,
            cancel: cancelled,
        }) {
            entries.remove(&token);
            return Err(match error {
                mpsc::error::TrySendError::Closed(_) => AdmissionError::Unavailable,
                mpsc::error::TrySendError::Full(_) => AdmissionError::Full,
            });
        }
        Ok(AuthTicket {
            token,
            shared: self.shared.clone(),
            reply: Some(verdict),
        })
    }

    pub(crate) async fn authorize(
        &self,
        subject: String,
        fallback: bool,
        owner: String,
    ) -> Result<bool, AdmissionError> {
        let mut ticket = self.register(subject, fallback, Some(owner))?;
        Ok(ticket.verdict().await)
    }

    pub(crate) fn disconnect(&self, owner: &str) {
        self.shared
            .entries
            .lock()
            .unwrap()
            .retain(|_, entry| entry.owner.as_deref() != Some(owner));
    }

    pub(crate) fn drain(&self, mut deliver: impl FnMut(u64, bool)) {
        let ready = {
            let mut entries = self.shared.entries.lock().unwrap();
            let ready: Vec<_> = entries
                .iter()
                .filter_map(|(token, entry)| entry.result.map(|allowed| (*token, allowed)))
                .collect();
            for (token, _) in &ready {
                entries.remove(token);
            }
            ready
        };
        for (token, allowed) in ready {
            deliver(token, allowed);
        }
    }
}

pub fn channel(wake: Arc<dyn Fn() + Send + Sync>) -> (ScreencopyAuth, AuthReceiver) {
    let (requests, receiver) = mpsc::channel(CAPACITY);
    let shared = Arc::new(Shared {
        entries: Mutex::new(HashMap::new()),
        wake,
        next: AtomicU64::new(0),
    });
    (
        ScreencopyAuth {
            requests,
            shared: shared.clone(),
        },
        AuthReceiver { receiver, shared },
    )
}

pub async fn run(connection: zbus::Connection, mut requests: AuthReceiver) {
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            request = requests.receiver.recv(), if tasks.len() < CAPACITY => {
                let Some(mut request) = request else { break; };
                let shared = requests.shared.clone();
                let connection = connection.clone();
                let completion = Completion { token: request.token, shared: shared.clone() };
                tasks.spawn(async move {
                    let _completion = completion;
                    if request.cancel.try_recv().is_err_and(|error| error == oneshot::error::TryRecvError::Closed) { return; }
                    let result = ask(&connection, &request.subject, &mut request.cancel).await;
                    shared.complete(request.token, result);
                });
            }
            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(Err(error)) = result { tracing::error!(%error, "capture authorization worker failed"); }
            }
            _ = connection.closed() => break,
        }
    }
    tasks.shutdown().await;
}

async fn cancel(connection: &zbus::Connection, request: &str) {
    let call = connection.call_method(
        Some(SERVICE),
        PATH,
        Some(INTERFACE),
        "CancelRequest",
        &request,
    );
    match tokio::time::timeout(CONTROL_TIMEOUT, call).await {
        Ok(Ok(_)) => {}
        result => tracing::warn!(?result, "could not cancel capture permission request"),
    }
}

async fn ask(
    connection: &zbus::Connection,
    subject: &str,
    cancelled: &mut oneshot::Receiver<()>,
) -> Option<bool> {
    let arguments = (subject, "screen-capture");
    let begin = connection.call_method(
        Some(SERVICE),
        PATH,
        Some(INTERFACE),
        "BeginRequest",
        &arguments,
    );
    let request: String = match tokio::time::timeout(CONTROL_TIMEOUT, begin).await {
        Ok(Ok(reply)) => match reply.body().deserialize() {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(%error, "invalid permission request handle");
                return Some(false);
            }
        },
        result => {
            tracing::warn!(?result, "could not begin capture permission request");
            return None;
        }
    };
    let wait = connection.call_method(
        Some(SERVICE),
        PATH,
        Some(INTERFACE),
        "WaitRequest",
        &request,
    );
    let result = tokio::select! {
        result = tokio::time::timeout(ASK_TIMEOUT, wait) => match result {
            Ok(Ok(reply)) => match reply.body().deserialize::<String>() {
                Ok(verdict) => Some(verdict == "allow"),
                Err(error) => { tracing::warn!(%error, "invalid capture permission verdict"); Some(false) }
            },
            result => { tracing::warn!(?result, "capture permission request failed"); None }
        },
        _ = cancelled => { cancel(connection, &request).await; return Some(false); }
    };
    cancel(connection, &request).await;
    result
}

pub fn capture_subject(pid: u32) -> Option<String> {
    if let Ok(info) = std::fs::read_to_string(format!("/proc/{pid}/root/.flatpak-info")) {
        let mut application = false;
        for line in info.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                application = line == "[Application]";
            }
            if application {
                if let Some(name) = line.strip_prefix("name=").filter(|name| !name.is_empty()) {
                    return Some(format!("flatpak:{name}"));
                }
            }
        }
    }
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
pub(crate) mod tests;
