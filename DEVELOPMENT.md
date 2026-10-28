# Developing Blair

Blair is a Rust Wayland compositor built with Smithay. This guide covers its native libraries, development workflow, and local installation.

## Requirements

Use a current stable Rust toolchain. Smithay 0.7 requires Rust 1.80.1 or newer. Blair uses local path dependencies from CreamUI, so keep both repositories next to each other:

```text
workspace/
├── blair/
└── creamui/
```

If you use another layout, update the CreamUI paths in `Cargo.toml`.

### libseat and seatd

Blair's direct DRM/KMS backend is built against **libseat**, so its development package is required to compile. At runtime, libseat needs a seat-management provider: it can use logind on systemd systems, or **seatd** elsewhere. Installing `seatd` alone does not replace the libseat development package where they are packaged separately.

## Install system dependencies

### Arch Linux

```bash
sudo pacman -Syu --needed base-devel rustup pkgconf libseat seatd libinput \
  libdrm mesa wayland libxkbcommon systemd
rustup default stable
```

`seatd` is optional when logind is available, but useful on minimal or non-systemd installations.

### Debian and Ubuntu

```bash
sudo apt update
sudo apt install build-essential rustup pkg-config libseat-dev seatd \
  libinput-dev libudev-dev libdrm-dev libgbm-dev libegl1-mesa-dev \
  libgles2-mesa-dev libwayland-dev libxkbcommon-dev
rustup default stable
```

If `rustup` is unavailable through APT, use the [official Rust installer](https://rustup.rs/).

### Fedora and RHEL-compatible distributions

```bash
sudo dnf install @development-tools rust cargo pkgconf-pkg-config libseat-devel \
  seatd libinput-devel systemd-devel libdrm-devel mesa-libgbm-devel \
  mesa-libEGL-devel mesa-libGLES-devel wayland-devel libxkbcommon-devel
```

Enable the appropriate development repositories if a `*-devel` package is unavailable. Use [rustup](https://rustup.rs/) if the packaged Rust is too old.

### openSUSE

```bash
sudo zypper install -t pattern devel_basis
sudo zypper install rustup pkg-config libseat-devel seatd libinput-devel \
  libudev-devel libdrm-devel Mesa-libgbm-devel Mesa-libEGL-devel \
  Mesa-libGLESv2-devel wayland-devel libxkbcommon-devel
rustup default stable
```

For another distribution, install equivalents for the compiler toolchain, `pkg-config`, libseat, libinput, udev, DRM/GBM, Mesa EGL/GLES, Wayland, xkbcommon, and a seat provider (logind or seatd).

## Build and run

```bash
cargo build --workspace
```

For the safest development loop, run Blair nested inside an existing Wayland or X11 session:

```bash
RUST_LOG=blair=debug,warn cargo run -p blair
```

To test direct DRM/KMS, launch `blair` from a TTY only after confirming your seat provider works. It takes control of the active graphics seat; do not run it inside another compositor session.

## Verify changes

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

Run the offscreen GPU tests on a machine with an EGL device and OpenGL ES:

```bash
cargo test --workspace -- --include-ignored
```

These tests share an `EglTest` guard to serialize driver setup, rendering,
and teardown while other tests run in parallel. Declare the guard before
fixtures and GPU resources, and create renderers through `guard.renderer()`.
Keep multiple renderers within the same guard when testing context changes.

See [PERFORMANCE.md](PERFORMANCE.md) for release-build memory sampling.

## D-Bus queue limits

The compositor admits at most 128 pending D-Bus commands and 8 MiB of retained
command storage, including string/vector capacities. A command may retain up
to 1 MiB. Saturation and oversized commands return
`org.freedesktop.DBus.Error.LimitsExceeded`; stopped compositor receivers return
`org.freedesktop.DBus.Error.Failed`. Invalid settings still return `false`.

The service dispatches methods sequentially so waiting replies cannot create
an unbounded task backlog. Interface methods must not make dependent D-Bus
calls through that connection. Cleanup, theme, and sleep monitors await queue
capacity without blocking the compositor. Command batches yield after 16
requests or 2 ms, then wake the event loop again; an individual command can
exceed the time slice while executing.

The server event queue and each client subscription retain at most 512 events
and 8 MiB of accounted event storage, including string capacities. Each event
may retain up to 1 MiB. Queue bookkeeping, D-Bus transport buffers, and the
server's single in-flight signal are separate. Title, app-ID, geometry, and
work-area updates coalesce between lifecycle barriers; opening, closing, focus,
and visibility changes preserve their order.

Server overflow releases queued events and sends `EventsReset` before emitting
new events. `Events::next_result()` reports `EventStreamError::Reset`,
`Overflow`, or `Disconnected` and ends that subscription. The legacy `next()`
returns `None`; `error()` retains the reason. Subscribe again before refreshing
window, workspace, output, and configuration snapshots. Raw signal consumers
must also handle `EventsReset`. Subscriptions accept signals from their initial
service owner and end when ownership changes.

The D-Bus integration tests launch isolated `dbus-daemon` session buses and
terminate them on completion. They require `dbus-daemon` on `PATH` and do not
change the desktop session's bus address.

## Local installation

```bash
cargo build --release -p blair
sudo install -Dm0755 target/release/blair /usr/local/bin/blair
sudo install -Dm0755 packaging/sessions/blair-session /usr/local/bin/blair-session
sudo install -Dm0644 packaging/sessions/blair.desktop \
  /usr/share/wayland-sessions/blair.desktop
```

Choose **Blair** in the display manager's session picker, or launch `blair-session` from a TTY. User configuration lives at `~/.config/blair/config.toml`; see the [bundled example](packaging/config/config.toml).

## Troubleshooting

If Cargo cannot find `libseat`, install the libseat development package and confirm its metadata is visible:

```bash
pkg-config --modversion libseat
```

If a direct session cannot acquire a seat, ensure logind is running or start the `seatd` service. Your user must be permitted to access the seat; consult your distribution's seatd documentation for its group or service setup.
