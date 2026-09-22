use fontdue::layout::{
    CoordinateSystem, HorizontalAlign, Layout, LayoutSettings, TextStyle, VerticalAlign,
};
use fontdue::Font;

const ELLIPSIS: char = '…';

/// A tightly-boxed RGBA (straight alpha) bitmap ready to upload as a memory
/// render buffer.
pub struct RasterizedGlyphs {
    pub pixels: Vec<u8>,
    pub width: i32,
    pub height: i32,
}

/// Renders `text` inside a `max_width` x `height` box, truncating with an
/// ellipsis when it does not fit. Returns `None` when there is nothing to
/// draw, so callers can skip the render element entirely.
pub fn rasterize_title(
    font: &Font,
    text: &str,
    size: f32,
    color: [u8; 4],
    max_width: i32,
    height: i32,
    centered: bool,
) -> Option<RasterizedGlyphs> {
    if text.trim().is_empty() || max_width <= 0 || height <= 0 {
        return None;
    }
    let fitted = fit_within(font, text, size, max_width as f32);
    if fitted.is_empty() {
        return None;
    }
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        max_width: Some(max_width as f32),
        max_height: Some(height as f32),
        horizontal_align: if centered {
            HorizontalAlign::Center
        } else {
            HorizontalAlign::Left
        },
        vertical_align: VerticalAlign::Middle,
        ..LayoutSettings::default()
    });
    layout.append(&[font], &TextStyle::new(&fitted, size, 0));
    Some(paint_glyphs(font, &layout, max_width, height, color))
}

fn paint_glyphs(
    font: &Font,
    layout: &Layout,
    width: i32,
    height: i32,
    color: [u8; 4],
) -> RasterizedGlyphs {
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    // Blending happens in sRGB, which makes light-on-dark text look heavier
    // and dark-on-light text lighter than the font intends. Thin the
    // former and firm up the latter so both read at their real weight.
    let light = u32::from(color[0]) + u32::from(color[1]) + u32::from(color[2]) > 3 * 128;
    let exponent: f32 = if light { 1.3 } else { 0.85 };
    let weights: Vec<u16> = (0..=255u16)
        .map(|c| ((f32::from(c) / 255.0).powf(exponent) * 255.0).round() as u16)
        .collect();
    for g in layout.glyphs() {
        if g.width == 0 || g.height == 0 {
            continue;
        }
        let (_, coverage) = font.rasterize_config(g.key);
        let gx = g.x.round() as i32;
        let gy = g.y.round() as i32;
        for row in 0..g.height {
            for col in 0..g.width {
                let coverage = weights[usize::from(coverage[row * g.width + col])];
                if coverage == 0 {
                    continue;
                }
                let px = gx + col as i32;
                let py = gy + row as i32;
                if px < 0 || py < 0 || px >= width || py >= height {
                    continue;
                }
                let offset = (py as usize * width as usize + px as usize) * 4;
                let alpha = (coverage * u16::from(color[3]) / 255) as u8;
                pixels[offset] = color[0];
                pixels[offset + 1] = color[1];
                pixels[offset + 2] = color[2];
                pixels[offset + 3] = pixels[offset + 3].max(alpha);
            }
        }
    }
    RasterizedGlyphs {
        pixels,
        width,
        height,
    }
}

fn fit_within(font: &Font, text: &str, size: f32, max_width: f32) -> String {
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
        let advance = font.metrics(ch, size).advance_width;
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

fn measure(font: &Font, text: &str, size: f32) -> f32 {
    text.chars()
        .map(|ch| font.metrics(ch, size).advance_width)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn font() -> Font {
        (*creamui_fonts::resolve(
            creamui_fonts::DEFAULT_FAMILY,
            creamui_fonts::FontWeight::Regular,
        ))
        .clone()
    }

    #[test]
    fn empty_title_renders_nothing() {
        let font = font();
        assert!(rasterize_title(&font, "", 14.0, [255, 255, 255, 255], 200, 20, false).is_none());
        assert!(rasterize_title(&font, "hi", 14.0, [255, 255, 255, 255], 0, 20, false).is_none());
    }

    #[test]
    fn short_title_fits_without_truncation() {
        let font = font();
        let glyphs = rasterize_title(
            &font,
            "Terminal",
            14.0,
            [255, 255, 255, 255],
            400,
            20,
            false,
        )
        .unwrap();
        assert!(glyphs.pixels.iter().any(|&byte| byte != 0));
        assert!(glyphs.width <= 400);
        assert!(glyphs.height <= 20);
    }

    #[test]
    fn long_title_is_truncated_with_an_ellipsis() {
        let font = font();
        let long = "A very long window title that will not fit".repeat(3);
        let fitted = fit_within(&font, &long, 14.0, 80.0);
        assert!(fitted.ends_with(ELLIPSIS));
        assert!(fitted.len() < long.len());
    }
}
