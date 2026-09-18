//! `wlr-screencopy-v1`, the protocol screenshot tools and screen recorders
//! use to read back an output.

use std::sync::Mutex;

use smithay::{
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            protocol::{wl_buffer::WlBuffer, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::{Physical, Rectangle},
    wayland::shm,
};

use crate::state::BlairState;

const VERSION: u32 = 3;

/// Registers the `zwlr_screencopy_manager_v1` global.
pub fn register<D>(display: &DisplayHandle)
where
    D: GlobalDispatch<ZwlrScreencopyManagerV1, ()>
        + Dispatch<ZwlrScreencopyManagerV1, ()>
        + Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameData>
        + 'static,
{
    display.create_global::<D, ZwlrScreencopyManagerV1, _>(VERSION, ());
}

/// A capture a client asked for, completed by the backend that owns the
/// renderer.
pub struct ScreencopyRequest {
    pub frame: ZwlrScreencopyFrameV1,
    pub buffer: WlBuffer,
    pub output: Output,
    pub region: Rectangle<i32, Physical>,
    pub overlay_cursor: bool,
    pub with_damage: bool,
}

impl ScreencopyRequest {
    /// Writes `pixels` (tightly packed, `region` sized) into the client's
    /// buffer and completes the capture.
    pub fn submit(self, pixels: &[u8], time: std::time::Duration) {
        let width = self.region.size.w as usize;
        let height = self.region.size.h as usize;
        let copied = shm::with_buffer_contents_mut(&self.buffer, |ptr, len, data| {
            let stride = data.stride as usize;
            let offset = data.offset as usize;
            if data.width as usize != width || data.height as usize != height {
                return false;
            }
            if offset + stride * height > len {
                return false;
            }
            for row in 0..height {
                let source = &pixels[row * width * 4..(row + 1) * width * 4];
                // SAFETY: the destination range was bounds-checked above and
                // the pool stays mapped for the duration of this closure.
                unsafe {
                    std::ptr::copy_nonoverlapping(
                        source.as_ptr(),
                        ptr.add(offset + row * stride),
                        width * 4,
                    );
                }
            }
            true
        });
        match copied {
            Ok(true) => {
                self.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
                if self.with_damage {
                    self.frame.damage(0, 0, width as u32, height as u32);
                }
                let seconds = time.as_secs();
                self.frame
                    .ready((seconds >> 32) as u32, seconds as u32, time.subsec_nanos());
            }
            Ok(false) => {
                tracing::warn!("screencopy buffer does not match the requested region");
                self.frame.failed();
            }
            Err(error) => {
                tracing::warn!(%error, "screencopy buffer is not accessible");
                self.frame.failed();
            }
        }
    }

    pub fn fail(self) {
        self.frame.failed();
    }
}

#[derive(Default)]
pub struct ScreencopyFrameData {
    inner: Mutex<Option<PendingFrame>>,
}

struct PendingFrame {
    output: Output,
    region: Rectangle<i32, Physical>,
    overlay_cursor: bool,
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for BlairState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ZwlrScreencopyManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for BlairState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _manager: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let (frame, output, region, overlay_cursor) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput {
                frame,
                overlay_cursor,
                output,
            } => (frame, output, None, overlay_cursor != 0),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion {
                frame,
                overlay_cursor,
                output,
                x,
                y,
                width,
                height,
            } => (
                frame,
                output,
                Some((x, y, width, height)),
                overlay_cursor != 0,
            ),
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => return,
        };

        let frame = data_init.init(frame, ScreencopyFrameData::default());
        let Some(output) = Output::from_resource(&output) else {
            tracing::warn!("screencopy requested for an unknown output");
            frame.failed();
            return;
        };
        let Some(full) = state
            .space
            .output_geometry(&output)
            .map(|geometry| Rectangle::from_size(geometry.size))
        else {
            frame.failed();
            return;
        };
        let scale = output.current_scale().fractional_scale();
        let logical = match region {
            Some((x, y, width, height)) => {
                let requested = Rectangle::new((x, y).into(), (width, height).into());
                match requested.intersection(full) {
                    Some(region) if !region.is_empty() => region,
                    _ => {
                        tracing::warn!("screencopy region is outside the output");
                        frame.failed();
                        return;
                    }
                }
            }
            None => full,
        };
        let region = logical.to_physical_precise_round(scale);

        frame.buffer(
            wl_shm::Format::Xrgb8888,
            region.size.w as u32,
            region.size.h as u32,
            region.size.w as u32 * 4,
        );
        if frame.version() >= 3 {
            frame.buffer_done();
        }
        *frame
            .data::<ScreencopyFrameData>()
            .expect("initialized above")
            .inner
            .lock()
            .expect("screencopy frame lock poisoned") = Some(PendingFrame {
            output,
            region,
            overlay_cursor,
        });
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, ScreencopyFrameData> for BlairState {
    fn request(
        state: &mut Self,
        _client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &ScreencopyFrameData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let (buffer, with_damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy => return,
            _ => return,
        };
        let Some(pending) = data
            .inner
            .lock()
            .expect("screencopy frame lock poisoned")
            .take()
        else {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::AlreadyUsed,
                "frame was already copied",
            );
            return;
        };
        state.pending_captures.push(ScreencopyRequest {
            frame: frame.clone(),
            buffer,
            output: pending.output,
            region: pending.region,
            overlay_cursor: pending.overlay_cursor,
            with_damage,
        });
        state.request_redraw();
    }
}
