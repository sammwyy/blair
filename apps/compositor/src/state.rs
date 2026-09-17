use std::{
    cell::RefCell, collections::HashMap, os::unix::io::OwnedFd, process::Command, sync::Arc,
};

use blair_integration::EventChannel;
use blair_protocol::{CompositorEvent, Rect, WindowId, WindowInfo, WorkspaceInfo};
use smithay::{
    backend::renderer::utils::on_commit_buffer_handler,
    delegate_compositor, delegate_data_device, delegate_layer_shell, delegate_output,
    delegate_seat, delegate_shm, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        layer_map_for_output, LayerSurface as DesktopLayerSurface, PopupKind, PopupManager, Space,
        Window, WindowSurfaceType,
    },
    input::{keyboard::XkbConfig, pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    output::Output,
    reexports::{
        calloop::LoopSignal,
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_buffer::WlBuffer, wl_output::WlOutput, wl_surface::WlSurface},
            Client, DisplayHandle,
        },
    },
    utils::{Logical, Point, Rectangle, Serial, SERIAL_COUNTER},
    wayland::{
        buffer::BufferHandler,
        compositor::{with_states, CompositorClientState, CompositorHandler, CompositorState},
        output::{OutputHandler, OutputManagerState},
        seat::WaylandFocus,
        selection::{
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            SelectionHandler,
        },
        shell::{
            wlr_layer::{
                Layer, LayerSurface, LayerSurfaceData, WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::{
                decoration::{XdgDecorationHandler, XdgDecorationState},
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
                XdgToplevelSurfaceData,
            },
        },
        shm::{ShmHandler, ShmState},
    },
};

use crate::config::{BindingConfig, CompositorConfig, WindowLayout, WorkspacesConfig};
use crate::shortcuts::{ActivatedShortcut, PhysicalMods, ShortcutRegistry};

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

pub struct BlairState {
    pub loop_signal: LoopSignal,
    pub config: CompositorConfig,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub layer_shell_state: WlrLayerShellState,

    pub space: Space<Window>,
    pub popup_manager: PopupManager,
    pub layer_surfaces: Vec<DesktopLayerSurface>,
    workspaces: Vec<Workspace>,
    /// Workspace currently displayed by each output. A workspace is assigned
    /// to at most one output; switching to one visible elsewhere swaps them.
    output_workspaces: HashMap<String, u64>,
    focused_output: Option<String>,
    /// Workspace membership is independent from Smithay's `Space`, which
    /// keeps every non-minimized window mapped for multi-output rendering.
    window_workspaces: HashMap<WindowId, u64>,
    next_workspace_id: u64,
    pub shortcuts: ShortcutRegistry,
    static_bindings: HashMap<String, StaticBindingAction>,
    pub physical_mods: PhysicalMods,

    pub seat: Seat<Self>,
    pub focused_window: Option<WindowId>,

    pub primary_client: Option<std::process::Child>,
    pub exit_requested: bool,
    pub redraw_requested: bool,

    pub events: Arc<dyn EventChannel>,

    pub window_counter: u64,
    pub pending_move_request: Option<Window>,
}

pub struct MinimizedWindow {
    pub window: Window,
    pub location: Point<i32, Logical>,
}

struct Workspace {
    id: u64,
    name: String,
    output: Option<String>,
    minimized_windows: Vec<MinimizedWindow>,
    focused_window: Option<WindowId>,
}

const CONFIG_BINDING_OWNER: &str = "blair-config";

#[derive(Clone)]
enum StaticBindingAction {
    Close,
    Exec(String),
    Workspace(u64),
    MoveToWorkspace(u64),
}

fn configured_workspaces(config: &WorkspacesConfig) -> Vec<Workspace> {
    (1..=config.count)
        .map(|id| {
            let definition = config.definitions.get(&id.to_string());
            Workspace {
                id,
                name: definition
                    .and_then(|definition| definition.name.clone())
                    .unwrap_or_else(|| id.to_string()),
                output: definition.and_then(|definition| definition.output.clone()),
                minimized_windows: Vec::new(),
                focused_window: None,
            }
        })
        .collect()
}

impl BlairState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_signal: LoopSignal,
        config: CompositorConfig,
        events: Arc<dyn EventChannel>,
    ) -> Self {
        let compositor_state = CompositorState::new::<Self>(&display_handle);
        let xdg_shell_state = XdgShellState::new::<Self>(&display_handle);
        XdgDecorationState::new::<Self>(&display_handle);
        OutputManagerState::new_with_xdg_output::<Self>(&display_handle);
        let shm_state = ShmState::new::<Self>(&display_handle, vec![]);
        let data_device_state = DataDeviceState::new::<Self>(&display_handle);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&display_handle);

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&display_handle, "blair-seat-0");
        let keyboard_config = XkbConfig {
            layout: &config.input.keyboard.layout,
            variant: &config.input.keyboard.variant,
            ..Default::default()
        };
        seat.add_keyboard(
            keyboard_config,
            config.input.keyboard.repeat_delay,
            config.input.keyboard.repeat_rate,
        )
        .expect("failed to init keyboard");
        seat.add_pointer();

        let workspaces = configured_workspaces(&config.workspaces);
        let next_workspace_id = config.workspaces.count + 1;
        let mut state = Self {
            loop_signal,
            config,
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            data_device_state,
            layer_shell_state,
            space: Space::default(),
            popup_manager: PopupManager::default(),
            layer_surfaces: Vec::new(),
            workspaces,
            output_workspaces: HashMap::new(),
            focused_output: None,
            window_workspaces: HashMap::new(),
            next_workspace_id,
            shortcuts: ShortcutRegistry::default(),
            static_bindings: HashMap::new(),
            physical_mods: PhysicalMods::default(),
            seat,
            focused_window: None,
            primary_client: None,
            exit_requested: false,
            redraw_requested: false,
            events,
            window_counter: 0,
            pending_move_request: None,
        };
        state.replace_static_bindings(&state.config.bindings.clone());
        state
    }

    pub fn emit(&self, event: CompositorEvent) {
        self.events.publish(event);
    }

    fn replace_static_bindings(&mut self, bindings: &[BindingConfig]) {
        self.shortcuts.unregister_client(CONFIG_BINDING_OWNER);
        self.static_bindings.clear();
        for (index, binding) in bindings.iter().enumerate() {
            let id = index.to_string();
            let action = match (&binding.action, &binding.exec, binding.value) {
                (Some(action), None, None) if action == "close" => StaticBindingAction::Close,
                (Some(action), None, Some(value)) if action == "workspace" => {
                    StaticBindingAction::Workspace(value)
                }
                (Some(action), None, Some(value)) if action == "move-to-workspace" => {
                    StaticBindingAction::MoveToWorkspace(value)
                }
                (None, Some(command), None) => StaticBindingAction::Exec(command.clone()),
                _ => continue,
            };
            if self
                .shortcuts
                .bind(CONFIG_BINDING_OWNER, &id, &binding.accelerator())
            {
                self.static_bindings.insert(id, action);
            }
        }
    }

    pub fn activate_shortcuts(&mut self, shortcuts: Vec<ActivatedShortcut>) -> bool {
        if shortcuts.is_empty() {
            return false;
        }
        for shortcut in shortcuts {
            if shortcut.client == CONFIG_BINDING_OWNER {
                let Some(action) = self.static_bindings.get(&shortcut.id).cloned() else {
                    continue;
                };
                match action {
                    StaticBindingAction::Close => {
                        if let Some(id) = self.focused_window {
                            self.close_window_by_id(id);
                        }
                    }
                    StaticBindingAction::Exec(command) => {
                        if let Err(error) = Command::new("sh").arg("-c").arg(&command).spawn() {
                            tracing::warn!(%error, command, "failed to execute configured binding");
                        }
                    }
                    StaticBindingAction::Workspace(id) => {
                        self.ensure_workspace(id);
                        self.switch_workspace(id);
                    }
                    StaticBindingAction::MoveToWorkspace(id) => {
                        if let Some(window) = self.focused_window {
                            self.ensure_workspace(id);
                            self.move_window_to_workspace(window, id);
                        }
                    }
                }
            } else {
                self.emit(CompositorEvent::ShortcutActivated {
                    client: shortcut.client,
                    id: shortcut.id,
                });
            }
        }
        true
    }

    pub fn request_exit(&mut self) {
        tracing::info!("compositor exit requested");
        self.exit_requested = true;
        self.loop_signal.stop();
    }

    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    pub fn take_redraw_request(&mut self) -> bool {
        std::mem::take(&mut self.redraw_requested)
    }

    /// Applies the parts of a successfully parsed configuration that can
    /// safely change while the compositor is running.
    pub fn apply_config(&mut self, mut next: CompositorConfig) {
        if self.config.general.backend != next.general.backend {
            tracing::warn!(
                old = %self.config.general.backend,
                new = %next.general.backend,
                "backend changes require a compositor restart"
            );
            next.general.backend = self.config.general.backend.clone();
        }
        if self.config.integrations.dbus != next.integrations.dbus {
            tracing::warn!(
                old = self.config.integrations.dbus,
                new = next.integrations.dbus,
                "integration transport changes require a compositor restart"
            );
            next.integrations.dbus = self.config.integrations.dbus;
        }
        if self.config.outputs != next.outputs {
            tracing::warn!("output changes require a compositor restart");
            next.outputs = self.config.outputs.clone();
        }
        if self.config.input != next.input {
            tracing::warn!("input changes require a compositor restart");
            next.input = self.config.input.clone();
        }
        if self.config.workspaces != next.workspaces {
            tracing::warn!("workspace topology changes require a compositor restart");
            next.workspaces = self.config.workspaces.clone();
        }

        if self.config.general.primary_client != next.general.primary_client
            || self.config.general.spawn_primary_client != next.general.spawn_primary_client
        {
            tracing::info!("primary client settings will be used on the next compositor start");
        }

        let window_changed = self.config.window != next.window;
        let decoration_changed = self.config.decoration != next.decoration;
        let bindings_changed = self.config.bindings != next.bindings;
        let layout_changed = self.config.window.layout != next.window.layout;
        self.config = next;
        if bindings_changed {
            self.replace_static_bindings(&self.config.bindings.clone());
        }

        if layout_changed && self.config.window.layout == WindowLayout::Tiling {
            self.tile_focused_window();
        }
        if window_changed || decoration_changed {
            self.request_redraw();
        }
    }

    pub fn spawn_primary_client(&mut self) {
        if !self.config.general.spawn_primary_client {
            return;
        }
        let command = &self.config.general.primary_client;
        tracing::info!(command = %command, "spawning primary client");
        match primary_client_command(command).spawn() {
            Ok(child) => self.primary_client = Some(child),
            Err(err) => tracing::warn!(%err, command = %command, "failed to spawn primary client"),
        }
    }

    pub fn add_output(&mut self, output: &Output, location: Point<i32, Logical>) {
        self.space.map_output(output, location);
        let name = output.name();
        let workspace = self.workspace_for_new_output(&name);
        self.output_workspaces.insert(name.clone(), workspace);
        self.focused_output.get_or_insert(name.clone());
        self.emit(CompositorEvent::OutputAdded { name });
    }

    pub fn output_resized(&mut self, output: &Output) {
        let changed = layer_map_for_output(output).arrange();
        if changed {
            tracing::debug!(output = %output.name(), "rearranged layer surfaces after output resize");
            self.emit_work_area_changed(output);
        }
        if self.config.window.layout == WindowLayout::Tiling {
            self.tile_focused_window();
        }
    }

    fn reflow_layer_surface(&mut self, surface: &WlSurface) {
        let Some(output) = self
            .space
            .outputs()
            .find(|output| {
                layer_map_for_output(output)
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned()
        else {
            return;
        };
        let changed = layer_map_for_output(&output).arrange();
        if changed {
            self.emit_work_area_changed(&output);
        }
    }

    fn emit_work_area_changed(&self, output: &Output) {
        let zone = layer_map_for_output(output).non_exclusive_zone();
        self.emit(CompositorEvent::WorkAreaChanged {
            output: output.name(),
            area: Rect {
                x: zone.loc.x,
                y: zone.loc.y,
                width: zone.size.w,
                height: zone.size.h,
            },
        });
    }

    pub fn work_area(&self, output: &str) -> Rect {
        let output = if output.is_empty() {
            self.focused_output_name().and_then(|name| {
                self.space
                    .outputs()
                    .find(|candidate| candidate.name() == name)
            })
        } else {
            self.space
                .outputs()
                .find(|candidate| candidate.name() == output)
        };
        let Some(output) = output else {
            return Rect::default();
        };
        let zone = layer_map_for_output(output).non_exclusive_zone();
        Rect {
            x: zone.loc.x,
            y: zone.loc.y,
            width: zone.size.w,
            height: zone.size.h,
        }
    }

    pub fn outputs(&self) -> Vec<String> {
        self.space.outputs().map(Output::name).collect()
    }

    fn workspace(&self, id: u64) -> &Workspace {
        self.workspaces
            .iter()
            .find(|workspace| workspace.id == id)
            .expect("workspace must exist")
    }

    fn workspace_mut(&mut self, id: u64) -> &mut Workspace {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
            .expect("workspace must exist")
    }

    fn focused_output_name(&self) -> Option<String> {
        self.focused_output
            .clone()
            .or_else(|| self.space.outputs().next().map(Output::name))
    }

    fn focused_workspace_id(&self) -> u64 {
        self.focused_output_name()
            .and_then(|output| self.output_workspaces.get(&output).copied())
            .unwrap_or(1)
    }

    fn focused_workspace(&self) -> &Workspace {
        self.workspace(self.focused_workspace_id())
    }

    fn focused_workspace_mut(&mut self) -> &mut Workspace {
        let id = self.focused_workspace_id();
        self.workspace_mut(id)
    }

    fn workspace_for_new_output(&self, output: &str) -> u64 {
        self.workspaces
            .iter()
            .find(|workspace| {
                workspace.output.as_deref() == Some(output)
                    && !self
                        .output_workspaces
                        .values()
                        .any(|id| *id == workspace.id)
            })
            .or_else(|| {
                self.workspaces.iter().find(|workspace| {
                    !self
                        .output_workspaces
                        .values()
                        .any(|id| *id == workspace.id)
                })
            })
            .map(|workspace| workspace.id)
            .unwrap_or(1)
    }

    pub fn set_focused_output_at(&mut self, pos: Point<f64, Logical>) {
        if let Some(output) = self.space.outputs().find(|output| {
            self.space
                .output_geometry(output)
                .is_some_and(|geometry| geometry.to_f64().contains(pos))
        }) {
            self.focused_output = Some(output.name());
        }
    }

    pub fn workspace_for_output(&self, output: &Output) -> u64 {
        self.workspace_for_output_name(&output.name())
    }

    fn workspace_for_output_name(&self, output: &str) -> u64 {
        self.output_workspaces.get(output).copied().unwrap_or(1)
    }

    pub fn window_visible_on_output(&self, window: &Window, output: &Output) -> bool {
        self.window_id(window)
            .and_then(|id| self.window_workspaces.get(&id))
            .is_some_and(|workspace| *workspace == self.workspace_for_output(output))
    }

    pub fn create_workspace(&mut self, name: String) -> u64 {
        if !self.config.workspaces.dynamic {
            tracing::warn!("refusing API workspace creation while dynamic workspaces are disabled");
            return 0;
        }
        self.create_workspace_unchecked(name)
    }

    fn create_workspace_unchecked(&mut self, name: String) -> u64 {
        let id = self.next_workspace_id;
        self.next_workspace_id += 1;
        self.workspaces.push(Workspace {
            id,
            name: if name.trim().is_empty() {
                id.to_string()
            } else {
                name
            },
            output: None,
            minimized_windows: Vec::new(),
            focused_window: None,
        });
        id
    }

    fn ensure_workspace(&mut self, id: u64) {
        if id > self.config.workspaces.count && !self.config.workspaces.dynamic {
            tracing::warn!(
                workspace = id,
                "workspace is outside the static workspace set"
            );
            return;
        }
        while self.next_workspace_id <= id {
            self.create_workspace_unchecked(self.next_workspace_id.to_string());
        }
    }

    pub fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        self.workspaces
            .iter()
            .map(|workspace| WorkspaceInfo {
                id: workspace.id,
                name: workspace.name.clone(),
                active: self
                    .output_workspaces
                    .values()
                    .any(|id| *id == workspace.id),
                output: self
                    .output_workspaces
                    .iter()
                    .find_map(|(output, id)| (*id == workspace.id).then(|| output.clone())),
                window_count: self
                    .space
                    .elements()
                    .filter(|window| {
                        self.window_id(window)
                            .and_then(|id| self.window_workspaces.get(&id))
                            == Some(&workspace.id)
                    })
                    .count()
                    + workspace.minimized_windows.len(),
            })
            .collect()
    }

    pub fn switch_workspace(&mut self, id: u64) -> bool {
        let id = if self.workspaces.iter().any(|workspace| workspace.id == id) {
            id
        } else if self.config.workspaces.wrap && !self.config.workspaces.dynamic {
            let count = self.config.workspaces.count;
            if id == 0 {
                count
            } else {
                ((id - 1) % count) + 1
            }
        } else {
            id
        };
        let Some(output) = self.focused_output_name() else {
            return false;
        };
        let current_id = self.workspace_for_output_name(&output);
        if id == current_id {
            return true;
        }
        if !self.workspaces.iter().any(|workspace| workspace.id == id) {
            return false;
        }
        if let Some((other_output, _)) = self
            .output_workspaces
            .iter()
            .find(|(name, workspace)| **workspace == id && **name != output)
            .map(|(name, workspace)| (name.clone(), *workspace))
        {
            self.output_workspaces.insert(other_output, current_id);
        }
        self.output_workspaces.insert(output, id);
        self.focused_window = None;

        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        }
        if let Some(id) = self.workspace(id).focused_window {
            let _ = self.focus_window_by_id(id);
        }
        self.request_redraw();
        true
    }

    pub fn move_window_to_workspace(&mut self, id: WindowId, workspace_id: u64) -> bool {
        if !self
            .workspaces
            .iter()
            .any(|workspace| workspace.id == workspace_id)
        {
            return false;
        }
        if workspace_id == self.focused_workspace_id() {
            return self.window_by_id(id).is_some();
        }

        let mapped = self
            .space
            .elements()
            .find(|window| self.window_id(window) == Some(id))
            .cloned();
        if mapped.is_some() && self.window_workspaces.get(&id) == Some(&self.focused_workspace_id())
        {
            self.window_workspaces.insert(id, workspace_id);
            if self.focused_window == Some(id) {
                self.focused_window = None;
                self.focused_workspace_mut().focused_window = None;
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
                }
            }
            self.request_redraw();
            return true;
        }

        let Some(index) = self
            .focused_workspace()
            .minimized_windows
            .iter()
            .position(|entry| self.window_id(&entry.window) == Some(id))
        else {
            return false;
        };
        let entry = self.focused_workspace_mut().minimized_windows.remove(index);
        self.window_workspaces.insert(id, workspace_id);
        self.workspace_mut(workspace_id)
            .minimized_windows
            .push(entry);
        true
    }

    pub fn window_id(&self, window: &Window) -> Option<WindowId> {
        Self::window_id_of(window)
    }

    fn window_id_of(window: &Window) -> Option<WindowId> {
        let surface = window.wl_surface()?;
        with_states(&surface, |states| {
            states
                .data_map
                .get::<RefCell<WindowId>>()
                .map(|cell| *cell.borrow())
        })
    }

    fn window_by_id(&self, id: WindowId) -> Option<Window> {
        self.space
            .elements()
            .find(|window| {
                self.window_id(window) == Some(id)
                    && self.window_workspaces.get(&id) == Some(&self.focused_workspace_id())
            })
            .cloned()
            .or_else(|| {
                self.focused_workspace()
                    .minimized_windows
                    .iter()
                    .find(|entry| self.window_id(&entry.window) == Some(id))
                    .map(|entry| entry.window.clone())
            })
    }

    fn window_info(&self, window: &Window) -> Option<WindowInfo> {
        let id = self.window_id(window)?;
        let (title, app_id) = window_meta(window);
        let minimized = self
            .focused_workspace()
            .minimized_windows
            .iter()
            .any(|entry| self.window_id(&entry.window) == Some(id));
        let geometry = if minimized {
            self.focused_workspace()
                .minimized_windows
                .iter()
                .find(|entry| self.window_id(&entry.window) == Some(id))
                .map(|entry| {
                    let bbox = window.bbox();
                    Rect {
                        x: entry.location.x,
                        y: entry.location.y,
                        width: bbox.size.w,
                        height: bbox.size.h,
                    }
                })
                .unwrap_or_default()
        } else {
            self.space
                .element_location(window)
                .map(|loc| {
                    let bbox = window.bbox();
                    Rect {
                        x: loc.x,
                        y: loc.y,
                        width: bbox.size.w,
                        height: bbox.size.h,
                    }
                })
                .unwrap_or_default()
        };
        let maximized = window
            .toplevel()
            .map(|toplevel| {
                toplevel
                    .current_state()
                    .states
                    .contains(xdg_toplevel::State::Maximized)
            })
            .unwrap_or(false);

        Some(WindowInfo {
            id,
            title,
            app_id,
            geometry,
            focused: self.focused_window == Some(id),
            minimized,
            maximized,
        })
    }

    pub fn list_windows(&self) -> Vec<WindowInfo> {
        let workspace_id = self.focused_workspace_id();
        let mapped = self
            .space
            .elements()
            .filter(|window| {
                self.window_id(window)
                    .and_then(|id| self.window_workspaces.get(&id))
                    == Some(&workspace_id)
            })
            .filter_map(|window| self.window_info(window));
        let minimized = self
            .focused_workspace()
            .minimized_windows
            .iter()
            .filter_map(|entry| self.window_info(&entry.window));
        mapped.chain(minimized).collect()
    }

    pub fn focus_window(&mut self, window: &Window) -> bool {
        let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) else {
            return false;
        };
        self.space.raise_element(window, true);
        self.tile_window(window);
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        }
        true
    }

    pub fn tile_window(&mut self, window: &Window) {
        if self.config.window.layout != WindowLayout::Tiling {
            return;
        }
        let area = self.work_area("");
        let padding = self
            .config
            .window
            .work_area_padding
            .clamp(0, area.width.min(area.height) / 2);
        let width = (area.width - padding * 2).max(1);
        let height = (area.height - padding * 2).max(1);
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        toplevel.with_pending_state(|state| state.size = Some((width, height).into()));
        toplevel.send_configure();
        let location = (area.x + padding, area.y + padding);
        self.space.map_element(window.clone(), location, true);
        self.window_geometry_changed(window, location);
    }

    pub fn tile_focused_window(&mut self) {
        let Some(id) = self.focused_window else {
            return;
        };
        let window = self.window_by_id(id);
        if let Some(window) = window {
            self.tile_window(&window);
        }
    }

    pub fn focus_window_by_id(&mut self, id: WindowId) -> bool {
        let mapped_window = self
            .space
            .elements()
            .find(|window| self.window_id(window) == Some(id))
            .cloned();
        if let Some(window) = mapped_window {
            return self.focus_window(&window);
        }

        let Some(index) = self
            .focused_workspace()
            .minimized_windows
            .iter()
            .position(|entry| self.window_id(&entry.window) == Some(id))
        else {
            return false;
        };
        let entry = self.focused_workspace_mut().minimized_windows.remove(index);
        let window = entry.window;
        self.space.map_element(window.clone(), entry.location, true);
        self.emit(CompositorEvent::WindowRestored { id });
        self.focus_window(&window)
    }

    pub fn close_window(&mut self, window: &Window) -> bool {
        let Some(toplevel) = window.toplevel() else {
            return false;
        };
        toplevel.send_close();
        true
    }

    pub fn close_window_by_id(&mut self, id: WindowId) -> bool {
        self.window_by_id(id)
            .map(|window| self.close_window(&window))
            .unwrap_or(false)
    }

    pub fn minimize_window(&mut self, window: &Window) -> bool {
        let Some(id) = self.window_id(window) else {
            return false;
        };
        if self
            .focused_workspace()
            .minimized_windows
            .iter()
            .any(|entry| self.window_id(&entry.window) == Some(id))
        {
            return true;
        }
        let Some(location) = self.space.element_location(window) else {
            return false;
        };
        self.space.unmap_elem(window);
        self.focused_workspace_mut()
            .minimized_windows
            .push(MinimizedWindow {
                window: window.clone(),
                location,
            });
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        }
        self.emit(CompositorEvent::WindowMinimized { id });
        true
    }

    pub fn minimize_window_by_id(&mut self, id: WindowId) -> bool {
        self.window_by_id(id)
            .map(|window| self.minimize_window(&window))
            .unwrap_or(false)
    }

    pub fn toggle_maximize_window(&mut self, window: &Window) -> bool {
        let Some(toplevel) = window.toplevel() else {
            return false;
        };
        let Some(id) = self.window_id(window) else {
            return false;
        };
        let maximized = !toplevel
            .current_state()
            .states
            .contains(xdg_toplevel::State::Maximized);
        let size = self
            .space
            .outputs()
            .find(|output| Some(output.name()) == self.focused_output_name())
            .and_then(|output| self.space.output_geometry(output))
            .map(|geometry| geometry.size);

        toplevel.with_pending_state(|state| {
            if maximized {
                state.states.set(xdg_toplevel::State::Maximized);
                state.size = size;
            } else {
                state.states.unset(xdg_toplevel::State::Maximized);
                state.size = None;
            }
        });
        toplevel.send_configure();
        if maximized {
            let output_loc = self
                .space
                .outputs()
                .find(|output| Some(output.name()) == self.focused_output_name())
                .and_then(|output| self.space.output_geometry(output))
                .map(|geometry| geometry.loc);
            if let Some(loc) = output_loc {
                self.space.map_element(window.clone(), loc, true);
            }
        }
        self.emit(CompositorEvent::WindowMaximized { id, maximized });
        true
    }

    pub fn toggle_maximize_window_by_id(&mut self, id: WindowId) -> bool {
        self.window_by_id(id)
            .map(|window| self.toggle_maximize_window(&window))
            .unwrap_or(false)
    }

    pub fn move_resize_window(&mut self, id: WindowId, geometry: Rect) -> bool {
        let Some(window) = self.window_by_id(id) else {
            return false;
        };
        let Some(toplevel) = window.toplevel() else {
            return false;
        };
        toplevel.with_pending_state(|state| {
            state.size = Some((geometry.width, geometry.height).into());
        });
        toplevel.send_configure();
        self.space
            .map_element(window.clone(), (geometry.x, geometry.y), true);
        self.window_geometry_changed(&window, (geometry.x, geometry.y));
        true
    }

    pub fn window_geometry_changed(&self, window: &Window, location: (i32, i32)) {
        let Some(id) = self.window_id(window) else {
            return;
        };
        let bbox = window.bbox();
        self.emit(CompositorEvent::WindowGeometryChanged {
            id,
            geometry: Rect {
                x: location.0,
                y: location.1,
                width: bbox.size.w,
                height: bbox.size.h,
            },
        });
    }

    pub fn take_pending_move_request(&mut self) -> Option<Window> {
        self.pending_move_request.take()
    }

    pub fn focus_interactive_layer_surface(&mut self, surface: &WlSurface) -> bool {
        let Some(layer_surface) = self.layer_surfaces.iter().find(|layer_surface| {
            layer_surface.wl_surface() == surface && layer_surface.can_receive_keyboard_focus()
        }) else {
            return false;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        if keyboard.current_focus().is_some() {
            return false;
        }

        let surface = layer_surface.wl_surface().clone();
        keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
        true
    }

    fn popup_constraint_target(&self, surface: &PopupSurface) -> Rectangle<i32, Logical> {
        let output_size = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map(|geo| geo.size)
            .unwrap_or_default();
        let parent_origin = surface
            .get_parent_surface()
            .and_then(|parent| self.toplevel_origin(&parent))
            .unwrap_or_default();
        Rectangle::new((-parent_origin.x, -parent_origin.y).into(), output_size)
    }

    fn toplevel_origin(&self, surface: &WlSurface) -> Option<Point<i32, Logical>> {
        let window = self
            .space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(surface))?;
        self.space.element_location(window)
    }

    fn toplevel_window_id(surface: &ToplevelSurface) -> Option<WindowId> {
        with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<RefCell<WindowId>>()
                .map(|cell| *cell.borrow())
        })
    }
}

fn window_meta(window: &Window) -> (String, Option<String>) {
    let Some(surface) = window.wl_surface() else {
        return (String::new(), None);
    };
    with_states(&surface, |states| {
        states.data_map.get::<XdgToplevelSurfaceData>().map(|data| {
            let data = data.lock().unwrap();
            (data.title.clone().unwrap_or_default(), data.app_id.clone())
        })
    })
    .unwrap_or_default()
}

fn primary_client_command(command: &str) -> Command {
    let mut parts = command.split_whitespace();
    let program = parts.next().unwrap_or(command);
    let args: Vec<&str> = parts.collect();
    let mut process = Command::new(program);
    process.args(&args);
    process
}

impl BufferHandler for BlairState {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl CompositorHandler for BlairState {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);

        if let Some(window) = self
            .space
            .elements()
            .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
            .cloned()
        {
            window.on_commit();
        }

        self.popup_manager.commit(surface);
        ensure_initial_configure(surface, &self.space, &mut self.popup_manager);
        self.focus_interactive_layer_surface(surface);
        self.reflow_layer_surface(surface);
    }
}

delegate_compositor!(BlairState);

fn ensure_initial_configure(surface: &WlSurface, space: &Space<Window>, popups: &mut PopupManager) {
    if let Some(window) = space
        .elements()
        .find(|window| window.wl_surface().map(|s| &*s == surface).unwrap_or(false))
        .cloned()
    {
        if let Some(toplevel) = window.toplevel() {
            let initial_configure_sent = with_states(surface, |states| {
                states
                    .data_map
                    .get::<XdgToplevelSurfaceData>()
                    .unwrap()
                    .lock()
                    .unwrap()
                    .initial_configure_sent
            });
            if !initial_configure_sent {
                toplevel.send_configure();
            }
        }
        return;
    }

    if let Some(popup) = popups.find_popup(surface) {
        let popup = match popup {
            PopupKind::Xdg(ref popup) => popup,
            PopupKind::InputMethod(_) => return,
        };
        if !popup.is_initial_configure_sent() {
            let _ = popup.send_configure();
        }
        return;
    }

    if let Some(output) = space.outputs().find(|output| {
        layer_map_for_output(output)
            .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
            .is_some()
    }) {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });
        if !initial_configure_sent {
            let map = layer_map_for_output(output);
            let layer = map
                .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                .unwrap();
            layer.layer_surface().send_configure();
        }
    }
}

impl XdgShellHandler for BlairState {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let (title, app_id) = with_states(surface.wl_surface(), |states| {
            let data = states
                .data_map
                .get::<XdgToplevelSurfaceData>()?
                .lock()
                .unwrap();
            Some((data.title.clone(), data.app_id.clone()))
        })
        .unwrap_or_default();

        let window = Window::new_wayland_window(surface);

        let workspace_id = self.focused_workspace_id();
        let workspace_output = self.focused_output_name();
        let pos: Point<i32, Logical> = self
            .space
            .outputs()
            .find(|output| {
                workspace_output
                    .as_deref()
                    .is_some_and(|name| output.name() == name)
            })
            .or_else(|| self.space.outputs().next())
            .and_then(|output| self.space.output_geometry(output))
            .map(|geometry| {
                let width = self.config.window.default_width;
                let height = self.config.window.default_height;
                (
                    (geometry.size.w - width) / 2,
                    (geometry.size.h - height) / 2,
                )
                    .into()
            })
            .unwrap_or_else(|| (100, 100).into());

        self.window_counter += 1;
        let id = WindowId(self.window_counter);

        if let Some(wl_surface) = window.wl_surface() {
            with_states(&wl_surface, |states| {
                states.data_map.insert_if_missing(|| RefCell::new(id));
                if let Some(cell) = states.data_map.get::<RefCell<WindowId>>() {
                    *cell.borrow_mut() = id;
                }
            });
        }

        self.space.map_element(window.clone(), pos, true);
        self.window_workspaces.insert(id, workspace_id);

        self.emit(CompositorEvent::WindowOpened {
            id,
            title: title.unwrap_or_default(),
            app_id,
        });

        self.focus_window(&window);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        if let Some(id) = Self::toplevel_window_id(&surface) {
            self.window_workspaces.remove(&id);
            for workspace in &mut self.workspaces {
                workspace
                    .minimized_windows
                    .retain(|entry| Self::window_id_of(&entry.window) != Some(id));
                if workspace.focused_window == Some(id) {
                    workspace.focused_window = None;
                }
            }
            self.emit(CompositorEvent::WindowClosed { id });
        }
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        let Some(id) = Self::toplevel_window_id(&surface) else {
            return;
        };
        let title = with_states(surface.wl_surface(), |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|data| data.lock().ok()?.title.clone())
        })
        .unwrap_or_default();
        self.emit(CompositorEvent::WindowTitleChanged { id, title });
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        let target = self.popup_constraint_target(&surface);
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_unconstrained_geometry(target);
            state.positioner = positioner;
        });
        self.popup_manager.track_popup(surface.into()).ok();
    }

    fn move_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _serial: Serial,
    ) {
        let Some(window) = self
            .space
            .elements()
            .find(|window| {
                window
                    .wl_surface()
                    .map(|wl_surface| &*wl_surface == surface.wl_surface())
                    .unwrap_or(false)
            })
            .cloned()
        else {
            tracing::warn!("xdg move requested for an unmapped toplevel");
            return;
        };

        tracing::debug!("xdg interactive move requested");
        self.pending_move_request = Some(window);
    }

    fn grab(
        &mut self,
        _surface: PopupSurface,
        _seat: smithay::reexports::wayland_server::protocol::wl_seat::WlSeat,
        _serial: Serial,
    ) {
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        let target = self.popup_constraint_target(&surface);
        surface.with_pending_state(|state| {
            state.geometry = positioner.get_unconstrained_geometry(target);
            state.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}

impl XdgDecorationHandler for BlairState {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_decoration_mode();
        tracing::debug!(app_id = ?toplevel_app_id(&toplevel), ?mode, "xdg-decoration: new_decoration");
        set_surface_decoration_mode(toplevel.wl_surface(), mode);
        toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
        toplevel.send_configure();
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        tracing::debug!(app_id = ?toplevel_app_id(&toplevel), ?mode, "xdg-decoration: request_mode");
        set_surface_decoration_mode(toplevel.wl_surface(), mode);
        toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
        toplevel.send_configure();
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        let mode = self.preferred_decoration_mode();
        tracing::debug!(app_id = ?toplevel_app_id(&toplevel), ?mode, "xdg-decoration: unset_mode");
        set_surface_decoration_mode(toplevel.wl_surface(), mode);
        toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
        toplevel.send_configure();
    }
}

fn toplevel_app_id(toplevel: &ToplevelSurface) -> Option<String> {
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .map(|data| data.lock().unwrap().app_id.clone())
    })
    .flatten()
}

impl BlairState {
    fn preferred_decoration_mode(&self) -> DecorationMode {
        if self.config.window.server_side_decorations {
            DecorationMode::ServerSide
        } else {
            DecorationMode::ClientSide
        }
    }
}

fn set_surface_decoration_mode(surface: &WlSurface, mode: DecorationMode) {
    with_states(surface, |states| {
        states
            .data_map
            .insert_if_missing(|| std::cell::Cell::new(mode));
        if let Some(cell) = states.data_map.get::<std::cell::Cell<DecorationMode>>() {
            cell.set(mode);
        }
    });
}

pub fn window_wants_server_decoration(window: &Window) -> bool {
    let Some(surface) = window.wl_surface() else {
        return false;
    };
    with_states(&surface, |states| {
        states
            .data_map
            .get::<std::cell::Cell<DecorationMode>>()
            .map(|cell| cell.get() == DecorationMode::ServerSide)
            .unwrap_or(true)
    })
}

delegate_xdg_shell!(BlairState);
delegate_xdg_decoration!(BlairState);

impl WlrLayerShellHandler for BlairState {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        _output: Option<WlOutput>,
        layer: Layer,
        namespace: String,
    ) {
        tracing::debug!(namespace = %namespace, ?layer, "new layer surface");
        let desktop_surface = DesktopLayerSurface::new(surface, namespace.clone());
        let output = self.space.outputs().next().cloned();
        if let Some(output) = &output {
            let mut layer_map = layer_map_for_output(output);
            if let Err(err) = layer_map.map_layer(&desktop_surface) {
                tracing::warn!(?err, namespace = %namespace, "failed to map layer surface");
            }
        }
        if desktop_surface.can_receive_keyboard_focus() {
            let surface = desktop_surface.wl_surface().clone();
            if let Some(keyboard) = self.seat.get_keyboard() {
                tracing::debug!(namespace = %namespace, "focusing interactive layer surface");
                keyboard.set_focus(self, Some(surface), SERIAL_COUNTER.next_serial());
            }
        }
        self.layer_surfaces.push(desktop_surface);
        if let Some(output) = &output {
            self.emit_work_area_changed(output);
        }
    }

    fn new_popup(&mut self, parent: LayerSurface, popup: PopupSurface) {
        let Some(desktop_parent) = self
            .layer_surfaces
            .iter()
            .find(|layer| layer.wl_surface() == parent.wl_surface())
        else {
            return;
        };
        tracing::debug!(
            namespace = desktop_parent.namespace(),
            "layer-shell popup parented"
        );

        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(parent_geo) = layer_map_for_output(&output).layer_geometry(desktop_parent) else {
            return;
        };
        let output_size = self
            .space
            .output_geometry(&output)
            .map(|geo| geo.size)
            .unwrap_or_default();
        let target = Rectangle::new((-parent_geo.loc.x, -parent_geo.loc.y).into(), output_size);
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        tracing::debug!("layer surface destroyed");
        let Some(index) = self
            .layer_surfaces
            .iter()
            .position(|mapped| mapped.wl_surface() == surface.wl_surface())
        else {
            return;
        };
        let mapped = self.layer_surfaces.remove(index);
        let outputs: Vec<Output> = self.space.outputs().cloned().collect();
        for output in &outputs {
            layer_map_for_output(output).unmap_layer(&mapped);
        }
        for output in &outputs {
            self.emit_work_area_changed(output);
        }
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

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let id = focused.and_then(|surface| {
            with_states(surface, |states| {
                states
                    .data_map
                    .get::<RefCell<WindowId>>()
                    .map(|cell| *cell.borrow())
            })
        });
        self.focused_window = id;
        if let Some(id) = id {
            if let Some(workspace_id) = self.window_workspaces.get(&id).copied() {
                self.workspace_mut(workspace_id).focused_window = Some(id);
            }
        }
        match id {
            Some(id) => self.emit(CompositorEvent::WindowFocused { id }),
            None => self.emit(CompositorEvent::FocusCleared),
        }
    }
}

delegate_seat!(BlairState);

impl SelectionHandler for BlairState {
    type SelectionUserData = ();
}

impl DataDeviceHandler for BlairState {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for BlairState {}
impl ServerDndGrabHandler for BlairState {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd, _seat: Seat<Self>) {}
}

delegate_data_device!(BlairState);

impl OutputHandler for BlairState {
    fn output_bound(&mut self, _output: Output, _wl_output: WlOutput) {}
}

delegate_output!(BlairState);
