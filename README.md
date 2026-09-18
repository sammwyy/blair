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
- Damage-tracked rendering with hardware cursor and overlay planes on DRM.
- GPU clients through `linux-dmabuf`, plus themed `xcursor` pointers.
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

### Layout

```toml
[window]
layout = "tiling" # floating | tiling
work_area_padding = 16
gap = 8
master_ratio = 0.55
```

Floating windows open centered and are moved and resized with the pointer.
Tiling gives the first window of a workspace a master column and stacks the
rest beside it, keeping `gap` between tiles and `work_area_padding` around
them; windows with a `floating = true` rule stay free even while tiling.

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

[cursor]
theme = "Adwaita"
size = 24
```

With no output entries, Blair uses a safe automatic profile. Current animations
fade window opens and workspace changes; curves are `linear`, `ease-in`,
`ease-out`, and `ease-in-out`. The cursor theme falls back to `XCURSOR_THEME`
and `XCURSOR_SIZE`, and whatever Blair resolves is exported to the programs it
starts so applications match the compositor.

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
| Rendering | GLES2, damage tracking, DRM planes, shader-based decorations |
| Integration | Transport-neutral `CompositorApi`, `Transport`, `EventChannel` |
| Default transport | Optional D-Bus (`integrations.dbus = true`) |

### Wayland protocols

`wl_compositor` (v6), `wl_subcompositor`, `wl_shm`, `wl_seat` (v9), `wl_output`,
`xdg_shell`, `xdg-decoration`, `xdg-activation`, `xdg-output`,
`wlr-layer-shell`, `linux-dmabuf` (v5, with feedback), `presentation-time`,
`viewporter`, `fractional-scale`, `cursor-shape`, `relative-pointer`,
`pointer-constraints`, `single-pixel-buffer`, `primary-selection`,
`wlr-data-control`, `wlr-screencopy`, `ext-foreign-toplevel-list`,
`ext-idle-notify`, `idle-inhibit`, `pointer-gestures`, `alpha-modifier`,
`keyboard-shortcuts-inhibit`, and KDE's `server-decoration`.

Screenshot and recording tools that speak `wlr-screencopy` (`grim`,
`wf-recorder`, the wlroots desktop portal) work out of the box; the D-Bus
`Screenshot` method writes a PNG directly for shells that prefer it.

### Rendering and pacing

Every repaint goes through damage tracking: only the regions that changed are
redrawn and submitted. The DRM backend composites with Smithay's
`DrmCompositor`, so the pointer lands on the hardware cursor plane and moves
without recomposing the screen, and unchanged clients can be scanned out
directly. Repaints are paced by the display: one per vblank, and nothing is
rendered while the screen is idle.

Frame timings are summarized in the log every five seconds at `debug` level
(`frame timings`), and the same numbers are available to integrations through
the `RenderStats` D-Bus method. Building with `--features profile-with-tracy`
streams detailed spans to a [Tracy](https://github.com/wolfpld/tracy) profiler.

D-Bus is a transport, not a compositor dependency. Additional integrations can
implement the same contracts. The D-Bus service is `org.blair.Compositor`, path
`/org/blair/Compositor`, interface `org.blair.Compositor1`; it exposes window
and workspace control, work areas, outputs, settings, shortcuts, and temporary
window rules. It also renders PNG screenshots (`Screenshot`) and reports frame
timings (`RenderStats`). `blair-client` is the async Rust wrapper.

Temporary shortcuts and rules belong to the calling D-Bus connection and are
removed automatically when it disconnects. Temporary rules use the same fields
as one `[[rules]]` item, without the array-table header.

The direct DRM backend currently activates one connector. State and rendering
are prepared for per-output workspaces; full multi-connector DRM activation is
the next backend step. Winit inherits display mode and VRR limits from its host.
Known gaps are tracked in [TODO.md](TODO.md).

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
