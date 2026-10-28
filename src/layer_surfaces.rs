use std::cell::Cell;

use smithay::reexports::wayland_protocols_wlr::layer_shell::v1::server::zwlr_layer_surface_v1::Error;
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{layer_map_for_output, LayerSurface as DesktopLayerSurface, PopupKind, PopupManager},
    output::{Output, WeakOutput},
    utils::{Logical, Rectangle, Serial, Size, SERIAL_COUNTER},
    wayland::{
        compositor::{add_pre_commit_hook, with_states, BufferAssignment, SurfaceAttributes},
        shell::wlr_layer::{
            Anchor, ExclusiveZone, Layer, LayerSurface, LayerSurfaceCachedState,
            LayerSurfaceConfigure, LayerSurfaceData,
        },
    },
};
use wayland_server::{protocol::wl_surface::WlSurface, Resource};

use crate::state::{send_scale_to_surface, BlairState};

#[derive(Debug, Clone, Copy)]
enum Phase {
    Initial,
    AwaitingAck(Serial),
    Ready,
    Mapped,
    Closed,
}

struct Lifecycle {
    // Outputs own their layer maps, so the retained assignment must be weak.
    output: WeakOutput,
    initial_layer: Layer,
    phase: Cell<Phase>,
}

struct CommitHook;

fn lifecycle(layer: &DesktopLayerSurface) -> &Lifecycle {
    layer.user_data().get().expect("missing layer lifecycle")
}

pub(crate) fn is_mapped(layer: &DesktopLayerSurface) -> bool {
    matches!(lifecycle(layer).phase.get(), Phase::Mapped)
}

pub(crate) fn owns_surface(layer: &DesktopLayerSurface, surface: &WlSurface) -> bool {
    let mut found = false;
    layer.with_surfaces(|candidate, _| found |= candidate == surface);
    found
}

// Unmapped layers must not reserve space while negotiating their initial size.
fn configure_size(
    state: &BlairState,
    layer: &DesktopLayerSurface,
    output: &Output,
) -> Size<i32, Logical> {
    let data = layer.cached_state();
    let mut available = if data.exclusive_zone == ExclusiveZone::DontCare {
        state.space.output_geometry(output).unwrap_or_default().size
    } else {
        layer_map_for_output(output).non_exclusive_zone().size
    };
    for (anchor, margin) in [
        (Anchor::LEFT, data.margin.left),
        (Anchor::RIGHT, data.margin.right),
    ] {
        if data.anchor.contains(anchor) {
            available.w = available.w.saturating_sub(margin);
        }
    }
    for (anchor, margin) in [
        (Anchor::TOP, data.margin.top),
        (Anchor::BOTTOM, data.margin.bottom),
    ] {
        if data.anchor.contains(anchor) {
            available.h = available.h.saturating_sub(margin);
        }
    }
    available.w = available.w.max(1);
    available.h = available.h.max(1);
    let mut size = data.size;
    size.w = size.w.min(available.w);
    size.h = size.h.min(available.h);
    if size.w == 0 {
        size.w = available.w / 2;
    }
    if size.h == 0 {
        size.h = available.h / 2;
    }
    if data.anchor.anchored_horizontally() {
        size.w = available.w;
    }
    if data.anchor.anchored_vertically() {
        size.h = available.h;
    }
    size
}

fn configure(state: &BlairState, layer: &DesktopLayerSurface, output: &Output) -> Option<Serial> {
    let size = configure_size(state, layer, output);
    layer
        .layer_surface()
        .with_pending_state(|pending| pending.size = Some(size));
    layer.layer_surface().send_pending_configure()
}

fn reset(layer: &DesktopLayerSurface) {
    let lifecycle = lifecycle(layer);
    lifecycle.phase.set(Phase::Initial);
    with_states(layer.wl_surface(), |states| {
        let mut data = states
            .data_map
            .get::<LayerSurfaceData>()
            .unwrap()
            .lock()
            .unwrap();
        data.configured = false;
        data.configure_serial = None;
        data.initial_configure_sent = false;
        data.server_pending = None;
        data.last_acked = None;
        data.current = Default::default();
        let mut cache = states.cached_state.get::<LayerSurfaceCachedState>();
        let initial = LayerSurfaceCachedState {
            layer: lifecycle.initial_layer,
            ..Default::default()
        };
        *cache.pending() = initial;
        *cache.current() = initial;
    });
}

fn validate_buffer(layer: &DesktopLayerSurface) -> bool {
    match lifecycle(layer).phase.get() {
        Phase::Ready | Phase::Mapped => return true,
        Phase::Closed => return false,
        Phase::Initial | Phase::AwaitingAck(_) => {}
    }
    lifecycle(layer).phase.set(Phase::Closed);
    layer.layer_surface().shell_surface().post_error(
        Error::InvalidSurfaceState,
        "buffer committed without acknowledging this mapping cycle",
    );
    false
}

impl BlairState {
    pub(crate) fn register_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<Output>,
        layer: Layer,
        namespace: String,
    ) {
        tracing::debug!(%namespace, ?layer, "new layer surface");
        if self
            .layer_surfaces
            .iter()
            .any(|existing| existing.wl_surface() == surface.wl_surface())
        {
            surface.shell_surface().post_error(
                Error::InvalidSurfaceState,
                "surface already has a layer-surface object",
            );
            return;
        }
        let Some(output) = output.or_else(|| self.focused_output()) else {
            tracing::warn!(%namespace, "no output for layer surface");
            surface.send_close();
            return;
        };
        let desktop = DesktopLayerSurface::new(surface, namespace);
        desktop.user_data().insert_if_missing(|| Lifecycle {
            output: output.downgrade(),
            initial_layer: layer,
            phase: Cell::new(Phase::Initial),
        });
        let install_hook = with_states(desktop.wl_surface(), |states| {
            states.data_map.insert_if_missing(|| CommitHook)
        });
        if install_hook {
            add_pre_commit_hook::<Self, _>(desktop.wl_surface(), |state, _, surface| {
                let attaches_buffer = with_states(surface, |states| {
                    matches!(
                        states
                            .cached_state
                            .get::<SurfaceAttributes>()
                            .pending()
                            .buffer,
                        Some(BufferAssignment::NewBuffer(_))
                    )
                });
                if !attaches_buffer {
                    return;
                }
                if let Some(layer) = state
                    .layer_surfaces
                    .iter()
                    .find(|layer| layer.wl_surface() == surface)
                {
                    validate_buffer(layer);
                }
            });
        }
        send_scale_to_surface(desktop.wl_surface(), &output);
        self.layer_surfaces.push(desktop);
    }

    pub fn output_for_layer(&self, surface: &WlSurface) -> Option<Output> {
        self.layer_surfaces
            .iter()
            .find(|layer| layer.wl_surface() == surface)
            .and_then(|layer| lifecycle(layer).output.upgrade())
    }

    pub(crate) fn layer_configure_acknowledged(
        &mut self,
        surface: &WlSurface,
        configure: LayerSurfaceConfigure,
    ) {
        let Some(layer) = self
            .layer_surfaces
            .iter()
            .find(|layer| layer.wl_surface() == surface)
        else {
            return;
        };
        let valid = match lifecycle(layer).phase.get() {
            Phase::AwaitingAck(first) => {
                let valid = configure.serial >= first;
                if valid {
                    lifecycle(layer).phase.set(Phase::Ready);
                }
                valid
            }
            Phase::Ready | Phase::Mapped => true,
            Phase::Initial | Phase::Closed => false,
        };
        // The first acknowledgement drains Smithay's previous-cycle entries.
        if !valid {
            lifecycle(layer).phase.set(Phase::Closed);
            tracing::debug!(serial = ?configure.serial, "stale layer configure acknowledgement");
            layer.layer_surface().shell_surface().post_error(
                Error::InvalidSurfaceState,
                "configure belongs to a previous mapping cycle",
            );
        }
    }

    pub(crate) fn configure_unmapped_layers(&self, output: &Output) {
        for layer in &self.layer_surfaces {
            if matches!(
                lifecycle(layer).phase.get(),
                Phase::AwaitingAck(_) | Phase::Ready
            ) && lifecycle(layer).output.upgrade().as_ref() == Some(output)
            {
                configure(self, layer, output);
            }
        }
    }

    fn layer_layout_changed(&mut self, output: &Output, previous: Rectangle<i32, Logical>) {
        self.configure_unmapped_layers(output);
        if layer_map_for_output(output).non_exclusive_zone() != previous {
            self.emit_work_area_changed(output);
            self.relayout_all();
        }
        self.mark_pointer_focus_dirty();
        self.request_redraw();
    }

    fn unmap_layer_surface(&mut self, layer: &DesktopLayerSurface, output: &Output) {
        let pointer = self.seat.get_pointer();
        let pointer_owned = pointer.as_ref().and_then(|pointer| {
            pointer
                .with_grab(|serial, grab| {
                    grab.start_data()
                        .focus
                        .as_ref()
                        .filter(|(focus, _)| owns_surface(layer, focus))
                        .map(|_| serial)
                })
                .flatten()
        });
        let pointer_focused = pointer
            .as_ref()
            .and_then(|pointer| pointer.current_focus())
            .is_some_and(|focus| owns_surface(layer, &focus));
        let touch_owned = self
            .seat
            .get_touch()
            .and_then(|touch| touch.grab_start_data())
            .and_then(|start| start.focus)
            .is_some_and(|(focus, _)| owns_surface(layer, &focus));
        let previous = layer_map_for_output(output).non_exclusive_zone();
        layer_map_for_output(output).unmap_layer(layer);
        self.release_layer_focus(layer.wl_surface());
        for (popup, _) in PopupManager::popups_for_surface(layer.wl_surface()).filter(|(popup, _)| {
            matches!(popup, PopupKind::Xdg(popup) if popup.get_parent_surface().as_ref() == Some(layer.wl_surface()))
        }) {
            if let Err(error) = PopupManager::dismiss_popup(layer.wl_surface(), &popup) {
                tracing::debug!(?error, "layer popup dismissal failed");
            }
        }
        if touch_owned {
            crate::input::cancel_touch_input(self);
        }
        if let Some(pointer) = pointer {
            if pointer_owned.is_some_and(|serial| pointer.has_grab(serial)) {
                pointer.unset_grab(
                    self,
                    SERIAL_COUNTER.next_serial(),
                    self.clock_now().as_millis() as u32,
                );
            }
        }
        if pointer_focused || pointer_owned.is_some() {
            self.refresh_pointer_focus();
        }
        self.layer_layout_changed(output, previous);
    }

    pub(crate) fn remove_layer_surface(&mut self, surface: &WlSurface) {
        let Some(index) = self
            .layer_surfaces
            .iter()
            .position(|layer| layer.wl_surface() == surface)
        else {
            return;
        };
        let layer = self.layer_surfaces[index].clone();
        lifecycle(&layer).phase.set(Phase::Closed);
        if let Some(output) = lifecycle(&layer).output.upgrade() {
            self.unmap_layer_surface(&layer, &output);
        }
        self.layer_surfaces.remove(index);
    }

    pub(crate) fn commit_layer(&mut self, surface: &WlSurface) {
        let Some(layer) = self
            .layer_surfaces
            .iter()
            .find(|layer| layer.wl_surface() == surface)
            .cloned()
        else {
            return;
        };
        if matches!(lifecycle(&layer).phase.get(), Phase::Closed) {
            return;
        }
        let Some(output) = lifecycle(&layer).output.upgrade() else {
            lifecycle(&layer).phase.set(Phase::Closed);
            layer.layer_surface().send_close();
            return;
        };
        let has_buffer =
            with_renderer_surface_state(surface, |state| state.buffer().is_some()).unwrap_or(false);
        match lifecycle(&layer).phase.get() {
            Phase::Initial if !has_buffer => {
                if let Some(serial) = configure(self, &layer, &output) {
                    lifecycle(&layer).phase.set(Phase::AwaitingAck(serial));
                }
            }
            Phase::Ready if has_buffer => {
                let previous = layer_map_for_output(&output).non_exclusive_zone();
                if let Err(error) = layer_map_for_output(&output).map_layer(&layer) {
                    tracing::warn!(?error, "layer mapping failed");
                    lifecycle(&layer).phase.set(Phase::Closed);
                    layer.layer_surface().send_close();
                    return;
                }
                lifecycle(&layer).phase.set(Phase::Mapped);
                self.layer_layout_changed(&output, previous);
                self.focus_interactive_layer_surface(surface);
            }
            Phase::AwaitingAck(_) if has_buffer => {
                validate_buffer(&layer);
            }
            Phase::AwaitingAck(_) | Phase::Ready => {
                configure(self, &layer, &output);
            }
            Phase::Mapped if !has_buffer => {
                reset(&layer);
                self.unmap_layer_surface(&layer, &output);
                tracing::debug!(surface = ?surface.id(), "layer unmapped");
            }
            Phase::Mapped => {
                let previous = layer_map_for_output(&output).non_exclusive_zone();
                layer_map_for_output(&output).arrange();
                self.layer_layout_changed(&output, previous);
                self.focus_interactive_layer_surface(surface);
            }
            Phase::Initial => {
                validate_buffer(&layer);
            }
            Phase::Closed => {}
        }
    }
}

#[cfg(test)]
mod tests;
