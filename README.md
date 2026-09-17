# Blair

Blair is a small, modern Wayland compositor for a desktop that feels yours.
It handles windows, workspaces, input, monitors, and session plumbing; you
choose the bar, wallpaper, launcher, notifications, and shell around it.

Blair is not a bundled desktop environment. It runs well on its own with a few
autostart entries. [Coconut](https://github.com/sammwyy/coconut) remains a
recommended optional shell for a more integrated desktop, but it is never
required to build, install, or run Blair.

## Highlights

- Native Wayland compositor with nested Winit and direct DRM/KMS backends.
- Per-output workspaces: switching on one monitor leaves the others alone.
- Layered TOML configuration with safe hot reload.
- Config bindings plus process-owned temporary bindings and window rules.
- Configurable output, input, focus, decorations, autostart, and animations.
- Optional D-Bus integration for shells, panels, launchers, and tools.

## Start simple

Create `~/.config/blair/config.toml`:

```toml
[[autostart]]
command = "waybar"
restart = true

[[autostart]]
command = "swaybg -i ~/Pictures/wallpaper.png"

[[autostart]]
command = "mako"
restart = true

[[bindings]]
keys = ["SUPER", "RETURN"]
exec = "foot"

[[bindings]]
keys = ["SUPER", "Q"]
action = "close"
```

No shell or “main client” is needed. Add a one-off command only for the
current session with `blair --run foot`; repeat `--run` when needed.

## Make it yours

Every setting is optional. Defaults are intentionally familiar: click-to-focus,
new windows focused, ten workspaces, safe automatic monitor selection, and
animations enabled.

### Startup

```toml
[[autostart]]
command = "waybar"
restart = true
```

`restart` defaults to `false`. Enabled commands restart one second after they
exit, avoiding a tight loop if a command is broken.

### Key bindings and workspaces

```toml
[[bindings]]
keys = ["SUPER", "1"]
action = "workspace"
value = 1

[[bindings]]
keys = ["SUPER", "SHIFT", "1"]
action = "move-to-workspace"
value = 1

[workspaces]
count = 10
dynamic = false
wrap = true

[workspaces."1"]
name = "dev"
output = "DP-1"
```

Workspaces are global identities, but every output displays a different one.
When a target is already visible elsewhere, Blair swaps the outputs.

### Windows and decorations

```toml
[[rules]]
app_id = "pavucontrol"
floating = true
size = [700, 500]

[[rules]]
title = "Picture-in-Picture"
floating = true
always_on_top = true

[decorations]
mode = "auto" # server | client | auto | none
border_width = 2
corner_radius = 8
titlebar_height = 28

[decorations.buttons]
layout = ["minimize", "maximize", "close"]
side = "right"
```

Rules match `app_id`, `title`, or `regex` today and can set floating/tiled,
workspace, output, size, position, opacity, always-on-top, and decoration.
`class`, `role`, and `type` are reserved for Xwayland support.

### Monitors, input, focus, and motion

```toml
[outputs."DP-1"]
enabled = true
mode = "2560x1440@165"
position = [0, 0]
scale = 1.0
transform = "normal"
vrr = true

[input.touchpad]
tap = true
natural_scroll = true
disable_while_typing = true

[focus]
policy = "click"
raise_on_focus = true
focus_new_windows = true
focus_previous_on_close = true
warp_cursor = false

[animations]
enabled = true

[animations.window_open]
duration = 150
curve = "ease-out"

[animations.workspace]
duration = 200
curve = "ease-in-out"
```

With no output entries, Blair uses a safe automatic profile. Current animations
fade window opens and workspace changes; curves are `linear`, `ease-in`,
`ease-out`, and `ease-in-out`.

## Install and run

```bash
./scripts/install.sh
```

The installer builds Blair, installs `blair` and `blair-session` under
`/usr/local/bin`, and registers a Wayland session with your display manager.
Choose “Blair” in its session picker.

For nested development in an existing desktop:

```bash
RUST_LOG=debug cargo run -p blair
```

## Configuration model

Later TOML layers override earlier ones:

```text
built-in defaults
/etc/blair/config.toml
/etc/blair/conf.d/*.toml
~/.config/blair/config.toml
~/.config/blair/conf.d/*.toml
```

One `config.toml` is enough; `conf.d` is optional organization. Existing paths
hot reload by default after a short debounce. Blair parses and validates all
layers before applying anything, so a bad edit keeps the previous configuration.
Set `general.hot_reload = false` and restart to disable it.

See [the bundled example](packaging/config/config.toml) for the current full
schema. Output/input/workspace topology and autostart changes apply on the
next session; bindings, focus, decorations, new-window rules, and animations
can hot reload safely.

---

## Technical reference

### Architecture

| Area | Implementation |
|---|---|
| Compositor | Smithay, XDG shell/decoration, layer-shell, clipboard/DnD |
| Backends | Winit nested development; DRM/KMS + libinput direct sessions |
| Integration | Transport-neutral `CompositorApi`, `Transport`, `EventChannel` |
| Default transport | Optional D-Bus (`integrations.dbus = true`) |

D-Bus is a transport, not a compositor dependency. Additional integrations can
implement the same contracts. The D-Bus service is `org.blair.Compositor`, path
`/org/blair/Compositor`, interface `org.blair.Compositor1`; it exposes window
and workspace control, work areas, outputs, settings, shortcuts, and temporary
window rules. `blair-client` is the async Rust wrapper.

Temporary shortcuts and rules belong to the calling D-Bus connection and are
removed automatically when it disconnects. Temporary rules use the same fields
as one `[[rules]]` item, without the array-table header.

The direct DRM backend currently activates one connector. State and rendering
are prepared for per-output workspaces; full multi-connector DRM activation is
the next backend step. Winit inherits display mode and VRR limits from its host.

### Build

- Rust stable, `rustfmt`, and `clippy`
- Wayland or X11 for nested development
- logind/seatd, DRM/KMS, libinput, and Mesa/EGL for direct sessions

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

### Repository layout

| Path | Role |
|---|---|
| `apps/compositor/` | Binary, state, rendering, config, and backends |
| `crates/protocol/` | Transport-independent types and events |
| `crates/integration/` | Integration contracts |
| `integrations/blair-integration-dbus/` | D-Bus transport |
| `crates/client/` | Async Rust client |

## Recovery

If a session becomes unresponsive, switch to a TTY (`Ctrl+Alt+F2`) and run
`pkill -9 blair`. Session logs live at
`$XDG_STATE_HOME/blair/compositor.log` (or `~/.local/state/blair`).

## License

Blair is distributed under the [Apache License 2.0](LICENSE).
