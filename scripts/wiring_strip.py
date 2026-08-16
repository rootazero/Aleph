#!/usr/bin/env python3
"""Shared test-code stripping for the wiring parity guards.

All three guards (`rpc_`, `tool_`, `config_wiring_audit.py`) must ignore test
code, so a `#[cfg(test)]` manual registration cannot mask a production
severance. They used to do it by cutting each file at the **first**
`#[cfg(test)]` occurrence — which is only correct when that occurrence is the
trailing `mod tests`. In 108 of the repo's files it is an inline
`#[cfg(test)] fn`/`impl` helper sitting *above* production code (e.g.
`src/teams/mod.rs:24`, `src/sandbox/mod.rs:70`,
`src/gateway/agent_instance.rs:13`), so the cut discarded most of the file.

That cut both ways and both were wrong:
  * DEFINED side  → symbols below the cut vanished → **false green** (a
    genuinely severed wire invisible to the guard).
  * CONSUMED side → live consumers below the cut vanished → **false red**
    (`PermissionMatch`, live in `src/tools/scoped/gate_chain.rs:427`, was
    reported as an inert policy type because that file's first `#[cfg(test)]`
    is at line 202).

`strip_tests` below removes each `#[cfg(test)]`-attributed item individually by
brace matching, wherever it sits, and keeps everything else. Strings and
comments are not parsed, so a `{` inside a string literal in a test body could
in principle skew the brace count; the fallback is to drop the rest of the file
from that attribute, i.e. the old (conservative) behaviour, never a false green
beyond it.
"""

from __future__ import annotations

import re

_CFG_TEST_RE = re.compile(r"^[ \t]*#\[cfg\(test\)\][ \t]*\r?\n", re.MULTILINE)


def _skip_item(text: str, start: int) -> int:
    """Index just past the `#[cfg(test)]`-attributed item beginning at `start`.

    Handles both brace-bodied items (`mod`, `fn`, `impl`, `struct { .. }`) and
    semicolon-terminated ones (`use x;`, `struct S;`). Returns `len(text)` when
    the item is unterminated, which degrades to the old cut-the-tail behaviour.
    """
    i = start
    n = len(text)
    # Further stacked attributes (`#[cfg(test)] #[allow(..)] fn ..`) are part of
    # the same item; the scan below walks over them naturally.
    while i < n:
        c = text[i]
        if c == "{":
            depth = 0
            while i < n:
                if text[i] == "{":
                    depth += 1
                elif text[i] == "}":
                    depth -= 1
                    if depth == 0:
                        return i + 1
                i += 1
            return n
        if c == ";":
            return i + 1
        i += 1
    return n


def strip_tests(text: str) -> str:
    """`text` with every `#[cfg(test)]`-attributed item removed."""
    out: list[str] = []
    pos = 0
    while True:
        m = _CFG_TEST_RE.search(text, pos)
        if not m:
            out.append(text[pos:])
            return "".join(out)
        out.append(text[pos : m.start()])
        pos = _skip_item(text, m.end())
