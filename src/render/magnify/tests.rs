use smithay::backend::renderer::utils::CommitCounter;

use super::*;

#[test]
fn magnification_keeps_the_pointer_anchored_and_the_source_inside_the_output() {
    let size = Size::from((1000, 600));
    for pointer in [
        Point::from((0.0, 0.0)),
        (500.0, 300.0).into(),
        (999.0, 599.0).into(),
    ] {
        let source = source_rect(size, pointer, 2.0);
        assert_eq!((pointer.x - source.loc.x) * 2.0, pointer.x);
        assert_eq!((pointer.y - source.loc.y) * 2.0, pointer.y);
        assert!(source.loc.x >= 0.0 && source.loc.x + source.size.w <= 1000.0);
        assert!(source.loc.y >= 0.0 && source.loc.y + source.size.h <= 600.0);
    }
}

fn renderer() -> GlesRenderer {
    use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
    let device = EGLDevice::enumerate().unwrap().next().unwrap();
    let display = unsafe { EGLDisplay::new(device).unwrap() };
    let context = EGLContext::new(&display).unwrap();
    unsafe { GlesRenderer::new(context).unwrap() }
}

fn texture_id(magnifier: &mut Magnifier) -> u32 {
    let mut id = 0;
    magnifier
        .buffer
        .render()
        .draw(|texture| {
            id = texture.tex_id();
            Ok::<_, ()>(Vec::new())
        })
        .unwrap();
    id
}

fn draw(
    renderer: &mut GlesRenderer,
    target: &mut GlesTexture,
    tracker: &mut OutputDamageTracker,
    element: TextureRenderElement<GlesTexture>,
    age: usize,
) -> Option<Vec<Rectangle<i32, Physical>>> {
    let mut framebuffer = renderer.bind(target).unwrap();
    tracker
        .render_output(renderer, &mut framebuffer, age, &[element], CLEAR_COLOR)
        .unwrap()
        .damage
        .cloned()
}

fn pixels(renderer: &mut GlesRenderer, target: &mut GlesTexture) -> Vec<u8> {
    use smithay::backend::renderer::{ExportMem, Texture};
    let region = Rectangle::from_size(target.size());
    let framebuffer = renderer.bind(target).unwrap();
    let mapping = renderer
        .copy_framebuffer(&framebuffer, region, Fourcc::Abgr8888)
        .unwrap();
    renderer.map_texture(&mapping).unwrap().to_vec()
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn magnifier_reuses_resources_and_tracks_scene_damage_and_panning() {
    use smithay::backend::renderer::element::{solid::SolidColorRenderElement, Element, Id};

    let mut renderer = renderer();
    let size = Size::from((64, 64));
    let colors = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
        [1.0, 1.0, 1.0, 1.0],
    ];
    let mut elements: Vec<OutputRenderElement> = colors
        .into_iter()
        .enumerate()
        .map(|(index, color)| {
            SolidColorRenderElement::new(
                Id::new(),
                Rectangle::new(
                    (((index % 2) * 32) as i32, ((index / 2) * 32) as i32).into(),
                    (32, 32).into(),
                ),
                CommitCounter::default(),
                color,
                Kind::Unspecified,
            )
            .into()
        })
        .collect();
    let overlay = Id::new();
    elements.insert(
        0,
        SolidColorRenderElement::new(
            overlay.clone(),
            Rectangle::new((8, 8).into(), (4, 4).into()),
            CommitCounter::default(),
            [1.0, 0.0, 0.0, 1.0],
            Kind::Unspecified,
        )
        .into(),
    );
    let source = source_rect(size, (0.0, 0.0).into(), 2.0);
    let mut magnifier = Magnifier::new(&mut renderer, size, 1.0).unwrap();
    let texture = texture_id(&mut magnifier);
    let element = magnifier
        .render(&mut renderer, &elements, (64, 64).into(), source)
        .unwrap();
    let id = element.id().clone();
    let commit = element.current_commit();
    let mut target =
        Offscreen::<GlesTexture>::create_buffer(&mut renderer, Fourcc::Abgr8888, (64, 64).into())
            .unwrap();
    let mut tracker = OutputDamageTracker::new(size, 1.0, Transform::Normal);
    assert_eq!(
        draw(&mut renderer, &mut target, &mut tracker, element, 0),
        Some(vec![Rectangle::from_size(size)])
    );
    let initial = pixels(&mut renderer, &mut target);
    for (x, y) in [(8, 8), (56, 8), (8, 56), (56, 56)] {
        assert_eq!(
            &initial[(y * 64 + x) * 4..(y * 64 + x + 1) * 4],
            &[255, 0, 0, 255]
        );
    }
    for _ in 0..8 {
        let element = magnifier
            .render(&mut renderer, &elements, (64, 64).into(), source)
            .unwrap();
        assert_eq!(element.id(), &id);
        assert_eq!(element.current_commit(), commit);
        assert!(element.damage_since(1.0.into(), Some(commit)).is_empty());
        assert!(draw(&mut renderer, &mut target, &mut tracker, element, 1).is_none());
        assert_eq!(texture_id(&mut magnifier), texture);
        for element in &elements {
            assert!(magnifier.states.element_was_presented(element.id().clone()));
        }
    }
    let mut overlay_commit = CommitCounter::default();
    overlay_commit.increment();
    elements[0] = SolidColorRenderElement::new(
        overlay,
        Rectangle::new((8, 8).into(), (4, 4).into()),
        overlay_commit,
        [1.0, 1.0, 0.0, 1.0],
        Kind::Unspecified,
    )
    .into();
    let element = magnifier
        .render(&mut renderer, &elements, (64, 64).into(), source)
        .unwrap();
    assert_eq!(element.id(), &id);
    assert_ne!(element.current_commit(), commit);
    let damage: Vec<_> = element
        .damage_since(1.0.into(), Some(commit))
        .into_iter()
        .collect();
    assert_eq!(damage, vec![Rectangle::new((16, 16).into(), (8, 8).into())]);
    let commit = element.current_commit();
    assert_eq!(
        draw(&mut renderer, &mut target, &mut tracker, element, 1),
        Some(damage)
    );
    let updated = pixels(&mut renderer, &mut target);
    assert_eq!(
        &updated[(20 * 64 + 20) * 4..(20 * 64 + 21) * 4],
        &[255, 255, 0, 255]
    );
    assert_eq!(
        &updated[(56 * 64 + 56) * 4..(56 * 64 + 57) * 4],
        &[255, 0, 0, 255]
    );

    let source = source_rect(size, (64.0, 0.0).into(), 2.0);
    let element = magnifier
        .render(&mut renderer, &elements, (64, 64).into(), source)
        .unwrap();
    assert_eq!(element.id(), &id);
    assert_eq!(element.current_commit(), commit);
    assert_eq!(texture_id(&mut magnifier), texture);
    assert_eq!(
        draw(&mut renderer, &mut target, &mut tracker, element, 1),
        Some(vec![Rectangle::from_size(size)])
    );
    let panned = pixels(&mut renderer, &mut target);
    assert_eq!(
        &panned[(32 * 64 + 32) * 4..(32 * 64 + 33) * 4],
        &[0, 255, 0, 255]
    );

    let element = magnifier
        .render(&mut renderer, &[], (64, 64).into(), source)
        .unwrap();
    assert!(draw(&mut renderer, &mut target, &mut tracker, element, 1).is_some());
    let cleared = pixels(&mut renderer, &mut target);
    assert!(cleared
        .chunks_exact(4)
        .all(|pixel| pixel == [20, 20, 31, 255]));
    assert_eq!(texture_id(&mut magnifier), texture);
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn magnifier_cache_follows_output_modes_renderer_contexts_and_configuration() {
    use crate::render::{output_elements, CursorMode};
    use smithay::{
        backend::renderer::element::Element,
        output::{Mode, Scale},
    };

    let (_display, _event_loop, mut state) = crate::state::tests::fixture();
    let output = crate::test_support::add_output(&mut state, "first", (0, 0), (64, 64));
    let second = crate::test_support::add_output(&mut state, "second", (64, 0), (64, 64));
    state.config.accessibility.zoom = true;
    state.config.accessibility.zoom_factor = 2.0;
    let mut first_renderer = renderer();
    let elements = output_elements(
        &mut first_renderer,
        &mut state,
        &output,
        None,
        CursorMode::Hidden,
    );
    let first_id = elements[0].id().clone();
    let first_texture = texture_id(state.magnifiers.get_mut(&output).unwrap());
    let elements = output_elements(
        &mut first_renderer,
        &mut state,
        &output,
        None,
        CursorMode::Hidden,
    );
    assert_eq!(elements[0].id(), &first_id);
    assert_eq!(
        texture_id(state.magnifiers.get_mut(&output).unwrap()),
        first_texture
    );
    let elements = output_elements(
        &mut first_renderer,
        &mut state,
        &second,
        None,
        CursorMode::Hidden,
    );
    assert_ne!(elements[0].id(), &first_id);
    assert_eq!(state.magnifiers.len(), 2);

    output.change_current_state(
        Some(Mode {
            size: (32, 48).into(),
            refresh: 60_000,
        }),
        None,
        None,
        None,
    );
    let elements = output_elements(
        &mut first_renderer,
        &mut state,
        &output,
        None,
        CursorMode::Hidden,
    );
    let resized_id = elements[0].id().clone();
    assert_ne!(resized_id, first_id);
    assert_eq!(state.magnifiers[&output].size, (32, 48).into());
    output.change_current_state(None, None, Some(Scale::Fractional(1.5)), None);
    let elements = output_elements(
        &mut first_renderer,
        &mut state,
        &output,
        None,
        CursorMode::Hidden,
    );
    let scaled_id = elements[0].id().clone();
    assert_ne!(scaled_id, resized_id);
    assert_eq!(state.magnifiers[&output].scale, 1.5);
    state.config.accessibility.zoom_factor = 3.0;
    let elements = output_elements(
        &mut first_renderer,
        &mut state,
        &output,
        None,
        CursorMode::Hidden,
    );
    assert_eq!(elements[0].id(), &scaled_id);

    let mut second_renderer = renderer();
    let elements = output_elements(
        &mut second_renderer,
        &mut state,
        &output,
        None,
        CursorMode::Hidden,
    );
    assert_ne!(elements[0].id(), &scaled_id);
    assert_eq!(
        state.magnifiers[&output].context,
        second_renderer.context_id()
    );
    state.space.unmap_output(&output);
    state.refresh();
    assert!(!state.magnifiers.contains_key(&output));
    assert!(state.magnifiers.contains_key(&second));
    state.config.accessibility.zoom = false;
    state.refresh();
    assert!(state.magnifiers.is_empty());
    state.config.accessibility.zoom = true;
    output_elements(
        &mut second_renderer,
        &mut state,
        &second,
        None,
        CursorMode::Hidden,
    );
    assert_eq!(state.magnifiers.len(), 1);
    state.config.accessibility.zoom_factor = 1.0;
    state.refresh();
    assert!(state.magnifiers.is_empty());
}
