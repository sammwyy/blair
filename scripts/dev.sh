#!/usr/bin/env bash
# Build and run Blair nested inside your current desktop, for development.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PROFILE="dev"
CARGO_PROFILE_DIR="debug"
if [ "${1:-}" = "--release" ]; then
  PROFILE="release"
  CARGO_PROFILE_DIR="release"
fi

info() { printf '==> %s\n' "$*"; }

info "building blair ($PROFILE)"
cargo build --profile "$PROFILE" -p blair

export PATH="$ROOT_DIR/target/$CARGO_PROFILE_DIR:$PATH"
export RUST_LOG="${RUST_LOG:-blair=debug,warn}"

info "starting blair (nested — needs an existing Wayland/X session)"
exec blair
