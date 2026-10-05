//! Blair's opt-in per-window hybrid-decoration protocol.

use std::sync::Mutex;

use blair_window_integration_protocol::server::{
    blair_window_integration_manager_v1::{self, BlairWindowIntegrationManagerV1},
    blair_window_integration_v1::{self, BlairWindowIntegrationV1, Mode},
};
use smithay::{
    reexports::wayland_server::{
        protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch,
        New,
    },
    wayland::compositor::with_states,
};

use crate::state::BlairState;

const VERSION: u32 = 1;

#[derive(Default)]
struct SurfaceIntegrationState {
    requested: bool,
    effective: bool,
    controls: (i32, i32, i32, i32),
    object: Option<BlairWindowIntegrationV1>,
}

#[derive(Default)]
struct SurfaceIntegration(Mutex<SurfaceIntegrationState>);

pub fn register(display: &DisplayHandle) {
    display.create_global::<BlairState, BlairWindowIntegrationManagerV1, _>(VERSION, ());
}

pub fn hybrid_requested(surface: Option<&WlSurface>) -> bool {
    surface.is_some_and(|surface| {
        with_states(surface, |states| {
            states
                .data_map
                .get::<SurfaceIntegration>()
                .is_some_and(|state| {
                    state
                        .0
                        .lock()
                        .expect("integration state lock poisoned")
                        .requested
                })
        })
    })
}

fn set_requested(surface: &WlSurface, requested: bool) {
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing(SurfaceIntegration::default);
        states
            .data_map
            .get::<SurfaceIntegration>()
            .expect("inserted above")
            .0
            .lock()
            .expect("integration state lock poisoned")
            .requested = requested;
    });
}

/// Updates the client with the effective mode and the client-local controls
/// rectangle. Called from rendering, where the actual window geometry exists.
pub fn publish(surface: &WlSurface, enabled: bool, controls: (i32, i32, i32, i32)) {
    with_states(surface, |states| {
        let Some(state) = states.data_map.get::<SurfaceIntegration>() else {
            return;
        };
        let mut state = state.0.lock().expect("integration state lock poisoned");
        let changed = state.effective != enabled || state.controls != controls;
        state.effective = enabled;
        state.controls = controls;
        if changed {
            if let Some(object) = state.object.as_ref() {
                object.mode(if enabled { Mode::Hybrid } else { Mode::None });
                let (x, y, width, height) = if enabled { controls } else { (0, 0, 0, 0) };
                object.controls(x, y, width, height);
            }
        }
    });
}

pub struct IntegrationData {
    surface: WlSurface,
}

impl GlobalDispatch<BlairWindowIntegrationManagerV1, ()> for BlairState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<BlairWindowIntegrationManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<BlairWindowIntegrationManagerV1, ()> for BlairState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &BlairWindowIntegrationManagerV1,
        request: blair_window_integration_manager_v1::Request,
        _data: &(),
        _handle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            blair_window_integration_manager_v1::Request::GetIntegration { id, surface } => {
                let object = data_init.init(
                    id,
                    IntegrationData {
                        surface: surface.clone(),
                    },
                );
                with_states(&surface, |states| {
                    states
                        .data_map
                        .insert_if_missing(SurfaceIntegration::default);
                    states
                        .data_map
                        .get::<SurfaceIntegration>()
                        .expect("inserted above")
                        .0
                        .lock()
                        .expect("integration state lock poisoned")
                        .object = Some(object.clone());
                });
                object.mode(Mode::None);
                object.controls(0, 0, 0, 0);
            }
            blair_window_integration_manager_v1::Request::Unset { surface } => {
                set_requested(&surface, false)
            }
            _ => {}
        }
    }
}

impl Dispatch<BlairWindowIntegrationV1, IntegrationData> for BlairState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _object: &BlairWindowIntegrationV1,
        request: blair_window_integration_v1::Request,
        data: &IntegrationData,
        _handle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let blair_window_integration_v1::Request::SetHybrid { enabled } = request {
            set_requested(&data.surface, enabled != 0);
            state.refresh_decoration_modes();
            state.request_redraw();
        }
    }
}
