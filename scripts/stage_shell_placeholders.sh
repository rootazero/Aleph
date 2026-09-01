#!/usr/bin/env bash
# Single owner for the Tauri externalBin placeholders.
#
# `tauri-build` fails the aleph-desktop-shell build script outright when a file
# named in `externalBin` is missing, so ANY command that compiles that crate —
# `cargo check -p aleph-desktop-shell`, `cargo clippy --workspace`, and every
# tool that shells out to one of those (rust-doctor) — needs the placeholders
# staged first, even though it never runs the binaries.
#
# `touch` leaves a real staged binary intact, so this is safe to run before a
# packaging build that has already copied the daemon in.
#
# The naming is derived, never spelled: the daemon is suffixed with the host
# triple (plus `.exe` on Windows), and the Swift bridge exists on macOS only.
# That derivation used to live in three places — the justfile recipe, the
# `desktop-shell` job in aleph-core-ci.yml, and nowhere at all for rust-doctor,
# which is how that job spent three CI rounds failing at `Clippy exited with
# status 101`. One copy now; the callers pass the directory.
set -euo pipefail

shell_dir="${1:-desktop/shell}"
triple="$(rustc -vV | sed -n 's/host: //p')"
[ -n "$triple" ] || { echo "cannot read host triple from rustc -vV" >&2; exit 1; }

ext=""
case "$triple" in *windows*) ext=".exe" ;; esac

mkdir -p "$shell_dir/binaries"
touch "$shell_dir/binaries/aleph-server-$triple$ext"
case "$triple" in *apple-darwin) touch "$shell_dir/binaries/AlephBridge-$triple" ;; esac

echo "✓ staged externalBin placeholders in $shell_dir/binaries for $triple"
