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

Blair opens a nested window and does not require a shell or main client.
`scripts/dev.sh` runs the freshly built binary.

## Startup and autostart

Start desktop components declaratively:

```toml
[[autostart]]
command = "waybar"
restart = true

[[autostart]]
command = "swaybg -i ~/wallpaper.png"

[[autostart]]
command = "mako"
restart = true
```

`restart` defaults to `false`. When enabled, Blair restarts the process after
it exits, with a one-second retry delay. Autostart changes take effect on the
next compositor start. Add an extra one-off command without changing files:

```bash
blair --run foot
blair --run "waybar -c ~/.config/waybar/dev.json" --run mako
```

## Focus

```toml
[focus]
policy = "click"
raise_on_focus = true
focus_new_windows = true
focus_previous_on_close = true
warp_cursor = false
```

The default is click-to-focus. Blair can raise a window as it receives focus,
focus new windows, and restore the most recently focused surviving window when
the focused one closes. Cursor warping remains disabled by default and is not
performed implicitly.

## Animations

```toml
[animations]
enabled = true

[animations.window_open]
duration = 150
curve = "ease-out"

[animations.workspace]
duration = 200
curve = "ease-in-out"
```

`linear`, `ease-in`, `ease-out`, and `ease-in-out` are supported. Window-open
and workspace transitions currently animate with a compositor-side fade;
disable all animation with `animations.enabled = false`. The schema also
reserves `animations.window_close` and `animations.minimize` for future
surface-snapshot transitions.

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

## Window rules

Rules are persistent TOML entries, evaluated in order when a new window is
created. Later matching rules override only the actions they specify:

```toml
[[rules]]
app_id = "pavucontrol"
floating = true
size = [700, 500]

[[rules]]
app_id = "firefox"
workspace = "web"

[[rules]]
title = "Picture-in-Picture"
floating = true
always_on_top = true
```

Exact match fields are `app_id`, `title`, `class`, `role`, and `type`; `regex`
matches either title or app ID. Native Wayland XDG windows currently expose
only app ID and title, so class/role/type are reserved for the future Xwayland
backend. Actions are `floating`, `tiled`, `workspace`, `output`, `size`,
`position`, `opacity`, `always_on_top`, and `decoration`. Workspace accepts an
ID or its declared name. Opacity includes client surfaces and their popups.

Integrations may register a temporary rule with the same schema. D-Bus exposes
`RegisterWindowRule(id, rule_toml)` and `UnregisterWindowRule(id)`; the TOML is
one rule table without `[[rules]]`. Temporary rules are owned by the calling
bus client, are applied after persistent rules, and are removed automatically
when that client disconnects. Rules affect subsequently created windows.

## Decorations

```toml
[decorations]
mode = "auto" # server | client | auto | none
border_width = 2
corner_radius = 8
titlebar_height = 28

[decorations.buttons]
layout = ["minimize", "maximize", "close"]
side = "right" # left | right
```

`server` forces Blair's frame, `client` requests client-side decoration,
`auto` honors the XDG-decoration negotiation, and `none` never draws a Blair
frame. Button layout controls both drawing and hit-testing. The former singular
`[decoration]` table remains accepted as a compatibility alias.

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
returns 0 and does not alter the declared topology. An output assignment seeds
which workspace that connector shows when it appears; there is always a safe
automatic assignment when the connector is unavailable. Workspace topology
changes require a restart.

## Workspaces

Workspaces are global identities, but every output displays its own workspace.
Windows belong to exactly one workspace and are rendered only on the output
displaying it. Switching changes the workspace on the focused output; the
other outputs remain unchanged. If the target is already visible on another
output, Blair swaps the two assignments so a workspace is never shown twice.

Blair starts with workspace `1`. The D-Bus interface
`org.blair.Compositor1` provides `CreateWorkspace(name)`,
`ListWorkspaces()`, `SwitchWorkspace(id)`, and
`MoveWindowToWorkspace(window_id, workspace_id)`. `ListWindows()` and all
window/focus operations are scoped to the workspace shown on the focused
output. `ListWorkspaces()` includes the output currently showing each active
workspace.

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
