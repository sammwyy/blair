# Blair

> A small, modern Wayland compositor that leaves the desktop yours.

> [!WARNING]
> Blair is under active development and is not yet intended for production or
> daily use.

Blair handles windows, workspaces, input, outputs, and session plumbing while
leaving the rest to you. Choose your own bar, wallpaper, launcher,
notifications, and shell. [Coconut](https://github.com/sammwyy/coconut) is an
optional companion shell, never a requirement.

## Highlights

- Native Wayland compositor with nested Winit and direct DRM/KMS backends
- Damage-tracked rendering, hardware cursors, and DRM overlay planes
- Per-output workspaces and configurable tiling or floating layouts
- Layered TOML configuration with safe hot reload
- Configurable outputs, input, focus, decorations, autostart, and animations
- D-Bus APIs for shells, panels, launchers, and external tools
- Modern Wayland protocols including layer-shell, dmabuf, screencopy, and data
  control

## Quick start

Create `~/.config/blair/config.toml`:

```toml
[[autostart]]
command = "waybar"
restart = true

[[autostart]]
command = "swaybg -i ~/Pictures/wallpaper.png"

[[autostart]]
command = "mako"

[[bindings]]
keys = ["SUPER", "RETURN"]
exec = "foot"

[[bindings]]
keys = ["SUPER", "Q"]
action = "close"
```

No shell or main client is required. Use `blair --run foot` to start a one-off
application for the current session; repeat `--run` when needed.

## Configuration

Every setting is optional. Defaults provide click-to-focus, focused new
windows, ten workspaces, automatic output selection, and animations.

### Workspaces and layout

```toml
[[bindings]]
keys = ["SUPER", "1"]
action = "workspace"
value = 1

[workspaces]
count = 10
dynamic = false
wrap = true

[window]
layout = "tiling" # floating | tiling
work_area_padding = 16
gap = 8
master_ratio = 0.55
```

Workspaces are global identities, while each output displays a different one.
Floating windows open centered; tiling assigns the first window a master column
and stacks the rest beside it.

### Rules and appearance

```toml
[[rules]]
app_id = "pavucontrol"
floating = true
size = [700, 500]

[decorations]
mode = "auto" # server | client | auto | none
border_width = 2
corner_radius = 8
titlebar_height = 28
```

Rules match `app_id`, `title`, or `regex` and can set layout, workspace, output,
size, position, opacity, stacking, and decorations.

### Outputs, input, and motion

```toml
[outputs."DP-1"]
enabled = true
mode = "2560x1440@165"
position = [0, 0]
scale = 1.0
vrr = true

[input.touchpad]
tap = true
natural_scroll = true
disable_while_typing = true

[animations]
enabled = true
```

Without explicit output entries, Blair uses a safe automatic profile. Cursor
settings fall back to `XCURSOR_THEME` and `XCURSOR_SIZE` and are passed to
applications Blair starts.

## Development

[DEVELOPMENT.md](DEVELOPMENT.md) covers the required checkout layout, native
dependencies for Arch, Debian/Ubuntu, Fedora/RHEL, and openSUSE, the distinction
between libseat and seatd, testing, and local installation.

For a nested local session:

```bash
RUST_LOG=blair=debug,warn cargo run -p blair
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

Blair validates every layer before applying it, so an invalid edit preserves the
previous configuration. See the [bundled example](packaging/config/config.toml)
for the full schema.

## Architecture

| Area | Implementation |
| --- | --- |
| Compositor | Smithay, XDG shell/decoration, layer-shell, clipboard and DnD |
| Backends | Winit nested development; DRM/KMS, libinput, and libseat direct sessions |
| Rendering | GLES2, damage tracking, DRM planes, shader-based decorations |
| Integration | Transport-neutral `CompositorApi`, `Transport`, and `EventChannel` |
| Default transport | Optional D-Bus (`integrations.dbus = true`) |

Blair supports the core Wayland protocols plus xdg-shell, xdg-decoration,
layer-shell, linux-dmabuf, presentation-time, fractional-scale, screencopy,
data control, and KDE server decoration. Tools that use `wlr-screencopy`, such
as `grim` and `wf-recorder`, work out of the box.

The D-Bus service is `org.blair.Compositor` at `/org/blair/Compositor`, using
the `org.blair.Compositor1` interface. It exposes window and workspace control,
outputs, settings, shortcuts, screenshots, and render statistics.

## Recovery

If a session becomes unresponsive, switch to a TTY with `Ctrl+Alt+F2` and run:

```bash
pkill -9 blair
```

Session logs live in `$XDG_STATE_HOME/blair/compositor.log` or, by default,
`~/.local/state/blair`.

## License

Blair is distributed under the [Apache License 2.0](LICENSE).
