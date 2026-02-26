#!/bin/bash
# One-line installer: curl -fsSL https://raw.githubusercontent.com/rootazero/Aleph/main/scripts/install.sh | bash
set -euo pipefail

REPO="rootazero/Aleph"
INSTALL_DIR="/usr/local/bin"
BINARY_NAME="aleph-server"

# Detect platform
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
case "$OS" in
    darwin) PLATFORM="darwin" ;;
    linux)  PLATFORM="linux" ;;
    *)      echo "Unsupported OS: $OS"; exit 1 ;;
esac
case "$ARCH" in
    x86_64|amd64)  ARCH="x86_64" ;;
    arm64|aarch64) ARCH="aarch64" ;;
    *)             echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

ASSET_NAME="${BINARY_NAME}-${PLATFORM}-${ARCH}"
echo "Detected: $PLATFORM/$ARCH"

# Download from GitHub Releases
LATEST_URL="https://api.github.com/repos/$REPO/releases/latest"
echo "Fetching latest release..."
DOWNLOAD_URL=$(curl -fsSL "$LATEST_URL" | grep "browser_download_url.*$ASSET_NAME\"" | head -1 | cut -d'"' -f4)

if [ -z "$DOWNLOAD_URL" ]; then
    echo "ERROR: No binary found for $ASSET_NAME"
    exit 1
fi

echo "Downloading $ASSET_NAME..."
TMP_FILE=$(mktemp)
curl -fsSL -o "$TMP_FILE" "$DOWNLOAD_URL"
chmod +x "$TMP_FILE"

echo "Installing to $INSTALL_DIR/$BINARY_NAME..."
if [ -w "$INSTALL_DIR" ]; then
    mv "$TMP_FILE" "$INSTALL_DIR/$BINARY_NAME"
else
    sudo mv "$TMP_FILE" "$INSTALL_DIR/$BINARY_NAME"
fi

mkdir -p "$HOME/.aleph"
echo ""
echo "aleph-server installed successfully!"
echo "Run: aleph-server"

# Offer service installation
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
        SERVICE_FILE="$HOME/.config/systemd/user/aleph-server.service"
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
        systemctl --user enable aleph-server
        systemctl --user start aleph-server
        echo "Service installed. Use: systemctl --user status aleph-server"
    fi
fi
