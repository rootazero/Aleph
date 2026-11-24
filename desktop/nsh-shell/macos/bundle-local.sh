#!/bin/bash
set -euo pipefail

server=
if [[ $# == 4 && "$1" == --server ]]; then
    server=$2
    if [[ ! -f "$server" || ! -x "$server" ]]; then
        printf 'Expected an executable server binary.\n' >&2
        exit 2
    fi
    shift 2
fi
if [[ $# != 2 || "$2" != *.app ]]; then
    printf 'Usage: %s [--server <aleph-server binary>] <aleph-nsh-shell binary> <new bundle.app>\n' "$0" >&2
    exit 2
fi
script_dir=$(cd "$(dirname "$0")" && pwd)
binary=$1
destination=$2
if [[ ! -f "$binary" || ! -x "$binary" || -e "$destination" || -L "$destination" ]]; then
    printf 'Expected an executable binary and a new bundle path.\n' >&2
    exit 2
fi

stage=$(mktemp -d "$(dirname "$destination")/.aleph-nsh-bundle.XXXXXXXX")
trap 'rm -rf -- "$stage"' EXIT
bundle="$stage/$(basename "$destination")"
mkdir -p "$bundle/Contents/MacOS"
install -m 644 "$script_dir/Info.plist" "$bundle/Contents/Info.plist"
install -m 755 "$binary" "$bundle/Contents/MacOS/aleph-nsh-shell"
plutil -lint "$bundle/Contents/Info.plist"
if [[ -n "$server" ]]; then
    sibling="$bundle/Contents/MacOS/aleph-server"
    install -m 755 "$server" "$sibling"
    codesign --force --sign - "$sibling"
    codesign --verify --strict "$sibling"
fi
# The supplied executable may already be signed; only the staged copy changes.
codesign --force --sign - "$bundle"
codesign --verify --deep --strict "$bundle"
# Unlike mv, this renames to the exact path even if a directory wins the race.
python3 - "$bundle" "$destination" <<'PY'
import ctypes
import errno
import os
import sys

rename = ctypes.CDLL(None, use_errno=True).renamex_np
rename.argtypes = (ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint)
rename.restype = ctypes.c_int
rename_excl = 0x00000004  # RENAME_EXCL from the macOS SDK.
source, destination = map(os.fsencode, sys.argv[1:])
if rename(source, destination, rename_excl) != 0:
    error = ctypes.get_errno()
    if error == errno.EEXIST:
        print('Bundle destination appeared during assembly.', file=sys.stderr)
        sys.exit(2)
    raise OSError(error, os.strerror(error), os.fsdecode(destination))
PY
printf '%s\n' "$destination"
