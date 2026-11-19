#!/usr/bin/env python3
"""Shared test-code stripping for the wiring parity guards.

All three guards (`rpc_`, `tool_`, `config_wiring_audit.py`) must ignore test
code, so a `#[cfg(test)]` manual registration cannot mask a production
severance. They used to do it by cutting each file at the **first**
`#[cfg(test)]` occurrence — which is only correct when that occurrence is the
trailing `mod tests`. In 108 of the repo's files it is an inline
`#[cfg(test)]` fn/`impl` helper sitting *above* production code (e.g.
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
brace matching, wherever it sits, and keeps everything else.

Two defects were found in that first implementation while auditing
`src/builtin_tools` (2026-11-19):

  1. A `#[cfg(test)]` on a **struct field** (not an item) was skipped as if it
     were an item. The scanner then hunted for the next `;`/`{` — which is the
     end of the *next function* — and deleted every line between the field and
     that function. `src/builtin_tools/loop_manage.rs` lost lines 159..911
     (752 lines of PRODUCTION code, including the live `parse_interval_ms`
     call sites at lines 335/367/800/818), so the guard reported a live
     function as a severed wire.
  2. The brace counter was blind to string and comment literals, so a `{` or
     `}` inside a string in a test body unbalanced the depth and swallowed the
     rest of the file.

Both defects cut PRODUCTION code, which is the dangerous direction: a name that
is consumed only below the cut reads as severed (false red), and a name
DEFINED below the cut reads as absent (false green). The scanner below is
therefore literal-aware (strings, char literals, raw strings and comments are
skipped before any brace counting), and a field-attributed attribute is
recognised as a one-line item.

Verified over every `.rs` file in the repo: 3,340 removed regions, 0 of them
brace-unbalanced, and 5,643 production lines recovered that the previous
implementation had cut away.
"""

from __future__ import annotations

import re

_CFG_TEST_RE = re.compile(r"^[ \t]*#\[cfg\(test\)\][ \t]*\r?\n", re.MULTILINE)
_ATTR_LINE_RE = re.compile(r"^[ \t]*#\[[^\n]*\][ \t]*\r?\n")


def _skip_string(text: str, i: int) -> int:
    """Index just past the string literal starting at `i`."""
    n = len(text)
    if text[i] == "r" and i + 1 < n and text[i + 1] in '#"':
        j = i + 1
        while j < n and text[j] == "#":
            j += 1
        if j < n and text[j] == '"':
            hashes = text[i + 1 : j]
            end = text.find('"' + hashes, j + 1)
            if end == -1:
                return n
            return end + 1 + len(hashes)
        return i + 1
    if text[i] == '"':
        j = i + 1
        while j < n:
            if text[j] == "\\":
                j += 2
                continue
            if text[j] == '"':
                return j + 1
            j += 1
        return n
    # char literal or lifetime
    if i + 2 < n and text[i + 1] == "\\":
        end = text.find("'", i + 2)
        return n if end == -1 else end + 1
    if i + 2 < n and text[i + 2] == "'":
        return i + 3
    return i + 1


def _skip_block(text: str, i: int) -> int:
    """Index just past the `}` matching the `{` at `i`."""
    n = len(text)
    depth = 0
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            nl = text.find("\n", i)
            if nl == -1:
                return n
            i = nl + 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            end = text.find("*/", i + 2)
            if end == -1:
                return n
            i = end + 2
            continue
        if c == '"' or (c == "r" and i + 1 < n and text[i + 1] in '#"'):
            i = _skip_string(text, i)
            continue
        if c == "'":
            i = _skip_string(text, i)
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return n


def _skip_group(text: str, i: int) -> int:
    """Index just past the `)`/`]` matching the opener at `i`."""
    n = len(text)
    close = ")" if text[i] == "(" else "]"
    depth = 0
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            nl = text.find("\n", i)
            if nl == -1:
                return n
            i = nl + 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            end = text.find("*/", i + 2)
            if end == -1:
                return n
            i = end + 2
            continue
        if c == '"' or (c == "r" and i + 1 < n and text[i + 1] in '#"'):
            i = _skip_string(text, i)
            continue
        if c == "'":
            i = _skip_string(text, i)
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return i + 1
            if c == close:
                return i + 1
        i += 1
    return n


def _skip_item(text: str, start: int) -> int:
    """Index just past the `#[cfg(test)]`-attributed item beginning at `start`.

    The attributed thing is NOT always an item: a `#[cfg(test)]` also appears
    on a struct FIELD (`f: Type,`), on a struct-LITERAL field (`f: expr,`),
    on a call ARGUMENT (`None,`) and on a STATEMENT (`if let .. { .. }`).
    Each of those has its own terminator, and getting it wrong deletes
    production code:

      * a brace-bodied item (`fn`, `impl`, `mod`, `struct { .. }`) ends after
        its matching `}` — braces inside strings do not count;
      * a field or argument ends at the `,` that ends its LINE — a comma
        inside a generic argument list (`HashMap<String, u32>`) or inside a
        call (`map(f)`) does NOT end the item, so the scan keeps going;
      * a statement or `use` ends at its `;`.

    Returns `len(text)` when the item is unterminated, which degrades to the
    old cut-the-tail behaviour.
    """
    n = len(text)
    i = start
    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            nl = text.find("\n", i)
            if nl == -1:
                return n
            i = nl + 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            end = text.find("*/", i + 2)
            if end == -1:
                return n
            i = end + 2
            continue
        if c == '"' or (c == "r" and i + 1 < n and text[i + 1] in '#"'):
            i = _skip_string(text, i)
            continue
        if c == "'":
            i = _skip_string(text, i)
            continue
        if c == "{":
            return _skip_block(text, i)
        if c in "([":
            i = _skip_group(text, i)
            continue
        if c in ")]}":
            # The enclosing scope closed: the attributed item ended before it.
            return i
        if c == ";":
            return i + 1
        if c == ",":
            nl = text.find("\n", i)
            line_end = n if nl == -1 else nl
            if text[i + 1 : line_end].strip() == "":
                return i + 1
            i += 1
            continue
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
