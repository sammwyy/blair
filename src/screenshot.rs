use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
};

use anyhow::{bail, Context, Result};
use smithay::{
    output::Output,
    utils::{Physical, Size},
};

const MAX_REQUESTS: usize = 2;
const MAX_CAPTURE_BYTES: usize = 128 * 1024 * 1024;

type Reply = Box<dyn FnOnce(bool) + Send>;

#[derive(Default)]
pub struct Screenshots {
    pending: VecDeque<ScreenshotRequest>,
    budget: Arc<Mutex<Budget>>,
    worker: Option<mpsc::SyncSender<EncodeJob>>,
}

pub struct ScreenshotRequest {
    output: Output,
    pub size: Size<i32, Physical>,
    pub path: PathBuf,
    completion: Completion,
    reservation: Reservation,
}

#[derive(Default)]
struct Budget {
    requests: usize,
    bytes: usize,
}

struct Reservation {
    budget: Arc<Mutex<Budget>>,
    bytes: usize,
}

struct Completion(Option<Reply>);

struct EncodeJob {
    pixels: Box<[u8]>,
    size: Size<i32, Physical>,
    path: PathBuf,
    completion: Completion,
    _reservation: Reservation,
}

impl Screenshots {
    pub fn request(&mut self, output: Option<Output>, path: &str, reply: Reply) -> bool {
        let completion = Completion(Some(reply));
        let path = Path::new(path);
        let Some(output) = output else {
            tracing::warn!("screenshot output was not found");
            return false;
        };
        let Some(mode) = output.current_mode() else {
            tracing::warn!(
                output = output.name(),
                "screenshot output has no current mode"
            );
            return false;
        };
        if !path.is_absolute() {
            tracing::warn!(path = %path.display(), "screenshot path must be absolute");
            return false;
        }
        let reservation = match self.reserve(mode.size, path.as_os_str().len()) {
            Ok(reservation) => reservation,
            Err(error) => {
                tracing::warn!(%error, output = output.name(), "screenshot rejected");
                return false;
            }
        };
        self.pending.push_back(ScreenshotRequest {
            output,
            size: mode.size,
            path: path.to_path_buf(),
            completion,
            reservation,
        });
        true
    }

    pub fn next_for(&mut self, output: &Output) -> Option<ScreenshotRequest> {
        let index = self
            .pending
            .iter()
            .position(|request| request.output == *output)?;
        self.pending.remove(index)
    }

    pub fn submit(&mut self, request: ScreenshotRequest, pixels: Vec<u8>) {
        if rgba_bytes(request.size) != Some(pixels.len()) {
            tracing::warn!(path = %request.path.display(), "invalid screenshot pixel length");
            return;
        }
        let job = EncodeJob {
            pixels: pixels.into_boxed_slice(),
            size: request.size,
            path: request.path,
            completion: request.completion,
            _reservation: request.reservation,
        };
        if self.worker.is_none() {
            match start_worker(|job| write_png(&job.path, job.size, &job.pixels)) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    tracing::warn!(%error, "failed to start screenshot worker");
                    return;
                }
            }
        }
        match self.worker.as_ref().unwrap().try_send(job) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(job)) => {
                tracing::warn!(path = %job.path.display(), "screenshot worker queue is full");
            }
            Err(mpsc::TrySendError::Disconnected(job)) => {
                tracing::warn!(path = %job.path.display(), "screenshot worker disconnected");
                self.worker = None;
            }
        }
    }

    fn reserve(&self, size: Size<i32, Physical>, path_bytes: usize) -> Result<Reservation> {
        let bytes = rgba_bytes(size)
            .and_then(|bytes| bytes.checked_add(path_bytes))
            .context("invalid screenshot dimensions or path length")?;
        let mut budget = self.budget.lock().unwrap();
        if budget.requests >= MAX_REQUESTS || bytes > MAX_CAPTURE_BYTES - budget.bytes {
            bail!("screenshot capture budget exhausted");
        }
        budget.requests += 1;
        budget.bytes += bytes;
        Ok(Reservation {
            budget: self.budget.clone(),
            bytes,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut budget = self.budget.lock().unwrap();
        budget.requests -= 1;
        budget.bytes -= self.bytes;
    }
}

impl Completion {
    fn finish(&mut self, success: bool) {
        if let Some(reply) = self.0.take() {
            reply(success);
        }
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        self.finish(false);
    }
}

fn rgba_bytes(size: Size<i32, Physical>) -> Option<usize> {
    let width = usize::try_from(size.w).ok().filter(|width| *width > 0)?;
    let height = usize::try_from(size.h).ok().filter(|height| *height > 0)?;
    width.checked_mul(height)?.checked_mul(4)
}

fn start_worker(
    mut encode: impl FnMut(&EncodeJob) -> Result<()> + Send + 'static,
) -> Result<mpsc::SyncSender<EncodeJob>> {
    let (sender, receiver) = mpsc::sync_channel::<EncodeJob>(MAX_REQUESTS);
    std::thread::Builder::new()
        .name("blair-screenshots".into())
        .spawn(move || {
            for job in receiver {
                let success = match encode(&job) {
                    Ok(()) => true,
                    Err(error) => {
                        tracing::warn!(error = ?error, path = %job.path.display(), "screenshot failed");
                        false
                    }
                };
                let EncodeJob {
                    pixels,
                    path,
                    mut completion,
                    _reservation: reservation,
                    ..
                } = job;
                drop(pixels);
                drop(path);
                drop(reservation);
                completion.finish(success);
            }
        })
        .context("failed to spawn screenshot worker")?;
    Ok(sender)
}

fn write_png(path: &Path, size: Size<i32, Physical>, pixels: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let file = std::fs::File::create(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    let mut encoder =
        png::Encoder::new(std::io::BufWriter::new(file), size.w as u32, size.h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .context("failed to write the PNG header")?;
    writer
        .write_image_data(pixels)
        .context("failed to write the PNG data")?;
    writer
        .finish()
        .context("failed to finish and flush the PNG")?;
    tracing::info!(path = %path.display(), width = size.w, height = size.h, "screenshot written");
    Ok(())
}

#[cfg(test)]
mod tests;
