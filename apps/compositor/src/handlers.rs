use std::os::unix::io::OwnedFd;

use smithay::{
    backend::allocator::dmabuf::Dmabuf,
    delegate_alpha_modifier, delegate_compositor, delegate_cursor_shape, delegate_data_control,
    delegate_data_device, delegate_dmabuf, delegate_foreign_toplevel_list,
    delegate_fractional_scale, delegate_idle_inhibit, delegate_idle_notify,
    delegate_kde_decoration, delegate_keyboard_shortcuts_inhibit, delegate_layer_shell,
    delegate_output, delegate_pointer_constraints, delegate_pointer_gestures,
    delegate_presentation, delegate_primary_selection, delegate_relative_pointer, delegate_seat,
    delegate_shm, delegate_single_pixel_buffer, delegate_viewporter, delegate_xdg_activation,
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        find_popup_root_surface, layer_map_for_output, LayerSurface as DesktopLayerSurface,
        PopupKeyboardGrab, PopupKind, PopupPointerGrab, PopupUngrabStrategy,
    },
    input::{
        keyboard::LedState,
        pointer::{CursorImageStatus, Focus, PointerHandle},
        Seat, SeatHandler, SeatState,
    },
    output::Output,
    reexports::{
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration::{
            Mode as KdeDecorationMode, OrgKdeKwinServerDecoration,
        },
        wayland_server::{
            protocol::{
                wl_buffer::WlBuffer, wl_data_source::WlDataSource, wl_output::WlOutput,
                wl_seat::WlSeat, wl_surface::WlSurface,
            },
            Client, Resource,
        },
    },
    utils::{Logical, Point, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        foreign_toplevel_list::{ForeignToplevelListHandler, ForeignToplevelListState},
        fractional_scale::FractionalScaleHandler,
        idle_inhibit::IdleInhibitHandler,
        idle_notify::{IdleNotifierHandler, IdleNotifierState},
        keyboard_shortcuts_inhibit::{
            KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState,
            KeyboardShortcutsInhibitor,
        },
        output::OutputHandler,
        pointer_constraints::{with_pointer_constraint, PointerConstraintsHandler},
        selection::{
            data_device::{
                set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
                ServerDndGrabHandler,
            },
            primary_selection::{
                set_primary_focus, PrimarySelectionHandler, PrimarySelectionState,
            },
            wlr_data_control::{DataControlHandler, DataControlState},
            SelectionHandler,
        },
        shell::{
            kde::decoration::{KdeDecorationHandler, KdeDecorationState},
            wlr_layer::{Layer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState},
            xdg::{
                decoration::XdgDecorationHandler, PopupSurface, PositionerState, ToplevelSurface,
                XdgShellHandler, XdgShellState,
            },
        },
        shm::{ShmHandler, ShmState},
        tablet_manager::TabletSeatHandler,
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

use crate::{
    grabs::ResizeEdges,
    state::{
        send_scale_to_surface, set_client_decoration_request, set_surface_decoration_mode,
        surface_window_id, BlairState, ClientState,
    },
};

const ACTIVATION_TOKEN_TIMEOUT_SECS: u64 = 10;

impl BufferHandler for BlairState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl CompositorHandler for BlairState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client
            .get_data::<ClientState>()
            .expect("every client is inserted with ClientState")
            .compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
        self.handle_commit(surface);
    }
}

delegate_compositor!(BlairState);

impl XdgShellHandler for BlairState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        BlairState::new_toplevel(self, surface);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        BlairState::toplevel_destroyed(self, &surface);
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.toplevel_metadata_changed(&surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.toplevel_metadata_changed(&surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|state| state.positioner = positioner);
        self.unconstrain_popup(&surface);
        if let Err(error) = self.popup_manager.track_popup(PopupKind::Xdg(surface)) {
            tracing::warn!(%error, "failed to track popup");
        }
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| state.positioner = positioner);
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: WlSeat, serial: Serial) {
        let Some((window, button)) = self.validated_interactive_request(&surface, &seat, serial)
        else {
            return;
        };
        self.begin_move(&window, serial, button);
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        let Some((window, button)) = self.validated_interactive_request(&surface, &seat, serial)
        else {
            return;
        };
        self.begin_resize(&window, ResizeEdges::from_xdg(edges), serial, button);
    }

    fn grab(&mut self, surface: PopupSurface, seat: WlSeat, serial: Serial) {
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return;
        };
        let kind = PopupKind::Xdg(surface);
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let mut grab = match self.popup_manager.grab_popup(root, kind, &seat, serial) {
            Ok(grab) => grab,
            Err(error) => {
                tracing::debug!(?error, "popup grab rejected");
                return;
            }
        };
        if let Some(keyboard) = seat.get_keyboard() {
            if keyboard.is_grabbed()
                && !(keyboard.has_grab(serial)
                    || keyboard.has_grab(grab.previous_serial().unwrap_or(serial)))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            keyboard.set_focus(self, grab.current_grab(), serial);
            keyboard.set_grab(self, PopupKeyboardGrab::new(&grab), serial);
        }
        if let Some(pointer) = seat.get_pointer() {
            if pointer.is_grabbed()
                && !(pointer.has_grab(serial)
                    || pointer.has_grab(grab.previous_serial().unwrap_or_else(|| grab.serial())))
            {
                grab.ungrab(PopupUngrabStrategy::All);
                return;
            }
            pointer.set_grab(self, PopupPointerGrab::new(&grab), serial, Focus::Keep);
        }
    }

    fn popup_destroyed(&mut self, _surface: PopupSurface) {
        self.request_redraw();
        self.mark_pointer_focus_dirty();
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.request_maximized(&surface, true);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.request_maximized(&surface, false);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        self.request_fullscreen(&surface, true);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.request_fullscreen(&surface, false);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(window) = self.window_for_toplevel(&surface) {
            self.minimize_window(&window);
        }
    }
}

impl BlairState {
    fn request_maximized(&mut self, surface: &ToplevelSurface, maximized: bool) {
        let handled = self
            .window_for_toplevel(surface)
            .is_some_and(|window| self.set_maximized(&window, maximized));
        if !handled {
            surface.send_configure();
        }
    }

    fn request_fullscreen(&mut self, surface: &ToplevelSurface, fullscreen: bool) {
        match self.window_for_toplevel(surface) {
            Some(window) => self.set_fullscreen(&window, fullscreen),
            None => {
                surface.send_configure();
            }
        }
    }

    fn window_for_toplevel(&self, surface: &ToplevelSurface) -> Option<smithay::desktop::Window> {
        let id = surface_window_id(surface.wl_surface())?;
        self.windows
            .get(&id)
            .filter(|managed| managed.mapped)
            .map(|managed| managed.window.clone())
    }

    fn validated_interactive_request(
        &self,
        surface: &ToplevelSurface,
        seat: &WlSeat,
        serial: Serial,
    ) -> Option<(smithay::desktop::Window, u32)> {
        let seat = Seat::<Self>::from_resource(seat)?;
        let pointer = seat.get_pointer()?;
        if !pointer.has_grab(serial) {
            tracing::debug!("ignoring interactive request without a matching pointer grab");
            return None;
        }
        let start = pointer.grab_start_data()?;
        let focus = start.focus.as_ref()?;
        if !focus.0.id().same_client_as(&surface.wl_surface().id()) {
            return None;
        }
        Some((self.window_for_toplevel(surface)?, start.button))
    }
}

delegate_xdg_shell!(BlairState);

impl XdgDecorationHandler for BlairState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.set_decoration_mode(&toplevel);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        set_client_decoration_request(toplevel.wl_surface(), Some(mode));
        self.set_decoration_mode(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_client_decoration_request(toplevel.wl_surface(), None);
        self.set_decoration_mode(&toplevel);
    }
}

impl BlairState {
    fn set_decoration_mode(&mut self, toplevel: &ToplevelSurface) {
        let mode = self.resolve_decoration_mode(toplevel.wl_surface());
        tracing::debug!(?mode, "xdg-decoration mode selected");
        set_surface_decoration_mode(toplevel.wl_surface(), mode);
        toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
        self.request_redraw();
    }
}

delegate_xdg_decoration!(BlairState);

impl WlrLayerShellHandler for BlairState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        tracing::debug!(namespace = %namespace, ?layer, "new layer surface");
        let output = output
            .as_ref()
            .and_then(Output::from_resource)
            .or_else(|| self.focused_output());
        let Some(output) = output else {
            tracing::warn!(namespace = %namespace, "no output for layer surface");
            surface.send_close();
            return;
        };
        let desktop_surface = DesktopLayerSurface::new(surface, namespace.clone());
        if let Err(error) = layer_map_for_output(&output).map_layer(&desktop_surface) {
            tracing::warn!(?error, namespace = %namespace, "failed to map layer surface");
            return;
        }
        send_scale_to_surface(desktop_surface.wl_surface(), &output);
        self.layer_surfaces.push(desktop_surface);
        self.emit_work_area_changed(&output);
    }

    fn new_popup(&mut self, _parent: LayerSurface, popup: PopupSurface) {
        self.unconstrain_popup(&popup);
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        tracing::debug!("layer surface destroyed");
        let wl_surface = surface.wl_surface().clone();
        let Some(index) = self
            .layer_surfaces
            .iter()
            .position(|mapped| mapped.wl_surface() == &wl_surface)
        else {
            return;
        };
        let mapped = self.layer_surfaces.remove(index);
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for output in &outputs {
            let mut map = layer_map_for_output(output);
            if map.layers().any(|layer| layer == &mapped) {
                map.unmap_layer(&mapped);
                drop(map);
                self.emit_work_area_changed(output);
                self.output_changed(output);
            }
        }
        self.layer_surface_removed(&wl_surface);
    }
}

delegate_layer_shell!(BlairState);

impl ShmHandler for BlairState {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

delegate_shm!(BlairState);

impl SeatHandler for BlairState {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.client_cursor = image;
        self.request_redraw();
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|surface| self.display_handle.get_client(surface.id()).ok());
        set_data_device_focus(&self.display_handle, seat, client.clone());
        set_primary_focus(&self.display_handle, seat, client);
        self.keyboard_focus_changed(focused);
    }

    fn led_state_changed(&mut self, _seat: &Seat<Self>, led_state: LedState) {
        self.led_state = led_state;
    }
}

impl TabletSeatHandler for BlairState {}

delegate_seat!(BlairState);
delegate_cursor_shape!(BlairState);

impl SelectionHandler for BlairState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for BlairState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for BlairState {
    fn started(
        &mut self,
        _source: Option<WlDataSource>,
        icon: Option<WlSurface>,
        _seat: Seat<Self>,
    ) {
        self.dnd_icon = icon;
        self.request_redraw();
    }

    fn dropped(&mut self, _target: Option<WlSurface>, _validated: bool, _seat: Seat<Self>) {
        self.dnd_icon = None;
        self.request_redraw();
    }
}

impl ServerDndGrabHandler for BlairState {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

delegate_data_device!(BlairState);

impl PrimarySelectionHandler for BlairState {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}

delegate_primary_selection!(BlairState);

impl DataControlHandler for BlairState {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}

delegate_data_control!(BlairState);

impl OutputHandler for BlairState {}

delegate_output!(BlairState);

impl DmabufHandler for BlairState {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        self.pending_dmabuf_imports.push((dmabuf, notifier));
    }
}

delegate_dmabuf!(BlairState);
delegate_presentation!(BlairState);
delegate_viewporter!(BlairState);
delegate_single_pixel_buffer!(BlairState);
delegate_relative_pointer!(BlairState);

impl FractionalScaleHandler for BlairState {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let mut root = surface.clone();
        while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
            root = parent;
        }
        let output = self
            .output_for_layer(&root)
            .or_else(|| self.focused_output());
        if let Some(output) = output {
            send_scale_to_surface(&surface, &output);
        }
    }
}

delegate_fractional_scale!(BlairState);

impl XdgActivationHandler for BlairState {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        let Some((serial, seat)) = data.serial else {
            return false;
        };
        let Some(seat) = Seat::<Self>::from_resource(&seat) else {
            return false;
        };
        let keyboard_ok = seat.get_keyboard().is_some_and(|keyboard| {
            keyboard
                .last_enter()
                .is_some_and(|last| serial.is_no_older_than(&last))
        });
        let pointer_ok = seat.get_pointer().is_some_and(|pointer| {
            pointer
                .last_enter()
                .is_some_and(|last| serial.is_no_older_than(&last))
        });
        keyboard_ok || pointer_ok
    }

    fn request_activation(
        &mut self,
        _token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.timestamp.elapsed().as_secs() >= ACTIVATION_TOKEN_TIMEOUT_SECS {
            tracing::debug!("ignoring expired activation token");
            return;
        }
        if let Some(id) = surface_window_id(&surface) {
            tracing::debug!(%id, "activation request granted");
            self.focus_window_by_id(id);
        }
    }
}

delegate_xdg_activation!(BlairState);

impl ForeignToplevelListHandler for BlairState {
    fn foreign_toplevel_list_state(&mut self) -> &mut ForeignToplevelListState {
        &mut self.foreign_toplevel_state
    }
}

delegate_foreign_toplevel_list!(BlairState);

impl PointerConstraintsHandler for BlairState {
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>) {
        if pointer.current_focus().as_ref() == Some(surface) {
            with_pointer_constraint(surface, pointer, |constraint| {
                if let Some(constraint) = constraint {
                    constraint.activate();
                }
            });
        }
    }

    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    ) {
        let active = with_pointer_constraint(surface, pointer, |constraint| {
            constraint.is_some_and(|constraint| constraint.is_active())
        });
        if !active {
            return;
        }
        let Some(origin) = crate::input::surface_origin(self, surface) else {
            return;
        };
        pointer.set_location(origin + location);
        self.request_redraw();
    }
}

delegate_pointer_constraints!(BlairState);

impl IdleNotifierHandler for BlairState {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}

delegate_idle_notify!(BlairState);

impl IdleInhibitHandler for BlairState {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.insert(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_inhibitors.remove(&surface);
    }
}

delegate_idle_inhibit!(BlairState);
delegate_pointer_gestures!(BlairState);
delegate_alpha_modifier!(BlairState);

impl KeyboardShortcutsInhibitHandler for BlairState {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        if focused.as_ref() == Some(inhibitor.wl_surface()) {
            tracing::debug!("keyboard shortcuts inhibited by the focused surface");
            inhibitor.activate();
        }
    }
}

delegate_keyboard_shortcuts_inhibit!(BlairState);

impl KdeDecorationHandler for BlairState {
    fn kde_decoration_state(&self) -> &KdeDecorationState {
        &self.kde_decoration_state
    }

    fn new_decoration(&mut self, surface: &WlSurface, decoration: &OrgKdeKwinServerDecoration) {
        self.announce_kde_decoration(surface, decoration);
    }

    fn request_mode(
        &mut self,
        surface: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
        mode: smithay::reexports::wayland_server::WEnum<KdeDecorationMode>,
    ) {
        let requested = match mode {
            smithay::reexports::wayland_server::WEnum::Value(KdeDecorationMode::Server) => {
                Some(DecorationMode::ServerSide)
            }
            // `None` asks for no decorations at all, which the server side
            // honors the same way as client-side: by not drawing a frame.
            smithay::reexports::wayland_server::WEnum::Value(
                KdeDecorationMode::Client | KdeDecorationMode::None,
            ) => Some(DecorationMode::ClientSide),
            _ => None,
        };
        set_client_decoration_request(surface, requested);
        self.announce_kde_decoration(surface, decoration);
    }
}

impl BlairState {
    /// Mirrors the xdg-decoration policy onto the KDE protocol, which older
    /// Qt applications use instead.
    fn announce_kde_decoration(
        &mut self,
        surface: &WlSurface,
        decoration: &OrgKdeKwinServerDecoration,
    ) {
        let mode = self.resolve_decoration_mode(surface);
        set_surface_decoration_mode(surface, mode);
        decoration.mode(match mode {
            DecorationMode::ServerSide => KdeDecorationMode::Server,
            _ => KdeDecorationMode::Client,
        });
        self.request_redraw();
    }
}

delegate_kde_decoration!(BlairState);
