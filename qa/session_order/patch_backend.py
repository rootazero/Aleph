#!/usr/bin/env python3
"""Point `[general] session_store_backend` at one backend, and assert it took.

Its own script rather than a flag on `busy_input/patch_config.py` because the
two answer different questions: that one makes the daemon inert, this one picks
which of the two `SessionStore` implementations the server boots. Nothing else
in `qa/` has ever exercised the SQLite one — every fixture takes the default,
and the default is `file`.

The assert is the point. `default_session_store_backend()` returned `"file"`
while the doc comment beside it said `"sqlite" (default)` for as long as both
existed, which is exactly the class of mistake a fixture that merely WRITES the
key cannot catch.

Usage:  patch_backend.py CONFIG {file|sqlite}
"""
import re
import sys

path, backend = sys.argv[1], sys.argv[2]
assert backend in ("file", "sqlite"), backend

src = open(path).read()

if re.search(r"^\s*session_store_backend\s*=", src, re.M):
    src = re.sub(
        r"^\s*session_store_backend\s*=.*$",
        f'session_store_backend = "{backend}"',
        src,
        count=1,
        flags=re.M,
    )
elif re.search(r"^\[general\]\s*$", src, re.M):
    src = re.sub(
        r"^\[general\]\s*$",
        f'[general]\nsession_store_backend = "{backend}"',
        src,
        count=1,
        flags=re.M,
    )
else:
    src = f'[general]\nsession_store_backend = "{backend}"\n\n' + src

open(path, "w").write(src)

check = open(path).read()
hits = re.findall(r'^\s*session_store_backend\s*=\s*"([^"]+)"', check, re.M)
assert hits == [backend], f"config says {hits}, wanted exactly ['{backend}']"
print(f"session_store_backend = {backend}")
