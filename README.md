# Blair

Blair is a small Wayland compositor built with Smithay. It manages windows,
input, outputs, and exposes desktop integration over D-Bus. It has no bundled
bar, launcher, or shell.

Any desktop shell can use the D-Bus interface for window lists, focus,
work-area reservations, and global shortcuts. [Coconut](https://github.com/sammwyy/coconut)
is a recommended shell for Blair, but it is a separate project.

## Workspace layout

| Path | Package | Role |
|---|---|---|
| `apps/compositor/` | `blair` | The compositor binary: Smithay backends (winit, DRM/KMS), window/seat/output state, server-side decoration fallback, D-Bus service glue |
| `crates/protocol/` | `blair-protocol` | Wire-independent domain types: `WindowId`, `Rect`, `WindowInfo`, `CompositorEvent` |
| `crates/dbus/` | `blair-dbus` | The `org.blair.Compositor1` D-Bus interface: server-side (`CompositorInterface`) and the generated client proxy (`CompositorProxy`) |
| `crates/client/` | `blair-client` | Ergonomic async client (`BlairClient`) wrapping `blair-dbus` for shell processes to depend on directly |

`apps/compositor` depends on `blair-protocol` and `blair-dbus`; desktop shells
can use `blair-client` without depending on the compositor crate.

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
| `BindShortcut(s id, s accelerator) -> b` | accelerator syntax: `"Super+Space"`, `"Ctrl+Alt+T"` |
| `UnbindShortcut(s id)` | |
| `Quit()` | |

Signals: `WindowOpened`, `WindowClosed`, `WindowFocused`, `FocusCleared`,
`WindowTitleChanged`, `WindowGeometryChanged`, `WindowMinimized`,
`WindowRestored`, `WindowMaximized`, `OutputAdded`, `OutputRemoved`,
`WorkAreaChanged`, `ShortcutActivated`.

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

## Install

```bash
./scripts/install.sh
```

Builds `blair`, installs the binary and `blair-session` wrapper into
`/usr/local/bin`, and registers `/usr/share/wayland-sessions/blair.desktop`.

## Config

```
~/.config/blair/compositor.toml     (created with defaults on first run)
packaging/config/compositor.toml    (reference)
```

Each user gets their own config, created the first time `blair` runs for
them. There is no system-wide config file — Blair never reads `/etc`.

See `packaging/config/compositor.toml` for backend selection, the primary
client command, window sizing, and server-side decoration fallback settings.

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
