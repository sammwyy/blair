use super::*;

const TRANSFORMS: [Transform; 8] = [
    Transform::Normal,
    Transform::_90,
    Transform::_180,
    Transform::_270,
    Transform::Flipped,
    Transform::Flipped90,
    Transform::Flipped180,
    Transform::Flipped270,
];

#[test]
fn transformed_copies_preserve_pixels_and_destination_padding() {
    let size = Size::<i32, Physical>::from((3, 2));
    let expected: Vec<u8> = (0..24).collect();
    for transform in TRANSFORMS {
        let source_width = transform.transform_size(size).w as usize;
        let mut source = vec![0; expected.len()];
        for y in 0..2 {
            for x in 0..3 {
                let loc = transform
                    .transform_rect_in(Rectangle::new((x, y).into(), (1, 1).into()), &size)
                    .loc;
                let start = (loc.y as usize * source_width + loc.x as usize) * 4;
                let canonical = ((y * 3 + x) * 4) as usize;
                source[start..start + 4].copy_from_slice(&expected[canonical..canonical + 4]);
            }
        }
        let mut target = vec![0xab; 36];
        assert!(copy_pixels(&source, &mut target, 20, size, transform));
        assert_eq!(&target[..12], &expected[..12]);
        assert_eq!(&target[20..32], &expected[12..]);
        assert_eq!(&target[12..20], &[0xab; 8]);
        assert_eq!(&target[32..], &[0xab; 4]);
        for (source, stride, len) in [
            (&source[..23], 20, 36),
            (&source[..], 11, 36),
            (&source[..], 20, 31),
        ] {
            let mut invalid = vec![0xab; len];
            assert!(!copy_pixels(source, &mut invalid, stride, size, transform));
            assert!(invalid.iter().all(|byte| *byte == 0xab));
        }
    }
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn framebuffer_readback_matches_canonical_pixels_for_every_output_transform() {
    use smithay::backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{texture::TextureRenderElement, Id, Kind},
            gles::GlesTexture,
            Bind, Color32F, ExportMem, ImportMem, Offscreen, Renderer,
        },
    };
    let egl = crate::test_support::EglTest::new();
    let mut renderer = egl.renderer();
    let size: Size<i32, Physical> = (12, 8).into();
    let pixels: Vec<u8> = (0..8)
        .flat_map(|y| (0..12).flat_map(move |x| [x * 17, y * 23, x + y, 255]))
        .collect();
    let source = renderer
        .import_memory(&pixels, Fourcc::Abgr8888, (12, 8).into(), false)
        .unwrap();
    for transform in TRANSFORMS {
        let raw_size = transform.transform_size(size);
        let mut target = Offscreen::<GlesTexture>::create_buffer(
            &mut renderer,
            Fourcc::Abgr8888,
            (raw_size.w, raw_size.h).into(),
        )
        .unwrap();
        let element = TextureRenderElement::from_static_texture(
            Id::new(),
            renderer.context_id(),
            (0.0, 0.0),
            source.clone(),
            1,
            Transform::Normal,
            None,
            None,
            None,
            None,
            Kind::Unspecified,
        );
        let mut framebuffer = renderer.bind(&mut target).unwrap();
        OutputDamageTracker::new(raw_size, 1.0, transform.invert())
            .render_output(
                &mut renderer,
                &mut framebuffer,
                0,
                &[element],
                Color32F::BLACK,
            )
            .unwrap();
        for region in [
            Rectangle::from_size(size),
            Rectangle::new((3, 2).into(), (5, 3).into()),
        ] {
            let mapping = crate::render::read_framebuffer(
                &mut renderer,
                &framebuffer,
                size,
                region,
                transform,
            )
            .unwrap();
            let mapped = renderer.map_texture(&mapping).unwrap();
            let mut actual = vec![0; region.size.w as usize * region.size.h as usize * 4];
            assert!(copy_pixels(
                mapped,
                &mut actual,
                region.size.w as usize * 4,
                region.size,
                transform
            ));
            let expected: Vec<u8> = (region.loc.y..region.loc.y + region.size.h)
                .flat_map(|y| {
                    (region.loc.x..region.loc.x + region.size.w)
                        .flat_map(move |x| [(x + y) as u8, (y * 23) as u8, (x * 17) as u8, 255])
                })
                .collect();
            for (actual, expected) in actual.chunks_exact(4).zip(expected.chunks_exact(4)) {
                assert_eq!(
                    &actual[..3],
                    &expected[..3],
                    "transform={transform:?}, region={region:?}"
                );
            }
        }
    }
}
