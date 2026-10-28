use super::*;
use crate::render::decoration::{ButtonGlyph, ButtonLook};

fn key(index: usize) -> ButtonKey {
    ButtonKey {
        look: ButtonLook {
            glyph: ButtonGlyph::Close,
            glyph_visible: false,
            disc: [index as u8, (index / 256) as u8, 0, 255],
            glyph_color: [0; 4],
        },
        size: 1,
        buffer_scale: 1,
    }
}

#[test]
fn buttons_share_buffers_without_retaining_unused_pixels() {
    let mut buffers = ButtonBuffers::default();
    let key = ButtonKey {
        size: 24,
        ..key(255)
    };
    let mut first = ButtonCache::default();
    let mut second = ButtonCache::default();
    first.update(key, &mut buffers);
    assert_eq!(
        crate::test_support::count_allocations(|| second.update(key, &mut buffers)),
        0
    );
    assert!(Rc::ptr_eq(
        first.buffer.as_ref().unwrap(),
        second.buffer.as_ref().unwrap()
    ));
    let weak = Rc::downgrade(first.buffer.as_ref().unwrap());
    first.clear();
    assert!(weak.upgrade().is_some());
    second.clear();
    assert!(weak.upgrade().is_none());
    assert!(buffers.entries[&key].buffer.upgrade().is_none());
    let replacement = buffers.get(key).unwrap();
    assert!(!weak.ptr_eq(&Rc::downgrade(&replacement)));
    assert_eq!(buffers.entries.len(), 1);
}

#[test]
fn button_variants_are_distinct_and_invalid_dimensions_do_not_allocate_pixels() {
    let mut buffers = ButtonBuffers::default();
    let original_key = ButtonKey {
        look: ButtonLook {
            glyph_visible: true,
            glyph_color: [255; 4],
            ..key(0).look
        },
        size: 20,
        ..key(0)
    };
    let original = buffers.get(original_key).unwrap();
    for key in [
        ButtonKey {
            size: 21,
            ..original_key
        },
        ButtonKey {
            buffer_scale: 2,
            ..original_key
        },
        ButtonKey {
            look: ButtonLook {
                glyph_visible: false,
                ..original_key.look
            },
            ..original_key
        },
        ButtonKey {
            look: ButtonLook {
                glyph: ButtonGlyph::Maximize,
                ..original_key.look
            },
            ..original_key
        },
        ButtonKey {
            look: ButtonLook {
                disc: [255; 4],
                ..original_key.look
            },
            ..original_key
        },
        ButtonKey {
            look: ButtonLook {
                glyph_color: [30, 30, 30, 255],
                ..original_key.look
            },
            ..original_key
        },
    ] {
        let changed = buffers.get(key).unwrap();
        assert!(!Rc::ptr_eq(&changed, &original));
        assert!(Rc::ptr_eq(&changed, &buffers.get(key).unwrap()));
    }
    for (size, buffer_scale) in [(0, 1), (-1, 1), (1, 0), (1, -1), (i32::MAX, 2)] {
        assert!(buffers
            .get(ButtonKey {
                size,
                buffer_scale,
                ..original_key
            })
            .is_none());
    }
}

#[test]
fn hidden_glyphs_and_transparent_colors_share_pixel_buffers() {
    let mut buffers = ButtonBuffers::default();
    let hidden = ButtonKey { size: 20, ..key(1) };
    let first = buffers.get(hidden).unwrap();
    let second = buffers
        .get(ButtonKey {
            look: ButtonLook {
                glyph: ButtonGlyph::Minimize,
                glyph_color: [255; 4],
                ..hidden.look
            },
            ..hidden
        })
        .unwrap();
    assert!(Rc::ptr_eq(&first, &second));
    let visible = ButtonKey {
        look: ButtonLook {
            glyph_visible: true,
            glyph_color: [255; 4],
            disc: [255, 0, 0, 0],
            ..hidden.look
        },
        ..hidden
    };
    let first = buffers.get(visible).unwrap();
    let second = buffers
        .get(ButtonKey {
            look: ButtonLook {
                disc: [0, 0, 255, 0],
                ..visible.look
            },
            ..visible
        })
        .unwrap();
    assert!(Rc::ptr_eq(&first, &second));
}

#[test]
fn weak_index_stays_bounded_and_eviction_preserves_live_buffers() {
    let mut buffers = ButtonBuffers::default();
    let live: Vec<_> = (0..MAX_ENTRIES)
        .map(|index| buffers.get(key(index)).unwrap())
        .collect();
    assert!(Rc::ptr_eq(&buffers.get(key(1)).unwrap(), &live[1]));
    let extra = buffers.get(key(MAX_ENTRIES)).unwrap();
    assert_eq!(buffers.entries.len(), MAX_ENTRIES);
    assert!(!buffers.entries.contains_key(&key(0)));
    assert!(buffers.entries.contains_key(&key(1)));
    let weak = Rc::downgrade(&live[0]);
    assert!(weak.upgrade().is_some());
    drop(live);
    assert!(weak.upgrade().is_none());
    drop(extra);
    for index in 0..4096 {
        buffers.get(key(index));
        assert!(buffers.entries.len() <= MAX_ENTRIES);
    }
    assert!(buffers
        .entries
        .values()
        .all(|entry| entry.buffer.upgrade().is_none()));
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn shared_buttons_keep_independent_damage_and_disappear_without_ghosts() {
    use smithay::backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker, gles::GlesTexture, Bind, Color32F, ExportMem, Offscreen,
        },
    };
    let egl = crate::test_support::EglTest::new();
    let mut renderer = egl.renderer();
    let mut buffers = ButtonBuffers::default();
    let key = ButtonKey {
        size: 24,
        ..key(255)
    };
    let mut first = ButtonCache::default();
    let mut second = ButtonCache::default();
    first.update(key, &mut buffers);
    second.update(key, &mut buffers);
    let a = first
        .element(&mut renderer, (0.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    let b = second
        .element(&mut renderer, (32.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    assert_eq!(a.element.id(), b.element.id());
    assert_ne!(a.id(), b.id());
    let first_id = a.id().clone();
    let first_commit = a.current_commit();
    let second_commit = b.current_commit();
    let mut texture =
        Offscreen::<GlesTexture>::create_buffer(&mut renderer, Fourcc::Abgr8888, (64, 32).into())
            .unwrap();
    let mut tracker = OutputDamageTracker::new((64, 32), 1.0, Transform::Normal);
    let mut render = |renderer: &mut GlesRenderer, elements: &[ButtonRenderElement], age| {
        let mut target = renderer.bind(&mut texture).unwrap();
        let damage = tracker
            .render_output(
                renderer,
                &mut target,
                age,
                elements,
                Color32F::new(0.0, 0.0, 0.0, 0.0),
            )
            .unwrap()
            .damage
            .cloned();
        let mapping = renderer
            .copy_framebuffer(
                &target,
                Rectangle::from_size((64, 32).into()),
                Fourcc::Abgr8888,
            )
            .unwrap();
        (damage, renderer.map_texture(&mapping).unwrap().to_vec())
    };
    let elements = vec![a, b];
    let (damage, pixels) = render(&mut renderer, &elements, 0);
    assert!(damage.is_some());
    let pixel = |pixels: &[u8], x: usize, y: usize| {
        let start = (y * 64 + x) * 4;
        <[u8; 4]>::try_from(&pixels[start..start + 4]).unwrap()
    };
    assert_eq!(pixel(&pixels, 12, 12), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 44, 12), [255, 0, 0, 255]);
    assert!(render(&mut renderer, &elements, 1).0.is_none());

    second.clear();
    let a = first
        .element(&mut renderer, (0.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    let (damage, pixels) = render(&mut renderer, &[a], 1);
    assert!(damage.is_some());
    assert_eq!(pixel(&pixels, 12, 12), [255, 0, 0, 255]);
    assert_eq!(pixel(&pixels, 44, 12), [0; 4]);
    second.update(key, &mut buffers);
    let a = first
        .element(&mut renderer, (0.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    let b = second
        .element(&mut renderer, (32.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    render(&mut renderer, &[a, b], 1);

    let blue = ButtonKey {
        look: ButtonLook {
            disc: [0, 0, 255, 255],
            ..key.look
        },
        ..key
    };
    first.update(blue, &mut buffers);
    let a = first
        .element(&mut renderer, (0.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    let b = second
        .element(&mut renderer, (32.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    assert_eq!(a.id(), &first_id);
    assert_ne!(a.current_commit(), first_commit);
    assert_ne!(b.current_commit(), second_commit);
    let unchanged_second_commit = b.current_commit();
    let (damage, pixels) = render(&mut renderer, &[a, b], 1);
    assert!(damage
        .unwrap()
        .iter()
        .all(|rect| rect.loc.x + rect.size.w <= 24));
    assert_eq!(pixel(&pixels, 12, 12), [0, 0, 255, 255]);
    assert_eq!(pixel(&pixels, 44, 12), [255, 0, 0, 255]);
    let a = first
        .element(&mut renderer, (0.0, 0.0).into(), 1.0)
        .unwrap()
        .unwrap();
    let b = second
        .element(&mut renderer, (36.0, 0.0).into(), 0.5)
        .unwrap()
        .unwrap();
    assert_eq!(b.current_commit(), unchanged_second_commit);
    let (_, pixels) = render(&mut renderer, &[a, b], 1);
    assert_eq!(pixel(&pixels, 48, 12), [128, 0, 0, 128]);
    assert_eq!(pixel(&pixels, 32, 12), [0; 4]);
}
