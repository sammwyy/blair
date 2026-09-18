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

/// Renders a single-letter badge on a filled circular background, used as
/// the titlebar icon fallback when no themed app icon is available.
pub fn rasterize_monogram(
    font: &Font,
    letter: char,
    background: [u8; 4],
    foreground: [u8; 4],
    size: i32,
) -> RasterizedGlyphs {
    let size = size.max(1);
    let mut pixels = vec![0u8; size as usize * size as usize * 4];
    let radius = size as f32 / 2.0;
    let center = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            if dx * dx + dy * dy > radius * radius {
                continue;
            }
            let offset = (y as usize * size as usize + x as usize) * 4;
            pixels[offset] = background[0];
            pixels[offset + 1] = background[1];
            pixels[offset + 2] = background[2];
            pixels[offset + 3] = background[3];
        }
    }

    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        max_width: Some(size as f32),
        max_height: Some(size as f32),
        horizontal_align: HorizontalAlign::Center,
        vertical_align: VerticalAlign::Middle,
        ..LayoutSettings::default()
    });
    let glyph = letter.to_uppercase().collect::<String>();
    layout.append(&[font], &TextStyle::new(&glyph, size as f32 * 0.6, 0));
    for g in layout.glyphs() {
        if g.width == 0 || g.height == 0 {
            continue;
        }
        let (_, coverage) = font.rasterize_config(g.key);
        let gx = g.x.round() as i32;
        let gy = g.y.round() as i32;
        for row in 0..g.height {
            for col in 0..g.width {
                let alpha = u16::from(coverage[row * g.width + col]);
                if alpha == 0 {
                    continue;
                }
                let px = gx + col as i32;
                let py = gy + row as i32;
                if px < 0 || py < 0 || px >= size || py >= size {
                    continue;
                }
                let offset = (py as usize * size as usize + px as usize) * 4;
                for channel in 0..3 {
                    let bg = u16::from(pixels[offset + channel]);
                    let fg = u16::from(foreground[channel]);
                    pixels[offset + channel] = ((fg * alpha + bg * (255 - alpha)) / 255) as u8;
                }
                pixels[offset + 3] = 255;
            }
        }
    }
    RasterizedGlyphs {
        pixels,
        width: size,
        height: size,
    }
}

fn paint_glyphs(
    font: &Font,
    layout: &Layout,
    width: i32,
    height: i32,
    color: [u8; 4],
) -> RasterizedGlyphs {
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for g in layout.glyphs() {
        if g.width == 0 || g.height == 0 {
            continue;
        }
        let (_, coverage) = font.rasterize_config(g.key);
        let gx = g.x.round() as i32;
        let gy = g.y.round() as i32;
        for row in 0..g.height {
            for col in 0..g.width {
                let coverage = u16::from(coverage[row * g.width + col]);
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

    #[test]
    fn monogram_paints_a_circular_badge() {
        let font = font();
        let glyphs = rasterize_monogram(&font, 'f', [10, 10, 10, 255], [255, 255, 255, 255], 24);
        assert_eq!(glyphs.width, 24);
        assert_eq!(glyphs.height, 24);
        let center = (12 * 24 + 12) * 4;
        assert_eq!(glyphs.pixels[center + 3], 255);
        let corner = 0;
        assert_eq!(glyphs.pixels[corner + 3], 0);
    }
}
