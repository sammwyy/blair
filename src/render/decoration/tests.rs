mod policy;

use super::*;

fn title_id(
    decoration: &mut WindowDecoration,
    renderer: &mut smithay::backend::renderer::pixman::PixmanRenderer,
    font: &creamui_fonts::FontFace,
) -> Id {
    decoration.update_title(
        font,
        "Terminal",
        TitleStyle {
            font_size: 14.0,
            color: [255; 4],
            area: (200, 24).into(),
            centered: false,
            buffer_scale: 1,
        },
    );
    memory_id(decoration.title_buffer().unwrap().0, renderer)
}

#[test]
fn cropped_titles_align_to_the_buffer_scale_without_losing_pixels() {
    use smithay::backend::renderer::{
        element::{memory::MemoryRenderBufferRenderElement, UnderlyingStorage},
        pixman::PixmanRenderer,
    };
    let glyphs = RasterizedGlyphs {
        pixels: [200, 100, 50, 128].repeat(9),
        bounds: Rectangle::new((3, 5).into(), (3, 3).into()),
    };
    let mut renderer = PixmanRenderer::new().unwrap();
    for scale in [1, 2, 3, 4] {
        let (buffer, bounds) = buffer_from_glyphs(&glyphs, scale, (12, 12).into());
        let physical = bounds.to_physical_precise_round(scale);
        assert!(physical.contains_rect(glyphs.bounds));
        assert!(physical.size.w - glyphs.bounds.size.w < scale * 2 + 2);
        assert!(physical.size.h - glyphs.bounds.size.h < scale * 2 + 2);
        let element = MemoryRenderBufferRenderElement::from_buffer(
            &mut renderer,
            physical.loc.to_f64(),
            &buffer,
            None,
            None,
            Some(bounds.size),
            Kind::Unspecified,
        )
        .unwrap();
        assert_eq!(element.geometry(f64::from(scale).into()), physical);
        let Some(UnderlyingStorage::Memory(memory)) = element.underlying_storage(&mut renderer)
        else {
            panic!("expected a memory buffer");
        };
        assert_eq!(memory.size(), (physical.size.w, physical.size.h).into());
        for y in 0..physical.size.h {
            for x in 0..physical.size.w {
                let index = (y * physical.size.w + x) as usize * 4;
                let expected = if glyphs.bounds.contains(physical.loc + Point::from((x, y))) {
                    [100, 50, 25, 128]
                } else {
                    [0; 4]
                };
                assert_eq!(memory[index..index + 4], expected);
            }
        }
    }
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn cropped_titles_match_full_bitmaps_after_output_scaling() {
    use smithay::backend::{
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::{element::memory::MemoryRenderBufferRenderElement, Bind, ExportMem, Offscreen},
    };
    let device = EGLDevice::enumerate().unwrap().next().unwrap();
    let display = unsafe { EGLDisplay::new(device).unwrap() };
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
    let font = creamui_fonts::resolve(
        creamui_fonts::DEFAULT_FAMILY,
        creamui_fonts::FontWeight::Regular,
    );
    for scale in [
        0.5_f64, 0.75, 1.0, 1.1, 1.25, 1.333333, 1.5, 1.75, 2.0, 2.5, 3.0,
    ] {
        for centered in [false, true] {
            let buffer_scale = scale.ceil() as i32;
            let width = 401 * buffer_scale;
            let height = 24 * buffer_scale;
            let glyphs = crate::decorations::rasterize_title(
                &font,
                "Terminal",
                14.0 * buffer_scale as f32,
                [255; 4],
                width,
                height,
                centered,
            )
            .unwrap();
            let mut original = vec![0; width as usize * height as usize * 4];
            let stride = glyphs.bounds.size.w as usize * 4;
            for (row, source) in glyphs.pixels.chunks_exact(stride).enumerate() {
                let offset = ((glyphs.bounds.loc.y as usize + row) * width as usize
                    + glyphs.bounds.loc.x as usize)
                    * 4;
                original[offset..offset + stride].copy_from_slice(source);
            }
            let full = buffer_from_pixels(&premultiply(&original), width, height, buffer_scale);
            let (cropped, bounds) =
                buffer_from_glyphs(&glyphs, buffer_scale, (width, height).into());
            let location = Point::<i32, Logical>::from((11, 7));
            let original_element = MemoryRenderBufferRenderElement::from_buffer(
                &mut renderer,
                location.to_physical_precise_round::<_, i32>(scale).to_f64(),
                &full,
                None,
                None,
                Some((401, 24).into()),
                Kind::Unspecified,
            )
            .unwrap();
            let cropped_element = MemoryRenderBufferRenderElement::from_buffer(
                &mut renderer,
                location.to_physical_precise_round::<_, i32>(scale).to_f64(),
                &cropped,
                None,
                Some(Rectangle::new(
                    Point::default() - bounds.loc.to_f64(),
                    (401.0, 24.0).into(),
                )),
                Some((401, 24).into()),
                Kind::Unspecified,
            )
            .unwrap();
            let size = Size::<i32, Logical>::from((430, 48)).to_physical_precise_round(scale);
            let mut results = Vec::new();
            for element in [original_element, cropped_element] {
                let mut texture = Offscreen::<GlesTexture>::create_buffer(
                    &mut renderer,
                    Fourcc::Abgr8888,
                    (size.w, size.h).into(),
                )
                .unwrap();
                let mut target = renderer.bind(&mut texture).unwrap();
                OutputDamageTracker::new(size, scale, Transform::Normal)
                    .render_output(
                        &mut renderer,
                        &mut target,
                        0,
                        &[element],
                        Color32F::new(0.0, 0.0, 0.0, 0.0),
                    )
                    .unwrap();
                let mapping = renderer
                    .copy_framebuffer(
                        &target,
                        Rectangle::from_size((size.w, size.h).into()),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                results.push(renderer.map_texture(&mapping).unwrap().to_vec());
            }
            let difference = results[0]
                .iter()
                .zip(&results[1])
                .map(|(original, cropped)| original.abs_diff(*cropped))
                .max()
                .unwrap();
            assert!(
                difference <= 1,
                "scale={scale}, centered={centered}, maximum byte difference={difference}"
            );
        }
    }
}

#[test]
fn titles_reuse_the_bitmap_until_the_font_identity_changes() {
    let font = creamui_fonts::resolve(
        creamui_fonts::DEFAULT_FAMILY,
        creamui_fonts::FontWeight::Regular,
    );
    let replacement = creamui_fonts::FontFace::from_bytes(font.font_ref().data).unwrap();
    assert_ne!(font.id(), replacement.id());
    let mut renderer = smithay::backend::renderer::pixman::PixmanRenderer::new().unwrap();
    let mut decoration = WindowDecoration::default();
    let first = title_id(&mut decoration, &mut renderer, &font);
    assert_eq!(title_id(&mut decoration, &mut renderer, &font), first);
    let second = title_id(&mut decoration, &mut renderer, &replacement);
    assert_ne!(second, first);
    assert_eq!(
        title_id(&mut decoration, &mut renderer, &replacement),
        second
    );
}

#[test]
fn title_bitmaps_notice_registry_changes_without_a_new_primary_font() {
    let font = creamui_fonts::resolve(
        creamui_fonts::DEFAULT_FAMILY,
        creamui_fonts::FontWeight::Regular,
    );
    let mut renderer = smithay::backend::renderer::pixman::PixmanRenderer::new().unwrap();
    let mut decoration = WindowDecoration::default();
    let first = title_id(&mut decoration, &mut renderer, &font);
    creamui_fonts::register_bytes(
        "Blair Title Cache Test",
        creamui_fonts::FontWeight::Regular,
        font.font_ref().data,
    )
    .unwrap();
    assert_eq!(
        creamui_fonts::resolve(
            creamui_fonts::DEFAULT_FAMILY,
            creamui_fonts::FontWeight::Regular
        )
        .id(),
        font.id()
    );
    let second = title_id(&mut decoration, &mut renderer, &font);
    assert_ne!(second, first);
    assert_eq!(title_id(&mut decoration, &mut renderer, &font), second);
}

#[test]
fn button_is_a_disc_with_a_glyph_and_empty_corners() {
    let size = 20;
    let look = ButtonLook {
        glyph: ButtonGlyph::Close,
        glyph_visible: true,
        disc: [0, 0, 0, 0],
        glyph_color: [255, 255, 255, 255],
    };
    let pixels = rasterize_button(look, size);
    let center = (10 * size as usize + 10) * 4;
    // The cross passes through the center at full coverage.
    assert!(pixels[center + 3] > 200);
    assert_eq!(pixels[3], 0);
    // Premultiplied: no channel may exceed alpha.
    assert!(pixels
        .chunks_exact(4)
        .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]));
}

#[test]
fn premultiply_scales_color_by_alpha() {
    assert_eq!(premultiply(&[255, 128, 0, 128]), vec![128, 64, 0, 128]);
}

#[test]
fn contrast_picks_dark_text_on_light_backgrounds() {
    assert_eq!(contrasting([250, 250, 250, 255])[0], 0x1d);
    assert_eq!(contrasting([20, 20, 30, 255])[0], 0xf5);
}

#[test]
fn mix_interpolates_each_channel() {
    assert_eq!(
        mix([0, 0, 0, 255], [255, 255, 255, 255], 0.0),
        [0, 0, 0, 255]
    );
    assert_eq!(
        mix([0, 0, 0, 255], [255, 255, 255, 255], 1.0),
        [255, 255, 255, 255]
    );
    assert_eq!(
        mix([0, 0, 0, 0], [100, 200, 50, 255], 0.5),
        [50, 100, 25, 128]
    );
}

fn memory_id(
    buffer: &MemoryRenderBuffer,
    renderer: &mut smithay::backend::renderer::pixman::PixmanRenderer,
) -> Id {
    smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement::from_buffer(
        renderer,
        (0.0, 0.0),
        buffer,
        None,
        None,
        None,
        Kind::Unspecified,
    )
    .unwrap()
    .id()
    .clone()
}

#[test]
fn decoration_cache_hits_do_not_allocate_and_title_styles_invalidate() {
    let font = creamui_fonts::resolve(
        creamui_fonts::DEFAULT_FAMILY,
        creamui_fonts::FontWeight::Regular,
    );
    let style = TitleStyle {
        font_size: 14.0,
        color: [255; 4],
        area: (200, 24).into(),
        centered: false,
        buffer_scale: 1,
    };
    let look = ButtonLook {
        glyph: ButtonGlyph::Close,
        glyph_visible: true,
        disc: [0; 4],
        glyph_color: [255; 4],
    };
    let icon = Rc::new(RgbaBitmap {
        pixels: [255, 0, 0, 128].repeat(4),
        width: 2,
        height: 2,
    });
    let theme = crate::config::DecorationConfig::default().to_theme(None);
    let mut buttons = ButtonBuffers::default();
    let mut decoration = WindowDecoration::default();
    let allocations = crate::test_support::count_allocations(|| {
        decoration.update_title(&font, "Terminal", style);
        decoration.update_button(0, look, 20, 1, &mut buttons);
        decoration.update_icon(Some("test"), Some(&icon), 2, 1);
    });
    assert!(allocations > 0);
    assert_eq!(
        crate::test_support::count_allocations(|| {
            for _ in 0..1000 {
                std::hint::black_box(theme.clone());
                decoration.update_title(&font, "Terminal", style);
                decoration.update_button(0, look, 20, 1, &mut buttons);
                decoration.update_icon(Some("test"), Some(&icon), 2, 1);
            }
        }),
        0
    );
    let mut renderer = smithay::backend::renderer::pixman::PixmanRenderer::new().unwrap();
    for changed in [
        TitleStyle {
            font_size: 14.5,
            ..style
        },
        TitleStyle {
            color: [30, 30, 30, 255],
            ..style
        },
        TitleStyle {
            area: (201, 24).into(),
            ..style
        },
        TitleStyle {
            area: (200, 25).into(),
            ..style
        },
        TitleStyle {
            centered: true,
            ..style
        },
        TitleStyle {
            buffer_scale: 2,
            ..style
        },
    ] {
        decoration.update_title(&font, "Terminal", style);
        let previous = memory_id(decoration.title_buffer().unwrap().0, &mut renderer);
        decoration.update_title(&font, "Terminal", changed);
        let current = memory_id(decoration.title_buffer().unwrap().0, &mut renderer);
        assert_ne!(current, previous, "{changed:?}");
        assert_eq!(
            crate::test_support::count_allocations(
                || decoration.update_title(&font, "Terminal", changed)
            ),
            0
        );
        assert_eq!(
            memory_id(decoration.title_buffer().unwrap().0, &mut renderer),
            current
        );
    }
    let previous = memory_id(decoration.title_buffer().unwrap().0, &mut renderer);
    decoration.update_title(&font, "A different title", style);
    assert_ne!(
        memory_id(decoration.title_buffer().unwrap().0, &mut renderer),
        previous
    );
}

#[test]
fn icon_buffers_track_identity_dimensions_scale_and_availability() {
    let mut decoration = WindowDecoration::default();
    let mut renderer = smithay::backend::renderer::pixman::PixmanRenderer::new().unwrap();
    let first = Rc::new(RgbaBitmap {
        pixels: [255, 0, 0, 255].repeat(4),
        width: 2,
        height: 2,
    });
    decoration.update_icon(Some("test"), None, 2, 1);
    assert!(decoration.icon_buffer().is_none());
    decoration.update_icon(Some("test"), Some(&first), 2, 1);
    let initial = memory_id(decoration.icon_buffer().unwrap().0, &mut renderer);
    decoration.update_icon(Some("test"), Some(&first), 2, 1);
    assert_eq!(
        memory_id(decoration.icon_buffer().unwrap().0, &mut renderer),
        initial
    );
    for (app_id, size, scale) in [
        (Some("test"), 3, 1),
        (Some("test"), 2, 2),
        (Some("other"), 2, 1),
    ] {
        decoration.update_icon(app_id, Some(&first), size, scale);
        assert_ne!(
            memory_id(decoration.icon_buffer().unwrap().0, &mut renderer),
            initial
        );
    }
    decoration.update_icon(Some("test"), Some(&first), 2, 1);
    let previous = memory_id(decoration.icon_buffer().unwrap().0, &mut renderer);
    let second = Rc::new(RgbaBitmap {
        pixels: [0, 0, 255, 255].repeat(4),
        width: 2,
        height: 2,
    });
    decoration.update_icon(Some("test"), Some(&second), 2, 1);
    assert_ne!(
        memory_id(decoration.icon_buffer().unwrap().0, &mut renderer),
        previous
    );
    drop(second);
    assert!(decoration
        .icon
        .key
        .as_ref()
        .unwrap()
        .bitmap
        .as_ref()
        .unwrap()
        .upgrade()
        .is_none());
    decoration.update_icon(Some("test"), None, 2, 1);
    assert!(decoration.icon_buffer().is_none());
}

#[test]
fn hidden_decoration_parts_release_their_buffers() {
    let mut decoration = WindowDecoration::default();
    let id = decoration.id.clone();
    let font = creamui_fonts::resolve(
        creamui_fonts::DEFAULT_FAMILY,
        creamui_fonts::FontWeight::Regular,
    );
    decoration.update_title(
        &font,
        "Terminal",
        TitleStyle {
            font_size: 14.0,
            color: [255; 4],
            area: (200, 24).into(),
            centered: false,
            buffer_scale: 1,
        },
    );
    let icon = Rc::new(RgbaBitmap {
        pixels: [255; 4].repeat(4),
        width: 2,
        height: 2,
    });
    decoration.update_icon(Some("test"), Some(&icon), 2, 1);
    let mut buffers = ButtonBuffers::default();
    let look = ButtonLook {
        glyph: ButtonGlyph::Close,
        glyph_visible: false,
        disc: [255; 4],
        glyph_color: [0; 4],
    };
    decoration.update_button(0, look, 20, 1, &mut buffers);
    decoration.retain_parts(DecorationParts {
        buttons: [true; 3],
        ..Default::default()
    });
    assert!(decoration.title_buffer().is_none());
    assert!(decoration.icon_buffer().is_none());
    assert!(decoration.button_buffer(0).is_some());
    decoration.retain_parts(DecorationParts::default());
    assert!(decoration.button_buffer(0).is_none());
    assert_eq!(decoration.id, id);
    assert_eq!(
        crate::test_support::count_allocations(
            || decoration.retain_parts(DecorationParts::default())
        ),
        0
    );
}
