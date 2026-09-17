#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SESSION_FILE="$ROOT_DIR/packaging/sessions/blair-session"
DESKTOP_FILE="$ROOT_DIR/packaging/sessions/blair.desktop"

INSTALL_PREFIX="${INSTALL_PREFIX:-/usr/local}"
BIN_DIR="$INSTALL_PREFIX/bin"
WAYLAND_SESSIONS_DIR="${WAYLAND_SESSIONS_DIR:-/usr/share/wayland-sessions}"

die()  { printf 'error: %s\n' "$*" >&2; exit 1; }
info() { printf '==> %s\n' "$*"; }

command_exists()  { command -v "$1" >/dev/null 2>&1; }

unit_exists() {
  command_exists systemctl && systemctl list-unit-files "$1" --no-legend 2>/dev/null | grep -q .
}

sudo_cmd() {
  if [ "${EUID:-$(id -u)}" -eq 0 ]; then
    "$@"
  else
    sudo "$@"
  fi
}

choose_from_menu() {
  local prompt="$1"; shift
  local options=("$@")
  [ "${#options[@]}" -gt 0 ] || die "no options for: $prompt"
  printf '\n%s\n' "$prompt" >&2
  local i=1
  for opt in "${options[@]}"; do
    printf '  %d) %s\n' "$i" "$opt" >&2
    i=$((i + 1))
  done
  local choice
  while true; do
    printf '> ' >&2
    if ! read -r choice; then
      printf '\n' >&2
      die "no input received; run from an interactive terminal"
    fi
    if [[ "$choice" =~ ^[0-9]+$ ]] && [ "$choice" -ge 1 ] && [ "$choice" -le "${#options[@]}" ]; then
      printf '%s\n' "${options[$((choice - 1))]}"
      return 0
    fi
    printf 'Invalid selection. Choose 1-%d.\n' "${#options[@]}" >&2
  done
}

detect_session_managers() {
  SESSION_MANAGERS=()
  command_exists sddm    || unit_exists sddm.service    && SESSION_MANAGERS+=("sddm")    || true
  command_exists gdm     || unit_exists gdm.service     && SESSION_MANAGERS+=("gdm")     || true
  command_exists gdm3    || unit_exists gdm3.service    && SESSION_MANAGERS+=("gdm3")    || true
  command_exists lightdm || unit_exists lightdm.service && SESSION_MANAGERS+=("lightdm") || true
  command_exists ly      || unit_exists ly.service      && SESSION_MANAGERS+=("ly")      || true
  command_exists greetd  || unit_exists greetd.service  && SESSION_MANAGERS+=("greetd")  || true
}

build_blair() {
  local cargo_args=("build" "-p" "blair")
  [ "$1" = "release" ] && cargo_args+=("--release")
  info "Building Blair ($1)"
  cargo "${cargo_args[@]}"
}

install_binary() {
  local dir="$1"
  [ -x "$ROOT_DIR/target/$dir/blair" ] || die "blair binary not found in target/$dir"
  info "Installing binary into $BIN_DIR"
  sudo_cmd install -d "$BIN_DIR"
  sudo_cmd install -m 0755 "$ROOT_DIR/target/$dir/blair" "$BIN_DIR/blair"
  sudo_cmd install -m 0755 "$SESSION_FILE" "$BIN_DIR/blair-session"
}

install_session() {
  info "Registering Wayland session in $WAYLAND_SESSIONS_DIR"
  sudo_cmd install -d "$WAYLAND_SESSIONS_DIR"
  sudo_cmd install -m 0644 "$DESKTOP_FILE" "$WAYLAND_SESSIONS_DIR/blair.desktop"
}

main() {
  cd "$ROOT_DIR"

  info "Blair installer"

  command_exists cargo || die "cargo is not installed"
  if [ "${EUID:-$(id -u)}" -ne 0 ] && ! command_exists sudo; then
    die "sudo is required to install into $INSTALL_PREFIX and $WAYLAND_SESSIONS_DIR"
  fi

  detect_session_managers
  if [ "${#SESSION_MANAGERS[@]}" -eq 0 ]; then
    die "no supported session manager detected (sddm, gdm, lightdm, ly, greetd)"
  fi

  local build_mode
  build_mode="$(choose_from_menu "Choose build mode:" "debug" "release")"

  local profile_dir="debug"
  [ "$build_mode" = "release" ] && profile_dir="release"

  build_blair "$build_mode"
  install_binary "$profile_dir"
  install_session

  info ""
  info "Installed:"
  info "  $BIN_DIR/blair"
  info "  $BIN_DIR/blair-session"
  info "  $WAYLAND_SESSIONS_DIR/blair.desktop"
  info ""
  info "Blair reads user configuration from ~/.config/blair/config.toml."
  info "Detected session manager(s): ${SESSION_MANAGERS[*]}"
  info "Restart your session manager and choose 'Blair' from the session picker."
  info "Or run 'blair-session' directly from a TTY."
}

main "$@"
