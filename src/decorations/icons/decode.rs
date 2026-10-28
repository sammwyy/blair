use std::{
    fs::File,
    io::Read,
    path::Path,
    sync::{Arc, Once},
};

use anyhow::{ensure, Context, Result};
use image::imageops::FilterType;
use resvg::{
    tiny_skia::{Pixmap, Transform},
    usvg,
};

use super::{RgbaBitmap, MAX_CACHE_BYTES};

const MAX_SVG_BYTES: u64 = 8 * 1024 * 1024;

pub(super) fn supported_size(size: i32) -> bool {
    let size = size.max(1) as usize;
    size.checked_mul(size)
        .and_then(|pixels| pixels.checked_mul(4))
        .is_some_and(|bytes| bytes <= MAX_CACHE_BYTES)
}

pub(super) fn decode_and_resize(path: &Path, size: i32) -> Result<RgbaBitmap> {
    ensure!(supported_size(size), "icon exceeds the pixel budget");
    let size = size.max(1) as u32;
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return render_svg(path, size);
    }
    let mut reader = image::ImageReader::open(path)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    reader.limits(limits);
    let decoded = reader.decode()?.into_rgba8();
    let resized = if decoded.width() == size && decoded.height() == size {
        decoded
    } else {
        image::imageops::resize(&decoded, size, size, FilterType::Triangle)
    };
    Ok(RgbaBitmap {
        width: size as i32,
        height: size as i32,
        pixels: resized.into_raw(),
    })
}

fn render_svg(path: &Path, size: u32) -> Result<RgbaBitmap> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_SVG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_SVG_BYTES,
        "SVG exceeds the input budget"
    );
    let mut options = usvg::Options {
        resources_dir: path.parent().map(Path::to_path_buf),
        ..Default::default()
    };
    let fonts = Once::new();
    let select_font = usvg::FontResolver::default_font_selector();
    options.font_resolver.select_font = Box::new(move |font, database| {
        fonts.call_once(|| Arc::make_mut(database).load_system_fonts());
        select_font(font, database)
    });
    let tree = usvg::Tree::from_data(&bytes, &options)?;
    drop(bytes);
    let source = tree.size();
    let scale = size as f32 / source.width().max(source.height());
    let x = (size as f32 - source.width() * scale) / 2.0;
    let y = (size as f32 - source.height() * scale) / 2.0;
    let mut pixmap = Pixmap::new(size, size).context("could not allocate the icon pixmap")?;
    resvg::render(
        &tree,
        Transform::from_row(scale, 0.0, 0.0, scale, x, y),
        &mut pixmap.as_mut(),
    );
    let mut pixels = pixmap.take();
    // Decoration upload premultiplies pixels, so the cache must contain straight alpha.
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = (u32::from(*channel) * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255) as u8;
        }
    }
    Ok(RgbaBitmap {
        pixels,
        width: size as i32,
        height: size as i32,
    })
}
