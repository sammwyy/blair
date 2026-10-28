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
    utils::{Physical, Rectangle, Size, Transform},
    wayland::shm,
};

use crate::state::BlairState;

const VERSION: u32 = 3;

#[cfg(test)]
mod tests;

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
        self.submit_transformed(pixels, Transform::Normal, None, time);
    }

    pub fn submit_transformed(
        self,
        pixels: &[u8],
        transform: Transform,
        overlay: Option<CursorOverlay<'_>>,
        time: std::time::Duration,
    ) {
        let width = self.region.size.w;
        let height = self.region.size.h;
        let copied = shm::with_buffer_contents_mut(&self.buffer, |ptr, len, data| {
            if data.width != width
                || data.height != height
                || data.format != wl_shm::Format::Xrgb8888
                || data.offset < 0
                || data.stride < 0
                || data.offset as usize > len
            {
                return false;
            }
            let offset = data.offset as usize;
            // SAFETY: the pool remains mapped during the closure and offset is within it.
            let target = unsafe { std::slice::from_raw_parts_mut(ptr.add(offset), len - offset) };
            if !copy_pixels(
                pixels,
                target,
                data.stride as usize,
                self.region.size,
                transform,
            ) {
                return false;
            }
            overlay.as_ref().is_none_or(|overlay| {
                composite_cursor(target, data.stride as usize, self.region, overlay)
            })
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

pub struct CursorOverlay<'a> {
    pub region: Rectangle<i32, Physical>,
    pub pixels: &'a [u8],
}

fn composite_cursor(
    target: &mut [u8],
    stride: usize,
    region: Rectangle<i32, Physical>,
    overlay: &CursorOverlay<'_>,
) -> bool {
    let Some(bytes) = (overlay.region.size.w as usize)
        .checked_mul(overlay.region.size.h as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return false;
    };
    if overlay.region.is_empty() || overlay.pixels.len() < bytes {
        return false;
    }
    let Some(affected) = region.intersection(overlay.region) else {
        return true;
    };
    for y in affected.loc.y..affected.loc.y + affected.size.h {
        for x in affected.loc.x..affected.loc.x + affected.size.w {
            let source = (((y - overlay.region.loc.y) as usize * overlay.region.size.w as usize)
                + (x - overlay.region.loc.x) as usize)
                * 4;
            let dest = (y - region.loc.y) as usize * stride + (x - region.loc.x) as usize * 4;
            let alpha = overlay.pixels[source + 3] as u16;
            if alpha == 0 {
                continue;
            }
            if alpha == 255 {
                target[dest..dest + 3].copy_from_slice(&overlay.pixels[source..source + 3]);
                continue;
            }
            for channel in 0..3 {
                target[dest + channel] = overlay.pixels[source + channel].saturating_add(
                    ((target[dest + channel] as u16 * (255 - alpha) + 127) / 255) as u8,
                );
            }
        }
    }
    true
}

fn copy_pixels(
    source: &[u8],
    target: &mut [u8],
    stride: usize,
    size: Size<i32, Physical>,
    transform: Transform,
) -> bool {
    if size.is_empty() {
        return false;
    }
    let width = size.w as usize;
    let height = size.h as usize;
    let Some(row_bytes) = width.checked_mul(4) else {
        return false;
    };
    let Some(source_bytes) = row_bytes.checked_mul(height) else {
        return false;
    };
    let Some(target_bytes) = (height - 1)
        .checked_mul(stride)
        .and_then(|bytes| bytes.checked_add(row_bytes))
    else {
        return false;
    };
    if stride < row_bytes || source.len() < source_bytes || target.len() < target_bytes {
        return false;
    }
    match transform {
        Transform::Normal | Transform::Flipped180 => {
            for y in 0..height {
                let sy = if transform == Transform::Normal {
                    y
                } else {
                    height - 1 - y
                };
                target[y * stride..y * stride + row_bytes]
                    .copy_from_slice(&source[sy * row_bytes..(sy + 1) * row_bytes]);
            }
        }
        _ => {
            let source_width = transform.transform_size(size).w as usize;
            for y in 0..height {
                for x in 0..width {
                    let pixel = Rectangle::new((x as i32, y as i32).into(), (1, 1).into());
                    let loc = transform.transform_rect_in(pixel, &size).loc;
                    let start = (loc.y as usize * source_width + loc.x as usize) * 4;
                    let dest = y * stride + x * 4;
                    target[dest..dest + 4].copy_from_slice(&source[start..start + 4]);
                }
            }
        }
    }
    true
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
