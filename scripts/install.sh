#!/usr/bin/env bash
# Aleph server installer — downloads the standalone aleph-server binary.
# Usage: curl -fsSL https://github.com/rootazero/Aleph/releases/latest/download/install.sh | bash
set -euo pipefail

REPO="${ALEPH_REPO:-rootazero/Aleph}"
VERSION="${ALEPH_VERSION:-latest}"

# Asset names mirror the GNU host triples the release workflow uploads
# (desktop/shell/binaries/aleph-server-<triple>). Only macOS arm64 and
# Linux x86_64 ship a server binary; everything else downloads manually.
os="$(uname -s)"; arch="$(uname -m)"
case "$os-$arch" in
  Darwin-arm64)  asset="aleph-server-aarch64-apple-darwin" ;;
  Linux-x86_64)  asset="aleph-server-x86_64-unknown-linux-gnu" ;;
  *) echo "Unsupported platform: $os/$arch (download manually from GitHub Releases)"; exit 1 ;;
esac

if [ "$VERSION" = "latest" ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

dest_dir="/usr/local/bin"
[ -w "$dest_dir" ] || dest_dir="$HOME/.local/bin"
mkdir -p "$dest_dir"

echo "Downloading $asset -> $dest_dir/aleph-server"
curl -fsSL "$url" -o "$dest_dir/aleph-server"
chmod +x "$dest_dir/aleph-server"

echo "Installed. Start it with:  aleph-server start"
echo "LAN access: set [gateway] host = \"0.0.0.0\" in ~/.aleph/config.toml (trusts your whole LAN)."
