#!/usr/bin/env bash
# Packages the desktop app with the nsh toolchain, then adds the bundled
# aleph-server, the one thing `nsh package` cannot place itself.
#
#   scripts/nsh_package.sh --target <macos|windows|linux> --out <dir>
#                          [--server <path>] [--bridge <path>]
#
# Output: `nsh package`'s app (<dir>/Aleph.app on macOS, <dir>/Aleph/ on
# Windows and Linux) with aleph-server beside the app's executable (on macOS
# also AlephBridge, which aleph-server looks for beside itself), and the
# release archive <dir>/Aleph-<version>-<os>-<arch>.{tar.gz|zip}. On macOS
# the helpers and then the app are signed again (ad hoc), keeping the app's
# entitlements; without that the bundle no longer verifies and the updater
# refuses to install it. `--server` and `--bridge` use prebuilt binaries
# instead of building them.
set -euo pipefail

usage() {
    echo "usage: $0 --target <macos|windows|linux> --out <dir> [--server <path>] [--bridge <path>]" >&2
    exit 2
}

fail() {
    echo "nsh_package: $*" >&2
    exit 1
}

target=""
out=""
server=""
bridge=""
while [ $# -gt 0 ]; do
    case "$1" in
        --target) target="${2:-}"; shift 2 || usage ;;
        --out) out="${2:-}"; shift 2 || usage ;;
        --server) server="${2:-}"; shift 2 || usage ;;
        --bridge) bridge="${2:-}"; shift 2 || usage ;;
        *) usage ;;
    esac
done
case "$target" in
    macos | windows | linux) ;;
    *) usage ;;
esac
[ -n "$out" ] || usage

root="$(cd "$(dirname "$0")/.." && pwd)"
app_dir="$root/desktop/nsh-shell"
name="Aleph"

# VERSION is the single source of the release version; nsh.json carries a
# copy because `nsh package` reads it from there.
version="$(tr -d '[:space:]' < "$root/VERSION")"
packaged_version="$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' "$app_dir/nsh.json")"
[ "$packaged_version" = "$version" ] ||
    fail "desktop/nsh-shell/nsh.json version '$packaged_version' differs from VERSION '$version'"

case "$(uname -m)" in
    arm64 | aarch64) arch="aarch64" ;;
    x86_64 | amd64) arch="x86_64" ;;
    *) fail "unsupported machine $(uname -m)" ;;
esac

exe_suffix=""
[ "$target" = "windows" ] && exe_suffix=".exe"

if [ -z "$server" ]; then
    cargo build --release -p alephcore --bin aleph-server --manifest-path "$root/Cargo.toml"
    # The workspace may build into a shared target directory; ask cargo
    # where. JSON escapes Windows backslashes, which Git Bash reads as `/`.
    target_dir="$(cargo metadata --format-version 1 --no-deps --manifest-path "$root/Cargo.toml" |
        sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p' | sed 's/\\\\/\//g')"
    server="$target_dir/release/aleph-server$exe_suffix"
fi
[ -f "$server" ] || fail "aleph-server not found at $server"
if [ "$target" = "macos" ]; then
    if [ -z "$bridge" ]; then
        (cd "$root/desktop/macos/bridge" && swift build -c release --product AlephBridge)
        bridge="$root/desktop/macos/bridge/.build/release/AlephBridge"
    fi
    [ -f "$bridge" ] || fail "AlephBridge not found at $bridge"
fi

mkdir -p "$out"
out="$(cd "$out" && pwd)"
(cd "$app_dir" && nsh package --target "$target" -o "$out")

case "$target" in
    macos)
        app="$out/$name.app"
        install -m 755 "$server" "$app/Contents/MacOS/aleph-server"
        install -m 755 "$bridge" "$app/Contents/MacOS/AlephBridge"
        codesign --force --sign - --identifier ai.aleph.server "$app/Contents/MacOS/aleph-server"
        codesign --force --sign - --identifier ai.aleph.bridge "$app/Contents/MacOS/AlephBridge"
        codesign --force --sign - \
            --preserve-metadata=entitlements,requirements,flags,runtime "$app"
        codesign --verify --deep --strict "$app"
        release="$out/$name-$version-macos-$arch.tar.gz"
        rm -f "$release"
        # COPYFILE_DISABLE keeps out the AppleDouble files (`._*`) the
        # signature does not cover.
        COPYFILE_DISABLE=1 tar -czf "$release" -C "$out" "$name.app"
        ;;
    linux)
        install -m 755 "$server" "$out/$name/aleph-server"
        # nsh's own archive predates the server; only the release one ships.
        rm -f "$out/$name.tar.gz"
        release="$out/$name-$version-linux-$arch.tar.gz"
        rm -f "$release"
        tar -czf "$release" -C "$out" "$name"
        ;;
    windows)
        # The server sits at the package root: the updater swaps root
        # entries and cannot rename a directory holding a running program.
        cp "$server" "$out/$name/aleph-server.exe"
        rm -f "$out/$name.zip"
        release="$out/$name-$version-windows-$arch.zip"
        rm -f "$release"
        # Windows' own tar (bsdtar) writes zip by the suffix, with `/`
        # separators and the directory as the root entry, as nsh does.
        (cd "$out" && "${SYSTEMROOT:-C:/Windows}/System32/tar.exe" -a -cf "$(basename "$release")" "$name")
        # `nsh package` compiles the installer before the server is added;
        # compile it again from the same script, or drop the stale one.
        if [ -f "$out/$name-setup.exe" ]; then
            iscc="$(command -v ISCC.exe || command -v iscc || true)"
            for base in "$(printenv 'ProgramFiles(x86)' || true)" "${ProgramFiles:-}" "${LOCALAPPDATA:-}"; do
                [ -n "$iscc" ] && break
                for candidate in "$base/Inno Setup 6/ISCC.exe" "$base/Programs/Inno Setup 6/ISCC.exe"; do
                    [ -n "$base" ] && [ -f "$candidate" ] && iscc="$candidate" && break
                done
            done
            rm -f "$out/$name-setup.exe"
            if [ -n "$iscc" ]; then
                installer_dir="$(cygpath -w "$out")"
                (cd "$out" && MSYS2_ARG_CONV_EXCL=/O "$iscc" "/O$installer_dir" "$name.iss")
                [ -s "$out/$name-setup.exe" ] || fail "Inno Setup did not write $name-setup.exe"
            else
                echo "nsh_package: removed the installer built without aleph-server (Inno Setup not found)" >&2
            fi
        fi
        ;;
esac

echo "packaged $release"
