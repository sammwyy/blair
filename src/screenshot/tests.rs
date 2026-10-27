use std::time::Duration;

use smithay::output::{Mode, PhysicalProperties, Subpixel};

use super::*;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("blair-screenshots-{}-{name}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str) -> String {
        self.0.join(name).to_str().unwrap().to_owned()
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn output(name: &str, size: Option<(i32, i32)>) -> Output {
    let output = Output::new(
        name.into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Test".into(),
            model: "Test".into(),
        },
    );
    output.change_current_state(
        size.map(|size| Mode {
            size: size.into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    output
}

fn reply() -> (Reply, mpsc::Receiver<(bool, std::thread::ThreadId)>) {
    let (sender, receiver) = mpsc::channel();
    (
        Box::new(move |success| {
            sender.send((success, std::thread::current().id())).unwrap();
        }),
        receiver,
    )
}

fn receive(
    receiver: &mpsc::Receiver<(bool, std::thread::ThreadId)>,
) -> (bool, std::thread::ThreadId) {
    receiver.recv_timeout(Duration::from_secs(5)).unwrap()
}

fn assert_png(path: &str, size: (u32, u32), pixels: &[u8]) {
    let mut reader = png::Decoder::new(std::fs::File::open(path).unwrap())
        .read_info()
        .unwrap();
    let mut decoded = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut decoded).unwrap();
    assert_eq!((info.width, info.height), size);
    assert_eq!(&decoded[..info.buffer_size()], pixels);
}

#[test]
fn invalid_requests_fail_before_worker_start() {
    let mut screenshots = Screenshots::default();
    for (output, path) in [
        (None, "/tmp/unused.png"),
        (Some(output("no-mode", None)), "/tmp/unused.png"),
        (Some(output("relative", Some((2, 2)))), "relative.png"),
        (Some(output("zero", Some((0, 2)))), "/tmp/unused.png"),
        (
            Some(output("large", Some((16384, 16384)))),
            "/tmp/unused.png",
        ),
    ] {
        let (reply, receiver) = reply();
        assert!(!screenshots.request(output, path, reply));
        assert!(!receive(&receiver).0);
        assert!(receiver.try_recv().is_err());
    }
    assert!(screenshots.worker.is_none());
    assert!(screenshots.pending.is_empty());
    assert_eq!(screenshots.budget.lock().unwrap().bytes, 0);
}

#[test]
fn reservations_bound_capture_count_and_total_bytes() {
    let screenshots = Screenshots::default();
    let first = screenshots.reserve((2, 2).into(), 16).unwrap();
    let second = screenshots.reserve((2, 2).into(), 16).unwrap();
    assert!(screenshots.reserve((1, 1).into(), 1).is_err());
    drop((first, second));
    let large = screenshots.reserve((4096, 4096).into(), 1).unwrap();
    assert!(screenshots.reserve((4096, 4096).into(), 1).is_err());
    assert!(screenshots.reserve((1, 1).into(), usize::MAX).is_err());
    assert!(screenshots.reserve((i32::MAX, i32::MAX).into(), 0).is_err());
    drop(large);
    assert_eq!(screenshots.budget.lock().unwrap().requests, 0);
    assert_eq!(screenshots.budget.lock().unwrap().bytes, 0);
    assert!(screenshots.reserve((4096, 4096).into(), 1).is_ok());
}

#[test]
fn pending_requests_follow_output_identity_and_cancel_on_drop() {
    let mut screenshots = Screenshots::default();
    let first = output("first", Some((2, 2)));
    let second = output("second", Some((2, 2)));
    let (reply_first, receiver_first) = reply();
    let (reply_second, receiver_second) = reply();
    assert!(screenshots.request(Some(first.clone()), "/tmp/first.png", reply_first));
    assert!(screenshots.request(Some(second.clone()), "/tmp/second.png", reply_second));
    assert!(screenshots
        .next_for(&output("second", Some((2, 2))))
        .is_none());
    drop(screenshots.next_for(&second).unwrap());
    assert!(!receive(&receiver_second).0);
    assert!(receiver_first.try_recv().is_err());
    drop(screenshots);
    assert!(!receive(&receiver_first).0);
}

#[test]
fn invalid_captured_pixels_release_the_reservation() {
    let mut screenshots = Screenshots::default();
    let output = output("test", Some((2, 2)));
    let (reply, receiver) = reply();
    assert!(screenshots.request(Some(output.clone()), "/tmp/unused.png", reply));
    let request = screenshots.next_for(&output).unwrap();
    screenshots.submit(request, vec![0; 15]);
    assert!(!receive(&receiver).0);
    assert!(screenshots.worker.is_none());
    assert_eq!(screenshots.budget.lock().unwrap().requests, 0);
}

#[test]
fn slow_encoding_keeps_admission_bounded_and_reuses_the_worker() {
    let directory = TestDirectory::new("bounded-worker");
    let mut screenshots = Screenshots::default();
    let output = output("test", Some((2, 1)));
    let (started_sender, started_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let mut release_receiver = Some(release_receiver);
    screenshots.worker = Some(
        start_worker(move |job| {
            if let Some(release) = release_receiver.take() {
                started_sender.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
            write_png(&job.path, job.size, &job.pixels)
        })
        .unwrap(),
    );
    let pixels = [255, 0, 0, 255, 0, 255, 0, 128];
    let (reply_first, receiver_first) = reply();
    let first_path = directory.file("nested/first.png");
    assert!(screenshots.request(Some(output.clone()), &first_path, reply_first));
    let first = screenshots.next_for(&output).unwrap();
    screenshots.submit(first, pixels.to_vec());
    started_receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(!Path::new(&first_path).exists());
    assert!(receiver_first.try_recv().is_err());
    let (reply_second, receiver_second) = reply();
    let second_path = directory.file("second.png");
    assert!(screenshots.request(Some(output.clone()), &second_path, reply_second));
    let second = screenshots.next_for(&output).unwrap();
    screenshots.submit(second, pixels.to_vec());
    let (reply_rejected, receiver_rejected) = reply();
    assert!(!screenshots.request(
        Some(output.clone()),
        &directory.file("rejected.png"),
        reply_rejected
    ));
    assert!(!receive(&receiver_rejected).0);
    assert_eq!(screenshots.budget.lock().unwrap().requests, MAX_REQUESTS);
    release_sender.send(()).unwrap();
    let (success_first, worker_id) = receive(&receiver_first);
    let (success_second, next_worker_id) = receive(&receiver_second);
    assert!(success_first && success_second);
    assert_ne!(worker_id, std::thread::current().id());
    assert_eq!(worker_id, next_worker_id);
    assert_eq!(screenshots.budget.lock().unwrap().bytes, 0);
    assert_png(&first_path, (2, 1), &pixels);
    assert_png(&second_path, (2, 1), &pixels);
    let (reply_third, receiver_third) = reply();
    assert!(screenshots.request(
        Some(output.clone()),
        &directory.file("third.png"),
        reply_third
    ));
    let third = screenshots.next_for(&output).unwrap();
    screenshots.submit(third, pixels.to_vec());
    assert_eq!(receive(&receiver_third), (true, worker_id));
}

#[test]
fn worker_recovers_after_filesystem_failure() {
    let directory = TestDirectory::new("write-failure");
    let mut screenshots = Screenshots::default();
    let output = output("test", Some((1, 1)));
    let (reply_failure, receiver_failure) = reply();
    assert!(screenshots.request(
        Some(output.clone()),
        directory.0.to_str().unwrap(),
        reply_failure
    ));
    let failed = screenshots.next_for(&output).unwrap();
    screenshots.submit(failed, vec![0, 0, 0, 255]);
    let (success, worker_id) = receive(&receiver_failure);
    assert!(!success);
    assert_eq!(screenshots.budget.lock().unwrap().bytes, 0);
    let (reply_success, receiver_success) = reply();
    let path = directory.file("success.png");
    assert!(screenshots.request(Some(output.clone()), &path, reply_success));
    let request = screenshots.next_for(&output).unwrap();
    screenshots.submit(request, vec![0, 0, 0, 255]);
    assert_eq!(receive(&receiver_success), (true, worker_id));
    assert_png(&path, (1, 1), &[0, 0, 0, 255]);
}

#[test]
fn disconnected_worker_cancels_the_job_and_allows_restart() {
    let directory = TestDirectory::new("worker-restart");
    let mut screenshots = Screenshots::default();
    let output = output("test", Some((1, 1)));
    let (sender, receiver) = mpsc::sync_channel(MAX_REQUESTS);
    drop(receiver);
    screenshots.worker = Some(sender);
    let (reply_failure, receiver_failure) = reply();
    let path = directory.file("capture.png");
    assert!(screenshots.request(Some(output.clone()), &path, reply_failure));
    let request = screenshots.next_for(&output).unwrap();
    screenshots.submit(request, vec![0; 4]);
    assert!(!receive(&receiver_failure).0);
    assert!(screenshots.worker.is_none());
    let (reply_success, receiver_success) = reply();
    assert!(screenshots.request(Some(output.clone()), &path, reply_success));
    let request = screenshots.next_for(&output).unwrap();
    screenshots.submit(request, vec![0; 4]);
    assert!(receive(&receiver_success).0);
    assert_png(&path, (1, 1), &[0; 4]);
}

#[test]
fn png_final_flush_errors_are_reported() {
    assert!(write_png(Path::new("/dev/full"), (1, 1).into(), &[0; 4]).is_err());
}
