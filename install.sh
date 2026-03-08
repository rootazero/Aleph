#!/bin/bash
# One-line installer: curl -fsSL https://raw.githubusercontent.com/rootazero/Aleph/main/install.sh | bash
# With version:       curl -fsSL ... | bash -s -- v0.1.0
set -euo pipefail

REPO="rootazero/Aleph"
INSTALL_DIR="/usr/local/bin"
BINARY_NAME="aleph"
VERSION="${1:-latest}"

# Cleanup on exit
TMP_DIR=""
cleanup() { [ -n "$TMP_DIR" ] && rm -rf "$TMP_DIR"; }
trap cleanup EXIT

# ── Detect platform ──────────────────────────────────────────────

OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$OS" in
    darwin) PLATFORM="darwin" ;;
    linux)  PLATFORM="linux" ;;
    *)      echo "Error: unsupported OS: $OS"; exit 1 ;;
esac
case "$ARCH" in
    x86_64|amd64)  ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
    *)             echo "Error: unsupported architecture: $ARCH"; exit 1 ;;
esac

ASSET_NAME="${BINARY_NAME}-${PLATFORM}-${ARCH}"
echo "Detected platform: $PLATFORM/$ARCH"

# ── Fetch release info ───────────────────────────────────────────

if [ "$VERSION" = "latest" ]; then
    RELEASE_URL="https://api.github.com/repos/$REPO/releases/latest"
    echo "Fetching latest release..."
else
    RELEASE_URL="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
    echo "Fetching release $VERSION..."
fi

RELEASE_JSON=$(curl -fsSL "$RELEASE_URL") || {
    echo "Error: failed to fetch release info from GitHub."
    echo "Check your network connection and that the release exists."
    exit 1
}

# Try .tar.gz first, then raw binary
DOWNLOAD_URL=$(echo "$RELEASE_JSON" | grep -o "\"browser_download_url\": *\"[^\"]*${ASSET_NAME}\\.tar\\.gz\"" | head -1 | grep -o 'https://[^"]*') || true

if [ -n "$DOWNLOAD_URL" ]; then
    ARCHIVE_MODE="tar.gz"
else
    DOWNLOAD_URL=$(echo "$RELEASE_JSON" | grep -o "\"browser_download_url\": *\"[^\"]*${ASSET_NAME}\"" | head -1 | grep -o 'https://[^"]*') || true
    ARCHIVE_MODE="raw"
fi

if [ -z "$DOWNLOAD_URL" ]; then
    echo "Error: no binary found for $ASSET_NAME in this release."
    echo "Available assets:"
    echo "$RELEASE_JSON" | grep -o '"name": *"[^"]*"' | sed 's/"name": *"//;s/"//' | sed 's/^/  /'
    exit 1
fi

# ── Download and extract ─────────────────────────────────────────

TMP_DIR=$(mktemp -d)
echo "Downloading $ASSET_NAME ($ARCHIVE_MODE)..."

if [ "$ARCHIVE_MODE" = "tar.gz" ]; then
    curl -fsSL -o "$TMP_DIR/archive.tar.gz" "$DOWNLOAD_URL"
    tar -xzf "$TMP_DIR/archive.tar.gz" -C "$TMP_DIR"
    # Find the binary inside the archive
    if [ -f "$TMP_DIR/$BINARY_NAME" ]; then
        BIN_PATH="$TMP_DIR/$BINARY_NAME"
    elif [ -f "$TMP_DIR/$ASSET_NAME/$BINARY_NAME" ]; then
        BIN_PATH="$TMP_DIR/$ASSET_NAME/$BINARY_NAME"
    else
        BIN_PATH=$(find "$TMP_DIR" -name "$BINARY_NAME" -type f | head -1)
        if [ -z "$BIN_PATH" ]; then
            echo "Error: could not find '$BINARY_NAME' binary in archive."
            exit 1
        fi
    fi
else
    curl -fsSL -o "$TMP_DIR/$BINARY_NAME" "$DOWNLOAD_URL"
    BIN_PATH="$TMP_DIR/$BINARY_NAME"
fi

chmod +x "$BIN_PATH"

# ── Install ──────────────────────────────────────────────────────

echo "Installing to $INSTALL_DIR/$BINARY_NAME..."
if [ -w "$INSTALL_DIR" ]; then
    mv "$BIN_PATH" "$INSTALL_DIR/$BINARY_NAME"
else
    sudo mv "$BIN_PATH" "$INSTALL_DIR/$BINARY_NAME"
fi

# Create config directory
mkdir -p "$HOME/.aleph"

# Verify installation
INSTALLED_VERSION=$("$INSTALL_DIR/$BINARY_NAME" --version 2>/dev/null || echo "unknown")
echo ""
echo "Aleph installed successfully! ($INSTALLED_VERSION)"
echo "  Binary:  $INSTALL_DIR/$BINARY_NAME"
echo "  Config:  ~/.aleph/"
echo ""
echo "Run:  aleph"

# ── Optional: system service ─────────────────────────────────────

# Skip service prompt when running via pipe (stdin is not a terminal)
if [ -t 0 ]; then
    echo ""
    read -p "Install as system service? [y/N] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        if [ "$PLATFORM" = "darwin" ]; then
            PLIST="$HOME/Library/LaunchAgents/com.aleph.server.plist"
            cat > "$PLIST" << EOFPLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>com.aleph.server</string>
    <key>ProgramArguments</key><array><string>$INSTALL_DIR/$BINARY_NAME</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>StandardOutPath</key><string>$HOME/.aleph/server.log</string>
    <key>StandardErrorPath</key><string>$HOME/.aleph/server.err</string>
</dict>
</plist>
EOFPLIST
            launchctl load "$PLIST"
            echo "Service installed. Use: launchctl start com.aleph.server"
        else
            SERVICE_FILE="$HOME/.config/systemd/user/aleph.service"
            mkdir -p "$(dirname "$SERVICE_FILE")"
            cat > "$SERVICE_FILE" << EOFSVC
[Unit]
Description=Aleph AI Server
After=network.target
[Service]
ExecStart=$INSTALL_DIR/$BINARY_NAME
Restart=on-failure
RestartSec=5
[Install]
WantedBy=default.target
EOFSVC
            systemctl --user daemon-reload
            systemctl --user enable aleph
            systemctl --user start aleph
            echo "Service installed. Use: systemctl --user status aleph"
        fi
    fi
fi
