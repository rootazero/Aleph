#!/bin/bash
# Aleph Tauri Release Script
# Creates a new release by updating version and creating a git tag
# Usage: ./scripts/release.sh <version>
# Example: ./scripts/release.sh 0.2.0

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
ROOT_DIR="$(dirname "$(dirname "$PROJECT_DIR")")"

cd "$PROJECT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
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

# Check if version is provided
if [ -z "$1" ]; then
    log_error "Version not provided"
    echo "Usage: ./scripts/release.sh <version>"
    echo "Example: ./scripts/release.sh 0.2.0"
    exit 1
fi

VERSION="$1"

# Validate version format (semver)
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.]+)?$ ]]; then
    log_error "Invalid version format: $VERSION"
    echo "Version must follow semver format: X.Y.Z or X.Y.Z-suffix"
    exit 1
fi

log_info "Preparing release v$VERSION"

# Check for uncommitted changes
if [ -n "$(git status --porcelain)" ]; then
    log_warn "You have uncommitted changes. Please commit or stash them first."
    git status --short
    read -p "Continue anyway? (y/N) " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        exit 1
    fi
fi

# Update version in package.json
log_info "Updating package.json..."
sed -i.bak "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" package.json
rm -f package.json.bak

# Update version in tauri.conf.json
log_info "Updating tauri.conf.json..."
sed -i.bak "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" src-tauri/tauri.conf.json
rm -f src-tauri/tauri.conf.json.bak

# Update version in Cargo.toml
log_info "Updating Cargo.toml..."
sed -i.bak "s/^version = \".*\"/version = \"$VERSION\"/" src-tauri/Cargo.toml
rm -f src-tauri/Cargo.toml.bak

# Update Cargo.lock
log_info "Updating Cargo.lock..."
cd src-tauri
cargo update -p aleph-tauri
cd "$PROJECT_DIR"

# Run CI checks
log_info "Running CI checks..."
./scripts/ci-local.sh

# Commit version changes
log_info "Committing version bump..."
cd "$ROOT_DIR"
git add \
    platforms/tauri/package.json \
    platforms/tauri/src-tauri/tauri.conf.json \
    platforms/tauri/src-tauri/Cargo.toml \
    platforms/tauri/src-tauri/Cargo.lock

git commit -m "chore(tauri): bump version to v$VERSION"

# Create and push tag
log_info "Creating tag v$VERSION..."
git tag -a "v$VERSION" -m "Release v$VERSION"

echo ""
echo -e "${GREEN}=========================================${NC}"
echo -e "${GREEN}  Release v$VERSION prepared!${NC}"
echo -e "${GREEN}=========================================${NC}"
echo ""
log_info "To publish the release:"
echo "  1. Review the changes: git show HEAD"
echo "  2. Push the commit: git push"
echo "  3. Push the tag: git push origin v$VERSION"
echo ""
log_info "The CI will automatically build and create a GitHub release."
