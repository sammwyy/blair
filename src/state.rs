use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use blair_integration::EventChannel;
use blair_protocol::{CompositorEvent, Rect, WindowId, WindowInfo, WorkspaceInfo};
use smithay::{
    backend::{allocator::dmabuf::Dmabuf, renderer::utils::with_renderer_surface_state},
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output,
        LayerSurface as DesktopLayerSurface, PopupGrab, PopupKind, PopupManager, Space, Window,
        WindowSurfaceType,
    },
    input::{
        keyboard::{LedState, XkbConfig},
        pointer::{CursorImageStatus, Focus, GrabStartData, MotionEvent},
        Seat, SeatState,
    },
    output::Output,
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            LoopHandle, LoopSignal, RegistrationToken,
        },
        wayland_protocols::xdg::{
            decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
            shell::server::xdg_toplevel,
        },
        wayland_protocols_misc::server_decoration::server::org_kde_kwin_server_decoration_manager::Mode as KdeDecorationMode,
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
            DisplayHandle,
        },
    },
    utils::{
        Clock, ClockSource, IsAlive, Logical, Monotonic, Point, Rectangle, Serial, Size,
        SERIAL_COUNTER,
    },
    wayland::{
        alpha_modifier::AlphaModifierState,
        compositor::{get_parent, with_states, CompositorClientState, CompositorState},
        cursor_shape::CursorShapeManagerState,
        dmabuf::{DmabufGlobal, DmabufState, ImportNotifier},
        foreign_toplevel_list::{ForeignToplevelHandle, ForeignToplevelListState},
        fractional_scale::FractionalScaleManagerState,
        idle_inhibit::IdleInhibitManagerState,
        idle_notify::IdleNotifierState,
        keyboard_shortcuts_inhibit::KeyboardShortcutsInhibitState,
        output::OutputManagerState,
        pointer_constraints::PointerConstraintsState,
        pointer_gestures::PointerGesturesState,
        presentation::PresentationState,
        relative_pointer::RelativePointerManagerState,
        seat::WaylandFocus,
        selection::{
            data_device::DataDeviceState, primary_selection::PrimarySelectionState,
            wlr_data_control::DataControlState,
        },
        session_lock::SessionLockManagerState,
        shell::{
            kde::decoration::KdeDecorationState,
            wlr_layer::{KeyboardInteractivity, Layer, LayerSurfaceData, WlrLayerShellState},
            xdg::{
                decoration::XdgDecorationState, PopupSurface, ToplevelSurface, XdgShellState,
                XdgToplevelSurfaceData,
            },
        },
        shm::ShmState,
        single_pixel_buffer::SinglePixelBufferState,
        viewporter::ViewporterState,
        xdg_activation::XdgActivationState,
    },
};

use crate::{
    blur,
    config::{
        AnimationConfig, AnimationCurve, BindingConfig, CompositorConfig, DecorationModeConfig,
        SystemAccent, WindowLayout, WindowRuleConfig, WorkspacesConfig,
    },
    cursor::CursorManager,
    decorations::DecorationTheme,
    grabs::{MoveGrab, ResizeAnchor, ResizeEdges, ResizeGrab},
    input::surface_under,
    render::{from_rect, to_rect, WindowDecoration},
    rules::{AppliedWindowRule, RuleSet},
    screencopy::{self, ScreencopyRequest},
    screenshot::Screenshots,
    shortcuts::{ActivatedShortcut, PhysicalMods, ShortcutRegistry},
    stats::RenderStats,
    window_integration,
};

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        tracing::debug!(?client_id, "wayland client connected");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        tracing::debug!(?client_id, ?reason, "wayland client disconnected");
    }
}

pub struct ManagedWindow {
    pub window: Window,
    pub workspace: u64,
    pub rules: AppliedWindowRule,
    pub mapped: bool,
    pub opened_at: Option<Instant>,
    pub minimized: Option<Point<i32, Logical>>,
    pub restore: Option<Rectangle<i32, Logical>>,
    pub fullscreen: bool,
    pub resize: Option<(ResizeAnchor, bool)>,
    pub foreign: Option<ForeignToplevelHandle>,
    pub title: String,
    pub app_id: Option<String>,
}

struct Workspace {
    id: u64,
    name: String,
    output: Option<String>,
    focused_window: Option<WindowId>,
    focus_history: Vec<WindowId>,
}

impl Workspace {
    fn new(id: u64, name: String, output: Option<String>) -> Self {
        Self {
            id,
            name,
            output,
            focused_window: None,
            focus_history: Vec::new(),
        }
    }
}

const CONFIG_BINDING_OWNER: &str = "blair-config";

#[derive(Clone)]
enum StaticBindingAction {
    Close,
    Exec(String),
    Workspace(u64),
    MoveToWorkspace(u64),
}

struct AutostartProcess {
    command: String,
    restart: bool,
    child: Child,
    next_restart: Instant,
}

pub struct BlairState {
    pub display_handle: DisplayHandle,
    pub loop_handle: LoopHandle<'static, BlairState>,
    pub loop_signal: LoopSignal,
    pub clock: Clock<Monotonic>,
    pub config: CompositorConfig,
    decoration_theme: DecorationTheme,
    system_accent: Option<SystemAccent>,
    pub icon_cache: crate::decorations::IconCache,

    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub touch_points: HashMap<smithay::backend::input::TouchSlot, Point<f64, Logical>>,
    pub data_device_state: DataDeviceState,
    pub primary_selection_state: PrimarySelectionState,
    pub data_control_state: DataControlState,
    pub layer_shell_state: WlrLayerShellState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub xdg_activation_state: XdgActivationState,
    pub foreign_toplevel_state: ForeignToplevelListState,
    pub idle_notifier_state: IdleNotifierState<Self>,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub session_lock_state: SessionLockManagerState,
    pub pending_session_lock: Option<crate::session_lock::PendingSessionLock>,
    pub session_lock_client: Option<ClientId>,
    pub kde_decoration_state: KdeDecorationState,
    pub idle_inhibitors: HashSet<WlSurface>,
    idle_dimming: crate::idle_dim::IdleDimming,
    idle_dim_timer: Option<RegistrationToken>,
    pub session_locked: bool,
    pub session_lock_surfaces: Vec<crate::session_lock::LockSurfaceEntry>,
    pub session_lock_buffer: smithay::backend::renderer::element::solid::SolidColorBuffer,

    pub space: Space<Window>,
    pub popup_manager: PopupManager,
    pub active_popup_grab: Option<PopupGrab<Self>>,
    pub layer_surfaces: Vec<DesktopLayerSurface>,
    pub windows: BTreeMap<WindowId, ManagedWindow>,
    workspaces: Vec<Workspace>,
    output_workspaces: HashMap<String, u64>,
    focused_output: Option<String>,
    workspace_animation_started: HashMap<String, Instant>,
    next_workspace_id: u64,
    next_window_id: u64,
    pub rules: RuleSet,
    pub shortcuts: ShortcutRegistry,
    static_bindings: HashMap<String, StaticBindingAction>,
    pub physical_mods: PhysicalMods,
    pub suppressed_keys: HashSet<u32>,

    pub seat: Seat<Self>,
    pub focused_window: Option<WindowId>,
    pub hovered_button: Option<(WindowId, crate::config::DecorationButton)>,
    pub hot_corners: crate::hot_corners::HotCornerState,
    pub sticky_keys: crate::accessibility::StickyKeys,
    pub visual_bell_until: Option<Instant>,
    pub visual_bell_buffer: smithay::backend::renderer::element::solid::SolidColorBuffer,
    pub magnifiers: HashMap<Output, crate::render::Magnifier>,
    /// The cursor last requested by the focused client.
    pub client_cursor: CursorImageStatus,
    /// A cursor the compositor shows instead of the client's, over its own
    /// decorations and during interactive moves and resizes.
    pub compositor_cursor: Option<smithay::input::pointer::CursorIcon>,
    pub cursor: CursorManager,
    pub dnd_icon: Option<WlSurface>,
    pub decorations: HashMap<WindowId, WindowDecoration>,
    pub button_buffers: crate::render::ButtonBuffers,
    pub led_state: LedState,
    pub pending_dmabuf_imports: Vec<(Dmabuf, ImportNotifier)>,
    pub screenshots: Screenshots,
    pub pending_captures: Vec<ScreencopyRequest>,
    pub input_config_changed: bool,
    pub input_devices: crate::input_devices::InputDevices,
    pub pending_display_requests: Vec<crate::outputs::DisplayRequest>,
    pub mutable_displays: HashSet<String>,
    pub display_confirmations: HashMap<String, Instant>,
    pointer_focus_dirty: bool,

    autostart_processes: Vec<AutostartProcess>,
    spawned_children: Vec<Child>,
    pub child_env: Vec<(String, String)>,
    pub exit_requested: bool,
    redraw_requested: bool,
    pub render_stats: RenderStats,

    pub events: Arc<dyn EventChannel>,
}

fn load_system_accent() -> Option<SystemAccent> {
    match creamui_theme_loader::SystemThemeLoader::new().load() {
        Ok(resolved) => {
            if let Some(font_family) = &resolved.font_family {
                creamui_fonts::use_system_font(font_family);
            }
            Some(resolved.into())
        }
        Err(error) => {
            tracing::debug!(%error, "no CreamUI system theme found, using configured colors");
            None
        }
    }
}

fn configured_workspaces(config: &WorkspacesConfig) -> Vec<Workspace> {
    (1..=config.count)
        .map(|id| {
            let definition = config.definitions.get(&id.to_string());
            Workspace::new(
                id,
                definition
                    .and_then(|definition| definition.name.clone())
                    .unwrap_or_else(|| id.to_string()),
                definition.and_then(|definition| definition.output.clone()),
            )
        })
        .collect()
}

impl BlairState {
    pub fn new(
        display_handle: DisplayHandle,
        loop_handle: LoopHandle<'static, BlairState>,
        loop_signal: LoopSignal,
        mut config: CompositorConfig,
        events: Arc<dyn EventChannel>,
    ) -> Self {
        let dh = &display_handle;
        let compositor_state = CompositorState::new_v6::<Self>(dh);
        let xdg_shell_state = XdgShellState::new::<Self>(dh);
        smithay::wayland::xdg_system_bell::XdgSystemBellState::new::<Self>(dh);
        XdgDecorationState::new::<Self>(dh);
        OutputManagerState::new_with_xdg_output::<Self>(dh);
        let shm_state = ShmState::new::<Self>(dh, vec![]);
        let data_device_state = DataDeviceState::new::<Self>(dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(dh);
        let data_control_state =
            DataControlState::new::<Self, _>(dh, Some(&primary_selection_state), |_| true);
        let layer_shell_state = WlrLayerShellState::new::<Self>(dh);
        PresentationState::new::<Self>(dh, Monotonic::ID as u32);
        CursorShapeManagerState::new::<Self>(dh);
        ViewporterState::new::<Self>(dh);
        FractionalScaleManagerState::new::<Self>(dh);
        let xdg_activation_state = XdgActivationState::new::<Self>(dh);
        let foreign_toplevel_state = ForeignToplevelListState::new::<Self>(dh);
        SinglePixelBufferState::new::<Self>(dh);
        RelativePointerManagerState::new::<Self>(dh);
        PointerConstraintsState::new::<Self>(dh);
        IdleInhibitManagerState::new::<Self>(dh);
        PointerGesturesState::new::<Self>(dh);
        AlphaModifierState::new::<Self>(dh);
        let idle_notifier_state = IdleNotifierState::new(dh, loop_handle.clone());
        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(dh);
        let session_lock_state = SessionLockManagerState::new::<Self, _>(dh, |_| true);
        screencopy::register::<Self>(dh);
        blur::register(dh);
        window_integration::register(dh);
        let kde_decoration_state = KdeDecorationState::new::<Self>(
            dh,
            if config.window.server_side_decorations {
                KdeDecorationMode::Server
            } else {
                KdeDecorationMode::Client
            },
        );

        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");
        let keyboard = &config.input.keyboard;
        let xkb = XkbConfig {
            layout: &keyboard.layout,
            variant: &keyboard.variant,
            ..Default::default()
        };
        if let Err(error) = seat.add_keyboard(xkb, keyboard.repeat_delay, keyboard.repeat_rate) {
            tracing::error!(?error, layout = %keyboard.layout, "invalid keymap, falling back to us");
            let repeat_delay = keyboard.repeat_delay;
            let repeat_rate = keyboard.repeat_rate;
            config.input.keyboard.layout = "us".into();
            config.input.keyboard.variant.clear();
            seat.add_keyboard(
                XkbConfig {
                    layout: "us",
                    ..Default::default()
                },
                repeat_delay,
                repeat_rate,
            )
            .expect("the default xkb keymap must compile");
        }
        seat.add_pointer();

        let cursor = CursorManager::new(config.cursor.theme.as_deref(), config.cursor.size);
        let mut rules = RuleSet::default();
        rules.set_configured(&config.rules);
        let workspaces = configured_workspaces(&config.workspaces);
        let next_workspace_id = config.workspaces.count + 1;
        let system_accent = load_system_accent();
        let decoration_theme = config.decorations.to_theme(system_accent);
        let mut state = Self {
            display_handle,
            loop_handle,
            loop_signal,
            clock: Clock::new(),
            decoration_theme,
            system_accent,
            icon_cache: crate::decorations::IconCache::new(config.appearance.icon_theme.as_deref()),
            compositor_state,
            xdg_shell_state,
            shm_state,
            seat_state,
            touch_points: HashMap::new(),
            data_device_state,
            primary_selection_state,
            data_control_state,
            layer_shell_state,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            xdg_activation_state,
            foreign_toplevel_state,
            idle_notifier_state,
            keyboard_shortcuts_inhibit_state,
            session_lock_state,
            pending_session_lock: None,
            session_lock_client: None,
            kde_decoration_state,
            idle_inhibitors: HashSet::new(),
            idle_dimming: crate::idle_dim::IdleDimming::default(),
            idle_dim_timer: None,
            session_locked: false,
            session_lock_surfaces: Vec::new(),
            session_lock_buffer: crate::session_lock::background_buffer(),
            space: Space::default(),
            popup_manager: PopupManager::default(),
            active_popup_grab: None,
            layer_surfaces: Vec::new(),
            windows: BTreeMap::new(),
            workspaces,
            output_workspaces: HashMap::new(),
            focused_output: None,
            workspace_animation_started: HashMap::new(),
            next_workspace_id,
            next_window_id: 1,
            rules,
            shortcuts: ShortcutRegistry::default(),
            static_bindings: HashMap::new(),
            physical_mods: PhysicalMods::default(),
            suppressed_keys: HashSet::new(),
            seat,
            focused_window: None,
            hovered_button: None,
            hot_corners: crate::hot_corners::HotCornerState::default(),
            sticky_keys: crate::accessibility::StickyKeys::default(),
            visual_bell_until: None,
            magnifiers: HashMap::new(),
            visual_bell_buffer: smithay::backend::renderer::element::solid::SolidColorBuffer::new(
                (1, 1),
                smithay::backend::renderer::Color32F::new(0.18, 0.18, 0.18, 0.18),
            ),
            client_cursor: CursorImageStatus::default_named(),
            compositor_cursor: None,
            cursor,
            dnd_icon: None,
            decorations: HashMap::new(),
            button_buffers: crate::render::ButtonBuffers::default(),
            led_state: LedState::default(),
            pending_dmabuf_imports: Vec::new(),
            screenshots: Screenshots::default(),
            pending_captures: Vec::new(),
            input_config_changed: false,
            input_devices: crate::input_devices::InputDevices::default(),
            pending_display_requests: Vec::new(),
            mutable_displays: HashSet::new(),
            display_confirmations: HashMap::new(),
            pointer_focus_dirty: false,
            autostart_processes: Vec::new(),
            spawned_children: Vec::new(),
            child_env: Vec::new(),
            exit_requested: false,
            redraw_requested: true,
            render_stats: RenderStats::default(),
            events,
            config,
        };
        state.refresh_child_env();
        state.replace_static_bindings(&state.config.bindings.clone());
        state.schedule_idle_dim();
        state
    }

    pub fn emit(&self, event: CompositorEvent) {
        self.events.publish(event);
    }

    pub fn clock_now(&self) -> Duration {
        self.clock.now().into()
    }

    pub fn pointer_location(&self) -> Point<f64, Logical> {
        self.seat
            .get_pointer()
            .map(|pointer| pointer.current_location())
            .unwrap_or_default()
    }

    pub fn decoration_theme(&self) -> &DecorationTheme {
        &self.decoration_theme
    }

    /// Re-reads the CreamUI system theme and re-derives the decoration
    /// theme from it, in response to an `org.creamui.Theme.ReloadTheme`
    /// D-Bus signal.
    pub fn reload_system_theme(&mut self) {
        self.system_accent = load_system_accent();
        self.decoration_theme = self.config.decorations.to_theme(self.system_accent);
        self.relayout_all();
        tracing::info!("reloaded decoration colors from the CreamUI system theme");
        self.request_redraw();
    }

    /// The cursor to draw: the compositor's own while it owns the pointer,
    /// the client's otherwise.
    pub fn pointer_cursor(&self) -> CursorImageStatus {
        match self.compositor_cursor {
            Some(icon) => CursorImageStatus::Named(icon),
            None => self.client_cursor.clone(),
        }
    }

    pub fn corner_radius(&self, frame: Rectangle<i32, Logical>) -> i32 {
        self.decoration_theme
            .corner_radius
            .min(frame.size.w / 2)
            .min(frame.size.h / 2)
            .max(0)
    }

    pub fn popup_corner_radius(&self, size: Size<i32, Logical>) -> i32 {
        self.decoration_theme
            .popup_corner_radius
            .min(size.w / 2)
            .min(size.h / 2)
            .max(0)
    }

    pub fn request_exit(&mut self) {
        tracing::info!("compositor exit requested");
        self.exit_requested = true;
        self.loop_signal.stop();
        self.loop_signal.wakeup();
    }

    pub fn request_redraw(&mut self) {
        self.redraw_requested = true;
    }

    pub fn take_redraw_request(&mut self) -> bool {
        std::mem::take(&mut self.redraw_requested)
    }

    pub fn mark_pointer_focus_dirty(&mut self) {
        self.pointer_focus_dirty = true;
    }

    pub fn note_input_activity(&mut self) {
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
        self.idle_dimming.activity();
        self.schedule_idle_dim();
    }

    fn schedule_idle_dim(&mut self) {
        if !self.config.power.idle_dim || self.idle_dim_timer.is_some() {
            return;
        }
        let deadline = self
            .idle_dimming
            .deadline(self.config.power.idle_dim_timeout);
        match self.loop_handle.insert_source(
            Timer::from_deadline(deadline),
            |_, _, state: &mut BlairState| {
                let deadline = state
                    .idle_dimming
                    .deadline(state.config.power.idle_dim_timeout);
                if deadline > Instant::now() {
                    return TimeoutAction::ToInstant(deadline);
                }
                state.idle_dim_timer = None;
                state.refresh_idle_dimming();
                TimeoutAction::Drop
            },
        ) {
            Ok(token) => self.idle_dim_timer = Some(token),
            Err(error) => tracing::warn!(%error, "could not schedule idle dimming"),
        }
    }

    fn refresh_idle_dimming(&mut self) {
        self.idle_dimming
            .tick(&self.config.power, !self.idle_inhibitors.is_empty());
    }

    /// Runs once per event loop iteration after all sources were dispatched.
    pub fn refresh(&mut self) {
        profiling::scope!("state_refresh");
        self.space.refresh();
        if self.config.accessibility.zoom && self.config.accessibility.zoom_factor > 1.0 {
            self.magnifiers
                .retain(|output, _| self.space.outputs().any(|mapped| mapped == output));
        } else {
            self.magnifiers.clear();
        }
        self.popup_manager.cleanup();
        if self
            .active_popup_grab
            .as_ref()
            .is_some_and(PopupGrab::has_ended)
        {
            self.active_popup_grab = None;
        }
        self.layer_surfaces.retain(|layer| layer.alive());
        self.idle_inhibitors.retain(|surface| surface.alive());
        self.prune_finished_animations();
        if std::mem::take(&mut self.pointer_focus_dirty) {
            self.refresh_pointer_focus();
        }
        if self.animations_active() {
            self.request_redraw();
        }
        let inhibited = !self.idle_inhibitors.is_empty();
        self.idle_notifier_state.set_is_inhibited(inhibited);
        self.refresh_idle_dimming();
    }

    fn refresh_pointer_focus(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if pointer.is_grabbed() {
            return;
        }
        let location = pointer.current_location();
        let under = surface_under(self, location);
        if under.is_none() && pointer.current_focus().is_none() {
            return;
        }
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: self.clock_now().as_millis() as u32,
            },
        );
        pointer.frame(self);
    }

    fn refresh_child_env(&mut self) {
        let mut env = self
            .child_env
            .iter()
            .filter(|(key, _)| key == "WAYLAND_DISPLAY")
            .cloned()
            .collect::<Vec<_>>();
        env.push(("XCURSOR_THEME".into(), self.cursor.theme_name().to_owned()));
        env.push(("XCURSOR_SIZE".into(), self.cursor.size().to_string()));
        self.child_env = env;
    }

    pub fn set_wayland_display(&mut self, socket: &str) {
        self.child_env.retain(|(key, _)| key != "WAYLAND_DISPLAY");
        self.child_env
            .push(("WAYLAND_DISPLAY".into(), socket.to_owned()));
    }

    pub(crate) fn replace_static_bindings(&mut self, bindings: &[BindingConfig]) {
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

    pub fn activate_shortcuts(&mut self, shortcuts: Vec<ActivatedShortcut>) {
        for shortcut in shortcuts {
            if shortcut.client != CONFIG_BINDING_OWNER {
                self.emit(CompositorEvent::ShortcutActivated {
                    client: shortcut.client,
                    id: shortcut.id,
                });
                continue;
            }
            let Some(action) = self.static_bindings.get(&shortcut.id).cloned() else {
                continue;
            };
            match action {
                StaticBindingAction::Close => {
                    if let Some(id) = self.focused_window {
                        self.close_window_by_id(id);
                    }
                }
                StaticBindingAction::Exec(command) => self.spawn_command(&command),
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
        }
    }

    /// Applies the parts of a successfully parsed configuration that can
    /// safely change while the compositor is running.
    pub fn apply_config(&mut self, next: CompositorConfig) -> Result<(), String> {
        self.apply_config_with(next, |_| Ok(()))
    }

    pub fn apply_config_with(
        &mut self,
        mut next: CompositorConfig,
        mut persist: impl FnMut(&CompositorConfig) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.config.general.backend != next.general.backend {
            tracing::warn!(
                old = %self.config.general.backend,
                new = %next.general.backend,
                "backend changes require a compositor restart"
            );
            next.general
                .backend
                .clone_from(&self.config.general.backend);
        }
        if self.config.integrations.dbus != next.integrations.dbus {
            tracing::warn!("integration transport changes require a compositor restart");
            next.integrations.dbus = self.config.integrations.dbus;
        }
        if self.config.outputs != next.outputs {
            tracing::warn!("output changes require a compositor restart");
            next.outputs = self.config.outputs.clone();
        }
        if self.config.autostart != next.autostart {
            tracing::info!("autostart changes will be used on the next compositor start");
        }

        let keyboard_changed = self.config.input.keyboard != next.input.keyboard;
        let keymap_changed = self.config.input.keyboard.layout != next.input.keyboard.layout
            || self.config.input.keyboard.variant != next.input.keyboard.variant;
        let keyboard = keyboard_changed
            .then(|| {
                self.seat
                    .get_keyboard()
                    .ok_or_else(|| "no keyboard is available to apply the configuration".to_owned())
            })
            .transpose()?;
        let keymap = keymap_changed
            .then(|| {
                next.input
                    .keyboard
                    .compile_keymap()
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        let devices_changed = self.config.input.mouse != next.input.mouse
            || self.config.input.touchpad != next.input.touchpad;
        let rollback = devices_changed
            .then(|| {
                self.input_devices
                    .configure(&next.input, &self.config.input)
            })
            .transpose()?;
        let mut persisted = false;
        let result = (|| {
            persist(&next).map_err(|error| format!("could not persist configuration: {error}"))?;
            persisted = true;
            if let (Some(keyboard), Some(keymap)) = (keyboard.as_ref(), keymap) {
                keyboard
                    .set_keymap_from_string(self, keymap)
                    .map_err(|error| {
                        format!(
                            "could not apply keyboard layout '{}', variant '{}': {error}",
                            next.input.keyboard.layout, next.input.keyboard.variant,
                        )
                    })?;
            }
            Ok::<_, String>(())
        })();
        if let Err(error) = result {
            let mut errors = vec![error];
            if let Some(rollback) = rollback {
                if let Err(error) = self.input_devices.rollback(rollback) {
                    errors.push(error);
                }
            }
            if persisted {
                if let Err(error) = persist(&self.config) {
                    errors.push(format!(
                        "could not restore persisted configuration: {error}"
                    ));
                }
            }
            return Err(errors.join("; "));
        }
        let previous = std::mem::replace(&mut self.config, next);
        if previous.power != self.config.power {
            if let Some(token) = self.idle_dim_timer.take() {
                self.loop_handle.remove(token);
            }
            self.idle_dimming.activity();
            self.schedule_idle_dim();
        }
        if (previous.accessibility.sticky_keys && !self.config.accessibility.sticky_keys)
            || keymap_changed
        {
            crate::input::release_sticky_keys(self);
        }
        if previous.hot_corners != self.config.hot_corners {
            self.hot_corners.cancel();
        }
        if previous.workspaces != self.config.workspaces {
            self.apply_workspace_config();
        }
        if previous.bindings != self.config.bindings {
            self.replace_static_bindings(&self.config.bindings.clone());
        }
        if previous.rules != self.config.rules {
            self.rules.set_configured(&self.config.rules);
        }
        if previous.appearance.icon_theme != self.config.appearance.icon_theme {
            self.icon_cache
                .reconfigure(self.config.appearance.icon_theme.as_deref());
            self.decorations.clear();
            tracing::info!(theme = ?self.config.appearance.icon_theme, "reloaded window icon theme");
        }
        if let Some(keyboard) = keyboard {
            let config = &self.config.input.keyboard;
            keyboard.change_repeat_info(config.repeat_rate, config.repeat_delay);
            tracing::info!(layout = %config.layout, variant = %config.variant, "keyboard configuration applied");
        }
        if previous.input != self.config.input {
            self.input_config_changed = true;
        }
        if previous.cursor != self.config.cursor
            && self
                .cursor
                .reconfigure(self.config.cursor.theme.as_deref(), self.config.cursor.size)
        {
            self.refresh_child_env();
        }
        if previous.decorations != self.config.decorations {
            self.decoration_theme = self.config.decorations.to_theme(self.system_accent);
        }
        if previous.decorations.mode != self.config.decorations.mode
            || previous.window.server_side_decorations != self.config.window.server_side_decorations
        {
            self.refresh_decoration_modes();
        }
        if previous.window != self.config.window || previous.decorations != self.config.decorations
        {
            self.relayout_all();
        }
        self.request_redraw();
        self.emit(CompositorEvent::ConfigurationChanged);
        Ok(())
    }

    fn apply_workspace_config(&mut self) {
        let config = &self.config.workspaces;
        let count = config.count;
        let dynamic = config.dynamic;
        for id in 1..=count {
            let definition = config.definitions.get(&id.to_string());
            let name = definition
                .and_then(|definition| definition.name.clone())
                .unwrap_or_else(|| id.to_string());
            let output = definition.and_then(|definition| definition.output.clone());
            if let Some(workspace) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == id)
            {
                workspace.name = name;
                workspace.output = output;
            } else {
                self.workspaces.push(Workspace::new(id, name, output));
            }
        }
        if !dynamic {
            self.workspaces.retain(|workspace| workspace.id <= count);
            for managed in self
                .windows
                .values_mut()
                .filter(|managed| managed.workspace > count)
            {
                managed.workspace = count;
            }
            for workspace in self.output_workspaces.values_mut() {
                if *workspace > count {
                    *workspace = count;
                }
            }
        }
        self.workspaces.sort_by_key(|workspace| workspace.id);
        self.next_workspace_id = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .max()
            .unwrap_or(0)
            + 1;
        for workspace in &mut self.workspaces {
            workspace.focus_history.retain(|id| {
                self.windows
                    .get(id)
                    .is_some_and(|managed| managed.workspace == workspace.id)
            });
            if workspace
                .focused_window
                .is_some_and(|id| !workspace.focus_history.contains(&id))
            {
                workspace.focused_window = None;
            }
        }
        let assignments: Vec<_> = self
            .workspaces
            .iter()
            .filter_map(|workspace| {
                workspace
                    .output
                    .as_ref()
                    .filter(|name| self.output_by_name(name).is_some())
                    .map(|name| (name.clone(), workspace.id))
            })
            .collect();
        for (output, workspace) in assignments {
            self.output_workspaces.insert(output, workspace);
        }
        let mut assignments: Vec<_> = self.output_workspaces.keys().cloned().collect();
        assignments.sort();
        let mut assigned = HashSet::new();
        for output in assignments {
            let current = self.output_workspaces[&output];
            if assigned.insert(current) {
                continue;
            }
            let replacement = self
                .workspaces
                .iter()
                .find(|workspace| !assigned.contains(&workspace.id))
                .map(|workspace| workspace.id);
            if let Some(replacement) = replacement {
                self.output_workspaces.insert(output, replacement);
                assigned.insert(replacement);
            }
        }
        for (output, id) in &self.output_workspaces {
            self.emit(CompositorEvent::WorkspaceActivated {
                output: output.clone(),
                id: *id,
            });
        }
        self.relayout_all();
        let focused_is_visible = self
            .focused_window
            .and_then(|id| self.windows.get(&id))
            .is_some_and(|managed| {
                managed.mapped
                    && managed.minimized.is_none()
                    && managed.workspace == self.focused_workspace_id()
            });
        if !focused_is_visible {
            let target = self.topmost_window_on(self.focused_workspace_id());
            match target {
                Some(id) => {
                    self.focus_window_by_id(id);
                }
                None => self.set_keyboard_focus(None),
            }
        }
        self.mark_pointer_focus_dirty();
        self.emit(CompositorEvent::WorkspacesChanged);
        tracing::info!(count, dynamic, "workspace configuration applied");
    }

    pub fn spawn_autostarts(&mut self) {
        for entry in self.config.autostart.clone() {
            tracing::info!(command = %entry.command, restart = entry.restart, "starting autostart command");
            match self.spawn_child(&entry.command) {
                Ok(child) => self.autostart_processes.push(AutostartProcess {
                    command: entry.command,
                    restart: entry.restart,
                    child,
                    next_restart: Instant::now(),
                }),
                Err(error) => {
                    tracing::warn!(%error, command = %entry.command, "failed to start autostart command")
                }
            }
        }
    }

    fn spawn_child(&self, command: &str) -> std::io::Result<Child> {
        Command::new("sh")
            .arg("-c")
            .arg(command)
            .envs(self.child_env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
    }

    pub fn spawn_command(&mut self, command: &str) {
        match self.spawn_child(command) {
            Ok(child) => {
                tracing::info!(command, pid = child.id(), "spawned command");
                self.spawned_children.push(child);
            }
            Err(error) => tracing::warn!(%error, command, "failed to spawn command"),
        }
    }

    pub fn spawn_executable(&mut self, executable: &str) -> std::io::Result<()> {
        let child = Command::new(executable)
            .envs(self.child_env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()?;
        self.spawned_children.push(child);
        Ok(())
    }

    /// Reaps exited children and restarts opted-in autostarts after a short
    /// delay, avoiding a tight respawn loop for a broken command.
    pub fn supervise_children(&mut self) {
        self.spawned_children
            .retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        let now = Instant::now();
        let mut restarts = Vec::new();
        self.autostart_processes.retain_mut(|process| match process.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) if !process.restart => {
                tracing::info!(command = %process.command, %status, "autostart command exited");
                false
            }
            Ok(Some(_)) if now < process.next_restart => true,
            Ok(Some(status)) => {
                tracing::warn!(command = %process.command, %status, "restarting autostart command");
                restarts.push(process.command.clone());
                false
            }
            Err(error) => {
                tracing::warn!(%error, command = %process.command, "could not inspect autostart command");
                false
            }
        });
        for command in restarts {
            match self.spawn_child(&command) {
                Ok(child) => self.autostart_processes.push(AutostartProcess {
                    command,
                    restart: true,
                    child,
                    next_restart: now + Duration::from_secs(1),
                }),
                Err(error) => tracing::warn!(%error, command, "autostart restart failed"),
            }
        }
    }

    pub fn add_output(&mut self, output: &Output, location: Point<i32, Logical>) {
        self.space.map_output(output, location);
        let name = output.name();
        let workspace = self.workspace_for_new_output(&name);
        self.output_workspaces.insert(name.clone(), workspace);
        self.focused_output.get_or_insert(name.clone());
        self.emit(CompositorEvent::OutputAdded { name: name.clone() });
        self.emit(CompositorEvent::WorkspaceActivated {
            output: name,
            id: workspace,
        });
    }

    pub fn output_changed(&mut self, output: &Output) {
        if layer_map_for_output(output).arrange() {
            self.emit_work_area_changed(output);
        }
        self.relayout_all();
        crate::input::clamp_pointer_after_output_change(self);
        self.request_redraw();
        self.mark_pointer_focus_dirty();
    }

    fn relayout_all(&mut self) {
        let workspaces: Vec<_> = self
            .workspaces
            .iter()
            .map(|workspace| workspace.id)
            .collect();
        for workspace in workspaces {
            self.retile(workspace);
        }
        let ids: Vec<_> = self
            .windows
            .iter()
            .filter(|(_, managed)| managed.mapped && managed.minimized.is_none())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let Some(managed) = self.windows.get(&id) else {
                continue;
            };
            let window = managed.window.clone();
            if managed.fullscreen {
                self.apply_fullscreen_geometry(&window);
            } else if window_is_maximized(&window) {
                self.apply_maximized_geometry(&window);
            } else if self.window_floats(&window) {
                // Keep the settings confirmation window reachable after shrinking a display.
                let Some(output) = self.output_for_window(&window) else {
                    continue;
                };
                let Some(geometry) = self.space.element_geometry(&window) else {
                    continue;
                };
                let area = self.work_area_rect(&output);
                let (horizontal, vertical, left, top) = self.frame_insets(&window);
                let size = Size::from((
                    geometry.size.w.min((area.size.w - horizontal).max(1)),
                    geometry.size.h.min((area.size.h - vertical).max(1)),
                ));
                let location = Point::from((
                    geometry.loc.x.clamp(
                        area.loc.x + left,
                        (area.loc.x + area.size.w - size.w - horizontal + left)
                            .max(area.loc.x + left),
                    ),
                    geometry.loc.y.clamp(
                        area.loc.y + top,
                        (area.loc.y + area.size.h - size.h - vertical + top).max(area.loc.y + top),
                    ),
                ));
                if size != geometry.size || location != geometry.loc {
                    self.configure_and_place(&window, size, location);
                }
            }
        }
    }

    pub fn arrange_layers_for(&mut self, surface: &WlSurface) {
        let Some(output) = self.output_for_layer(surface) else {
            return;
        };
        if layer_map_for_output(&output).arrange() {
            self.emit_work_area_changed(&output);
            self.relayout_all();
        }
    }

    pub fn output_for_layer(&self, surface: &WlSurface) -> Option<Output> {
        self.space
            .outputs()
            .find(|output| {
                layer_map_for_output(output)
                    .layer_for_surface(surface, WindowSurfaceType::TOPLEVEL)
                    .is_some()
            })
            .cloned()
    }

    pub fn emit_work_area_changed(&self, output: &Output) {
        let zone = layer_map_for_output(output).non_exclusive_zone();
        self.emit(CompositorEvent::WorkAreaChanged {
            output: output.name(),
            area: to_rect(zone),
        });
    }

    fn output_by_name(&self, name: &str) -> Option<&Output> {
        self.space.outputs().find(|output| output.name() == name)
    }

    pub fn focused_output(&self) -> Option<Output> {
        self.focused_output
            .as_deref()
            .and_then(|name| self.output_by_name(name))
            .or_else(|| self.space.outputs().next())
            .cloned()
    }

    fn work_area_rect(&self, output: &Output) -> Rectangle<i32, Logical> {
        let Some(geometry) = self.space.output_geometry(output) else {
            return Rectangle::default();
        };
        let mut zone = layer_map_for_output(output).non_exclusive_zone();
        zone.loc += geometry.loc;
        zone
    }

    pub fn work_area(&self, output: &str) -> Rect {
        let output = if output.is_empty() {
            self.focused_output()
        } else {
            self.output_by_name(output).cloned()
        };
        output
            .map(|output| to_rect(self.work_area_rect(&output)))
            .unwrap_or_default()
    }

    pub fn request_screenshot(
        &mut self,
        output: &str,
        path: &str,
        reply: Box<dyn FnOnce(bool) + Send>,
    ) {
        let output = if output.is_empty() {
            self.focused_output()
        } else {
            self.output_by_name(output).cloned()
        };
        if self.screenshots.request(output, path, reply) {
            self.request_redraw();
        }
    }

    pub fn outputs(&self) -> Vec<String> {
        self.space.outputs().map(Output::name).collect()
    }

    fn workspace(&self, id: u64) -> Option<&Workspace> {
        self.workspaces.iter().find(|workspace| workspace.id == id)
    }

    fn workspace_mut(&mut self, id: u64) -> Option<&mut Workspace> {
        self.workspaces
            .iter_mut()
            .find(|workspace| workspace.id == id)
    }

    pub(crate) fn focused_workspace_id(&self) -> u64 {
        self.focused_output
            .as_ref()
            .and_then(|output| self.output_workspaces.get(output).copied())
            .unwrap_or(1)
    }

    fn workspace_for_new_output(&self, output: &str) -> u64 {
        let unassigned = |workspace: &&Workspace| {
            !self
                .output_workspaces
                .values()
                .any(|id| *id == workspace.id)
        };
        self.workspaces
            .iter()
            .filter(unassigned)
            .find(|workspace| workspace.output.as_deref() == Some(output))
            .or_else(|| self.workspaces.iter().find(unassigned))
            .map(|workspace| workspace.id)
            .unwrap_or(1)
    }

    pub fn set_focused_output_at(&mut self, pos: Point<f64, Logical>) {
        let output = self.space.output_under(pos).next().map(Output::name);
        if output.is_some() && output != self.focused_output {
            self.focused_output = output;
        }
    }

    pub fn output_at(&self, pos: Point<f64, Logical>) -> Option<Output> {
        self.space.output_under(pos).next().cloned()
    }

    pub fn workspace_for_output(&self, output: &Output) -> u64 {
        self.output_workspaces
            .get(&output.name())
            .copied()
            .unwrap_or(1)
    }

    pub fn window_visible_on_output(&self, window: &Window, output: &Output) -> bool {
        self.window_id(window)
            .and_then(|id| self.windows.get(&id))
            .is_some_and(|managed| {
                managed.mapped
                    && managed.minimized.is_none()
                    && managed.workspace == self.workspace_for_output(output)
            })
    }

    pub fn window_opacity(&self, window: &Window, output: &Output) -> f32 {
        let Some(managed) = self.window_id(window).and_then(|id| self.windows.get(&id)) else {
            return 1.0;
        };
        let rule_opacity = managed.rules.opacity.unwrap_or(1.0);
        if !self.config.animations.enabled {
            return rule_opacity;
        }
        let opened = managed
            .opened_at
            .map(|started| animation_progress(started, &self.config.animations.window_open))
            .unwrap_or(1.0);
        let workspace = self
            .workspace_animation_started
            .get(&output.name())
            .map(|started| animation_progress(*started, &self.config.animations.workspace))
            .unwrap_or(1.0);
        rule_opacity * opened * workspace
    }

    pub fn window_always_on_top(&self, window: &Window) -> bool {
        self.managed(window)
            .and_then(|managed| managed.rules.always_on_top)
            .unwrap_or(false)
    }

    pub fn window_is_fullscreen(&self, window: &Window) -> bool {
        self.managed(window)
            .is_some_and(|managed| managed.fullscreen)
    }

    fn managed(&self, window: &Window) -> Option<&ManagedWindow> {
        self.window_id(window).and_then(|id| self.windows.get(&id))
    }

    pub fn animations_active(&self) -> bool {
        self.config.animations.enabled
            && (self
                .windows
                .values()
                .any(|managed| managed.opened_at.is_some())
                || !self.workspace_animation_started.is_empty())
    }

    fn prune_finished_animations(&mut self) {
        let window_open = self.config.animations.window_open.duration;
        let workspace = self.config.animations.workspace.duration;
        let enabled = self.config.animations.enabled;
        for managed in self.windows.values_mut() {
            if managed
                .opened_at
                .is_some_and(|started| !enabled || animation_done(started, window_open))
            {
                managed.opened_at = None;
            }
        }
        self.workspace_animation_started
            .retain(|_, started| enabled && !animation_done(*started, workspace));
    }

    pub fn register_temporary_rule(&mut self, client: &str, id: &str, rule_toml: &str) -> bool {
        let rule = match toml::from_str::<WindowRuleConfig>(rule_toml) {
            Ok(rule) => rule,
            Err(error) => {
                tracing::warn!(client, id, %error, "temporary window rule has invalid TOML");
                return false;
            }
        };
        if id.trim().is_empty() || rule.validate(0).is_err() {
            return false;
        }
        self.rules.register(client, id, rule)
    }

    fn workspace_id_by_selector(&self, selector: &str) -> Option<u64> {
        selector
            .parse::<u64>()
            .ok()
            .filter(|id| self.workspace(*id).is_some())
            .or_else(|| {
                self.workspaces
                    .iter()
                    .find(|workspace| workspace.name == selector)
                    .map(|workspace| workspace.id)
            })
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
        let name = if name.trim().is_empty() {
            id.to_string()
        } else {
            name
        };
        self.workspaces.push(Workspace::new(id, name, None));
        self.emit(CompositorEvent::WorkspacesChanged);
        id
    }

    pub(crate) fn ensure_workspace(&mut self, id: u64) {
        if id > self.config.workspaces.count && !self.config.workspaces.dynamic {
            return;
        }
        while self.next_workspace_id <= id {
            self.create_workspace_unchecked(self.next_workspace_id.to_string());
        }
    }

    pub fn list_workspaces(&self) -> Vec<WorkspaceInfo> {
        self.workspaces
            .iter()
            .map(|workspace| {
                let output = self
                    .output_workspaces
                    .iter()
                    .find_map(|(output, id)| (*id == workspace.id).then(|| output.clone()));
                WorkspaceInfo {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    active: output.is_some(),
                    output,
                    window_count: self
                        .windows
                        .values()
                        .filter(|managed| managed.mapped && managed.workspace == workspace.id)
                        .count(),
                }
            })
            .collect()
    }

    fn resolve_workspace_target(&self, id: u64) -> Option<u64> {
        if self.workspace(id).is_some() {
            return Some(id);
        }
        if !self.config.workspaces.wrap || self.config.workspaces.dynamic {
            return None;
        }
        let count = self.config.workspaces.count;
        Some(if id == 0 { count } else { (id - 1) % count + 1 })
    }

    pub fn switch_workspace(&mut self, id: u64) -> bool {
        let Some(id) = self.resolve_workspace_target(id) else {
            return false;
        };
        let Some(output) = self.focused_output().map(|output| output.name()) else {
            return false;
        };
        let current = self.output_workspaces.get(&output).copied().unwrap_or(1);
        if id == current {
            return true;
        }
        let now = Instant::now();
        if let Some(other) = self
            .output_workspaces
            .iter()
            .find(|(name, workspace)| **workspace == id && **name != output)
            .map(|(name, _)| name.clone())
        {
            self.workspace_animation_started.insert(other.clone(), now);
            self.output_workspaces.insert(other.clone(), current);
            self.emit(CompositorEvent::WorkspaceActivated {
                output: other,
                id: current,
            });
        }
        self.workspace_animation_started.insert(output.clone(), now);
        self.output_workspaces.insert(output.clone(), id);
        tracing::debug!(output = %output, workspace = id, "workspace switched");
        self.emit(CompositorEvent::WorkspaceActivated { output, id });

        let target = self
            .workspace(id)
            .and_then(|workspace| workspace.focused_window)
            .or_else(|| self.topmost_window_on(id));
        match target {
            Some(window) if self.focus_window_by_id(window) => {}
            _ => self.set_keyboard_focus(None),
        }
        self.request_redraw();
        self.mark_pointer_focus_dirty();
        true
    }

    /// Topmost mapped window of a workspace, in stacking order.
    fn topmost_window_on(&self, workspace: u64) -> Option<WindowId> {
        self.space
            .elements()
            .rev()
            .filter_map(|window| self.window_id(window))
            .find(|id| {
                self.windows.get(id).is_some_and(|managed| {
                    managed.mapped && managed.minimized.is_none() && managed.workspace == workspace
                })
            })
    }

    pub fn move_window_to_workspace(&mut self, id: WindowId, workspace_id: u64) -> bool {
        if self.workspace(workspace_id).is_none() {
            return false;
        }
        let Some(managed) = self.windows.get_mut(&id) else {
            return false;
        };
        if managed.workspace == workspace_id {
            return true;
        }
        let previous = managed.workspace;
        managed.workspace = workspace_id;
        self.retile(previous);
        self.retile(workspace_id);
        let Some(_) = self.windows.get(&id) else {
            return false;
        };
        for workspace in &mut self.workspaces {
            if workspace.id == previous {
                workspace.focus_history.retain(|entry| *entry != id);
                if workspace.focused_window == Some(id) {
                    workspace.focused_window = workspace.focus_history.last().copied();
                }
            }
        }
        if self.focused_window == Some(id) {
            self.focus_previous_window();
        }
        self.request_redraw();
        self.mark_pointer_focus_dirty();
        self.emit(CompositorEvent::WorkspacesChanged);
        true
    }

    pub fn window_id(&self, window: &Window) -> Option<WindowId> {
        window
            .wl_surface()
            .and_then(|surface| surface_window_id(&surface))
    }

    fn window_by_id(&self, id: WindowId) -> Option<Window> {
        self.windows
            .get(&id)
            .filter(|managed| managed.mapped)
            .map(|managed| managed.window.clone())
    }

    fn window_info(&self, id: WindowId, managed: &ManagedWindow) -> WindowInfo {
        let geometry = match managed.minimized {
            Some(location) => Rectangle::new(location, managed.window.geometry().size),
            None => self
                .space
                .element_geometry(&managed.window)
                .unwrap_or_default(),
        };
        WindowInfo {
            id,
            title: managed.title.clone(),
            app_id: managed.app_id.clone(),
            geometry: to_rect(geometry),
            focused: self.focused_window == Some(id),
            minimized: managed.minimized.is_some(),
            maximized: window_is_maximized(&managed.window),
        }
    }

    pub fn list_windows(&self) -> Vec<WindowInfo> {
        let workspace = self.focused_workspace_id();
        self.windows
            .iter()
            .filter(|(_, managed)| managed.mapped && managed.workspace == workspace)
            .map(|(id, managed)| self.window_info(*id, managed))
            .collect()
    }

    fn exclusive_layer_surface(&self) -> Option<WlSurface> {
        self.layer_surfaces
            .iter()
            .filter(|layer| matches!(layer.layer(), Layer::Top | Layer::Overlay))
            .find(|layer| {
                layer.cached_state().keyboard_interactivity == KeyboardInteractivity::Exclusive
                    && with_states(layer.wl_surface(), |states| {
                        states
                            .data_map
                            .get::<LayerSurfaceData>()
                            .is_some_and(|data| data.lock().unwrap().initial_configure_sent)
                    })
            })
            .map(|layer| layer.wl_surface().clone())
    }

    pub fn set_keyboard_focus(&mut self, surface: Option<WlSurface>) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let target = if self.session_locked {
            self.session_lock_focus(surface)
        } else {
            self.exclusive_layer_surface().or(surface)
        };
        if keyboard.current_focus() == target {
            return;
        }
        keyboard.set_focus(self, target, SERIAL_COUNTER.next_serial());
    }

    pub fn focus_window(&mut self, window: &Window) -> bool {
        let Some(surface) = window.wl_surface().map(|surface| surface.into_owned()) else {
            return false;
        };
        if self.config.focus.raise_on_focus || self.window_is_fullscreen(window) {
            self.space.raise_element(window, false);
        }
        self.set_keyboard_focus(Some(surface));
        self.request_redraw();
        self.mark_pointer_focus_dirty();
        true
    }

    fn focus_previous_window(&mut self) {
        let workspace_id = self.focused_workspace_id();
        let candidate = self.workspace(workspace_id).and_then(|workspace| {
            workspace.focus_history.iter().rev().copied().find(|id| {
                Some(*id) != self.focused_window
                    && self.windows.get(id).is_some_and(|managed| {
                        managed.mapped
                            && managed.minimized.is_none()
                            && managed.workspace == workspace_id
                    })
            })
        });
        match candidate.and_then(|id| self.window_by_id(id)) {
            Some(window) => {
                self.focus_window(&window);
            }
            None => self.set_keyboard_focus(None),
        }
    }

    fn warp_pointer_to(&mut self, window: &Window) {
        if !self.config.focus.warp_cursor {
            return;
        }
        let Some(geometry) = self.space.element_geometry(window) else {
            return;
        };
        let Some(pointer) = self.seat.get_pointer() else {
            return;
        };
        if geometry.to_f64().contains(pointer.current_location()) {
            return;
        }
        let center = geometry.loc.to_f64() + geometry.size.to_f64().downscale(2.0).to_point();
        let under = surface_under(self, center);
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: center,
                serial: SERIAL_COUNTER.next_serial(),
                time: self.clock_now().as_millis() as u32,
            },
        );
        pointer.frame(self);
        self.request_redraw();
    }

    pub fn focus_window_by_id(&mut self, id: WindowId) -> bool {
        let Some(managed) = self.windows.get(&id).filter(|managed| managed.mapped) else {
            return false;
        };
        let window = managed.window.clone();
        let workspace = managed.workspace;
        if workspace != self.focused_workspace_id() {
            if let Some(workspace) = self.workspace_mut(workspace) {
                workspace.focused_window = Some(id);
            }
            return self.switch_workspace(workspace);
        }
        if let Some(location) = self
            .windows
            .get_mut(&id)
            .and_then(|managed| managed.minimized.take())
        {
            self.space.map_element(window.clone(), location, false);
            self.retile(workspace);
            self.emit(CompositorEvent::WindowRestored { id });
        }
        let focused = self.focus_window(&window);
        self.warp_pointer_to(&window);
        focused
    }

    /// Decoration insets as (horizontal, vertical, left, top).
    fn frame_insets(&self, window: &Window) -> (i32, i32, i32, i32) {
        if !self.window_has_server_decoration(window) || self.window_has_overlay_controls(window) {
            return (0, 0, 0, 0);
        }
        let theme = &self.decoration_theme;
        let titlebar = if self.config.window.layout == WindowLayout::Tiling {
            0
        } else {
            theme.titlebar_height
        };
        let border = theme.border_width;
        (border * 2, border * 2 + titlebar, border, border + titlebar)
    }

    fn window_floats(&self, window: &Window) -> bool {
        let floating = self
            .managed(window)
            .and_then(|managed| managed.rules.floating);
        floating == Some(true)
            || (self.config.window.layout != WindowLayout::Tiling && floating != Some(false))
    }

    /// Tiled windows of a workspace, in creation order: the first one is the
    /// master.
    fn tiled_windows(&self, workspace: u64) -> Vec<Window> {
        self.windows
            .values()
            .filter(|managed| {
                managed.workspace == workspace
                    && managed.mapped
                    && managed.minimized.is_none()
                    && !managed.fullscreen
                    && !self.window_floats(&managed.window)
            })
            .map(|managed| managed.window.clone())
            .collect()
    }

    /// Lays out a workspace as a master column with the remaining windows
    /// stacked beside it.
    pub fn retile(&mut self, workspace: u64) {
        let windows = self.tiled_windows(workspace);
        if windows.is_empty() {
            return;
        }
        let Some(output) = self.output_for_workspace(workspace) else {
            return;
        };
        let full = self.work_area_rect(&output);
        let padding = self
            .config
            .window
            .work_area_padding
            .clamp(0, full.size.w.min(full.size.h) / 2);
        let gap = self
            .config
            .window
            .gap
            .clamp(0, full.size.w.min(full.size.h) / 4);
        let area = Rectangle::new(
            full.loc + Point::from((padding, padding)),
            Size::from((
                (full.size.w - padding * 2).max(1),
                (full.size.h - padding * 2).max(1),
            )),
        );

        for (window, rect) in windows.iter().zip(master_stack(
            area,
            windows.len(),
            gap,
            self.config.window.master_ratio,
        )) {
            let (horizontal, vertical, left, top) = self.frame_insets(window);
            let size = Size::from((
                (rect.size.w - horizontal).max(1),
                (rect.size.h - vertical).max(1),
            ));
            self.configure_and_place(window, size, rect.loc + Point::from((left, top)));
        }
    }

    fn output_for_workspace(&self, workspace: u64) -> Option<Output> {
        self.output_workspaces
            .iter()
            .find(|(_, id)| **id == workspace)
            .and_then(|(name, _)| self.output_by_name(name))
            .cloned()
            .or_else(|| self.focused_output())
    }

    fn retile_window(&mut self, window: &Window) {
        if let Some(workspace) = self.managed(window).map(|managed| managed.workspace) {
            self.retile(workspace);
        }
    }

    fn configure_and_place(
        &mut self,
        window: &Window,
        size: Size<i32, Logical>,
        location: Point<i32, Logical>,
    ) {
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| state.size = Some(size));
            toplevel.send_pending_configure();
        }
        self.move_window(window, location);
    }

    pub fn move_window(&mut self, window: &Window, location: Point<i32, Logical>) {
        if self.space.element_geometry(window).map(|geo| geo.loc) == Some(location) {
            return;
        }
        self.space.map_element(window.clone(), location, false);
        self.window_geometry_changed(window);
        self.request_redraw();
        self.mark_pointer_focus_dirty();
    }

    fn output_for_window(&self, window: &Window) -> Option<Output> {
        let workspace = self.managed(window)?.workspace;
        self.output_workspaces
            .iter()
            .find(|(_, id)| **id == workspace)
            .and_then(|(name, _)| self.output_by_name(name))
            .cloned()
            .or_else(|| self.focused_output())
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
            .is_some_and(|window| self.close_window(&window))
    }

    pub fn minimize_window(&mut self, window: &Window) -> bool {
        let Some(id) = self.window_id(window) else {
            return false;
        };
        let Some(location) = self.space.element_geometry(window).map(|geo| geo.loc) else {
            return self
                .windows
                .get(&id)
                .is_some_and(|managed| managed.minimized.is_some());
        };
        let Some(managed) = self.windows.get_mut(&id) else {
            return false;
        };
        let workspace = managed.workspace;
        managed.minimized = Some(location);
        self.space.unmap_elem(window);
        self.retile(workspace);
        if self.focused_window == Some(id) {
            self.focus_previous_window();
        }
        self.request_redraw();
        self.mark_pointer_focus_dirty();
        self.emit(CompositorEvent::WindowMinimized { id });
        true
    }

    pub fn minimize_window_by_id(&mut self, id: WindowId) -> bool {
        self.window_by_id(id)
            .is_some_and(|window| self.minimize_window(&window))
    }

    pub fn toggle_maximize_window(&mut self, window: &Window) -> bool {
        let maximized = !window_is_maximized(window);
        self.set_maximized(window, maximized)
    }

    pub fn set_maximized(&mut self, window: &Window, maximized: bool) -> bool {
        let (Some(toplevel), Some(id)) = (window.toplevel(), self.window_id(window)) else {
            return false;
        };
        if self.window_is_fullscreen(window) {
            return false;
        }
        if maximized {
            self.remember_restore_geometry(window);
            toplevel.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Maximized);
            });
            self.apply_maximized_geometry(window);
        } else {
            toplevel.with_pending_state(|state| {
                state.states.unset(xdg_toplevel::State::Maximized);
            });
            self.restore_geometry(window);
        }
        self.emit(CompositorEvent::WindowMaximized { id, maximized });
        true
    }

    fn remember_restore_geometry(&mut self, window: &Window) {
        let geometry = self.space.element_geometry(window);
        if let Some(managed) = self
            .window_id(window)
            .and_then(|id| self.windows.get_mut(&id))
        {
            if managed.restore.is_none() {
                managed.restore = geometry;
            }
        }
    }

    fn restore_geometry(&mut self, window: &Window) {
        let restore = self
            .window_id(window)
            .and_then(|id| self.windows.get_mut(&id))
            .and_then(|managed| managed.restore.take());
        match restore {
            Some(geometry) if self.window_floats(window) => {
                self.configure_and_place(window, geometry.size, geometry.loc);
            }
            _ => {
                if let Some(toplevel) = window.toplevel() {
                    toplevel.with_pending_state(|state| state.size = None);
                    toplevel.send_pending_configure();
                }
                self.retile_window(window);
            }
        }
    }

    fn apply_maximized_geometry(&mut self, window: &Window) {
        let Some(output) = self.output_for_window(window) else {
            return;
        };
        let area = self.work_area_rect(&output);
        let (horizontal, vertical, left, top) = self.frame_insets(window);
        let size = Size::from((
            (area.size.w - horizontal).max(1),
            (area.size.h - vertical).max(1),
        ));
        self.configure_and_place(window, size, area.loc + Point::from((left, top)));
    }

    pub fn set_fullscreen(&mut self, window: &Window, fullscreen: bool) {
        if self.config.general.kiosk && !fullscreen {
            return;
        }
        let (Some(toplevel), Some(id)) = (window.toplevel(), self.window_id(window)) else {
            return;
        };
        let current = self.window_is_fullscreen(window);
        if current == fullscreen {
            toplevel.send_configure();
            return;
        }
        if fullscreen {
            self.remember_restore_geometry(window);
        }
        if let Some(managed) = self.windows.get_mut(&id) {
            managed.fullscreen = fullscreen;
        }
        toplevel.with_pending_state(|state| {
            if fullscreen {
                state.states.set(xdg_toplevel::State::Fullscreen);
            } else {
                state.states.unset(xdg_toplevel::State::Fullscreen);
            }
        });
        if fullscreen {
            self.apply_fullscreen_geometry(window);
            self.space.raise_element(window, false);
        } else if window_is_maximized(window) {
            self.apply_maximized_geometry(window);
        } else {
            self.restore_geometry(window);
        }
        self.request_redraw();
    }

    fn apply_fullscreen_geometry(&mut self, window: &Window) {
        let Some(geometry) = self
            .output_for_window(window)
            .and_then(|output| self.space.output_geometry(&output))
        else {
            return;
        };
        if let Some(toplevel) = window.toplevel() {
            toplevel.with_pending_state(|state| {
                state.size = Some(geometry.size);
                state.fullscreen_output = None;
            });
            toplevel.send_pending_configure();
        }
        self.move_window(window, geometry.loc);
    }

    pub fn toggle_maximize_window_by_id(&mut self, id: WindowId) -> bool {
        self.window_by_id(id)
            .is_some_and(|window| self.toggle_maximize_window(&window))
    }

    pub fn move_resize_window(&mut self, id: WindowId, geometry: Rect) -> bool {
        if geometry.width <= 0 || geometry.height <= 0 {
            return false;
        }
        let Some(window) = self.window_by_id(id) else {
            return false;
        };
        let geometry = from_rect(geometry);
        self.configure_and_place(&window, geometry.size, geometry.loc);
        true
    }

    pub fn window_geometry_changed(&self, window: &Window) {
        let (Some(id), Some(geometry)) =
            (self.window_id(window), self.space.element_geometry(window))
        else {
            return;
        };
        self.emit(CompositorEvent::WindowGeometryChanged {
            id,
            geometry: to_rect(geometry),
        });
    }

    pub fn begin_move(&mut self, window: &Window, serial: Serial, button: u32) -> bool {
        if self.session_locked || !self.window_floats(window) || self.window_is_fullscreen(window) {
            return false;
        }
        let (Some(pointer), Some(mut geometry)) =
            (self.seat.get_pointer(), self.space.element_geometry(window))
        else {
            return false;
        };
        let location = pointer.current_location();
        if window_is_maximized(window) {
            let ratio = ((location.x - f64::from(geometry.loc.x))
                / f64::from(geometry.size.w.max(1)))
            .clamp(0.0, 1.0);
            self.set_maximized(window, false);
            let restored = self.space.element_geometry(window).unwrap_or(geometry);
            geometry.loc = Point::from((
                (location.x - f64::from(restored.size.w) * ratio).round() as i32,
                geometry.loc.y,
            ));
            self.move_window(window, geometry.loc);
        }
        self.focus_window(window);
        let start_data = GrabStartData {
            focus: None,
            button,
            location,
        };
        self.compositor_cursor = Some(smithay::input::pointer::CursorIcon::Grabbing);
        tracing::debug!("interactive move started");
        pointer.set_grab(
            self,
            MoveGrab {
                start_data,
                window: window.clone(),
                initial_location: geometry.loc,
            },
            serial,
            Focus::Clear,
        );
        true
    }

    pub fn begin_resize(
        &mut self,
        window: &Window,
        edges: ResizeEdges,
        serial: Serial,
        button: u32,
    ) -> bool {
        if self.session_locked
            || edges.is_empty()
            || !self.window_floats(window)
            || self.window_is_fullscreen(window)
        {
            return false;
        }
        let (Some(pointer), Some(geometry), Some(id)) = (
            self.seat.get_pointer(),
            self.space.element_geometry(window),
            self.window_id(window),
        ) else {
            return false;
        };
        let anchor = ResizeAnchor {
            edges,
            initial: geometry,
        };
        if let Some(managed) = self.windows.get_mut(&id) {
            managed.resize = Some((anchor, true));
        }
        self.focus_window(window);
        let start_data = GrabStartData {
            focus: None,
            button,
            location: pointer.current_location(),
        };
        self.compositor_cursor = Some(edges.cursor());
        tracing::debug!(?edges, "interactive resize started");
        pointer.set_grab(
            self,
            ResizeGrab {
                start_data,
                window: window.clone(),
                anchor,
                last_size: geometry.size,
            },
            serial,
            Focus::Clear,
        );
        true
    }

    pub fn finish_resize(&mut self, window: &Window) {
        if let Some(managed) = self
            .window_id(window)
            .and_then(|id| self.windows.get_mut(&id))
        {
            if let Some((_, active)) = managed.resize.as_mut() {
                *active = false;
            }
        }
        self.window_geometry_changed(window);
    }

    pub fn focus_interactive_layer_surface(&mut self, surface: &WlSurface) {
        if self.session_locked {
            return;
        }
        let Some(layer) = self
            .layer_surfaces
            .iter()
            .find(|layer| layer.wl_surface() == surface)
        else {
            return;
        };
        let interactivity = layer.cached_state().keyboard_interactivity;
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let wants_focus = match interactivity {
            KeyboardInteractivity::Exclusive => {
                matches!(layer.layer(), Layer::Top | Layer::Overlay)
            }
            KeyboardInteractivity::OnDemand => keyboard.current_focus().is_none(),
            KeyboardInteractivity::None => false,
        };
        if wants_focus && keyboard.current_focus().as_ref() != Some(surface) {
            self.set_keyboard_focus(Some(surface.clone()));
        }
    }

    pub fn layer_surface_removed(&mut self, surface: &WlSurface) {
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        if focused.as_ref() == Some(surface) || focused.is_none() {
            let target = self
                .focused_workspace_window()
                .and_then(|window| window.wl_surface().map(|surface| surface.into_owned()));
            self.set_keyboard_focus(target);
        }
        self.mark_pointer_focus_dirty();
        self.request_redraw();
    }

    fn focused_workspace_window(&self) -> Option<Window> {
        let workspace = self.workspace(self.focused_workspace_id())?;
        workspace
            .focused_window
            .and_then(|id| self.window_by_id(id))
            .filter(|window| {
                self.managed(window)
                    .is_some_and(|managed| managed.minimized.is_none())
            })
    }

    pub fn unconstrain_popup(&self, popup: &PopupSurface) {
        let kind = PopupKind::Xdg(popup.clone());
        let Ok(root) = find_popup_root_surface(&kind) else {
            return;
        };
        let (root_location, output) = if let Some(window) = self
            .space
            .elements()
            .find(|window| window.wl_surface().as_deref() == Some(&root))
        {
            let Some(geometry) = self.space.element_geometry(window) else {
                return;
            };
            let output = self
                .output_for_window(window)
                .or_else(|| self.output_at(geometry.loc.to_f64()));
            (geometry.loc, output)
        } else if let Some(output) = self.output_for_layer(&root) {
            let map = layer_map_for_output(&output);
            let Some(layer_geometry) = map
                .layer_for_surface(&root, WindowSurfaceType::TOPLEVEL)
                .and_then(|layer| map.layer_geometry(layer))
            else {
                return;
            };
            let output_origin = self
                .space
                .output_geometry(&output)
                .map(|geo| geo.loc)
                .unwrap_or_default();
            drop(map);
            (output_origin + layer_geometry.loc, Some(output))
        } else {
            return;
        };
        let Some(mut target) = output.and_then(|output| self.space.output_geometry(&output)) else {
            return;
        };
        target.loc -= root_location + get_popup_toplevel_coords(&kind);
        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }

    pub fn handle_commit(&mut self, surface: &WlSurface) {
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }

        let window_id = surface_window_id(&root);
        if let Some(id) = window_id {
            self.commit_window(id, &root, surface == &root);
        }

        if self.session_locked
            && self
                .session_lock_surfaces
                .iter()
                .any(|entry| entry.surface.wl_surface() == &root)
        {
            self.mark_pointer_focus_dirty();
        }
        self.popup_manager.commit(surface);
        if let Some(PopupKind::Xdg(popup)) = self.popup_manager.find_popup(surface) {
            if !popup.is_initial_configure_sent() {
                self.unconstrain_popup(&popup);
                if let Err(error) = popup.send_configure() {
                    tracing::warn!(?error, "failed to configure popup");
                }
            }
        }

        if window_id.is_none() && surface == &root && self.output_for_layer(surface).is_some() {
            self.commit_layer(surface);
        }
        self.request_redraw();
    }

    fn commit_layer(&mut self, surface: &WlSurface) {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<LayerSurfaceData>()
                .is_some_and(|data| data.lock().unwrap().initial_configure_sent)
        });
        self.arrange_layers_for(surface);
        if !initial_configure_sent {
            if let Some(layer) = self
                .layer_surfaces
                .iter()
                .find(|layer| layer.wl_surface() == surface)
            {
                layer.layer_surface().send_configure();
            }
            return;
        }
        self.focus_interactive_layer_surface(surface);
        self.mark_pointer_focus_dirty();
    }

    fn commit_window(&mut self, id: WindowId, root: &WlSurface, is_root: bool) {
        let Some(managed) = self.windows.get(&id) else {
            return;
        };
        let window = managed.window.clone();
        window.on_commit();
        if !is_root {
            return;
        }
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        if !toplevel.is_initial_configure_sent() {
            self.configure_new_window(id, &window);
            return;
        }
        let has_buffer =
            with_renderer_surface_state(root, |state| state.buffer().is_some()).unwrap_or(false);
        let mapped = managed.mapped;
        match (mapped, has_buffer) {
            (false, true) => self.map_window(id, &window),
            (true, false) => self.unmap_window(id, &window),
            (true, true) => self.apply_resize_anchor(id, &window),
            (false, false) => {}
        }
    }

    fn configure_new_window(&mut self, id: WindowId, window: &Window) {
        let (title, app_id) = window_meta(window);
        let rules = self.rules.resolve(&title, app_id.as_deref());
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        // A floating client chooses its initial size. Only an explicit window
        // rule may request a compositor-provided size.
        let size = rules.size.map(|[w, h]| Size::from((w, h)));
        if let Some(decoration) = rules.decoration {
            let mode = if decoration {
                DecorationMode::ServerSide
            } else {
                DecorationMode::ClientSide
            };
            set_surface_decoration_mode(toplevel.wl_surface(), mode);
            toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
        }
        if let Some(managed) = self.windows.get_mut(&id) {
            managed.rules = rules;
            managed.title = title;
            managed.app_id = app_id;
        }
        let tiled = !self.window_floats(window);
        let output = self.focused_output();
        if self.config.general.kiosk {
            let size = output
                .as_ref()
                .and_then(|output| self.space.output_geometry(output))
                .map(|geometry| geometry.size);
            toplevel.with_pending_state(|state| {
                state.size = size;
                state.states.set(xdg_toplevel::State::Fullscreen);
            });
        } else if let (true, Some(output)) = (tiled, output.as_ref()) {
            let area = self.work_area_rect(output);
            let padding = self.config.window.work_area_padding.max(0);
            let (horizontal, vertical, _, _) = self.frame_insets(window);
            toplevel.with_pending_state(|state| {
                state.size = Some(
                    (
                        (area.size.w - padding * 2 - horizontal).max(1),
                        (area.size.h - padding * 2 - vertical).max(1),
                    )
                        .into(),
                );
                state.states.set(xdg_toplevel::State::TiledLeft);
                state.states.set(xdg_toplevel::State::TiledRight);
                state.states.set(xdg_toplevel::State::TiledTop);
                state.states.set(xdg_toplevel::State::TiledBottom);
            });
        } else {
            toplevel.with_pending_state(|state| state.size = size);
        }
        if let Some(output) = output.as_ref() {
            send_scale_to_surface(toplevel.wl_surface(), output);
        }
        toplevel.send_configure();
    }

    fn map_window(&mut self, id: WindowId, window: &Window) {
        let Some(managed) = self.windows.get(&id) else {
            return;
        };
        let rules = managed.rules.clone();
        let workspace = rules
            .workspace
            .as_deref()
            .and_then(|selector| self.workspace_id_by_selector(selector))
            .unwrap_or_else(|| self.focused_workspace_id());
        let output = rules
            .output
            .as_deref()
            .and_then(|name| self.output_by_name(name).cloned())
            .or_else(|| {
                self.output_workspaces
                    .iter()
                    .find(|(_, id)| **id == workspace)
                    .and_then(|(name, _)| self.output_by_name(name).cloned())
            })
            .or_else(|| self.focused_output());
        let kiosk_geometry = if self.config.general.kiosk {
            output
                .as_ref()
                .and_then(|output| self.space.output_geometry(output))
        } else {
            None
        };
        let area = output
            .as_ref()
            .map(|output| self.work_area_rect(output))
            .unwrap_or_else(|| Rectangle::from_size((1280, 720).into()));
        let size = window.geometry().size;
        let (_, _, _, top) = self.frame_insets(window);
        let location = rules
            .position
            .map(|[x, y]| Point::from((x, y)))
            .unwrap_or_else(|| {
                if !self.config.window.center_new_windows {
                    return Point::from((
                        area.loc.x + self.config.window.work_area_padding,
                        area.loc.y + top + self.config.window.work_area_padding,
                    ));
                }
                Point::from((
                    area.loc.x + (area.size.w - size.w) / 2,
                    area.loc.y + ((area.size.h - size.h + top) / 2).max(top),
                ))
            });
        let location = kiosk_geometry.map_or(location, |geometry| geometry.loc);

        let (title, app_id) = window_meta(window);
        let foreign = self
            .foreign_toplevel_state
            .new_toplevel::<Self>(&title, app_id.as_deref().unwrap_or(""));
        if let Some(managed) = self.windows.get_mut(&id) {
            managed.mapped = true;
            managed.fullscreen = self.config.general.kiosk;
            managed.workspace = workspace;
            managed.opened_at = self.config.animations.enabled.then(Instant::now);
            managed.foreign = Some(foreign);
            managed.title.clone_from(&title);
            managed.app_id.clone_from(&app_id);
        }
        self.space.map_element(window.clone(), location, false);
        tracing::debug!(%id, title, app_id = ?app_id, workspace, "window mapped");
        self.emit(CompositorEvent::WindowOpened { id, title, app_id });
        self.emit(CompositorEvent::WorkspacesChanged);
        self.retile(workspace);

        let visible = workspace == self.focused_workspace_id();
        if visible && (self.config.general.kiosk || self.config.focus.focus_new_windows) {
            self.focus_window(window);
        } else if let Some(workspace) = self.workspace_mut(workspace) {
            workspace.focus_history.insert(0, id);
            workspace.focused_window.get_or_insert(id);
        }
        self.mark_pointer_focus_dirty();
        self.request_redraw();
    }

    fn unmap_window(&mut self, id: WindowId, window: &Window) {
        self.space.unmap_elem(window);
        self.decorations.remove(&id);
        let workspace = self.windows.get(&id).map(|managed| managed.workspace);
        if let Some(managed) = self.windows.get_mut(&id) {
            managed.mapped = false;
            managed.minimized = None;
            if let Some(handle) = managed.foreign.take() {
                self.foreign_toplevel_state.remove_toplevel(&handle);
            }
        }
        if let Some(toplevel) = window.toplevel() {
            toplevel.reset_initial_configure_sent();
        }
        if let Some(workspace) = workspace {
            self.retile(workspace);
        }
        self.forget_window_focus(id);
        tracing::debug!(%id, "window unmapped");
        self.emit(CompositorEvent::WindowClosed { id });
        self.emit(CompositorEvent::WorkspacesChanged);
    }

    fn forget_window_focus(&mut self, id: WindowId) {
        for workspace in &mut self.workspaces {
            workspace.focus_history.retain(|entry| *entry != id);
            if workspace.focused_window == Some(id) {
                workspace.focused_window = workspace.focus_history.last().copied();
            }
        }
        if self.focused_window == Some(id) {
            self.focused_window = None;
            if self.config.focus.focus_previous_on_close {
                self.focus_previous_window();
            } else {
                self.set_keyboard_focus(None);
            }
        }
        self.mark_pointer_focus_dirty();
        self.request_redraw();
    }

    fn apply_resize_anchor(&mut self, id: WindowId, window: &Window) {
        let Some(managed) = self.windows.get_mut(&id) else {
            return;
        };
        let Some((anchor, active)) = managed.resize else {
            return;
        };
        if !active {
            managed.resize = None;
        }
        let location = anchor.location_for(window.geometry().size);
        self.space.map_element(window.clone(), location, false);
        if !active {
            self.window_geometry_changed(window);
        }
    }

    pub fn new_toplevel(&mut self, surface: ToplevelSurface) {
        if self.config.general.kiosk && !self.windows.is_empty() {
            tracing::warn!("rejecting additional toplevel in kiosk mode");
            surface.send_close();
            return;
        }
        let id = WindowId(self.next_window_id);
        self.next_window_id += 1;
        with_states(surface.wl_surface(), |states| {
            states.data_map.insert_if_missing(|| RefCell::new(id));
            *states
                .data_map
                .get::<RefCell<WindowId>>()
                .expect("inserted above")
                .borrow_mut() = id;
        });
        let window = Window::new_wayland_window(surface);
        self.windows.insert(
            id,
            ManagedWindow {
                window,
                workspace: self.focused_workspace_id(),
                rules: AppliedWindowRule::default(),
                mapped: false,
                opened_at: None,
                minimized: None,
                restore: None,
                fullscreen: false,
                resize: None,
                foreign: None,
                title: String::new(),
                app_id: None,
            },
        );
        tracing::debug!(%id, "toplevel created");
    }

    pub fn toplevel_destroyed(&mut self, surface: &ToplevelSurface) {
        let Some(id) = surface_window_id(surface.wl_surface()) else {
            return;
        };
        let Some(managed) = self.windows.remove(&id) else {
            return;
        };
        self.space.unmap_elem(&managed.window);
        self.retile(managed.workspace);
        self.decorations.remove(&id);
        if let Some(handle) = managed.foreign {
            self.foreign_toplevel_state.remove_toplevel(&handle);
        }
        self.forget_window_focus(id);
        if managed.mapped {
            tracing::debug!(%id, "window closed");
            self.emit(CompositorEvent::WindowClosed { id });
            self.emit(CompositorEvent::WorkspacesChanged);
        }
    }

    pub fn toplevel_metadata_changed(&mut self, surface: &ToplevelSurface) {
        let Some(id) = surface_window_id(surface.wl_surface()) else {
            return;
        };
        let Some(managed) = self.windows.get_mut(&id) else {
            return;
        };
        let (title, app_id) = window_meta(&managed.window);
        let title_changed = managed.title != title;
        let app_id_changed = managed.app_id != app_id;
        managed.title.clone_from(&title);
        managed.app_id.clone_from(&app_id);
        if let Some(handle) = managed.foreign.as_ref() {
            if title_changed {
                handle.send_title(&title);
            }
            if app_id_changed {
                handle.send_app_id(app_id.as_deref().unwrap_or(""));
            }
            if title_changed || app_id_changed {
                handle.send_done();
            }
        }
        if !managed.mapped {
            return;
        }
        if title_changed {
            self.emit(CompositorEvent::WindowTitleChanged { id, title });
        }
        if app_id_changed {
            self.emit(CompositorEvent::WindowAppIdChanged {
                id,
                app_id: app_id.unwrap_or_default(),
            });
        }
    }

    pub fn keyboard_focus_changed(&mut self, focused: Option<&WlSurface>) {
        let id = focused.and_then(|surface| {
            surface_window_id(surface).or_else(|| {
                let popup = self.popup_manager.find_popup(surface)?;
                surface_window_id(&find_popup_root_surface(&popup).ok()?)
            })
        });
        if id == self.focused_window {
            return;
        }
        self.focused_window = id;
        for (window_id, managed) in &self.windows {
            if managed.mapped && managed.window.set_activated(Some(*window_id) == id) {
                if let Some(toplevel) = managed.window.toplevel() {
                    toplevel.send_pending_configure();
                }
            }
        }
        if let Some(id) = id {
            if let Some(workspace_id) = self.windows.get(&id).map(|managed| managed.workspace) {
                if let Some(workspace) = self.workspace_mut(workspace_id) {
                    workspace.focused_window = Some(id);
                    workspace.focus_history.retain(|entry| *entry != id);
                    workspace.focus_history.push(id);
                }
            }
            self.emit(CompositorEvent::WindowFocused { id });
        } else {
            self.emit(CompositorEvent::FocusCleared);
        }
        self.request_redraw();
    }

    pub fn preferred_decoration_mode(&self) -> DecorationMode {
        match self.config.decorations.mode {
            DecorationModeConfig::Server | DecorationModeConfig::Hybrid => {
                DecorationMode::ServerSide
            }
            DecorationModeConfig::Client | DecorationModeConfig::None => DecorationMode::ClientSide,
            DecorationModeConfig::Auto if self.config.window.server_side_decorations => {
                DecorationMode::ServerSide
            }
            DecorationModeConfig::Auto => DecorationMode::ClientSide,
        }
    }

    /// The decoration mode a surface should use: window rules win, then (in
    /// auto mode) whatever the client asked for, then the configured default.
    pub fn resolve_decoration_mode(&self, surface: &WlSurface) -> DecorationMode {
        let rule = surface_window_id(surface)
            .and_then(|id| self.windows.get(&id))
            .and_then(|managed| managed.rules.decoration);
        if let Some(server) = rule {
            return if server {
                DecorationMode::ServerSide
            } else {
                DecorationMode::ClientSide
            };
        }
        if self.surface_allows_hybrid(surface) {
            return DecorationMode::ServerSide;
        }
        let preferred = self.preferred_decoration_mode();
        match self.config.decorations.mode {
            DecorationModeConfig::Auto if self.config.window.server_side_decorations => {
                client_decoration_request(surface).unwrap_or(preferred)
            }
            _ => preferred,
        }
    }

    pub fn window_has_server_decoration(&self, window: &Window) -> bool {
        if self.window_is_fullscreen(window) {
            return false;
        }
        match self.config.decorations.mode {
            DecorationModeConfig::None => false,
            // A client that never negotiated decorations is only framed when
            // the compositor forces them; otherwise it draws its own.
            mode => window_negotiated_decoration(window)
                .map(|negotiated| negotiated == DecorationMode::ServerSide)
                .unwrap_or(matches!(
                    mode,
                    DecorationModeConfig::Server | DecorationModeConfig::Hybrid
                )),
        }
    }

    /// Hybrid decorations keep server-side controls but paint them over the
    /// client surface, so no frame pixels are reserved around the window.
    pub fn window_has_overlay_controls(&self, window: &Window) -> bool {
        (self.config.decorations.mode == DecorationModeConfig::Hybrid
            || window_integration::hybrid_requested(window.wl_surface().as_deref()))
            && self.window_has_server_decoration(window)
    }

    fn surface_allows_hybrid(&self, surface: &WlSurface) -> bool {
        !matches!(
            self.config.decorations.mode,
            DecorationModeConfig::Client | DecorationModeConfig::None
        ) && window_integration::hybrid_requested(Some(surface))
    }

    pub(crate) fn refresh_decoration_modes(&mut self) {
        for managed in self.windows.values() {
            let Some(toplevel) = managed.window.toplevel() else {
                continue;
            };
            let mode = self.resolve_decoration_mode(toplevel.wl_surface());
            set_surface_decoration_mode(toplevel.wl_surface(), mode);
            toplevel.with_pending_state(|state| state.decoration_mode = Some(mode));
            if toplevel.is_initial_configure_sent() {
                toplevel.send_pending_configure();
            }
        }
    }
}

pub fn surface_window_id(surface: &WlSurface) -> Option<WindowId> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<RefCell<WindowId>>()
            .map(|cell| *cell.borrow())
    })
}

pub fn window_is_maximized(window: &Window) -> bool {
    window.toplevel().is_some_and(|toplevel| {
        toplevel.with_pending_state(|state| state.states.contains(xdg_toplevel::State::Maximized))
    })
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

pub fn send_scale_to_surface(surface: &WlSurface, output: &Output) {
    let scale = output.current_scale();
    let transform = output.current_transform();
    smithay::desktop::utils::with_surfaces_surface_tree(surface, |surface, states| {
        smithay::wayland::fractional_scale::with_fractional_scale(states, |fractional| {
            fractional.set_preferred_scale(scale.fractional_scale());
        });
        smithay::wayland::compositor::send_surface_state(
            surface,
            states,
            scale.integer_scale(),
            transform,
        );
    });
}

/// Splits `area` into a master rectangle and a vertical stack.
fn master_stack(
    area: Rectangle<i32, Logical>,
    windows: usize,
    gap: i32,
    master_ratio: f64,
) -> Vec<Rectangle<i32, Logical>> {
    if windows <= 1 {
        return vec![area];
    }
    let stacked = windows - 1;
    let master_width = (f64::from(area.size.w - gap) * master_ratio.clamp(0.1, 0.9)).round() as i32;
    let master_width = master_width.clamp(1, (area.size.w - gap - 1).max(1));
    let stack_x = area.loc.x + master_width + gap;
    let stack_width = (area.size.w - master_width - gap).max(1);
    let total_gaps = gap * (stacked as i32 - 1);
    let stack_height = ((area.size.h - total_gaps) / stacked as i32).max(1);

    let mut rectangles = Vec::with_capacity(windows);
    rectangles.push(Rectangle::new(
        area.loc,
        Size::from((master_width, area.size.h)),
    ));
    for index in 0..stacked {
        let y = area.loc.y + index as i32 * (stack_height + gap);
        let height = if index == stacked - 1 {
            (area.loc.y + area.size.h - y).max(1)
        } else {
            stack_height
        };
        rectangles.push(Rectangle::new(
            (stack_x, y).into(),
            Size::from((stack_width, height)),
        ));
    }
    rectangles
}

fn animation_done(started: Instant, duration: u64) -> bool {
    started.elapsed() >= Duration::from_millis(duration)
}

fn animation_progress(started: Instant, animation: &AnimationConfig) -> f32 {
    if animation.duration == 0 {
        return 1.0;
    }
    let t = (started.elapsed().as_secs_f32() * 1_000.0 / animation.duration as f32).min(1.0);
    match animation.curve {
        AnimationCurve::Linear => t,
        AnimationCurve::EaseIn => t * t,
        AnimationCurve::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        AnimationCurve::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
            }
        }
    }
}

pub fn set_surface_decoration_mode(surface: &WlSurface, mode: DecorationMode) {
    with_states(surface, |states| {
        states
            .data_map
            .get_or_insert(|| std::cell::Cell::new(mode))
            .set(mode);
    });
}

/// The mode last sent to the client, or `None` if it never bound a
/// decoration protocol for this surface.
pub fn window_negotiated_decoration(window: &Window) -> Option<DecorationMode> {
    let surface = window.wl_surface()?;
    with_states(&surface, |states| {
        states
            .data_map
            .get::<std::cell::Cell<DecorationMode>>()
            .map(std::cell::Cell::get)
    })
}

/// What the client itself asked for through xdg-decoration or the KDE
/// protocol, kept apart from the effective mode so policy changes (config
/// reloads, window rules) can be re-resolved without forgetting it.
struct ClientDecorationRequest(std::cell::Cell<Option<DecorationMode>>);

pub fn set_client_decoration_request(surface: &WlSurface, mode: Option<DecorationMode>) {
    with_states(surface, |states| {
        states
            .data_map
            .get_or_insert(|| ClientDecorationRequest(std::cell::Cell::new(None)))
            .0
            .set(mode);
    });
}

fn client_decoration_request(surface: &WlSurface) -> Option<DecorationMode> {
    with_states(surface, |states| {
        states
            .data_map
            .get::<ClientDecorationRequest>()
            .and_then(|request| request.0.get())
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    struct TestEvents(std::sync::Mutex<Vec<CompositorEvent>>);

    pub(crate) fn fixture() -> (
        smithay::reexports::wayland_server::Display<BlairState>,
        smithay::reexports::calloop::EventLoop<'static, BlairState>,
        BlairState,
    ) {
        let display = smithay::reexports::wayland_server::Display::<BlairState>::new().unwrap();
        let event_loop = smithay::reexports::calloop::EventLoop::<BlairState>::try_new().unwrap();
        let mut state = BlairState::new(
            display.handle(),
            event_loop.handle(),
            event_loop.get_signal(),
            CompositorConfig::default(),
            Arc::new(TestEvents(std::sync::Mutex::new(Vec::new()))),
        );
        state.idle_dimming = crate::idle_dim::IdleDimming::without_backlight();
        (display, event_loop, state)
    }

    fn keyboard_symbol(state: &mut BlairState) -> Vec<smithay::input::keyboard::Keysym> {
        state
            .seat
            .get_keyboard()
            .unwrap()
            .with_xkb_state(state, |context| {
                let xkb = context.xkb().lock().unwrap();
                xkb.raw_syms_for_key_in_layout(29u32.into(), xkb.active_layout())
                    .to_vec()
            })
    }

    #[test]
    fn the_startup_keymap_fallback_matches_the_effective_configuration() {
        let display = smithay::reexports::wayland_server::Display::<BlairState>::new().unwrap();
        let event_loop = smithay::reexports::calloop::EventLoop::<BlairState>::try_new().unwrap();
        let mut config = CompositorConfig::default();
        config.input.keyboard.layout = "blair_missing_layout".into();
        config.input.keyboard.variant = "blair_missing_variant".into();
        let mut state = BlairState::new(
            display.handle(),
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            Arc::new(TestEvents(std::sync::Mutex::new(Vec::new()))),
        );
        assert_eq!(state.config.input.keyboard.layout, "us");
        assert!(state.config.input.keyboard.variant.is_empty());
        let mut next = state.config.clone();
        next.input.keyboard.layout = "de".into();
        state.apply_config(next).unwrap();
        assert_eq!(state.config.input.keyboard.layout, "de");
    }

    #[test]
    fn invalid_keymaps_leave_configuration_keymap_and_repeat_info_unchanged() {
        use wayland_client::protocol::wl_seat::WlSeat;
        let (mut display, _event_loop, mut state) = fixture();
        let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
        let _keyboard = client.bind::<WlSeat>(7).get_keyboard(&client.handle(), ());
        client.dispatch(&mut display, &mut state);
        let previous = state.config.clone();
        let symbol = keyboard_symbol(&mut state);
        let repeat = client.events.repeat_info.clone();
        state.take_redraw_request();
        for (layout, variant) in [
            ("blair_missing_layout", ""),
            ("us", "blair_missing_variant"),
        ] {
            let mut next = previous.clone();
            next.input.keyboard.layout = layout.into();
            next.input.keyboard.variant = variant.into();
            next.input.keyboard.repeat_rate = 10;
            next.decorations.corner_radius = 24;
            crate::config::validate(&next).unwrap();
            let error = state.apply_config(next).unwrap_err();
            assert!(error.contains(layout));
            assert!(error.contains(variant));
            assert_eq!(state.config, previous);
            assert_eq!(keyboard_symbol(&mut state), symbol);
            assert!(!state.input_config_changed);
            assert!(!state.take_redraw_request());
            client.dispatch(&mut display, &mut state);
            assert_eq!(client.events.keymaps, 1);
            assert_eq!(client.events.repeat_info, repeat);
        }
    }

    #[test]
    fn a_device_failure_does_not_apply_a_prepared_keymap() {
        use wayland_client::protocol::wl_seat::WlSeat;
        let (mut display, _event_loop, mut state) = fixture();
        let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
        let _keyboard = client.bind::<WlSeat>(7).get_keyboard(&client.handle(), ());
        client.dispatch(&mut display, &mut state);
        let previous = state.config.clone();
        let symbol = keyboard_symbol(&mut state);
        let mut next = previous.clone();
        next.input.keyboard.layout = "de".into();
        next.input.mouse.sensitivity = 0.5;
        assert!(state.apply_config(next).is_err());
        assert_eq!(state.config, previous);
        assert_eq!(keyboard_symbol(&mut state), symbol);
        client.dispatch(&mut display, &mut state);
        assert_eq!(client.events.keymaps, 1);
    }

    #[test]
    fn keyboard_layout_changes_apply_and_repeat_changes_preserve_sticky_modifiers() {
        use wayland_client::protocol::wl_seat::WlSeat;
        struct Hooks;
        impl crate::input::InputHooks for Hooks {}
        let (mut display, _event_loop, mut state) = fixture();
        let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
        let _keyboard = client.bind::<WlSeat>(7).get_keyboard(&client.handle(), ());
        client.dispatch(&mut display, &mut state);
        let before = keyboard_symbol(&mut state);
        let mut next = state.config.clone();
        next.input.keyboard.layout = "de".into();
        next.input.keyboard.variant = "nodeadkeys".into();
        state.apply_config(next).unwrap();
        assert_ne!(keyboard_symbol(&mut state), before);
        client.dispatch(&mut display, &mut state);
        assert_eq!(client.events.keymaps, 2);
        state.config.accessibility.sticky_keys = true;
        for pressed in [true, false] {
            for event in state.sticky_keys.transform(50, pressed) {
                crate::input::keyboard_transition(
                    &mut state,
                    event.code,
                    event.pressed,
                    0,
                    &mut Hooks,
                );
            }
        }
        let keyboard = state.seat.get_keyboard().unwrap();
        assert!(keyboard.modifier_state().shift);
        let mut next = state.config.clone();
        next.input.keyboard.repeat_rate = 12;
        next.input.keyboard.repeat_delay = 500;
        state.apply_config(next).unwrap();
        assert!(keyboard.modifier_state().shift);
        client.dispatch(&mut display, &mut state);
        assert_eq!(client.events.keymaps, 2);
        assert_eq!(client.events.repeat_info.last(), Some(&(12, 500)));
    }

    #[test]
    fn idle_dimming_reuses_one_timer_during_sustained_input() {
        let (_display, mut event_loop, mut state) = fixture();
        state.config.power.idle_dim = true;
        state.config.power.idle_dim_timeout = 0;
        state.note_input_activity();
        let token = state.idle_dim_timer.unwrap();
        state.config.power.idle_dim_timeout = 300;
        for _ in 0..10_000 {
            state.note_input_activity();
            assert_eq!(state.idle_dim_timer, Some(token));
        }
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.idle_dim_timer, Some(token));
        assert!(state.loop_handle.update(&token).is_ok());

        let mut config = state.config.clone();
        config.power.idle_dim = false;
        state.apply_config(config).unwrap();
        assert!(state.idle_dim_timer.is_none());
        assert!(state.loop_handle.update(&token).is_err());
        state.note_input_activity();
        assert!(state.idle_dim_timer.is_none());
    }

    #[test]
    fn idle_dimming_replaces_the_timer_on_configuration_changes_and_drops_it_on_expiry() {
        let (_display, mut event_loop, mut state) = fixture();
        let mut config = state.config.clone();
        config.power.idle_dim = true;
        state.apply_config(config).unwrap();
        let token = state.idle_dim_timer.unwrap();
        let mut config = state.config.clone();
        config.power.idle_dim_timeout = 0;
        state.apply_config(config).unwrap();
        assert!(state.loop_handle.update(&token).is_err());
        let token = state.idle_dim_timer.unwrap();
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert!(state.idle_dim_timer.is_none());
        assert!(state.loop_handle.update(&token).is_err());
        state.note_input_activity();
        assert!(state.idle_dim_timer.is_some());
    }

    #[test]
    fn sticky_modifiers_reach_xkb_and_clear_after_a_key_or_disabling_the_preference() {
        struct Hooks;
        impl crate::input::InputHooks for Hooks {}
        fn feed(state: &mut BlairState, code: u32, pressed: bool) {
            for event in state.sticky_keys.transform(code, pressed) {
                crate::input::keyboard_transition(state, event.code, event.pressed, 0, &mut Hooks);
            }
        }
        let (_display, _event_loop, mut state) = fixture();
        state.config.accessibility.sticky_keys = true;
        let keyboard = state.seat.get_keyboard().unwrap();
        feed(&mut state, 50, true);
        feed(&mut state, 50, false);
        assert!(keyboard.modifier_state().shift);
        feed(&mut state, 38, true);
        assert!(state.physical_mods.shift);
        feed(&mut state, 38, false);
        assert!(!keyboard.modifier_state().shift);
        feed(&mut state, 50, true);
        feed(&mut state, 50, false);
        feed(&mut state, 50, true);
        feed(&mut state, 50, false);
        feed(&mut state, 38, true);
        feed(&mut state, 38, false);
        assert!(keyboard.modifier_state().shift);
        let mut config = state.config.clone();
        config.accessibility.sticky_keys = false;
        state.apply_config(config).unwrap();
        assert!(!keyboard.modifier_state().shift);
        assert!(!state.physical_mods.shift);
    }

    #[test]
    fn visual_bells_respect_the_preference_and_expire_through_the_event_loop() {
        use smithay::wayland::xdg_system_bell::XdgSystemBellHandler;
        let (_display, mut event_loop, mut state) = fixture();
        state.ring(None);
        assert!(state.visual_bell_until.is_none());
        state.config.accessibility.visual_alerts = true;
        state.ring(None);
        assert!(state.visual_bell_until.is_some());
        event_loop
            .dispatch(Duration::from_millis(200), &mut state)
            .unwrap();
        assert!(state.visual_bell_until.is_none());
    }

    impl EventChannel for TestEvents {
        fn publish(&self, event: CompositorEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn workspace_changes_apply_without_restart_and_keep_the_output_reachable() {
        let display = smithay::reexports::wayland_server::Display::<BlairState>::new().unwrap();
        let event_loop = smithay::reexports::calloop::EventLoop::<BlairState>::try_new().unwrap();
        let events = Arc::new(TestEvents(std::sync::Mutex::new(Vec::new())));
        let mut state = BlairState::new(
            display.handle(),
            event_loop.handle(),
            event_loop.get_signal(),
            CompositorConfig::default(),
            events.clone(),
        );
        let output = Output::new(
            "test-0".into(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "Test".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(smithay::output::Mode {
                size: (1280, 720).into(),
                refresh: 60_000,
            }),
            None,
            None,
            Some((0, 0).into()),
        );
        state.add_output(&output, (0, 0).into());
        assert!(state.switch_workspace(10));
        let mut config = state.config.clone();
        config.workspaces.count = 2;
        config.workspaces.wrap = false;
        config.workspaces.definitions.insert(
            "2".into(),
            crate::config::WorkspaceDefinition {
                name: Some("dev".into()),
                output: Some("test-0".into()),
            },
        );
        state.apply_config(config).unwrap();
        assert_eq!(state.config.workspaces.count, 2);
        assert_eq!(state.list_workspaces().len(), 2);
        assert_eq!(state.focused_workspace_id(), 2);
        assert_eq!(state.list_workspaces()[1].name, "dev");
        assert!(!state.switch_workspace(3));
        assert!(events
            .0
            .lock()
            .unwrap()
            .contains(&CompositorEvent::WorkspacesChanged));
        let mut config = state.config.clone();
        config.workspaces.dynamic = true;
        state.apply_config(config).unwrap();
        assert_eq!(state.create_workspace("extra".into()), 3);
        assert!(state.switch_workspace(3));
        let mut config = state.config.clone();
        config.workspaces.dynamic = false;
        config.workspaces.count = 4;
        config.workspaces.wrap = true;
        state.apply_config(config).unwrap();
        assert_eq!(state.list_workspaces().len(), 4);
        assert!(state.switch_workspace(5));
        assert_eq!(state.focused_workspace_id(), 1);
    }

    #[test]
    fn hot_corners_trigger_once_per_entry_and_cancel_when_the_pointer_leaves() {
        let display = smithay::reexports::wayland_server::Display::<BlairState>::new().unwrap();
        let mut event_loop =
            smithay::reexports::calloop::EventLoop::<BlairState>::try_new().unwrap();
        let mut state = BlairState::new(
            display.handle(),
            event_loop.handle(),
            event_loop.get_signal(),
            CompositorConfig::default(),
            Arc::new(TestEvents(std::sync::Mutex::new(Vec::new()))),
        );
        let output = Output::new(
            "test-0".into(),
            smithay::output::PhysicalProperties {
                size: (0, 0).into(),
                subpixel: smithay::output::Subpixel::Unknown,
                make: "Test".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(smithay::output::Mode {
                size: (1280, 720).into(),
                refresh: 60_000,
            }),
            None,
            None,
            Some((0, 0).into()),
        );
        state.add_output(&output, (0, 0).into());
        state.config.hot_corners.dwell_ms = 0;
        state.config.hot_corners.top_left.action = crate::hot_corners::CornerAction::NextWorkspace;
        let pointer = state.seat.get_pointer().unwrap();
        pointer.set_location((1.0, 1.0).into());
        state.update_hot_corners(pointer.current_location());
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.focused_workspace_id(), 2);
        state.update_hot_corners(pointer.current_location());
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.focused_workspace_id(), 2);
        pointer.set_location((100.0, 100.0).into());
        state.update_hot_corners(pointer.current_location());
        pointer.set_location((1.0, 1.0).into());
        state.update_hot_corners(pointer.current_location());
        pointer.set_location((100.0, 100.0).into());
        state.update_hot_corners(pointer.current_location());
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.focused_workspace_id(), 2);
        pointer.set_location((1.0, 1.0).into());
        state.update_hot_corners(pointer.current_location());
        event_loop.dispatch(Duration::ZERO, &mut state).unwrap();
        assert_eq!(state.focused_workspace_id(), 3);
    }

    fn area() -> Rectangle<i32, Logical> {
        Rectangle::new((10, 20).into(), (1000, 600).into())
    }

    #[test]
    fn a_single_window_fills_the_area() {
        assert_eq!(master_stack(area(), 1, 8, 0.5), vec![area()]);
    }

    #[test]
    fn the_master_keeps_its_share_and_the_stack_splits_the_rest() {
        let rectangles = master_stack(area(), 3, 8, 0.5);
        assert_eq!(rectangles.len(), 3);
        assert_eq!(
            rectangles[0],
            Rectangle::new((10, 20).into(), (496, 600).into())
        );
        assert_eq!(rectangles[1].loc, (514, 20).into());
        assert_eq!(rectangles[1].size, (496, 296).into());
        assert_eq!(rectangles[2].loc, (514, 324).into());
        // The last tile absorbs the rounding so the stack ends at the area edge.
        assert_eq!(rectangles[2].loc.y + rectangles[2].size.h, 620);
    }

    #[test]
    fn extreme_ratios_are_clamped_to_a_usable_split() {
        let rectangles = master_stack(area(), 2, 0, 5.0);
        assert!(rectangles[0].size.w < 1000);
        assert!(rectangles[1].size.w >= 1);
        let narrow = master_stack(Rectangle::from_size((4, 4).into()), 3, 8, 0.5);
        assert!(narrow
            .iter()
            .all(|rect| rect.size.w >= 1 && rect.size.h >= 1));
    }
}
