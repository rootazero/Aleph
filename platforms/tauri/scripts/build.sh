#!/bin/bash
# Aleph Tauri Build Script
# Usage: ./scripts/build.sh [platform] [--release]

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Parse arguments
PLATFORM="${1:-all}"
RELEASE_MODE=""

for arg in "$@"; do
    case $arg in
        --release)
            RELEASE_MODE="--release"
            shift
            ;;
    esac
done

# Check dependencies
check_deps() {
    log_info "Checking dependencies..."

    if ! command -v node &> /dev/null; then
        log_error "Node.js is not installed"
        exit 1
    fi

    if ! command -v npm &> /dev/null; then
        log_error "npm is not installed"
        exit 1
    fi

    if ! command -v cargo &> /dev/null; then
        log_error "Rust/Cargo is not installed"
        exit 1
    fi

    log_info "All dependencies found"
}

# Install dependencies
install_deps() {
    log_info "Installing dependencies..."
    if command -v pnpm &> /dev/null; then
        pnpm install
    else
        npm install
    fi
}

# Build frontend
build_frontend() {
    log_info "Building frontend..."
    if command -v pnpm &> /dev/null; then
        pnpm build
    else
        npm run build
    fi
}

# Get package manager command
get_pm() {
    if command -v pnpm &> /dev/null; then
        echo "pnpm"
    else
        echo "npm run"
    fi
}

# Build for macOS
build_macos() {
    log_info "Building for macOS..."

    if [[ "$OSTYPE" != "darwin"* ]]; then
        log_warn "Cross-compilation to macOS is not supported. Skipping."
        return
    fi

    PM=$(get_pm)
    if [ -n "$RELEASE_MODE" ]; then
        $PM tauri build -- --target universal-apple-darwin
    else
        $PM tauri build
    fi

    log_info "macOS build complete!"
    log_info "Output: src-tauri/target/release/bundle/dmg/"
}

# Build for Windows
build_windows() {
    log_info "Building for Windows..."

    if [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "win32" ]]; then
        PM=$(get_pm)
        $PM tauri build
    else
        log_warn "Cross-compilation to Windows requires additional setup. Skipping."
        log_warn "For Windows builds, run this script on Windows or use CI/CD."
    fi
}

# Build for Linux
build_linux() {
    log_info "Building for Linux..."

    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        PM=$(get_pm)
        $PM tauri build
    else
        log_warn "Cross-compilation to Linux requires additional setup. Skipping."
        log_warn "For Linux builds, run this script on Linux or use CI/CD."
    fi
}

# Main build process
main() {
    log_info "========================================="
    log_info "  Aleph Tauri Build Script"
    log_info "========================================="
    log_info "Platform: $PLATFORM"
    log_info "Release mode: ${RELEASE_MODE:-debug}"
    log_info ""

    check_deps
    install_deps
    build_frontend

    case $PLATFORM in
        macos)
            build_macos
            ;;
        windows)
            build_windows
            ;;
        linux)
            build_linux
            ;;
        all)
            build_macos
            build_windows
            build_linux
            ;;
        *)
            log_error "Unknown platform: $PLATFORM"
            log_info "Usage: ./scripts/build.sh [macos|windows|linux|all] [--release]"
            exit 1
            ;;
    esac

    log_info ""
    log_info "========================================="
    log_info "  Build Complete!"
    log_info "========================================="
}

main
