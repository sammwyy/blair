use super::*;

fn font() -> std::rc::Rc<FontFace> {
    creamui_fonts::resolve(
        creamui_fonts::DEFAULT_FAMILY,
        creamui_fonts::FontWeight::Regular,
    )
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
    assert!(glyphs.bounds.size.w < 100);
    assert!(glyphs.bounds.size.h <= 20);
}

#[test]
fn long_title_is_truncated_with_an_ellipsis() {
    let font = font();
    let long = "A very long window title that will not fit".repeat(3);
    let fitted = fit_within(&font, &long, 14.0, 80.0);
    assert!(fitted.ends_with(ELLIPSIS));
    assert!(fitted.len() < long.len());
}

fn uncropped_pixels(
    font: &FontFace,
    layout: &creamui_fonts::TextLayout,
    size: f32,
    width: i32,
    height: i32,
    color: [u8; 4],
) -> Vec<u8> {
    let mut pixels = vec![0; width as usize * height as usize * 4];
    let mut context = ScaleContext::new();
    let light = u32::from(color[0]) + u32::from(color[1]) + u32::from(color[2]) > 3 * 128;
    let exponent = if light { 1.3 } else { 0.85 };
    for (image, bounds) in glyph_images(font, layout, size, &mut context) {
        for row in 0..image.placement.height as usize {
            for col in 0..image.placement.width as usize {
                let coverage = image.data[row * image.placement.width as usize + col];
                let weight = ((f32::from(coverage) / 255.0).powf(exponent) * 255.0).round() as u16;
                let alpha = (weight * u16::from(color[3]) / 255) as u8;
                let x = bounds.loc.x + col as i32;
                let y = bounds.loc.y + row as i32;
                if alpha == 0 || x < 0 || y < 0 || x >= width || y >= height {
                    continue;
                }
                let offset = (y * width + x) as usize * 4;
                pixels[offset..offset + 3].copy_from_slice(&color[..3]);
                pixels[offset + 3] = pixels[offset + 3].max(alpha);
            }
        }
    }
    pixels
}

#[test]
fn cropped_bitmaps_preserve_title_pixels_and_alignment() {
    let font = font();
    for text in [
        "Terminal",
        "  Window  ",
        "Ágj Åy",
        "Window title that needs truncation",
    ] {
        for centered in [false, true] {
            for scale in [1, 2, 3] {
                for color in [[240, 240, 240, 255], [30, 30, 30, 127], [255, 255, 255, 1]] {
                    let width = 140 * scale;
                    let height = 24 * scale;
                    let size = 14.0 * scale as f32;
                    let fitted = fit_within(&font, text, size, width as f32);
                    let layout = creamui_fonts::layout(
                        &font,
                        &fitted,
                        size,
                        &LayoutSettings {
                            max_width: Some(width as f32),
                            max_height: Some(height as f32),
                            horizontal_align: if centered {
                                HorizontalAlign::Center
                            } else {
                                HorizontalAlign::Left
                            },
                        },
                    );
                    let expected = uncropped_pixels(&font, &layout, size, width, height, color);
                    let cropped =
                        rasterize_title(&font, text, size, color, width, height, centered).unwrap();
                    assert!(cropped.bounds.loc.x >= 0 && cropped.bounds.loc.y >= 0);
                    assert!(cropped.bounds.loc.x + cropped.bounds.size.w <= width);
                    assert!(cropped.bounds.loc.y + cropped.bounds.size.h <= height);
                    assert_eq!(
                        cropped.pixels.len(),
                        (cropped.bounds.size.w * cropped.bounds.size.h * 4) as usize
                    );
                    let mut restored = vec![0; expected.len()];
                    let stride = cropped.bounds.size.w as usize * 4;
                    for (row, pixels) in cropped.pixels.chunks_exact(stride).enumerate() {
                        let start = ((cropped.bounds.loc.y as usize + row) * width as usize
                            + cropped.bounds.loc.x as usize)
                            * 4;
                        restored[start..start + stride].copy_from_slice(pixels);
                    }
                    assert_eq!(
                        restored, expected,
                        "{text:?}, {centered}, {scale}, {color:?}"
                    );
                    let alpha = |x: usize, y: usize| {
                        cropped.pixels[(y * cropped.bounds.size.w as usize + x) * 4 + 3]
                    };
                    assert!((0..cropped.bounds.size.w as usize).any(|x| alpha(x, 0) > 0));
                    assert!((0..cropped.bounds.size.w as usize)
                        .any(|x| alpha(x, cropped.bounds.size.h as usize - 1) > 0));
                    assert!((0..cropped.bounds.size.h as usize).any(|y| alpha(0, y) > 0));
                    assert!((0..cropped.bounds.size.h as usize)
                        .any(|y| alpha(cropped.bounds.size.w as usize - 1, y) > 0));
                }
            }
        }
    }
}

#[test]
fn clipped_or_invisible_glyphs_do_not_allocate_a_bitmap() {
    let font = font();
    assert!(rasterize_title(&font, "Terminal", 14.0, [255, 255, 255, 0], 400, 24, false).is_none());
    assert!(rasterize_title(&font, "\u{200b}", 14.0, [255; 4], 400, 24, false).is_none());
    let mut layout = creamui_fonts::layout(&font, "Terminal", 14.0, &LayoutSettings::default());
    for glyph in &mut layout.glyphs {
        glyph.y = -100.0;
    }
    assert!(paint_glyphs(&font, &layout, 14.0, 400, 24, [255; 4]).is_none());
}

#[test]
fn short_titles_keep_storage_independent_of_the_available_width() {
    let font = font();
    let narrow = rasterize_title(&font, "Terminal", 28.0, [255; 4], 400, 48, false).unwrap();
    let wide = rasterize_title(&font, "Terminal", 28.0, [255; 4], 7680, 48, false).unwrap();
    assert_eq!(narrow.bounds, wide.bounds);
    assert_eq!(narrow.pixels, wide.pixels);
    assert!(wide.pixels.len() < 7680 * 48 * 4 / 20);
}

#[test]
fn partially_clipped_glyphs_keep_their_visible_pixels() {
    let font = font();
    let mut layout = creamui_fonts::layout(&font, "Ágj", 24.0, &LayoutSettings::default());
    for glyph in &mut layout.glyphs {
        glyph.x -= 3.0;
        glyph.y -= 12.0;
    }
    let color = [30, 30, 30, 255];
    let expected = uncropped_pixels(&font, &layout, 24.0, 25, 10, color);
    let cropped = paint_glyphs(&font, &layout, 24.0, 25, 10, color).unwrap();
    let mut restored = vec![0; expected.len()];
    let stride = cropped.bounds.size.w as usize * 4;
    for (row, pixels) in cropped.pixels.chunks_exact(stride).enumerate() {
        let start =
            ((cropped.bounds.loc.y as usize + row) * 25 + cropped.bounds.loc.x as usize) * 4;
        restored[start..start + stride].copy_from_slice(pixels);
    }
    assert_eq!(restored, expected);
}
