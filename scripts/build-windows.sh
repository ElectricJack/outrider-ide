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

MISSING_PKGS=()
check_cmd clang  || MISSING_PKGS+=(clang)
check_cmd lld    || MISSING_PKGS+=(lld)
check_cmd llvm-lib || MISSING_PKGS+=(llvm)

if [ ${#MISSING_PKGS[@]} -gt 0 ]; then
    echo "    Installing ${MISSING_PKGS[*]}..."
    sudo apt-get update -qq && sudo apt-get install -y -qq "${MISSING_PKGS[@]}"
fi

if ! rustup target list --installed | grep -q "$TARGET"; then
    echo "    Adding rustup target $TARGET..."
    rustup target add "$TARGET"
fi

if ! check_cmd cargo-xwin; then
    echo "    Installing cargo-xwin..."
    cargo install cargo-xwin
fi

GPUI_SRC="$(find "$HOME/.cargo/git/checkouts" -maxdepth 4 -path '*/crates/gpui' -type d 2>/dev/null | head -1)"
if [ -z "$GPUI_SRC" ]; then
    echo "    Warning: could not locate gpui source checkout for INCLUDE path."
    echo "    The build may fail on the Windows manifest resource."
fi

echo "==> Building for $TARGET..."
cd "$PROJECT_ROOT"
INCLUDE="${GPUI_SRC:-}" cargo xwin "${CARGO_XWIN_ARGS[@]}"

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
