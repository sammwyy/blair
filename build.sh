#!/bin/bash
# Blair builder: compiles the compositor with the release profile and stages
# the result in .build/ so install.sh can lay it down without a Rust
# toolchain or the sources at install time.
#
# When `upx` is available the staged binary is compressed in place; without
# it the binary is staged uncompressed — compression is optional size
# shaving, never a requirement.
set -e
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="${BLAIR_BUILD_DIR:-$ROOT/.build}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"

# ---------------------------------------------------------------- binaries

cargo build --release --manifest-path "$ROOT/Cargo.toml" -p blair

# ----------------------------------------------------------------- staging

mkdir -p "$BUILD_DIR"
install -m 755 "$TARGET_DIR/release/blair" "$BUILD_DIR/blair"
install -m 755 "$ROOT/packaging/sessions/blair-session" "$BUILD_DIR/blair-session"

# -------------------------------------------------------------------- size

if command -v upx >/dev/null 2>&1; then
    upx -q "$BUILD_DIR/blair" || echo "warning: upx could not compress blair" >&2
else
    echo "note: upx not found; staged binaries stay uncompressed" >&2
fi
