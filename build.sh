#!/usr/bin/env bash
# build.sh – local release builds for filetag
#
# Usage:
#   ./build.sh                  # build all targets
#   ./build.sh macos            # macOS arm64 only (native)
#   ./build.sh linux-amd64      # Linux x86-64
#   ./build.sh linux-arm64      # Linux aarch64 (Raspberry Pi)
#
# Prerequisites for cross-compilation (Linux targets on macOS):
#   brew tap messense/macos-cross-toolchains
#   brew install x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
#   rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu
#
# Alternatively, install 'cross' (requires Docker):
#   cargo install cross

set -euo pipefail

DIST="dist"
BINS=("filetag" "filetag-web")

mkdir -p "$DIST"

# ---------------------------------------------------------------------------
# Detect cross-compilation strategy
# ---------------------------------------------------------------------------

use_cross=0
if command -v cross &>/dev/null; then
    use_cross=1
    echo "Using 'cross' for Linux targets."
fi

# ---------------------------------------------------------------------------
# Build functions
# ---------------------------------------------------------------------------

build_target() {
    local target="$1"
    local artifact="$2"
    echo ""
    echo "==> Building $target ..."

    local cargo_cmd="cargo"
    if [[ $use_cross -eq 1 && "$target" == *linux* ]]; then
        cargo_cmd="cross"
    fi

    $cargo_cmd build --release --target "$target" -p filetag -p filetag-web

    local out_dir="target/$target/release"
    local archive="$DIST/$artifact.tar.gz"
    tar czf "$archive" -C "$out_dir" "${BINS[@]}"
    echo "    -> $archive"
}

build_macos() {
    build_target "aarch64-apple-darwin" "filetag-macos-arm64"
}

build_linux_amd64() {
    if [[ $use_cross -eq 0 ]]; then
        # Check that the Homebrew toolchain linker is available
        if ! command -v x86_64-unknown-linux-gnu-gcc &>/dev/null; then
            echo "ERROR: x86_64-unknown-linux-gnu-gcc not found."
            echo "       Run: brew tap messense/macos-cross-toolchains && brew install x86_64-unknown-linux-gnu"
            echo "       Or install Docker and 'cargo install cross' to use cross instead."
            exit 1
        fi
        export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-unknown-linux-gnu-gcc
    fi
    build_target "x86_64-unknown-linux-gnu" "filetag-linux-amd64"
}

build_linux_arm64() {
    if [[ $use_cross -eq 0 ]]; then
        if ! command -v aarch64-unknown-linux-gnu-gcc &>/dev/null; then
            echo "ERROR: aarch64-unknown-linux-gnu-gcc not found."
            echo "       Run: brew tap messense/macos-cross-toolchains && brew install aarch64-unknown-linux-gnu"
            echo "       Or install Docker and 'cargo install cross' to use cross instead."
            exit 1
        fi
        export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-unknown-linux-gnu-gcc
    fi
    build_target "aarch64-unknown-linux-gnu" "filetag-linux-arm64"
}

# ---------------------------------------------------------------------------
# Target selection
# ---------------------------------------------------------------------------

TARGET="${1:-all}"

case "$TARGET" in
    all)
        build_macos
        build_linux_amd64
        build_linux_arm64
        ;;
    macos)
        build_macos
        ;;
    linux-amd64)
        build_linux_amd64
        ;;
    linux-arm64)
        build_linux_arm64
        ;;
    *)
        echo "Unknown target: $TARGET"
        echo "Valid targets: all, macos, linux-amd64, linux-arm64"
        exit 1
        ;;
esac

echo ""
echo "Done. Archives in $DIST/:"
ls -lh "$DIST"/*.tar.gz 2>/dev/null || true
