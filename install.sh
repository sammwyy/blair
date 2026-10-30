#!/bin/bash
# Blair installer: lays down the release binary staged by ./build.sh in
# .build/ plus the blair-session launcher and a "Blair" wayland-sessions
# entry so greeters and session managers can discover it.
#
# Usage: ./install.sh [--build] [--user]
#
#   --build   run ./build.sh first to compile and stage the binaries.
#   --user    install for the current user (~/.local) instead of
#             system-wide; without it the script elevates with sudo.
set -e
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="${BLAIR_BUILD_DIR:-$ROOT/.build}"

# -------------------------------------------------------------- arguments

DO_BUILD=0
USER_INSTALL=0
for arg in "$@"; do
    case "$arg" in
        --build) DO_BUILD=1 ;;
        --user) USER_INSTALL=1 ;;
        -h | --help)
            sed -n '6,11p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "install.sh: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

# Build as the invoking user, before any sudo elevation, so target/ stays
# owned by them and sudo never touches the toolchain.
if [ "$DO_BUILD" -eq 1 ] && [ "${BLAIR_INSTALL_BUILT:-}" != 1 ]; then
    bash "$ROOT/build.sh"
fi

# Global install is the default: elevate with sudo unless --user was given.
if [ "$USER_INSTALL" -eq 0 ] && [ "$(id -u)" -ne 0 ]; then
    if ! command -v sudo >/dev/null 2>&1; then
        echo "install.sh: a system-wide install needs root; install sudo or pass --user" >&2
        exit 1
    fi
    exec sudo BLAIR_INSTALL_BUILT=1 bash "$ROOT/install.sh" "$@"
fi

# ---------------------------------------------------------------- binaries

for bin in blair blair-session; do
    if [ ! -f "$BUILD_DIR/$bin" ]; then
        echo "install.sh: staged file missing: $bin" >&2
        echo "Run ./build.sh first (as a regular user), then re-run this script." >&2
        exit 1
    fi
done

# ------------------------------------------------------------ install root

if [ "$(id -u)" -eq 0 ]; then
    INSTALL_BIN=/usr/bin
    DATA=/usr/share
else
    INSTALL_BIN="${BLAIR_INSTALL_BIN:-$HOME/.local/bin}"
    DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
fi

SESSIONS_DIR="${BLAIR_SESSIONS_DIR:-$DATA/wayland-sessions}"

mkdir -p "$INSTALL_BIN" "$SESSIONS_DIR" "$DATA/blair"

install -m 755 "$BUILD_DIR/blair" "$INSTALL_BIN/blair"
install -m 755 "$BUILD_DIR/blair-session" "$INSTALL_BIN/blair-session"

# ------------------------------------------------------- session + config

sed "s|Exec=blair-session|Exec=$INSTALL_BIN/blair-session|" \
    "$ROOT/packaging/sessions/blair.desktop" > "$SESSIONS_DIR/blair.desktop"

# Reference configuration; Blair runs on compiled-in defaults without one.
# Copy it to ~/.config/blair/config.toml to customize.
install -m 644 "$ROOT/packaging/config/config.toml" "$DATA/blair/config.toml"

echo "Blair session installed:"
echo "  compositor   : $INSTALL_BIN/blair"
echo "  launcher     : $INSTALL_BIN/blair-session"
echo "  session entry: $SESSIONS_DIR/blair.desktop"
echo "  sample config: $DATA/blair/config.toml"
if [ "$SESSIONS_DIR" != "/usr/share/wayland-sessions" ]; then
    echo "  note: system greeters may need it in /usr/share/wayland-sessions/"
    echo "        (re-run this script with sudo for a system-wide install)"
fi
