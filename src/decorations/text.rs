use creamui_fonts::{FontFace, HorizontalAlign, LayoutSettings};
use smithay::utils::{Physical, Rectangle};
use swash::scale::{image::Image, Render, ScaleContext, Source};
use swash::zeno::Format;

const ELLIPSIS: char = '…';

/// A tightly-boxed RGBA (straight alpha) bitmap ready to upload as a memory
/// render buffer.
pub struct RasterizedGlyphs {
    pub pixels: Vec<u8>,
    pub bounds: Rectangle<i32, Physical>,
}

/// Renders `text` inside a `max_width` x `height` box, truncating with an
/// ellipsis when it does not fit. Returns `None` when there is nothing to
/// draw, so callers can skip the render element entirely.
pub fn rasterize_title(
    font: &FontFace,
    text: &str,
    size: f32,
    color: [u8; 4],
    max_width: i32,
    height: i32,
    centered: bool,
) -> Option<RasterizedGlyphs> {
    if text.trim().is_empty() || color[3] == 0 || max_width <= 0 || height <= 0 {
        return None;
    }
    let fitted = fit_within(font, text, size, max_width as f32);
    if fitted.is_empty() {
        return None;
    }
    let layout = creamui_fonts::layout(
        font,
        &fitted,
        size,
        &LayoutSettings {
            max_width: Some(max_width as f32),
            max_height: Some(height as f32),
            horizontal_align: if centered {
                HorizontalAlign::Center
            } else {
                HorizontalAlign::Left
            },
        },
    );
    paint_glyphs(font, &layout, size, max_width, height, color)
}

fn glyph_images<'a>(
    font: &'a FontFace,
    layout: &'a creamui_fonts::TextLayout,
    size: f32,
    context: &'a mut ScaleContext,
) -> impl Iterator<Item = (Image, Rectangle<i32, Physical>)> + 'a {
    layout.glyphs.iter().filter_map(move |glyph| {
        let face = if glyph.face == 0 {
            font
        } else {
            &layout.fallback_faces[glyph.face - 1]
        };
        let mut scaler = context
            .builder(face.font_ref())
            .size(size)
            .hint(false)
            .build();
        let image = Render::new(&[Source::Outline])
            .format(Format::Alpha)
            .render(&mut scaler, glyph.id)?;
        let bounds = Rectangle::new(
            (
                (glyph.x.round() as i32).saturating_add(image.placement.left),
                (glyph.y.round() as i32).saturating_sub(image.placement.top),
            )
                .into(),
            (image.placement.width as i32, image.placement.height as i32).into(),
        );
        Some((image, bounds))
    })
}

fn paint_glyphs(
    font: &FontFace,
    layout: &creamui_fonts::TextLayout,
    size: f32,
    width: i32,
    height: i32,
    color: [u8; 4],
) -> Option<RasterizedGlyphs> {
    let area = Rectangle::from_size((width, height).into());
    let mut context = ScaleContext::new();
    // Blending in sRGB makes light text heavier and dark text lighter.
    let light = u32::from(color[0]) + u32::from(color[1]) + u32::from(color[2]) > 3 * 128;
    let exponent: f32 = if light { 1.3 } else { 0.85 };
    let alpha: [u8; 256] = std::array::from_fn(|coverage| {
        let weight = ((coverage as f32 / 255.0).powf(exponent) * 255.0).round() as u16;
        (weight * u16::from(color[3]) / 255) as u8
    });
    // A bounds pass keeps raster scratch space limited to one glyph.
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for (image, bounds) in glyph_images(font, layout, size, &mut context) {
        let Some(clip) = bounds.intersection(area) else {
            continue;
        };
        for y in clip.loc.y..clip.loc.y + clip.size.h {
            for x in clip.loc.x..clip.loc.x + clip.size.w {
                let index = ((y - bounds.loc.y) * bounds.size.w + x - bounds.loc.x) as usize;
                let value = alpha[usize::from(image.data[index])];
                if value != 0 {
                    left = left.min(x);
                    top = top.min(y);
                    right = right.max(x + 1);
                    bottom = bottom.max(y + 1);
                }
            }
        }
    }
    if left >= right || top >= bottom {
        return None;
    }
    let painted = Rectangle::from_extremities((left, top), (right, bottom));
    let mut pixels = vec![0; painted.size.w as usize * painted.size.h as usize * 4];
    for (image, bounds) in glyph_images(font, layout, size, &mut context) {
        let Some(clip) = bounds.intersection(painted) else {
            continue;
        };
        for y in clip.loc.y..clip.loc.y + clip.size.h {
            for x in clip.loc.x..clip.loc.x + clip.size.w {
                let index = ((y - bounds.loc.y) * bounds.size.w + x - bounds.loc.x) as usize;
                let alpha = alpha[usize::from(image.data[index])];
                if alpha == 0 {
                    continue;
                }
                let offset = ((y - top) * painted.size.w + x - left) as usize * 4;
                pixels[offset..offset + 3].copy_from_slice(&color[..3]);
                pixels[offset + 3] = pixels[offset + 3].max(alpha);
            }
        }
    }
    Some(RasterizedGlyphs {
        pixels,
        bounds: painted,
    })
}

fn fit_within(font: &FontFace, text: &str, size: f32, max_width: f32) -> String {
    if measure(font, text, size) <= max_width {
        return text.to_owned();
    }
    let ellipsis_width = measure(font, &ELLIPSIS.to_string(), size);
    if ellipsis_width > max_width {
        return String::new();
    }
    let mut result = String::new();
    let mut width = 0.0;
    for ch in text.chars() {
        let advance = measure(font, &ch.to_string(), size);
        if width + advance + ellipsis_width > max_width {
            break;
        }
        result.push(ch);
        width += advance;
    }
    if result.is_empty() {
        return String::new();
    }
    result.push(ELLIPSIS);
    result
}

fn measure(font: &FontFace, text: &str, size: f32) -> f32 {
    creamui_fonts::layout(font, text, size, &LayoutSettings::default())
        .lines
        .iter()
        .map(|line| line.width)
        .fold(0.0, f32::max)
}

#[cfg(test)]
mod tests;
