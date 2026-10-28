mod allocations;
mod egl;
pub use allocations::count_allocations;
pub use egl::EglTest;

pub mod input;

use std::{os::unix::net::UnixStream, sync::Arc};

use smithay::reexports::wayland_server::{Client, Display, DisplayHandle};
use wayland_client::{
    protocol::{
        wl_callback, wl_compositor, wl_keyboard, wl_output, wl_pointer, wl_registry, wl_seat,
        wl_surface, wl_touch,
    },
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
};

use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::{
    ext::session_lock::v1::client::{
        ext_session_lock_manager_v1, ext_session_lock_surface_v1, ext_session_lock_v1,
    },
    xdg::shell::client::{xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base},
};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::state::{BlairState, ClientState};

#[derive(Default)]
pub struct ClientEvents {
    pub globals: Vec<(u32, String, u32)>,
    pub keys: Vec<(u32, wl_keyboard::KeyState)>,
    pub key_serials: Vec<u32>,
    pub keymaps: usize,
    pub repeat_info: Vec<(i32, i32)>,
    pub decoration_modes: Vec<zxdg_toplevel_decoration_v1::Mode>,
    pub pointer: Vec<wl_pointer::Event>,
    pub touch: Vec<wl_touch::Event>,
    pub locks_confirmed: usize,
    pub locks_rejected: usize,
    pub popups_dismissed: usize,
}

impl Dispatch<wl_registry::WlRegistry, ()> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals.push((name, interface, version));
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ()> for ClientEvents {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Key {
                serial,
                key,
                state: key_state,
                ..
            } => {
                state.keys.push((key, key_state.into_result().unwrap()));
                state.key_serials.push(serial);
            }
            wl_keyboard::Event::Keymap { .. } => state.keymaps += 1,
            wl_keyboard::Event::RepeatInfo { rate, delay } => state.repeat_info.push((rate, delay)),
            _ => {}
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.pointer.push(event);
    }
}

impl Dispatch<wl_touch::WlTouch, ()> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &wl_touch::WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.touch.push(event);
    }
}

impl Dispatch<ext_session_lock_v1::ExtSessionLockV1, ()> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &ext_session_lock_v1::ExtSessionLockV1,
        event: ext_session_lock_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => state.locks_confirmed += 1,
            ext_session_lock_v1::Event::Finished => state.locks_rejected += 1,
            _ => {}
        }
    }
}

impl Dispatch<ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, ()> for ClientEvents {
    fn event(
        _: &mut Self,
        surface: &ext_session_lock_surface_v1::ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, .. } = event {
            surface.ack_configure(serial);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for ClientEvents {
    fn event(
        _: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_layer_surface_v1::Event::Configure { serial, .. } = event {
            surface.ack_configure(serial);
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for ClientEvents {
    fn event(
        _: &mut Self,
        shell: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for ClientEvents {
    fn event(
        _: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, ()> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure { mode } = event {
            state.decoration_modes.push(mode.into_result().unwrap());
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, ()> for ClientEvents {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_popup::Event::PopupDone = event {
            state.popups_dismissed += 1;
        }
    }
}

wayland_client::delegate_noop!(ClientEvents: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(ClientEvents: ignore wl_surface::WlSurface);

pub struct TestClient {
    display: DisplayHandle,
    pub client: Client,
    connection: Connection,
    queue: EventQueue<ClientEvents>,
    registry: wl_registry::WlRegistry,
    pub events: ClientEvents,
}

impl TestClient {
    pub fn new(display: &mut Display<BlairState>, state: &mut BlairState) -> Self {
        let (server, stream) = UnixStream::pair().unwrap();
        let client = display
            .handle()
            .insert_client(server, Arc::new(ClientState::default()))
            .unwrap();
        let connection = Connection::from_socket(stream).unwrap();
        let queue = connection.new_event_queue();
        let registry = connection.display().get_registry(&queue.handle(), ());
        let mut client = Self {
            display: display.handle(),
            client,
            connection,
            queue,
            registry,
            events: ClientEvents::default(),
        };
        client.dispatch(display, state);
        client
    }

    pub fn handle(&self) -> QueueHandle<ClientEvents> {
        self.queue.handle()
    }

    pub fn bind<I: Proxy + 'static>(&self, version: u32) -> I
    where
        ClientEvents: Dispatch<I, ()>,
    {
        let (name, _, advertised) = self
            .events
            .globals
            .iter()
            .find(|(_, interface, _)| interface == I::interface().name)
            .unwrap_or_else(|| panic!("missing global {}", I::interface().name));
        self.bind_global(*name, version.min(*advertised))
    }

    pub fn bind_global<I: Proxy + 'static>(&self, name: u32, version: u32) -> I
    where
        ClientEvents: Dispatch<I, ()>,
    {
        self.registry.bind(name, version, &self.handle(), ())
    }

    pub fn dispatch(&mut self, display: &mut Display<BlairState>, state: &mut BlairState) {
        self.connection.display().sync(&self.handle(), ());
        self.connection.flush().unwrap();
        display.dispatch_clients(state).unwrap();
        display.flush_clients().unwrap();
        self.connection.prepare_read().unwrap().read().unwrap();
        self.queue.dispatch_pending(&mut self.events).unwrap();
    }

    pub fn paint_surface(&self, surface: &wl_surface::WlSurface, width: i32, height: i32) {
        use wayland_protocols::wp::{
            single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1,
            viewporter::client::wp_viewporter::WpViewporter,
        };
        let manager = self.bind::<WpSinglePixelBufferManagerV1>(1);
        let buffer = manager.create_u32_rgba_buffer(u32::MAX, 0, 0, u32::MAX, &self.handle(), ());
        let viewporter = self.bind::<WpViewporter>(1);
        let viewport = viewporter.get_viewport(surface, &self.handle(), ());
        viewport.set_destination(width, height);
        surface.attach(Some(&buffer), 0, 0);
        surface.damage_buffer(0, 0, 1, 1);
        surface.commit();
    }

    pub fn surface(
        &self,
        surface: &wl_surface::WlSurface,
    ) -> wayland_server::protocol::wl_surface::WlSurface {
        self.client
            .object_from_protocol_id(&self.display, surface.id().protocol_id())
            .unwrap()
    }
}

wayland_client::delegate_noop!(ClientEvents: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(ClientEvents: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(ClientEvents: ignore ext_session_lock_manager_v1::ExtSessionLockManagerV1);
wayland_client::delegate_noop!(ClientEvents: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
wayland_client::delegate_noop!(ClientEvents: ignore xdg_positioner::XdgPositioner);
wayland_client::delegate_noop!(ClientEvents: ignore xdg_toplevel::XdgToplevel);

wayland_client::delegate_noop!(ClientEvents: ignore wayland_client::protocol::wl_region::WlRegion);
wayland_client::delegate_noop!(ClientEvents: ignore blair_blur_protocol::client::blair_blur_manager_v1::BlairBlurManagerV1);
wayland_client::delegate_noop!(ClientEvents: ignore blair_blur_protocol::client::blair_blur_v1::BlairBlurV1);

wayland_client::delegate_noop!(ClientEvents: ignore wayland_client::protocol::wl_buffer::WlBuffer);
wayland_client::delegate_noop!(ClientEvents: ignore wayland_protocols::wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1);
wayland_client::delegate_noop!(ClientEvents: ignore wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter);
wayland_client::delegate_noop!(ClientEvents: ignore wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport);

wayland_client::delegate_noop!(ClientEvents: ignore wayland_client::protocol::wl_subcompositor::WlSubcompositor);
wayland_client::delegate_noop!(ClientEvents: ignore wayland_client::protocol::wl_subsurface::WlSubsurface);

pub fn isolated_config_test(name: &str, test: impl FnOnce(&std::path::Path)) {
    const CHILD: &str = "BLAIR_TEST_CONFIG_CHILD";
    let name = name
        .strip_prefix(concat!(env!("CARGO_CRATE_NAME"), "::"))
        .unwrap_or(name);
    if std::env::var(CHILD).as_deref() == Ok(name) {
        eprintln!("{CHILD}={name}");
        let root = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap());
        test(&root);
        return;
    }
    let root = std::env::temp_dir().join(format!(
        "blair-{}-{}",
        name.replace("::", "-"),
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .env("XDG_CONFIG_HOME", &root)
        .output()
        .unwrap();
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stderr).contains(&format!("{CHILD}={name}")),
        "isolated test failed; artifacts: {}\n{}\n{}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
}

wayland_client::delegate_noop!(ClientEvents: ignore zxdg_decoration_manager_v1::ZxdgDecorationManagerV1);

pub fn add_output(
    state: &mut BlairState,
    name: &str,
    location: (i32, i32),
    size: (i32, i32),
) -> smithay::output::Output {
    use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
    let output = Output::new(
        name.into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "Test".into(),
            model: "Test".into(),
        },
    );
    output.change_current_state(
        Some(Mode {
            size: size.into(),
            refresh: 60_000,
        }),
        None,
        None,
        Some(location.into()),
    );
    output.create_global::<BlairState>(&state.display_handle);
    state.add_output(&output, location.into());
    output
}

wayland_client::delegate_noop!(ClientEvents: ignore blair_window_integration_protocol::client::blair_window_integration_manager_v1::BlairWindowIntegrationManagerV1);
wayland_client::delegate_noop!(ClientEvents: ignore blair_window_integration_protocol::client::blair_window_integration_v1::BlairWindowIntegrationV1);
