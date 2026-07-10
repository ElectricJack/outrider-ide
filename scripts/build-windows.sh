#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TARGET="x86_64-pc-windows-msvc"

CARGO_XWIN_ARGS=("build" "--target" "$TARGET")

for arg in "$@"; do
    case "$arg" in
        --release) CARGO_XWIN_ARGS+=("--release") ;;
        *) CARGO_XWIN_ARGS+=("$arg") ;;
    esac
done

check_cmd() {
    command -v "$1" &>/dev/null
}

echo "==> Checking prerequisites..."

if ! check_cmd clang; then
    echo "    Installing clang..."
    sudo apt-get update -qq && sudo apt-get install -y -qq clang
fi

if ! check_cmd lld; then
    echo "    Installing lld..."
    sudo apt-get update -qq && sudo apt-get install -y -qq lld
fi

if ! rustup target list --installed | grep -q "$TARGET"; then
    echo "    Adding rustup target $TARGET..."
    rustup target add "$TARGET"
fi

if ! check_cmd cargo-xwin; then
    echo "    Installing cargo-xwin..."
    cargo install cargo-xwin
fi

echo "==> Building for $TARGET..."
cd "$PROJECT_ROOT"
cargo xwin "${CARGO_XWIN_ARGS[@]}"

PROFILE="debug"
for arg in "$@"; do
    if [ "$arg" = "--release" ]; then
        PROFILE="release"
    fi
done

EXE_PATH="$PROJECT_ROOT/target/$TARGET/$PROFILE/outrider.exe"
if [ -f "$EXE_PATH" ]; then
    echo ""
    echo "==> Build complete: $EXE_PATH"
else
    echo ""
    echo "==> Build finished but .exe not found at expected path."
    echo "    Check target/$TARGET/$PROFILE/ for output."
fi
