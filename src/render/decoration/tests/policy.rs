use smithay::{
    desktop::Window,
    output::Output,
    reexports::{calloop::EventLoop, wayland_server::Display},
};
use wayland_client::protocol::{wl_compositor::WlCompositor, wl_surface::WlSurface};
use wayland_protocols::xdg::{
    decoration::zv1::client::{
        zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        zxdg_toplevel_decoration_v1::{Mode, ZxdgToplevelDecorationV1},
    },
    shell::client::{xdg_surface::XdgSurface, xdg_toplevel::XdgToplevel, xdg_wm_base::XdgWmBase},
};

use super::*;
use crate::{
    config::{CompositorConfig, DecorationButton, DecorationModeConfig, WindowLayout},
    render::{to_rect, window_decoration_parts, window_frame},
    state::BlairState,
    test_support::{add_output, count_allocations, TestClient},
};

struct Fixture {
    display: Display<BlairState>,
    _event_loop: EventLoop<'static, BlairState>,
    state: BlairState,
    output: Output,
    client: TestClient,
    surface: WlSurface,
    shell_surface: XdgSurface,
    toplevel: XdgToplevel,
    id: blair_protocol::WindowId,
}

impl Fixture {
    fn new() -> Self {
        let (mut display, event_loop, mut state) = crate::state::tests::fixture();
        let mut config = state.config.clone();
        config.decorations.mode = DecorationModeConfig::Server;
        config.decorations.show_icon = true;
        config.window.layout = WindowLayout::Floating;
        config.workspaces.count = 2;
        state.apply_config(config).unwrap();
        let output = add_output(&mut state, "test", (0, 0), (800, 600));
        let mut client = TestClient::new(&mut display, &mut state);
        let compositor = client.bind::<WlCompositor>(6);
        let wm = client.bind::<XdgWmBase>(6);
        let surface = compositor.create_surface(&client.handle(), ());
        let shell_surface = wm.get_xdg_surface(&surface, &client.handle(), ());
        let toplevel = shell_surface.get_toplevel(&client.handle(), ());
        toplevel.set_title("Terminal".into());
        surface.commit();
        client.dispatch(&mut display, &mut state);
        client.paint_surface(&surface, 400, 240);
        client.dispatch(&mut display, &mut state);
        let id = *state.windows.keys().next().unwrap();
        Self {
            display,
            _event_loop: event_loop,
            state,
            output,
            client,
            surface,
            shell_surface,
            toplevel,
            id,
        }
    }

    fn window(&self) -> Window {
        self.state.windows[&self.id].window.clone()
    }

    fn inactive(&mut self) {
        assert!(self.state.switch_workspace(2));
        assert!(!self
            .state
            .window_visible_on_output(&self.window(), &self.output));
    }

    fn configure(&mut self, change: impl FnOnce(&mut CompositorConfig)) {
        let mut config = self.state.config.clone();
        change(&mut config);
        self.state.apply_config(config).unwrap();
    }

    fn dispatch(&mut self) {
        self.client.dispatch(&mut self.display, &mut self.state);
    }

    fn negotiate(&mut self) -> ZxdgToplevelDecorationV1 {
        let manager = self.client.bind::<ZxdgDecorationManagerV1>(1);
        let decoration = manager.get_toplevel_decoration(&self.toplevel, &self.client.handle(), ());
        self.dispatch();
        decoration
    }

    fn seed_buffers(&mut self) {
        let window = self.window();
        let parts = window_decoration_parts(&self.state, &window);
        let frame = window_frame(&self.state, &window).unwrap();
        let theme = self.state.decoration_theme();
        let geometry = if self.state.window_has_overlay_controls(&window) {
            crate::decorations::DecorationFrame::overlay_controls(to_rect(frame.client), theme)
        } else {
            crate::decorations::DecorationFrame::compute(
                to_rect(frame.client),
                theme,
                frame.has_titlebar,
            )
        };
        let decoration = self.state.decorations.entry(self.id).or_default();
        if parts.title {
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
                    area: (geometry.title.width, geometry.title.height).into(),
                    centered: false,
                    buffer_scale: 1,
                },
            );
        }
        if parts.icon {
            let size = geometry.icon.width;
            let icon = Rc::new(RgbaBitmap {
                pixels: [255; 4].repeat((size * size) as usize),
                width: size,
                height: size,
            });
            decoration.update_icon(Some("test"), Some(&icon), size, 1);
        }
        for (index, (glyph, rect)) in [
            (ButtonGlyph::Close, geometry.close_btn),
            (ButtonGlyph::Maximize, geometry.maximize_btn),
            (ButtonGlyph::Minimize, geometry.minimize_btn),
        ]
        .into_iter()
        .enumerate()
        {
            if parts.buttons[index] {
                let look = ButtonLook {
                    glyph,
                    glyph_visible: true,
                    disc: [0; 4],
                    glyph_color: [255; 4],
                };
                decoration.update_button(
                    index,
                    look,
                    rect.width,
                    1,
                    &mut self.state.button_buffers,
                );
            }
        }
    }

    fn buffers(&self) -> [bool; 5] {
        let decoration = &self.state.decorations[&self.id];
        [
            decoration.title_buffer().is_some(),
            decoration.icon_buffer().is_some(),
            decoration.button_buffer(0).is_some(),
            decoration.button_buffer(1).is_some(),
            decoration.button_buffer(2).is_some(),
        ]
    }
}

#[test]
fn configuration_prunes_inactive_buffers_without_discarding_valid_parts() {
    let mut fixture = Fixture::new();
    fixture.seed_buffers();
    assert_eq!(fixture.buffers(), [true; 5]);
    fixture.inactive();
    let mut renderer = smithay::backend::renderer::pixman::PixmanRenderer::new().unwrap();
    let title = memory_id(
        fixture.state.decorations[&fixture.id]
            .title_buffer()
            .unwrap()
            .0,
        &mut renderer,
    );
    fixture.configure(|config| config.decorations.show_icon = false);
    assert_eq!(fixture.buffers(), [true, false, true, true, true]);
    assert_eq!(
        memory_id(
            fixture.state.decorations[&fixture.id]
                .title_buffer()
                .unwrap()
                .0,
            &mut renderer
        ),
        title
    );
    fixture.configure(|config| config.decorations.buttons.layout = vec![DecorationButton::Close]);
    assert_eq!(fixture.buffers(), [true, false, true, false, false]);
    fixture.configure(|config| config.decorations.titlebar_height = 0);
    assert_eq!(fixture.buffers(), [false; 5]);
}

#[test]
fn hidden_buffers_follow_layout_and_decoration_modes() {
    for layout in [WindowLayout::Floating, WindowLayout::Tiling] {
        for mode in [
            DecorationModeConfig::Server,
            DecorationModeConfig::Hybrid,
            DecorationModeConfig::Client,
            DecorationModeConfig::None,
        ] {
            let mut fixture = Fixture::new();
            fixture.seed_buffers();
            fixture.inactive();
            fixture.configure(|config| {
                config.window.layout = layout;
                config.decorations.mode = mode;
            });
            let title = layout == WindowLayout::Floating && mode == DecorationModeConfig::Server;
            let buttons = title || mode == DecorationModeConfig::Hybrid;
            assert_eq!(
                fixture.buffers(),
                [title, title, buttons, buttons, buttons],
                "{layout:?}, {mode:?}"
            );
            if mode == DecorationModeConfig::Hybrid {
                fixture.configure(|config| config.decorations.buttons.layout.clear());
                assert_eq!(fixture.buffers(), [false; 5]);
                assert!(
                    !window_decoration_parts(&fixture.state, &fixture.window()).sampled_titlebar
                );
            }
        }
    }
}

#[test]
fn unchanged_policy_refreshes_do_not_allocate_and_failed_saves_preserve_buffers() {
    let mut fixture = Fixture::new();
    fixture.seed_buffers();
    fixture.inactive();
    assert_eq!(
        count_allocations(|| {
            for _ in 0..100 {
                fixture.state.refresh_window_decoration_buffers(fixture.id);
            }
        }),
        0
    );
    assert_eq!(fixture.buffers(), [true; 5]);
    let mut config = fixture.state.config.clone();
    config.decorations.mode = DecorationModeConfig::None;
    assert!(fixture
        .state
        .apply_config_with(config, |_| Err("test persistence failure".into()))
        .is_err());
    assert_eq!(fixture.buffers(), [true; 5]);
}

#[test]
fn client_decoration_requests_prune_inactive_windows_without_surface_commits() {
    let mut fixture = Fixture::new();
    fixture.configure(|config| config.decorations.mode = DecorationModeConfig::Auto);
    let decoration = fixture.negotiate();
    fixture.seed_buffers();
    fixture.inactive();
    decoration.set_mode(Mode::ClientSide);
    fixture.dispatch();
    assert_eq!(fixture.buffers(), [false; 5]);
    assert_eq!(
        fixture.client.events.decoration_modes.last(),
        Some(&Mode::ClientSide)
    );
}

#[test]
fn fullscreen_and_minimized_windows_release_buffers_without_rendering() {
    for fullscreen in [false, true] {
        let mut fixture = Fixture::new();
        fixture.seed_buffers();
        fixture.inactive();
        let window = fixture.window();
        if fullscreen {
            fixture.state.set_fullscreen(&window, true);
        } else {
            assert!(fixture.state.minimize_window(&window));
        }
        assert_eq!(fixture.buffers(), [false; 5]);
    }
}

#[test]
fn unsetting_hybrid_mode_reconciles_policy_and_schedules_a_redraw() {
    use blair_window_integration_protocol::client::blair_window_integration_manager_v1::BlairWindowIntegrationManagerV1;
    let mut fixture = Fixture::new();
    fixture.configure(|config| config.decorations.mode = DecorationModeConfig::Auto);
    let decoration = fixture.negotiate();
    decoration.set_mode(Mode::ClientSide);
    fixture.dispatch();
    let manager = fixture.client.bind::<BlairWindowIntegrationManagerV1>(1);
    let integration = manager.get_integration(&fixture.surface, &fixture.client.handle(), ());
    integration.set_hybrid(1);
    fixture.dispatch();
    fixture.seed_buffers();
    assert_eq!(fixture.buffers(), [false, false, true, true, true]);
    fixture.inactive();
    fixture.state.take_redraw_request();
    manager.unset(&fixture.surface);
    fixture.dispatch();
    assert_eq!(fixture.buffers(), [false; 5]);
    assert!(fixture.state.take_redraw_request());
    assert_eq!(
        fixture.client.events.decoration_modes.last(),
        Some(&Mode::ClientSide)
    );
    integration.set_hybrid(0);
    fixture.dispatch();
    assert!(!fixture.state.take_redraw_request());
}

#[test]
#[ignore = "requires an EGL device with OpenGL ES support"]
fn policy_cleanup_releases_sampled_textures_and_keeps_empty_titles_uncached() {
    use smithay::backend::{
        allocator::Fourcc,
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::Offscreen,
    };
    let mut fixture = Fixture::new();
    fixture.seed_buffers();
    let device = EGLDevice::enumerate().unwrap().next().unwrap();
    let display = unsafe { EGLDisplay::new(device).unwrap() };
    let context = EGLContext::new(&display).unwrap();
    let mut renderer = unsafe { GlesRenderer::new(context).unwrap() };
    let mut texture =
        Offscreen::<GlesTexture>::create_buffer(&mut renderer, Fourcc::Abgr8888, (400, 1).into())
            .unwrap();
    fixture
        .state
        .decorations
        .get_mut(&fixture.id)
        .unwrap()
        .strip = Some(TitlebarStrip {
        texture: texture.clone(),
        tracker: OutputDamageTracker::new((400, 1), 1.0, Transform::Normal),
        size: (400, 1).into(),
        scale: 1.0,
        rendered: true,
        average: None,
        dominant: None,
    });
    assert!(!texture.is_unique_reference());
    fixture.inactive();
    fixture.configure(|config| {
        config.decorations.titlebar_color = crate::config::TitlebarColorMode::Color
    });
    assert!(fixture.state.decorations[&fixture.id].strip.is_none());
    assert!(texture.is_unique_reference());
    assert_eq!(fixture.buffers(), [true; 5]);

    fixture.shell_surface.set_window_geometry(0, 0, 20, 240);
    fixture.surface.commit();
    fixture.dispatch();
    assert!(!fixture.buffers()[0]);
    fixture.state.switch_workspace(1);
    let shaders = crate::render::Shaders::compile(&mut renderer).unwrap();
    for _ in 0..2 {
        crate::render::output_elements(
            &mut renderer,
            &mut fixture.state,
            &fixture.output,
            Some(&shaders),
            crate::render::CursorMode::Hidden,
        );
        assert!(fixture.state.decorations[&fixture.id].title.key.is_none());
    }
}

#[test]
fn unsupported_frame_shaders_retain_only_overlay_controls() {
    let mut fixture = Fixture::new();
    for overlay in [false, true] {
        fixture.configure(|config| {
            config.decorations.mode = if overlay {
                DecorationModeConfig::Hybrid
            } else {
                DecorationModeConfig::Server
            }
        });
        let frame = window_frame(&fixture.state, &fixture.window()).unwrap();
        let (_, parts) = crate::render::decoration_layout(
            &frame,
            fixture.state.decoration_theme(),
            overlay,
            false,
        );
        assert!(!parts.title);
        assert!(!parts.icon);
        assert_eq!(parts.buttons, [overlay; 3]);
        assert!(!parts.sampled_titlebar);
    }
}
