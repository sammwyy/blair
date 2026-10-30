//! Consent plumbing for `wlr-screencopy` requests.
//!
//! The compositor drops a request on this channel (never blocking the event
//! loop); the task running inside the D-Bus runtime asks
//! `org.coconut.Permissions1.Ask`, and the verdict travels back through the
//! regular command queue as `Command::ScreencopyVerdict`.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::command::{Command, CommandSender};

const SERVICE: &str = "org.coconut.Permissions";
const PATH: &str = "/org/coconut/Permissions1";
const INTERFACE: &str = "org.coconut.Permissions1";
const PERMISSION: &str = "screen-capture";
/// The daemon waits up to 180 s for the consent dialog; leave headroom.
const ASK_TIMEOUT: Duration = Duration::from_secs(190);

pub struct AuthRequest {
    pub token: u64,
    pub subject: String,
    /// `ask` denies when the permission service cannot be reached;
    /// `ask-safe` allows.
    pub allow_when_unreachable: bool,
}

/// Compositor-side handle: cheap to clone, never blocks.
#[derive(Clone)]
pub struct ScreencopyAuth {
    requests: mpsc::UnboundedSender<AuthRequest>,
}

impl ScreencopyAuth {
    pub fn request(&self, token: u64, subject: String, allow_when_unreachable: bool) {
        let _ = self.requests.send(AuthRequest {
            token,
            subject,
            allow_when_unreachable,
        });
    }
}

pub fn channel() -> (ScreencopyAuth, mpsc::UnboundedReceiver<AuthRequest>) {
    let (requests, receiver) = mpsc::unbounded_channel();
    (ScreencopyAuth { requests }, receiver)
}

pub async fn run(
    connection: zbus::Connection,
    mut requests: mpsc::UnboundedReceiver<AuthRequest>,
    commands: CommandSender,
) {
    while let Some(request) = requests.recv().await {
        let allowed = ask(&connection, &request.subject, request.allow_when_unreachable).await;
        if let Err(error) = commands.send(Command::ScreencopyVerdict {
            token: request.token,
            allowed,
        }) {
            tracing::warn!(%error, "could not queue screencopy verdict");
        }
    }
}

async fn ask(connection: &zbus::Connection, subject: &str, fallback: bool) -> bool {
    let arguments = (subject, PERMISSION);
    let call = connection.call_method(
        Some(SERVICE),
        PATH,
        Some(INTERFACE),
        "Ask",
        &arguments,
    );
    let verdict = match tokio::time::timeout(ASK_TIMEOUT, call).await {
        Ok(Ok(reply)) => reply
            .body()
            .deserialize::<String>()
            .unwrap_or_else(|_| "deny".into()),
        Ok(Err(error)) => {
            tracing::warn!(%error, "screencopy permission request failed");
            return fallback;
        }
        Err(_) => {
            tracing::warn!("screencopy permission request timed out");
            return fallback;
        }
    };
    verdict == "allow"
}
