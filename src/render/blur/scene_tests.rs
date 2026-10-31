use super::*;
use crate::{
    config::{BorderColorMode, DecorationModeConfig, WindowLayout},
    render::{capture_region, CursorMode},
    test_support::{add_output, EglTest, TestClient},
};
use blair_blur_protocol::client::blair_blur_manager_v1::BlairBlurManagerV1;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{Bind, ExportMem, ImportMem},
    },
    reexports::wayland_server::Display,
    wayland::shell::wlr_layer::Layer,
};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_surface::WlSurface};
use wayland_protocols::{
    wp::{
        single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
        viewporter::client::wp_viewporter::WpViewporter,
    },
    xdg::shell::client::{
        xdg_positioner::{Anchor as PopupAnchor, Gravity},
        xdg_wm_base::XdgWmBase,
    },
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{self, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::Anchor;

fn attach_transparent(client: &TestClient, surface: &WlSurface, width: i32, height: i32) {
    let buffer = client
        .bind::<WpSinglePixelBufferManagerV1>(1)
        .create_u32_rgba_buffer(0, 0, 0, 0, &client.handle(), ());
    client
        .bind::<WpViewporter>(1)
        .get_viewport(surface, &client.handle(), ())
        .set_destination(width, height);
    surface.attach(Some(&buffer), 0, 0);
    surface.commit();
}

fn layer(
    display: &mut Display<BlairState>,
    state: &mut BlairState,
    client: &mut TestClient,
    layer: zwlr_layer_shell_v1::Layer,
    blur: bool,
) -> WlSurface {
    let surface = client
        .bind::<WlCompositor>(6)
        .create_surface(&client.handle(), ());
    let shell = client.bind::<ZwlrLayerShellV1>(4).get_layer_surface(
        &surface,
        None,
        layer,
        "blur-test".into(),
        &client.handle(),
        (),
    );
    shell.set_size(if blur { 64 } else { 32 }, 48);
    shell.set_anchor(Anchor::Top | Anchor::Left);
    surface.commit();
    client.dispatch(display, state);
    if blur {
        attach_transparent(client, &surface, 64, 48);
        client
            .bind::<BlairBlurManagerV1>(1)
            .get_blur(&surface, &client.handle(), ())
            .commit();
    } else {
        client.paint_surface(&surface, 32, 48);
    }
    client.dispatch(display, state);
    surface
}

fn filtered(renderer: &mut GlesRenderer, shaders: &Shaders, pixels: &[u8]) -> Vec<u8> {
    let size = (64, 48).into();
    let texture = renderer
        .import_memory(pixels, Fourcc::Abgr8888, (64, 48).into(), false)
        .unwrap();
    let texture = horizontal_blur(
        renderer,
        texture,
        &shaders.blur,
        2.0,
        Rectangle::from_size(size),
    )
    .unwrap();
    let inner = TextureRenderElement::from_static_texture(
        Id::new(),
        renderer.context_id(),
        (0.0, 0.0),
        texture,
        1,
        Transform::Normal,
        None,
        None,
        None,
        None,
        Kind::Unspecified,
    );
    let element = BlurredBackdropElement::new(
        inner,
        shaders.blur.clone(),
        (1.0 / 64.0, 1.0 / 48.0),
        2.0,
        vec![Rectangle::from_size(size)],
        RoundedClip {
            rect: Rectangle::from_size(size),
            radii: [0.0; 4],
            viewport: size,
        },
    );
    let mut texture = super::super::capture::render_region(
        renderer,
        &mut [element.into()],
        Rectangle::from_size(size),
        1.0,
    )
    .unwrap();
    let framebuffer = renderer.bind(&mut texture).unwrap();
    let mapping = renderer
        .copy_framebuffer(
            &framebuffer,
            Rectangle::from_size((64, 48).into()),
            Fourcc::Abgr8888,
        )
        .unwrap();
    renderer.map_texture(&mapping).unwrap().to_vec()
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn window_and_titlebar_blur_stay_inside_rounded_content() {
    use crate::config::TitlebarColorMode;
    use crate::render::{output_elements, window_frame};
    use smithay::backend::renderer::{damage::OutputDamageTracker, Color32F, Offscreen};

    let egl = EglTest::new();
    let (mut display, _event_loop, mut state) = crate::state::tests::fixture();
    let mut config = state.config.clone();
    config.animations.enabled = false;
    config.window.layout = WindowLayout::Floating;
    config.decorations.corner_radius = 12;
    config.decorations.titlebar_color = TitlebarColorMode::Theme;
    config.blur.radius = 2;
    state.apply_config(config).unwrap();
    let output = add_output(&mut state, "test", (0, 0), (128, 128));
    let mut client = TestClient::new(&mut display, &mut state);
    let surface = client
        .bind::<WlCompositor>(6)
        .create_surface(&client.handle(), ());
    let shell = client
        .bind::<XdgWmBase>(6)
        .get_xdg_surface(&surface, &client.handle(), ());
    let _toplevel = shell.get_toplevel(&client.handle(), ());
    surface.commit();
    client.dispatch(&mut display, &mut state);
    attach_transparent(&client, &surface, 40, 32);
    client
        .bind::<BlairBlurManagerV1>(1)
        .get_blur(&surface, &client.handle(), ())
        .commit();
    client.dispatch(&mut display, &mut state);
    let id = *state.windows.keys().next().unwrap();
    let window = state.windows[&id].window.clone();
    let mut renderer = egl.renderer();
    let shaders = Shaders::compile(&mut renderer).unwrap();

    for (mode, titlebar_height) in [
        (DecorationModeConfig::Server, 16),
        (DecorationModeConfig::Server, 0),
        (DecorationModeConfig::Client, 0),
        (DecorationModeConfig::Hybrid, 0),
    ] {
        let mut config = state.config.clone();
        config.decorations.mode = mode;
        config.decorations.titlebar_height = titlebar_height;
        state.apply_config(config).unwrap();
        state.windows.get_mut(&id).unwrap().rules.opacity = Some(0.55);
        for scale in [1.0, 1.25, 2.0] {
            output.change_current_state(
                None,
                None,
                Some(smithay::output::Scale::Fractional(scale)),
                None,
            );
            state.output_changed(&output);
            state.move_window(&window, (12, 24).into());
            let frame = window_frame(&state, &window).unwrap();
            let elements = output_elements(
                &mut renderer,
                &mut state,
                &output,
                Some(&shaders),
                CursorMode::Hidden,
            );
            let blurs: Vec<_> = elements
                .into_iter()
                .filter(|element| matches!(element, OutputRenderElement::Blur(_)))
                .collect();
            assert_eq!(blurs.len(), if titlebar_height > 0 { 2 } else { 1 });
            for element in blurs {
                let bounds = element.geometry(scale.into());
                let titlebar = bounds.loc.y < frame.client.to_physical_precise_round(scale).loc.y;
                let mut texture = Offscreen::<GlesTexture>::create_buffer(
                    &mut renderer,
                    Fourcc::Abgr8888,
                    (128, 128).into(),
                )
                .unwrap();
                let mut target = renderer.bind(&mut texture).unwrap();
                OutputDamageTracker::new((128, 128), scale, Transform::Normal)
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
                        Rectangle::from_size((128, 128).into()),
                        Fourcc::Abgr8888,
                    )
                    .unwrap();
                let pixels = renderer.map_texture(&mapping).unwrap().to_vec();
                drop(target);
                let alpha = |x, y| pixels[((y * 128 + x) * 4 + 3) as usize];
                let left = bounds.loc.x;
                let right = left + bounds.size.w - 1;
                let top = bounds.loc.y;
                let bottom = top + bounds.size.h - 1;
                assert_eq!(alpha((left + right) / 2, (top + bottom) / 2), 255);
                let inset = if frame.has_border {
                    state.decoration_theme().border_width
                } else {
                    0
                };
                let radius =
                    (state.corner_radius(frame.frame) - inset).max(0) as f32 * scale as f32;
                for (at_right, at_bottom, rounded) in [
                    (false, false, titlebar || !frame.has_titlebar),
                    (true, false, titlebar || !frame.has_titlebar),
                    (false, true, !titlebar),
                    (true, true, !titlebar),
                ] {
                    if !rounded {
                        continue;
                    }
                    for dy in 0..(radius.ceil() as i32).min(bounds.size.h) {
                        for dx in 0..(radius.ceil() as i32).min(bounds.size.w) {
                            let x = if at_right { right - dx } else { left + dx };
                            let y = if at_bottom { bottom - dy } else { top + dy };
                            let distance =
                                (dx as f32 + 0.5 - radius).hypot(dy as f32 + 0.5 - radius) - radius;
                            let expected = ((0.5 - distance).clamp(0.0, 1.0) * 255.0).round() as u8;
                            assert!(
                                alpha(x, y).abs_diff(expected) <= 1,
                                "mode={mode:?}, scale={scale}, titlebar={titlebar}, pixel=({x},{y}), alpha={}, expected={expected}",
                                alpha(x, y)
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn windows_and_layers_capture_blurred_predecessors_in_stacking_order() {
    let egl = EglTest::new();
    let mut renderer = egl.renderer();
    let shaders = Shaders::compile(&mut renderer).unwrap();
    let (mut display, _event_loop, mut state) = crate::state::tests::fixture();
    let mut config = state.config.clone();
    config.animations.enabled = false;
    config.window.layout = WindowLayout::Floating;
    config.decorations.mode = DecorationModeConfig::Server;
    config.decorations.border = BorderColorMode::None;
    config.decorations.titlebar_height = 0;
    config.decorations.corner_radius = 0;
    config.blur.radius = 2;
    state.apply_config(config).unwrap();
    let output = add_output(&mut state, "test", (0, 0), (64, 48));
    let mut client = TestClient::new(&mut display, &mut state);
    let _background = layer(
        &mut display,
        &mut state,
        &mut client,
        zwlr_layer_shell_v1::Layer::Background,
        false,
    );
    let capture = |renderer: &mut GlesRenderer, state: &mut BlairState| {
        capture_region(
            renderer,
            state,
            &output,
            Some(&shaders),
            Rectangle::from_size((64, 48).into()),
            CursorMode::Hidden,
            Fourcc::Abgr8888,
        )
        .unwrap()
    };
    let mut previous = capture(&mut renderer, &mut state);
    for stage in [
        Some(Layer::Bottom),
        None,
        Some(Layer::Top),
        Some(Layer::Top),
        Some(Layer::Overlay),
    ] {
        let mut parent = None;
        let expected = filtered(&mut renderer, &shaders, &previous);
        if let Some(stage) = stage {
            layer(
                &mut display,
                &mut state,
                &mut client,
                match stage {
                    Layer::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
                    Layer::Top => zwlr_layer_shell_v1::Layer::Top,
                    Layer::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
                    _ => unreachable!(),
                },
                true,
            );
        } else {
            let surface = client
                .bind::<WlCompositor>(6)
                .create_surface(&client.handle(), ());
            let shell = client
                .bind::<XdgWmBase>(6)
                .get_xdg_surface(&surface, &client.handle(), ());
            let _toplevel = shell.get_toplevel(&client.handle(), ());
            surface.commit();
            client.dispatch(&mut display, &mut state);
            attach_transparent(&client, &surface, 64, 48);
            client
                .bind::<BlairBlurManagerV1>(1)
                .get_blur(&surface, &client.handle(), ())
                .commit();
            client.dispatch(&mut display, &mut state);
            let window = state.windows.values().next().unwrap().window.clone();
            state.move_window(&window, (0, 0).into());
            let frame = super::super::window_frame(&state, &window).unwrap();
            assert_eq!(frame.client, Rectangle::from_size((64, 48).into()));
            assert_eq!(state.decoration_theme().border_width, 0);
            assert_eq!(state.decoration_theme().titlebar_height, 0);
            parent = Some((surface, shell));
        }
        let actual = capture(&mut renderer, &mut state);
        let difference = actual
            .iter()
            .zip(&expected)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(difference <= 2, "stage={stage:?}, difference={difference}");
        assert_ne!(previous, actual);
        assert_eq!(actual, capture(&mut renderer, &mut state));
        if let Some((surface, shell)) = parent {
            let wm = client.bind::<XdgWmBase>(6);
            let popup_surface = client
                .bind::<WlCompositor>(6)
                .create_surface(&client.handle(), ());
            let popup_shell = wm.get_xdg_surface(&popup_surface, &client.handle(), ());
            let positioner = wm.create_positioner(&client.handle(), ());
            positioner.set_size(30, 30);
            positioner.set_anchor_rect(16, 8, 1, 1);
            positioner.set_anchor(PopupAnchor::BottomRight);
            positioner.set_gravity(Gravity::BottomRight);
            let popup = popup_shell.get_popup(Some(&shell), &positioner, &client.handle(), ());
            popup_surface.commit();
            client.dispatch(&mut display, &mut state);
            attach_transparent(&client, &popup_surface, 30, 30);
            let manager = client.bind::<BlairBlurManagerV1>(1);
            manager
                .get_blur(&popup_surface, &client.handle(), ())
                .commit();
            client.dispatch(&mut display, &mut state);
            let blurred_popup = capture(&mut renderer, &mut state);
            assert_ne!(blurred_popup, actual);
            let (_, offset) =
                smithay::desktop::PopupManager::popups_for_surface(&client.surface(&surface))
                    .next()
                    .unwrap();
            let bounds = Rectangle::new(offset, (30, 30).into());
            for y in 0..48 {
                for x in 0..64 {
                    if !bounds.contains((x, y)) {
                        let start = ((y * 64 + x) * 4) as usize;
                        assert_eq!(&blurred_popup[start..start + 4], &actual[start..start + 4]);
                    }
                }
            }
            manager.unset(&popup_surface);
            client.dispatch(&mut display, &mut state);
            assert_eq!(actual, capture(&mut renderer, &mut state));
            popup.destroy();
            popup_shell.destroy();
            popup_surface.destroy();
            client.dispatch(&mut display, &mut state);
            assert_eq!(actual, capture(&mut renderer, &mut state));
        }
        previous = actual;
    }
    state.config.blur.enabled = false;
    let disabled = capture(&mut renderer, &mut state);
    assert_eq!(
        &disabled[(24 * 64 + 16) * 4..(24 * 64 + 16) * 4 + 4],
        &[255, 0, 0, 255]
    );
    assert_ne!(disabled, previous);
}
