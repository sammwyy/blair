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
        .serve_at(
            crate::OBJECT_PATH,
            CompositorInterface::new(sender.clone(), crate::auth::channel(Arc::new(|| {})).0),
        )
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
async fn concurrent_calls_preserve_replies_and_bound_pending_work() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (_bus, server, client, _sender, mut receiver) = fixture().await;
        let proxy = CompositorProxy::builder(&client)
            .destination(server.unique_name().unwrap().to_string())
            .unwrap()
            .build()
            .await
            .unwrap();
        let first_proxy = proxy.clone();
        let first = tokio::spawn(async move { first_proxy.focus_window(1).await });
        let Command::FocusWindow(1, first_reply) = receiver.recv().await.unwrap().command else {
            panic!("unexpected command");
        };
        let second_proxy = proxy.clone();
        let second = tokio::spawn(async move { second_proxy.focus_window(2).await });
        let Command::FocusWindow(2, second_reply) = receiver.recv().await.unwrap().command else {
            panic!("unrelated call was blocked");
        };
        second_reply.send(false).unwrap();
        assert!(!second.await.unwrap().unwrap());
        first_reply.send(true).unwrap();
        assert!(first.await.unwrap().unwrap());
        let mut calls = tokio::task::JoinSet::new();
        let mut replies = Vec::new();
        for id in 0..128 {
            let proxy = proxy.clone();
            calls.spawn(async move { proxy.focus_window(id).await });
            let Command::FocusWindow(_, reply) = receiver.recv().await.unwrap().command else {
                panic!("unexpected command");
            };
            replies.push(reply);
        }
        assert!(matches!(
            proxy.list_windows().await,
            Err(zbus::fdo::Error::LimitsExceeded(_))
        ));
        drop(replies);
        while let Some(reply) = calls.join_next().await {
            assert!(reply.unwrap().is_err());
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn screenshot_consent_identifies_the_caller_without_blocking_other_methods() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let (bus, _portal, server, mut events, release) =
            crate::auth::tests::fixture_with(true, true).await;
        server.request_name(crate::SERVICE_NAME).await.unwrap();
        let (auth, rx) = crate::auth::channel(Arc::new(|| {}));
        let (commands, mut receiver) = CommandSender::channel(Arc::new(|| {}));
        server
            .object_server()
            .at(
                crate::OBJECT_PATH,
                CompositorInterface::new(commands, auth.clone()),
            )
            .await
            .unwrap();
        let worker = tokio::spawn(crate::auth::run(server.clone(), rx));
        let client = bus.builder().build().await.unwrap();
        let proxy = CompositorProxy::new(&client).await.unwrap();
        let capture_proxy = proxy.clone();
        let capture =
            tokio::spawn(
                async move { capture_proxy.screenshot("screen", "/tmp/capture.png").await },
            );
        let Command::ScreenCapturePolicy(reply) = receiver.recv().await.unwrap().command else {
            panic!("missing capture policy check");
        };
        reply.send(blair_integration::CapturePolicy::Ask).unwrap();
        let expected = crate::capture_subject(std::process::id()).unwrap();
        assert_eq!(events.recv().await.unwrap(), format!("begin:{expected}"));
        let windows_proxy = proxy.clone();
        let windows = tokio::spawn(async move { windows_proxy.list_windows().await });
        let Command::ListWindows(reply) = receiver.recv().await.unwrap().command else {
            panic!("capture blocked unrelated methods");
        };
        reply.send(Vec::new()).unwrap();
        assert!(windows.await.unwrap().unwrap().is_empty());
        release.notify_one();
        let Command::Screenshot {
            output,
            path,
            reply,
        } = receiver.recv().await.unwrap().command
        else {
            panic!("capture was not authorized");
        };
        assert_eq!(output, "screen");
        assert_eq!(path, "/tmp/capture.png");
        reply.send(true).unwrap();
        assert!(capture.await.unwrap().unwrap());
        drop(auth);
        worker.abort();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn screenshot_denials_never_reach_the_capture_backend() {
    for policy in [
        blair_integration::CapturePolicy::Deny,
        blair_integration::CapturePolicy::Ask,
    ] {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (bus, _portal, server, _events, _) =
                crate::auth::tests::fixture_with(false, false).await;
            server.request_name(crate::SERVICE_NAME).await.unwrap();
            let (auth, rx) = crate::auth::channel(Arc::new(|| {}));
            let (commands, mut receiver) = CommandSender::channel(Arc::new(|| {}));
            server
                .object_server()
                .at(crate::OBJECT_PATH, CompositorInterface::new(commands, auth))
                .await
                .unwrap();
            let worker = tokio::spawn(crate::auth::run(server.clone(), rx));
            let client = bus.builder().build().await.unwrap();
            let proxy = CompositorProxy::new(&client).await.unwrap();
            let capture =
                tokio::spawn(async move { proxy.screenshot("screen", "/tmp/capture.png").await });
            let Command::ScreenCapturePolicy(reply) = receiver.recv().await.unwrap().command else {
                panic!("missing policy check");
            };
            reply.send(policy).unwrap();
            assert!(!capture.await.unwrap().unwrap());
            assert!(receiver.try_recv().is_err());
            worker.abort();
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn screenshot_applies_unavailable_service_fallback_without_waiting_forever() {
    for (policy, allowed) in [
        (blair_integration::CapturePolicy::Ask, false),
        (blair_integration::CapturePolicy::AskSafe, true),
        (blair_integration::CapturePolicy::Allow, true),
    ] {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (_bus, server, client, _sender, mut receiver) = fixture().await;
            let proxy = CompositorProxy::builder(&client)
                .destination(server.unique_name().unwrap().to_string())
                .unwrap()
                .build()
                .await
                .unwrap();
            let capture =
                tokio::spawn(async move { proxy.screenshot("screen", "/tmp/capture.png").await });
            let Command::ScreenCapturePolicy(reply) = receiver.recv().await.unwrap().command else {
                panic!("missing policy check");
            };
            reply.send(policy).unwrap();
            if allowed {
                let Command::Screenshot { reply, .. } = receiver.recv().await.unwrap().command
                else {
                    panic!("missing authorized capture");
                };
                reply.send(true).unwrap();
            }
            assert_eq!(capture.await.unwrap().unwrap(), allowed);
            assert!(receiver.try_recv().is_err());
        })
        .await
        .unwrap();
    }
}
