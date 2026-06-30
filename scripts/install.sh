#!/usr/bin/env bash
# Aleph server installer — downloads the standalone aleph-server binary.
# Usage: curl -fsSL https://github.com/rootazero/Aleph/releases/latest/download/install.sh | bash
set -euo pipefail

REPO="${ALEPH_REPO:-rootazero/Aleph}"
VERSION="${ALEPH_VERSION:-latest}"

# Asset names mirror the GNU host triples the release workflow uploads
# (desktop/shell/binaries/aleph-server-<triple>). Only macOS arm64 and
# Linux x86_64 ship a server binary here; Windows users run install.ps1.
os="$(uname -s)"; arch="$(uname -m)"
case "$os-$arch" in
  Darwin-arm64)  asset="aleph-server-aarch64-apple-darwin" ;;
  Linux-x86_64)  asset="aleph-server-x86_64-unknown-linux-gnu" ;;
  MINGW*|MSYS*|CYGWIN*)
    echo "On Windows, install via PowerShell instead:"
    echo "  irm https://github.com/$REPO/releases/latest/download/install.ps1 | iex"
    exit 1 ;;
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
# Download to a temp file on the same filesystem, then atomically rename it into
# place. A plain `curl -o` straight over the destination fails with ETXTBSY on
# Linux when that path is the currently-running binary (e.g. `aleph-server
# update` re-running this installer to replace itself); rename(2) swaps the
# directory entry without touching the busy inode, so running processes keep
# their old binary until they restart onto the freshly-installed one.
tmp="$(mktemp "$dest_dir/.aleph-server.XXXXXX")"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp"
chmod +x "$tmp"
mv -f "$tmp" "$dest_dir/aleph-server"
trap - EXIT

echo "Installed."
# Enable start-on-boot by default (per-user service). Opt out with ALEPH_AUTOSTART=0.
# Skip when running as root: the service is per-user and would otherwise register
# for root rather than the human user.
if [ "${ALEPH_AUTOSTART:-1}" = "1" ] && [ "$(id -u)" != "0" ]; then
  echo "Enabling start-on-boot (set ALEPH_AUTOSTART=0 to skip)…"
  if ! "$dest_dir/aleph-server" service install; then
    echo "  Could not enable autostart; run '$dest_dir/aleph-server service install' later."
  fi
else
  echo "Start it with:  aleph-server start"
  [ "$(id -u)" = "0" ] && echo "  (running as root: re-run 'aleph-server service install' as your normal user for boot autostart)"
fi
echo "LAN access: set [gateway] host = \"0.0.0.0\" in ~/.aleph/config.toml (trusts your whole LAN)."
