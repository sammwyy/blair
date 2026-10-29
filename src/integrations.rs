use std::sync::Arc;

use blair_integration::{CompositorApi, EventChannel, EventFanout, Transport};
use blair_integration_dbus::DbusIntegration;
use blair_protocol::{
    InputCapabilities, Rect, RenderStats, ShortcutBinding, ShortcutCommand, WindowId, WindowInfo,
};

use crate::config::{BindingConfig, DecorationModeConfig, WindowLayout};
use crate::state::BlairState;

pub struct Integrations {
    transports: Vec<Box<dyn Transport>>,
    events: Arc<dyn EventChannel>,
}

impl Integrations {
    /// `wake` is handed to integration threads so queued requests wake the
    /// compositor's event loop.
    pub fn start(
        dbus_enabled: bool,
        wake: Arc<dyn Fn() + Send + Sync>,
        environment: Vec<(String, String)>,
    ) -> Self {
        let mut transports: Vec<Box<dyn Transport>> = Vec::new();
        let mut event_channels = Vec::new();
        if dbus_enabled {
            let dbus = DbusIntegration::start(wake, environment);
            event_channels.push(dbus.event_channel());
            transports.push(Box::new(dbus));
        }
        Self {
            transports,
            events: Arc::new(EventFanout::new(event_channels)),
        }
    }

    pub fn event_channel(&self) -> Arc<dyn EventChannel> {
        self.events.clone()
    }

    pub fn drain(&mut self, state: &mut BlairState) {
        for transport in &mut self.transports {
            transport.drain(state);
        }
    }
}

impl CompositorApi for BlairState {
    fn display_info(&self) -> Vec<blair_protocol::DisplayInfo> {
        fn mode(mode: smithay::output::Mode) -> blair_protocol::DisplayMode {
            blair_protocol::DisplayMode {
                width: mode.size.w,
                height: mode.size.h,
                refresh_millihz: mode.refresh,
            }
        }
        self.space
            .outputs()
            .filter_map(|output| {
                let current = output.current_mode()?;
                let name = output.name();
                let deadline = self.display_confirmations.get(&name);
                Some(blair_protocol::DisplayInfo {
                    current: mode(current),
                    modes: output.modes().into_iter().map(mode).collect(),
                    can_change_mode: self.mutable_displays.contains(&name),
                    pending_confirmation: deadline.is_some(),
                    confirmation_seconds: deadline
                        .map(|deadline| {
                            deadline
                                .saturating_duration_since(std::time::Instant::now())
                                .as_secs() as u32
                                + 1
                        })
                        .unwrap_or(0),
                    name,
                })
            })
            .collect()
    }

    fn apply_display_mode(
        &mut self,
        output: &str,
        mode: blair_protocol::DisplayMode,
        reply: Box<dyn FnOnce(Result<(), String>) + Send>,
    ) {
        self.pending_display_requests
            .push(crate::outputs::DisplayRequest {
                output: output.into(),
                action: crate::outputs::DisplayAction::Apply(mode),
                reply,
            });
        self.request_redraw();
    }

    fn confirm_display_mode(
        &mut self,
        output: &str,
        reply: Box<dyn FnOnce(Result<(), String>) + Send>,
    ) {
        self.pending_display_requests
            .push(crate::outputs::DisplayRequest {
                output: output.into(),
                action: crate::outputs::DisplayAction::Confirm,
                reply,
            });
        self.request_redraw();
    }

    fn revert_display_mode(
        &mut self,
        output: &str,
        reply: Box<dyn FnOnce(Result<(), String>) + Send>,
    ) {
        self.pending_display_requests
            .push(crate::outputs::DisplayRequest {
                output: output.into(),
                action: crate::outputs::DisplayAction::Revert,
                reply,
            });
        self.request_redraw();
    }

    fn list_windows(&self) -> Vec<WindowInfo> {
        BlairState::list_windows(self)
    }

    fn list_workspaces(&self) -> Vec<blair_protocol::WorkspaceInfo> {
        BlairState::list_workspaces(self)
    }

    fn create_workspace(&mut self, name: String) -> u64 {
        BlairState::create_workspace(self, name)
    }

    fn destroy_workspace(&mut self, id: u64) -> bool {
        BlairState::destroy_workspace(self, id)
    }

    fn workspace_capabilities(&self) -> (bool, bool) {
        BlairState::workspace_capabilities(self)
    }

    fn switch_workspace(&mut self, id: u64) -> bool {
        BlairState::switch_workspace(self, id)
    }

    fn move_window_to_workspace(&mut self, window: WindowId, workspace_id: u64) -> bool {
        BlairState::move_window_to_workspace(self, window, workspace_id)
    }

    fn focus_window(&mut self, id: WindowId) -> bool {
        self.focus_window_by_id(id)
    }

    fn close_window(&mut self, id: WindowId) -> bool {
        self.close_window_by_id(id)
    }

    fn minimize_window(&mut self, id: WindowId) -> bool {
        self.minimize_window_by_id(id)
    }

    fn toggle_maximize_window(&mut self, id: WindowId) -> bool {
        self.toggle_maximize_window_by_id(id)
    }

    fn move_resize_window(&mut self, id: WindowId, geometry: Rect) -> bool {
        BlairState::move_resize_window(self, id, geometry)
    }

    fn work_area(&self, output: &str) -> Rect {
        BlairState::work_area(self, output)
    }

    fn outputs(&self) -> Vec<String> {
        BlairState::outputs(self)
    }

    fn screenshot(&mut self, output: &str, path: &str, reply: Box<dyn FnOnce(bool) + Send>) {
        self.request_screenshot(output, path, reply);
    }

    fn render_stats(&self) -> RenderStats {
        self.render_stats
    }

    fn window_settings(&self) -> (i32, i32, i32, bool) {
        let decoration = &self.config.decorations;
        (
            decoration.titlebar_height,
            self.decoration_theme().border_width,
            self.decoration_theme().corner_radius,
            self.config.window.server_side_decorations,
        )
    }

    fn layout_settings(&self) -> (String, i32, i32) {
        let layout = match self.config.window.layout {
            WindowLayout::Floating => "floating",
            WindowLayout::Tiling => "tiling",
        };
        (
            layout.to_string(),
            self.config.window.work_area_padding,
            self.decoration_theme().corner_radius,
        )
    }

    fn set_layout_settings(
        &mut self,
        layout: &str,
        work_area_padding: i32,
        corner_radius: i32,
    ) -> bool {
        let layout = match layout {
            "floating" => WindowLayout::Floating,
            "tiling" => WindowLayout::Tiling,
            _ => return false,
        };
        if !(0..=128).contains(&work_area_padding) || !matches!(corner_radius, 0 | 12 | 24) {
            return false;
        }
        let mut next = self.config.clone();
        next.window.layout = layout;
        next.window.work_area_padding = work_area_padding;
        next.decorations.corner_radius = corner_radius;
        if !self.commit_configuration(next) {
            return false;
        }
        tracing::info!(
            ?layout,
            work_area_padding,
            corner_radius,
            "layout settings updated by an integration"
        );
        true
    }

    fn set_window_settings(
        &mut self,
        titlebar_height: i32,
        border_width: i32,
        corner_radius: i32,
        server_side_decorations: bool,
    ) -> bool {
        if !(20..=96).contains(&titlebar_height)
            || !(0..=16).contains(&border_width)
            || !(0..=64).contains(&corner_radius)
        {
            tracing::warn!(
                titlebar_height,
                border_width,
                corner_radius,
                "invalid window settings"
            );
            return false;
        }
        let mut next = self.config.clone();
        next.decorations.titlebar_height = titlebar_height;
        // Pixel widths from integrations snap to the nearest named size; 0
        // hides the border like the settings' "None" choice.
        if border_width == 0 {
            next.decorations.border = crate::config::BorderColorMode::None;
        } else {
            if next.decorations.border == crate::config::BorderColorMode::None {
                next.decorations.border = crate::config::BorderColorMode::Theme;
            }
            next.decorations.border_size = crate::config::BorderSize::nearest(border_width);
        }
        next.decorations.corner_radius = corner_radius;
        next.window.server_side_decorations = server_side_decorations;
        next.decorations.mode = if server_side_decorations {
            DecorationModeConfig::Server
        } else {
            DecorationModeConfig::Client
        };
        if !self.commit_configuration(next) {
            return false;
        }
        tracing::info!(
            titlebar_height,
            border_width,
            corner_radius,
            server_side_decorations,
            "window settings updated"
        );
        true
    }

    fn configuration(&self) -> String {
        toml::to_string_pretty(&self.config).unwrap_or_default()
    }

    fn input_capabilities(&self) -> InputCapabilities {
        self.input_devices.capabilities()
    }

    fn prepare_for_sleep(&mut self) {
        BlairState::prepare_for_sleep(self);
    }

    fn set_configuration(&mut self, configuration: &str) -> bool {
        let next = match toml::from_str(configuration) {
            Ok(next) => next,
            Err(error) => {
                tracing::warn!(%error, "integration sent an invalid configuration");
                return false;
            }
        };
        if !self.commit_configuration(next) {
            return false;
        }
        tracing::info!("configuration replaced by an integration");
        true
    }

    fn configured_shortcuts(&self) -> Vec<ShortcutBinding> {
        self.config
            .bindings
            .iter()
            .filter_map(
                |binding| match (&binding.action, &binding.exec, binding.value) {
                    (Some(action), None, None) if action == "close" => Some(ShortcutBinding {
                        accelerator: binding.accelerator(),
                        command: ShortcutCommand::Close,
                        argument: None,
                    }),
                    (Some(action), None, Some(value)) if action == "workspace" => {
                        Some(ShortcutBinding {
                            accelerator: binding.accelerator(),
                            command: ShortcutCommand::Workspace,
                            argument: Some(value.to_string()),
                        })
                    }
                    (Some(action), None, Some(value)) if action == "move-to-workspace" => {
                        Some(ShortcutBinding {
                            accelerator: binding.accelerator(),
                            command: ShortcutCommand::MoveToWorkspace,
                            argument: Some(value.to_string()),
                        })
                    }
                    (None, Some(command), None) => Some(ShortcutBinding {
                        accelerator: binding.accelerator(),
                        command: ShortcutCommand::Execute,
                        argument: Some(command.clone()),
                    }),
                    _ => None,
                },
            )
            .collect()
    }

    fn set_configured_shortcuts(&mut self, bindings: Vec<ShortcutBinding>) -> bool {
        if bindings.len() > crate::shortcuts::MAX_BINDINGS_PER_CLIENT {
            return false;
        }
        let mut configured = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let keys = binding
                .accelerator
                .split('+')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .collect();
            let next = match binding.command {
                ShortcutCommand::Close if binding.argument.is_none() => BindingConfig {
                    keys,
                    action: Some("close".to_owned()),
                    exec: None,
                    value: None,
                },
                ShortcutCommand::Workspace | ShortcutCommand::MoveToWorkspace => {
                    let Some(value) = binding
                        .argument
                        .as_deref()
                        .and_then(|value| value.parse().ok())
                    else {
                        return false;
                    };
                    BindingConfig {
                        keys,
                        action: Some(binding.command.id().to_owned()),
                        exec: None,
                        value: Some(value),
                    }
                }
                ShortcutCommand::Execute => {
                    let Some(command) = binding.argument.filter(|value| !value.trim().is_empty())
                    else {
                        return false;
                    };
                    BindingConfig {
                        keys,
                        action: None,
                        exec: Some(command),
                        value: None,
                    }
                }
                _ => return false,
            };
            if next.validate(configured.len() + 1).is_err() {
                return false;
            }
            configured.push(next);
        }
        let mut next = self.config.clone();
        next.bindings = configured;
        self.commit_configuration(next)
    }

    fn bind_shortcut(&mut self, client: &str, id: &str, accelerator: &str) -> bool {
        if client == "blair-config" {
            return false;
        }
        self.shortcuts.bind(client, id, accelerator)
    }

    fn unbind_shortcut(&mut self, client: &str, id: &str) {
        if client == "blair-config" {
            return;
        }
        self.shortcuts.unbind(client, id)
    }

    fn register_window_rule(&mut self, client: &str, id: &str, rule_toml: &str) -> bool {
        if client == "blair-config" {
            return false;
        }
        self.register_temporary_rule(client, id, rule_toml)
    }

    fn unregister_window_rule(&mut self, client: &str, id: &str) {
        self.rules.unregister(client, id);
    }

    fn unregister_client(&mut self, client: &str) {
        if client == "blair-config" {
            return;
        }
        self.shortcuts.unregister_client(client);
        self.rules.unregister_client(client);
    }

    fn theme_changed(&mut self) {
        self.reload_system_theme();
    }

    fn quit(&mut self) {
        self.request_exit();
    }
}

impl BlairState {
    fn commit_configuration(&mut self, next: crate::config::CompositorConfig) -> bool {
        if let Err(error) = crate::config::validate(&next) {
            tracing::warn!(%error, "integration sent a configuration that failed validation");
            return false;
        }
        if let Err(error) = self.apply_config_with(next, |config| {
            crate::config::save(config).map_err(|error| error.to_string())
        }) {
            tracing::warn!(%error, "integration configuration update rejected");
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn coalescing_geometry_events_allocates_nothing_after_initialization() {
        let (sender, _receiver) = blair_integration::events::channel();
        let event = blair_protocol::CompositorEvent::WindowGeometryChanged {
            id: blair_protocol::WindowId(1),
            geometry: Default::default(),
        };
        sender.send(event.clone());
        let allocations = crate::test_support::count_allocations(|| {
            for _ in 0..10_000 {
                sender.send(event.clone());
            }
        });
        assert_eq!(allocations, 0);
    }

    use super::*;
    use crate::{state::tests::fixture, test_support::isolated_config_test};
    use std::fs;

    #[test]
    fn legacy_settings_update_the_theme_and_persist_the_effective_configuration() {
        isolated_config_test(
            concat!(
                module_path!(),
                "::legacy_settings_update_the_theme_and_persist_the_effective_configuration"
            ),
            |_| {
                let (_display, _event_loop, mut state) = fixture();
                let system = creamui_theme_loader::SystemThemeLoader::new().load().ok();
                let radius = |fallback| {
                    system
                        .as_ref()
                        .map_or(fallback, |system| system.corners.window_radius())
                };
                let text_scale = state.decoration_theme().text_scale;
                assert!(state.set_layout_settings("floating", 24, 24));
                assert_eq!(state.layout_settings(), ("floating".into(), 24, radius(24)));
                assert_eq!(state.config.decorations.corner_radius, 24);
                assert_eq!(read_configuration(), state.config);
                assert!(state.set_window_settings(40, 0, 12, false));
                assert_eq!(state.window_settings(), (40, 0, radius(12), false));
                assert_eq!(
                    state.decoration_theme().titlebar_height,
                    (40.0 * text_scale).round() as i32
                );
                assert_eq!(read_configuration(), state.config);
                assert!(state.set_window_settings(32, 3, 24, true));
                assert_eq!(state.window_settings(), (32, 2, radius(24), true));
                assert_eq!(state.config.decorations.mode, DecorationModeConfig::Server);
                assert_eq!(read_configuration(), state.config);
            },
        );
    }

    fn read_configuration() -> crate::config::CompositorConfig {
        toml::from_str(&fs::read_to_string(crate::config::user_config_path()).unwrap()).unwrap()
    }

    #[test]
    fn failed_saves_leave_legacy_and_full_configuration_state_unchanged() {
        isolated_config_test(
            concat!(
                module_path!(),
                "::failed_saves_leave_legacy_and_full_configuration_state_unchanged"
            ),
            |root| {
                let (mut display, _event_loop, mut state) = fixture();
                let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
                client
                    .bind::<wayland_client::protocol::wl_seat::WlSeat>(7)
                    .get_keyboard(&client.handle(), ());
                client.dispatch(&mut display, &mut state);
                let keymaps = client.events.keymaps;
                let repeat = client.events.repeat_info.clone();
                state.take_redraw_request();
                let previous = state.config.clone();
                let previous_theme = state.window_settings();
                fs::write(root.join("blair"), "not a directory").unwrap();
                assert!(!state.set_layout_settings("floating", 64, 24));
                assert_eq!(state.config, previous);
                assert_eq!(state.window_settings(), previous_theme);
                assert!(!state.set_window_settings(48, 0, 24, false));
                assert_eq!(state.config, previous);
                assert_eq!(state.window_settings(), previous_theme);
                let mut next = state.config.clone();
                next.workspaces.count = 2;
                next.input.keyboard.layout = "de".into();
                let keyboard = state.seat.get_keyboard().unwrap();
                let layout = keyboard.with_xkb_state(&mut state, |context| {
                    let xkb = context.xkb().lock().unwrap();
                    xkb.layout_name(xkb.active_layout()).to_owned()
                });
                assert!(!state.set_configuration(&toml::to_string(&next).unwrap()));
                assert_eq!(state.config, previous);
                assert_eq!(
                    state.list_workspaces().len(),
                    previous.workspaces.count as usize
                );
                let actual = keyboard.with_xkb_state(&mut state, |context| {
                    let xkb = context.xkb().lock().unwrap();
                    xkb.layout_name(xkb.active_layout()).to_owned()
                });
                assert_eq!(actual, layout);
                client.dispatch(&mut display, &mut state);
                assert_eq!(client.events.keymaps, keymaps);
                assert_eq!(client.events.repeat_info, repeat);
                assert!(!state.take_redraw_request());
                assert_eq!(
                    fs::read_to_string(root.join("blair")).unwrap(),
                    "not a directory"
                );
            },
        );
    }

    fn map_window(
        client: &mut crate::test_support::TestClient,
        display: &mut smithay::reexports::wayland_server::Display<BlairState>,
        state: &mut BlairState,
    ) -> (
        WindowId,
        wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel,
    ) {
        use wayland_client::protocol::wl_compositor::WlCompositor;
        use wayland_protocols::xdg::shell::client::xdg_wm_base::XdgWmBase;
        let root = client
            .bind::<WlCompositor>(6)
            .create_surface(&client.handle(), ());
        let shell = client.bind::<XdgWmBase>(6);
        let xdg = shell.get_xdg_surface(&root, &client.handle(), ());
        let toplevel = xdg.get_toplevel(&client.handle(), ());
        root.commit();
        client.dispatch(display, state);
        client.paint_surface(&root, 400, 300);
        client.dispatch(display, state);
        let id = *state.windows.last_key_value().unwrap().0;
        assert!(state.windows[&id].mapped);
        (id, toplevel)
    }

    #[test]
    fn legacy_window_settings_refresh_decoration_negotiation() {
        isolated_config_test(
            concat!(
                module_path!(),
                "::legacy_window_settings_refresh_decoration_negotiation"
            ),
            |_| {
                use wayland_protocols::xdg::decoration::zv1::client::{
                    zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
                    zxdg_toplevel_decoration_v1::Mode,
                };
                let (mut display, _event_loop, mut state) = fixture();
                crate::test_support::add_output(&mut state, "test-0", (0, 0), (800, 600));
                let mut config = state.config.clone();
                config.decorations.mode = DecorationModeConfig::Server;
                state.apply_config(config).unwrap();
                let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
                let (_id, toplevel) = map_window(&mut client, &mut display, &mut state);
                let manager = client.bind::<ZxdgDecorationManagerV1>(1);
                let _decoration = manager.get_toplevel_decoration(&toplevel, &client.handle(), ());
                client.dispatch(&mut display, &mut state);
                assert_eq!(
                    client.events.decoration_modes.last(),
                    Some(&Mode::ServerSide)
                );
                assert!(state.set_window_settings(32, 2, 12, false));
                client.dispatch(&mut display, &mut state);
                assert_eq!(
                    client.events.decoration_modes.last(),
                    Some(&Mode::ClientSide)
                );
                assert!(state.set_window_settings(40, 3, 24, true));
                client.dispatch(&mut display, &mut state);
                assert_eq!(
                    client.events.decoration_modes.last(),
                    Some(&Mode::ServerSide)
                );
            },
        );
    }

    #[test]
    fn layout_updates_reach_all_workspaces_and_failed_renames_preserve_window_state() {
        isolated_config_test(
            concat!(
                module_path!(),
                "::layout_updates_reach_all_workspaces_and_failed_renames_preserve_window_state"
            ),
            |_| {
                let (mut display, _event_loop, mut state) = fixture();
                crate::test_support::add_output(&mut state, "test-0", (0, 0), (800, 600));
                crate::test_support::add_output(&mut state, "test-1", (800, 0), (800, 600));
                let mut config = state.config.clone();
                config.window.layout = WindowLayout::Tiling;
                config.decorations.mode = DecorationModeConfig::Server;
                state.apply_config(config).unwrap();
                let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
                let (first, _) = map_window(&mut client, &mut display, &mut state);
                state.set_focused_output_at((900.0, 100.0).into());
                let (second, _) = map_window(&mut client, &mut display, &mut state);
                assert_ne!(
                    state.windows[&first].workspace,
                    state.windows[&second].workspace
                );
                assert!(state.set_layout_settings("tiling", 40, 12));
                client.dispatch(&mut display, &mut state);
                let border = state.decoration_theme().border_width;
                for (id, x) in [(first, 0), (second, 800)] {
                    assert_eq!(
                        state.space.element_location(&state.windows[&id].window),
                        Some((x + 40 + border, 40 + border).into())
                    );
                }
                let locations: Vec<_> = [first, second]
                    .into_iter()
                    .map(|id| state.space.element_location(&state.windows[&id].window))
                    .collect();
                let workspaces: Vec<_> = [first, second]
                    .into_iter()
                    .map(|id| state.windows[&id].workspace)
                    .collect();
                let previous = state.config.clone();
                let path = crate::config::user_config_path();
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
                assert!(!state.set_layout_settings("floating", 64, 24));
                let mut next = previous.clone();
                next.workspaces.count = 1;
                assert!(!state.set_configuration(&toml::to_string(&next).unwrap()));
                assert_eq!(state.config, previous);
                assert_eq!(
                    state.list_workspaces().len(),
                    previous.workspaces.count as usize
                );
                assert_eq!(
                    [first, second]
                        .into_iter()
                        .map(|id| state.space.element_location(&state.windows[&id].window))
                        .collect::<Vec<_>>(),
                    locations
                );
                assert_eq!(
                    [first, second]
                        .into_iter()
                        .map(|id| state.windows[&id].workspace)
                        .collect::<Vec<_>>(),
                    workspaces
                );
            },
        );
    }

    #[test]
    fn empty_dynamic_workspaces_prune_when_their_last_window_leaves() {
        isolated_config_test(
            concat!(
                module_path!(),
                "::empty_dynamic_workspaces_prune_when_their_last_window_leaves"
            ),
            |_| {
                let (mut display, _event_loop, mut state) = fixture();
                crate::test_support::add_output(&mut state, "test-0", (0, 0), (800, 600));
                let mut config = state.config.clone();
                config.workspaces.count = 1;
                config.workspaces.dynamic = true;
                state.apply_config(config).unwrap();
                let mut client = crate::test_support::TestClient::new(&mut display, &mut state);
                let (id, toplevel) = map_window(&mut client, &mut display, &mut state);
                assert_eq!(state.create_workspace(String::new()), 2);
                assert!(state.move_window_to_workspace(id, 2));
                toplevel.destroy();
                client.dispatch(&mut display, &mut state);
                assert_eq!(state.list_workspaces().len(), 1);
            },
        );
    }

    #[test]
    fn configured_shortcuts_share_the_configuration_commit_boundary() {
        isolated_config_test(
            concat!(
                module_path!(),
                "::configured_shortcuts_share_the_configuration_commit_boundary"
            ),
            |_| {
                let (_display, _event_loop, mut state) = fixture();
                let binding = ShortcutBinding {
                    accelerator: "Super+Return".into(),
                    command: ShortcutCommand::Execute,
                    argument: Some("true".into()),
                };
                assert!(state.set_configured_shortcuts(vec![binding]));
                assert_eq!(state.config.bindings.len(), 1);
                assert_eq!(read_configuration(), state.config);
                let previous = state.config.clone();
                let bindings = state.configured_shortcuts();
                let path = crate::config::user_config_path();
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
                assert!(!state.set_configured_shortcuts(Vec::new()));
                assert_eq!(state.config, previous);
                assert_eq!(state.configured_shortcuts(), bindings);
            },
        );
    }

    #[test]
    fn rejected_configuration_is_never_persisted() {
        isolated_config_test(
            concat!(
                module_path!(),
                "::rejected_configuration_is_never_persisted"
            ),
            |_| {
                let (_display, _event_loop, mut state) = fixture();
                assert!(state.set_layout_settings("floating", 16, 12));
                let previous = state.config.clone();
                let saved = fs::read(crate::config::user_config_path()).unwrap();
                assert!(!state.set_layout_settings("tiling", 129, 12));
                assert!(!state.set_window_settings(5, 8, 12, true));
                assert!(!state.set_configuration("not valid TOML"));
                let mut next = state.config.clone();
                next.input.keyboard.layout = "blair_missing_layout".into();
                assert!(!state.set_configuration(&toml::to_string(&next).unwrap()));
                assert_eq!(state.config, previous);
                assert_eq!(fs::read(crate::config::user_config_path()).unwrap(), saved);
            },
        );
    }
}
