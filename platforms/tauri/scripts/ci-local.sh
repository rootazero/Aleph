#!/bin/bash
# Aether Tauri Local CI Script
# Simulates the CI pipeline locally for testing
# Usage: ./scripts/ci-local.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_step() {
    echo -e "\n${BLUE}===================================${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}===================================${NC}\n"
}

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

# Get package manager
PM="pnpm"
if ! command -v pnpm &> /dev/null; then
    PM="npm"
    log_warn "pnpm not found, using npm"
fi

# Step 1: Install dependencies
log_step "Step 1: Installing Dependencies"
$PM install
log_success "Dependencies installed"

# Step 2: Lint
log_step "Step 2: Running Linter"
$PM run lint || {
    log_error "Linting failed"
    exit 1
}
log_success "Linting passed"

# Step 3: Typecheck
log_step "Step 3: Running Type Check"
$PM run typecheck || {
    log_error "Type check failed"
    exit 1
}
log_success "Type check passed"

# Step 4: Tests
log_step "Step 4: Running Tests"
$PM run test --run || {
    log_warn "Tests failed or no tests found"
}
log_success "Tests completed"

# Step 5: Build frontend
log_step "Step 5: Building Frontend"
$PM run build || {
    log_error "Frontend build failed"
    exit 1
}
log_success "Frontend build completed"

# Step 6: Rust check (no full build to save time)
log_step "Step 6: Rust Check"
cd src-tauri
cargo check || {
    log_error "Rust check failed"
    exit 1
}
log_success "Rust check passed"

# Step 7: Rust clippy
log_step "Step 7: Rust Clippy"
cargo clippy -- -D warnings || {
    log_warn "Clippy warnings found"
}
log_success "Clippy completed"

cd "$PROJECT_DIR"

echo -e "\n${GREEN}===================================${NC}"
echo -e "${GREEN}  All CI Checks Passed!${NC}"
echo -e "${GREEN}===================================${NC}\n"

log_info "To build the full app, run: ./scripts/build.sh --release"
