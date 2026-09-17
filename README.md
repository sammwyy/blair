# Blair

Blair is a small Wayland compositor built with Smithay. It manages windows,
input, outputs, and exposes desktop integration through optional transports.
It has no bundled
bar, launcher, or shell.

Any desktop shell can use the built-in D-Bus transport for window lists, focus,
work-area reservations, and global shortcuts. [Coconut](https://github.com/sammwyy/coconut)
is a recommended shell for Blair, but it is a separate project.

## Workspace layout

| Path | Package | Role |
|---|---|---|
| `apps/compositor/` | `blair` | The compositor binary: Smithay backends (winit, DRM/KMS), window/seat/output state, and integration composition |
| `crates/protocol/` | `blair-protocol` | Wire-independent domain types: `WindowId`, `Rect`, `WindowInfo`, `CompositorEvent` |
| `crates/integration/` | `blair-integration` | Transport-neutral `CompositorApi`, `Transport`, and `EventChannel` contracts |
| `integrations/blair-integration-dbus/` | `blair-integration-dbus` | The optional `org.blair.Compositor1` D-Bus transport and generated proxy |
| `crates/client/` | `blair-client` | Ergonomic async client (`BlairClient`) wrapping the D-Bus integration |

`apps/compositor` depends only on the transport-neutral contracts plus its enabled
integrations. Desktop shells can use `blair-client` without depending on the
compositor crate.

## Protocols

- **`xdg_shell`**, **`xdg_decoration`**, **`wl_data_device`** — standard
  window management and clipboard/DnD.
- **`wlr_layer_shell`** — panels, docks, wallpapers, and overlays map as layer
  surfaces and reserve screen space through `exclusive_zone`. Blair publishes
  the remaining work area as `WorkAreaChanged` on D-Bus.

## D-Bus interface (`org.blair.Compositor1`)

Service `org.blair.Compositor`, object path `/org/blair/Compositor`, on the
session bus.

Methods:

| Method | Description |
|---|---|
| `ListWindows() -> a(tssiiiibbb)` | id, title, app_id, x, y, width, height, focused, minimized, maximized |
| `FocusWindow(t id) -> b` | |
| `CloseWindow(t id) -> b` | |
| `MinimizeWindow(t id) -> b` | |
| `ToggleMaximizeWindow(t id) -> b` | |
| `MoveResizeWindow(t id, i x, i y, i width, i height) -> b` | |
| `WorkArea(s output) -> (iiii)` | non-exclusive area of `output`; empty string = first output |
| `Outputs() -> as` | |
| `WindowSettings() -> (iiib)` | titlebar height, border width, corner radius, server-side decorations |
| `SetWindowSettings(i titlebar_height, i border_width, i corner_radius, b server_side_decorations) -> b` | applies and persists validated window settings |
| `BindShortcut(s id, s accelerator) -> b` | accepts modifier-only shortcuts and one to three additional keys, e.g. `"Super"`, `"Super+Space"`, `"Ctrl+Alt+Q+W+E"` |
| `UnbindShortcut(s id)` | |
| `Quit()` | |

Signals: `WindowOpened`, `WindowClosed`, `WindowFocused`, `FocusCleared`,
`WindowTitleChanged`, `WindowGeometryChanged`, `WindowMinimized`,
`WindowRestored`, `WindowMaximized`, `OutputAdded`, `OutputRemoved`,
`WorkAreaChanged`, `ShortcutActivated`.

Shortcut registrations belong to the caller's D-Bus connection. Reusing an ID
updates only that caller's registration; equal shortcuts from different clients
are all delivered to their respective live clients. Blair removes a client's
registrations when its D-Bus connection disappears.

`blair-client` wraps all of this behind `BlairClient` and a single merged
`Events` stream:

```rust
let client = blair_client::BlairClient::connect().await?;
let mut events = client.events().await?;
client.bind_shortcut("launcher", "Super+Space").await?;

while let Some(event) = events.next().await {
    // blair_protocol::CompositorEvent
}
```

## Requirements

- Rust stable, including `rustfmt` and `clippy`
- A Wayland or X11 session for nested development
- A logind or seatd session, DRM/KMS, libinput, and Mesa/EGL for direct DRM use

## Build and verify

```bash
cargo build --workspace
cargo check --workspace
cargo clippy --workspace --all-targets
```

## Run (nested, for development)

From an existing Wayland or X11 session:

```bash
RUST_LOG=debug cargo run -p blair
```

Blair opens a nested window. Its default configuration starts `coconut` as the
primary client; set `spawn_primary_client = false` or choose another command
to use a different shell. `scripts/dev.sh` runs the freshly built binary.

## Shell selection

Use `--primary-client` to override `config.toml` for one session. The
override always starts the supplied command.

```bash
blair --primary-client coconut
blair-session --primary-client "another-shell --config ~/.config/another-shell.toml"
```

This makes separate display-manager entries possible, for example:

```ini
Exec=blair-session --primary-client coconut
```

## Install

```bash
./scripts/install.sh
```

Builds `blair`, installs the binary and `blair-session` wrapper into
`/usr/local/bin`, and registers `/usr/share/wayland-sessions/blair.desktop`.

## Config

Configuration is TOML and is assembled in this order; later layers override
earlier ones:

```
built-in defaults
/etc/blair/config.toml
/etc/blair/conf.d/*.toml
~/.config/blair/config.toml
~/.config/blair/conf.d/*.toml
```

Fragment files are read in lexicographic order, so numeric prefixes such as
`10-input.toml` and `20-outputs.toml` give stable local precedence. Every
file uses the same schema, and a user may simply keep all settings in
`~/.config/blair/config.toml`; `conf.d` is optional.

Blair hot-reloads these existing paths by default, after a 100 ms debounce.
Set `general.hot_reload = false` and restart to disable it. Reloads parse and
validate every layer before applying the configuration; an invalid edit is
logged and the last valid configuration remains active. Backend changes still
require a compositor restart.

## Key bindings

Permanent bindings live in configuration and share the physical shortcut
registry with temporary, process-owned bindings. They are reapplied on a
successful config reload:

```toml
[[bindings]]
keys = ["SUPER", "Q"]
action = "close"

[[bindings]]
keys = ["SUPER", "RETURN"]
exec = "foot"

[[bindings]]
keys = ["SUPER", "1"]
action = "workspace"
value = 1

[[bindings]]
keys = ["SUPER", "SHIFT", "1"]
action = "move-to-workspace"
value = 1
```

Supported actions are `close`, `workspace`, and `move-to-workspace`.
The latter two require a positive integer `value`; missing numbered
workspaces are created on first use. `exec` runs its command via `sh -c`.

## Outputs

With no `[outputs."NAME"]` entries, Blair uses the safe automatic profile:
every detected output stays enabled with its preferred mode. Output names are
DRM connector names such as `DP-1` and `HDMI-A-1`.

```toml
[outputs."DP-1"]
enabled = true
mode = "2560x1440@165"
position = [0, 0]
scale = 1.0
transform = "normal"
vrr = true

[outputs."HDMI-A-1"]
position = [2560, 0]
scale = 1.0
```

`mode` requires `WIDTHxHEIGHT@REFRESH`; transforms are `normal`, `90`,
`180`, `270`, and the `flipped` variants. Blair refuses to disable the
last available output, falling back to its detected preferred mode. Output
profiles currently apply at startup; changing them requires a restart.

The current DRM backend drives one connector, so it applies the matching
profile for its selected connector; multi-output activation is the next
backend step. The nested Winit backend applies position and scale and
advertises the requested transform, but cannot change the mode or VRR selected
by its host window.

The built-in D-Bus transport is enabled by default with
`integrations.dbus = true`. Set it to `false` and restart to run with no request
transport; additional transports can implement the transport-neutral
`blair-integration` contracts.

## Input

    [input.keyboard]
    layout = "us"
    variant = ""
    repeat_delay = 250
    repeat_rate = 35

    [input.mouse]
    sensitivity = 0.0
    acceleration = "adaptive" # or "flat"

    [input.touchpad]
    tap = true
    natural_scroll = true
    disable_while_typing = true

Keyboard layout and repeat are applied through XKB. On DRM, mouse acceleration
and sensitivity plus touchpad tap, natural scrolling, and disable-while-typing
are applied through libinput when each device appears; unsupported options are
ignored per-device. The nested backend receives its pointer configuration from
its host compositor. Input configuration changes require a restart.

## Persistent workspaces

    [workspaces]
    count = 10
    dynamic = false
    wrap = true

    [workspaces."1"]
    name = "dev"
    output = "DP-1"

    [workspaces."2"]
    name = "web"

Workspaces 1 through count are created at startup and remain available for the
whole session. With dynamic disabled, CreateWorkspace through an integration
returns 0 and does not alter the declared topology. An output assignment is a
preferred target for new windows in that workspace, with a safe fallback if
the named connector is unavailable. Workspace topology changes require a
restart.

## Workspaces

Blair starts with workspace `1`. The D-Bus interface
`org.blair.Compositor1` provides `CreateWorkspace(name)`,
`ListWorkspaces()`, `SwitchWorkspace(id)`, and
`MoveWindowToWorkspace(window_id, workspace_id)`. `ListWindows()` and all
window/focus operations are scoped to the active workspace.

See `packaging/config/config.toml` for backend selection, the primary client
command, window sizing, and server-side decoration settings. Set
`decoration.corner_radius` to control rounded server-side window frames.

## Session files

```
packaging/sessions/blair-session   — sets env vars, execs blair
packaging/sessions/blair.desktop   — Wayland session entry for display managers
```

## Emergency recovery

If the compositor freezes inside a session, switch to a TTY
(`Ctrl+Alt+F2`) and run:

```bash
pkill -9 blair
```

View logs:

```bash
journalctl -b | grep -i blair
RUST_LOG=blair=debug cargo run -p blair
```

## License

Blair is distributed under the [Apache License 2.0](LICENSE).
