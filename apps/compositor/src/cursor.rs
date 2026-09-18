use std::{collections::HashMap, time::Duration};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Logical, Point, Transform},
};
use xcursor::{parser::Image, CursorTheme};

const DEFAULT_THEME: &str = "default";
const DEFAULT_SIZE: u32 = 24;

const FALLBACK_ARROW: [&str; 17] = [
    "X          ",
    "XX         ",
    "X.X        ",
    "X..X       ",
    "X...X      ",
    "X....X     ",
    "X.....X    ",
    "X......X   ",
    "X.......X  ",
    "X........X ",
    "X.....XXXXX",
    "X..X..X    ",
    "X.X X..X   ",
    "XX  X..X   ",
    "X    X..X  ",
    "     X..X  ",
    "      XX   ",
];

struct CursorFrame {
    buffer: MemoryRenderBuffer,
    hotspot: Point<i32, Logical>,
    delay_ms: u32,
}

struct LoadedCursor {
    frames: Vec<CursorFrame>,
    duration_ms: u32,
}

impl LoadedCursor {
    fn frame_at(&self, time: Duration) -> &CursorFrame {
        if self.duration_ms == 0 || self.frames.len() == 1 {
            return &self.frames[0];
        }
        let mut offset = (time.as_millis() % u128::from(self.duration_ms)) as u32;
        for frame in &self.frames {
            if offset < frame.delay_ms {
                return frame;
            }
            offset -= frame.delay_ms;
        }
        &self.frames[0]
    }
}

/// Resolves named cursors to themed images, cached per icon and buffer scale.
pub struct CursorManager {
    theme: CursorTheme,
    theme_name: String,
    size: u32,
    cache: HashMap<(CursorIcon, i32), LoadedCursor>,
}

impl CursorManager {
    pub fn new(theme: Option<&str>, size: Option<u32>) -> Self {
        let theme_name = resolve_theme(theme);
        let size = resolve_size(size);
        tracing::info!(theme = %theme_name, size, "cursor theme selected");
        Self {
            theme: CursorTheme::load(&theme_name),
            theme_name,
            size,
            cache: HashMap::new(),
        }
    }

    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn reconfigure(&mut self, theme: Option<&str>, size: Option<u32>) -> bool {
        let theme_name = resolve_theme(theme);
        let size = resolve_size(size);
        if theme_name == self.theme_name && size == self.size {
            return false;
        }
        *self = Self::new(Some(&theme_name), Some(size));
        true
    }

    /// Returns the buffer and logical hotspot to draw for `icon` at `time`.
    pub fn image(
        &mut self,
        icon: CursorIcon,
        buffer_scale: i32,
        time: Duration,
    ) -> (&MemoryRenderBuffer, Point<i32, Logical>) {
        let buffer_scale = buffer_scale.max(1);
        let cursor = self
            .cache
            .entry((icon, buffer_scale))
            .or_insert_with(|| load_cursor(&self.theme, icon, self.size, buffer_scale));
        let frame = cursor.frame_at(time);
        (&frame.buffer, frame.hotspot)
    }

    pub fn is_animated(&self, icon: CursorIcon, buffer_scale: i32) -> bool {
        self.cache
            .get(&(icon, buffer_scale.max(1)))
            .is_some_and(|cursor| cursor.frames.len() > 1 && cursor.duration_ms > 0)
    }
}

fn resolve_theme(theme: Option<&str>) -> String {
    theme
        .filter(|theme| !theme.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| std::env::var("XCURSOR_THEME").ok())
        .filter(|theme| !theme.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_THEME.to_owned())
}

fn resolve_size(size: Option<u32>) -> u32 {
    size.or_else(|| {
        std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|size| size.parse().ok())
    })
    .filter(|size| (8..=256).contains(size))
    .unwrap_or(DEFAULT_SIZE)
}

fn load_cursor(theme: &CursorTheme, icon: CursorIcon, size: u32, scale: i32) -> LoadedCursor {
    let nominal = size * scale as u32;
    let names = std::iter::once(icon.name())
        .chain(icon.alt_names().iter().copied())
        .chain(["default", "left_ptr"]);
    for name in names {
        let Some(images) = theme
            .load_icon(name)
            .and_then(|path| std::fs::read(&path).ok())
            .and_then(|bytes| xcursor::parser::parse_xcursor(&bytes))
        else {
            continue;
        };
        if let Some(cursor) = cursor_from_images(images, nominal, scale) {
            tracing::debug!(icon = name, nominal, "loaded cursor image");
            return cursor;
        }
    }
    tracing::warn!(
        icon = icon.name(),
        "no themed cursor found, using built-in arrow"
    );
    fallback_cursor(nominal, scale)
}

fn cursor_from_images(images: Vec<Image>, nominal: u32, scale: i32) -> Option<LoadedCursor> {
    let best = images
        .iter()
        .map(|image| image.size)
        .min_by_key(|size| (i64::from(*size) - i64::from(nominal)).abs())?;
    let frames: Vec<_> = images
        .into_iter()
        .filter(|image| image.size == best)
        .map(|image| CursorFrame {
            buffer: MemoryRenderBuffer::from_slice(
                &image.pixels_rgba,
                Fourcc::Abgr8888,
                (image.width as i32, image.height as i32),
                scale,
                Transform::Normal,
                None,
            ),
            hotspot: (image.xhot as i32 / scale, image.yhot as i32 / scale).into(),
            delay_ms: image.delay,
        })
        .collect();
    let duration_ms = frames.iter().map(|frame| frame.delay_ms).sum();
    (!frames.is_empty()).then_some(LoadedCursor {
        frames,
        duration_ms,
    })
}

fn fallback_cursor(nominal: u32, scale: i32) -> LoadedCursor {
    let (pixels, width, height) = fallback_pixels((nominal as usize / 16).max(1));
    LoadedCursor {
        frames: vec![CursorFrame {
            buffer: MemoryRenderBuffer::from_slice(
                &pixels,
                Fourcc::Abgr8888,
                (width as i32, height as i32),
                scale,
                Transform::Normal,
                None,
            ),
            hotspot: (0, 0).into(),
            delay_ms: 0,
        }],
        duration_ms: 0,
    }
}

fn fallback_pixels(factor: usize) -> (Vec<u8>, usize, usize) {
    let (width, height) = (
        FALLBACK_ARROW[0].len() * factor,
        FALLBACK_ARROW.len() * factor,
    );
    let mut pixels = vec![0u8; width * height * 4];
    for (y, row) in FALLBACK_ARROW.iter().enumerate() {
        for (x, cell) in row.bytes().enumerate() {
            let value = match cell {
                b'X' => 0x10,
                b'.' => 0xf0,
                _ => continue,
            };
            for dy in 0..factor {
                let start = ((y * factor + dy) * width + x * factor) * 4;
                for pixel in pixels[start..start + factor * 4].chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[value, value, value, 0xff]);
                }
            }
        }
    }
    (pixels, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(size: u32, delay: u32) -> Image {
        Image {
            size,
            width: size,
            height: size,
            xhot: 4,
            yhot: 2,
            delay,
            pixels_rgba: vec![0; (size * size * 4) as usize],
            pixels_argb: vec![0; (size * size * 4) as usize],
        }
    }

    #[test]
    fn picks_frames_closest_to_the_nominal_size() {
        let cursor =
            cursor_from_images(vec![image(24, 10), image(48, 10), image(48, 30)], 48, 2).unwrap();
        assert_eq!(cursor.frames.len(), 2);
        assert_eq!(cursor.duration_ms, 40);
        assert_eq!(cursor.frames[0].hotspot, (2, 1).into());
    }

    #[test]
    fn animation_frames_follow_their_delays() {
        let cursor = cursor_from_images(vec![image(24, 10), image(24, 30)], 24, 1).unwrap();
        let first = cursor.frame_at(Duration::from_millis(5)) as *const CursorFrame;
        let second = cursor.frame_at(Duration::from_millis(15)) as *const CursorFrame;
        let wrapped = cursor.frame_at(Duration::from_millis(45)) as *const CursorFrame;
        assert!(std::ptr::eq(first, &cursor.frames[0]));
        assert!(std::ptr::eq(second, &cursor.frames[1]));
        assert!(std::ptr::eq(wrapped, &cursor.frames[0]));
    }

    #[test]
    fn fallback_arrow_scales_with_the_requested_size() {
        let (pixels, width, height) = fallback_pixels(2);
        assert_eq!((width, height), (22, 34));
        assert_eq!(&pixels[..4], &[0x10, 0x10, 0x10, 0xff]);
        assert_eq!(&pixels[(width - 1) * 4..width * 4], &[0, 0, 0, 0]);
        let tip_fill = (2 * 2 * width + 2) * 4;
        assert_eq!(&pixels[tip_fill..tip_fill + 4], &[0xf0, 0xf0, 0xf0, 0xff]);
    }
}
