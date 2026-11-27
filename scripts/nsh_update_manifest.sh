#!/usr/bin/env bash
# Writes the signed update manifest for a release from the archives
# scripts/nsh_package.sh made on each platform.
#
#   scripts/nsh_update_manifest.sh --out <dir> [--key <secret file>] <archive>...
#
# The key is `nsh keygen`'s `.secret` for desktop/nsh-shell/nsh.json's
# `updater.publicKey`: the file `--key` names, or else the contents of
# NSH_UPDATE_SECRET_KEY (CI), written to a private temporary file for the
# run.
#
# Output in <dir>: `aleph-update.json`, the manifest the app's `updater.url`
# names, and `latest.json.sig`, its signature (nsh-updater always looks for
# the signature beside the manifest under that name). `nsh update-manifest`
# calls the manifest `latest.json`; it is renamed so the Tauri app's
# `latest.json` feed, which the new app does not continue, finds nothing.
# Publish both files with the archives as assets of one GitHub Release.
set -euo pipefail

usage() {
    echo "usage: $0 --out <dir> [--key <secret file>] <archive>..." >&2
    exit 2
}

fail() {
    echo "nsh_update_manifest: $*" >&2
    exit 1
}

out=""
key=""
archives=()
while [ $# -gt 0 ]; do
    case "$1" in
        --out) out="${2:-}"; shift 2 || usage ;;
        --key) key="${2:-}"; shift 2 || usage ;;
        -*) usage ;;
        *) archives+=("$1"); shift ;;
    esac
done
[ -n "$out" ] || usage
[ "${#archives[@]}" -gt 0 ] || usage

root="$(cd "$(dirname "$0")/.." && pwd)"
app_dir="$root/desktop/nsh-shell"

if [ -z "$key" ]; then
    [ -n "${NSH_UPDATE_SECRET_KEY:-}" ] || fail "pass --key or set NSH_UPDATE_SECRET_KEY"
    key="$(mktemp)"
    trap 'rm -f "$key"' EXIT
    chmod 600 "$key"
    printf '%s\n' "$NSH_UPDATE_SECRET_KEY" > "$key"
fi
[ -f "$key" ] || fail "no key file at $key"

mkdir -p "$out"
nsh update-manifest --entry "$app_dir" --key "$key" -o "$out" "${archives[@]}"
[ -f "$out/latest.json" ] || fail "nsh update-manifest wrote no latest.json"
[ -f "$out/latest.json.sig" ] || fail "nsh update-manifest wrote no signature"
mv "$out/latest.json" "$out/aleph-update.json"
echo "wrote $out/aleph-update.json and $out/latest.json.sig"
