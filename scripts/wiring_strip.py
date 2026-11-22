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

_CFG_ATTR_RE = re.compile(r"^[ \t]*#\[cfg\((?P<pred>[^\]\n]*)\)\][ \t]*\r?\n", re.MULTILINE)
_ATTR_LINE_RE = re.compile(r"^[ \t]*#\[[^\n]*\][ \t]*\r?\n")
# A module DECLARATION (`mod foo;`). The sibling file `foo.rs` (or
# `foo/mod.rs`) is the whole body of that module, so a declaration behind a
# test-only cfg means that file is compiled only under `cfg(test)`: none of
# its symbols exist in a production build. Reading them as production symbols
# turns every helper in such a file into a "severed wire" that never existed.
_MOD_DECL_RE = re.compile(
    r"[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;"
)

# `#[cfg(feature = "test-helpers")]` — a test-only feature (it exists so an
# integration test can reach a fake the library otherwise keeps private).
# The guards treat it like `cfg(test)`.
_TEST_ONLY_FEATURE = "test-helpers"


def _cfg_tokens(pred: str) -> list[str]:
    """Tokenise a `cfg` predicate: idents, strings, `=`, `(`, `)`, `,`."""
    out: list[str] = []
    i = 0
    n = len(pred)
    while i < n:
        c = pred[i]
        if c.isspace():
            i += 1
            continue
        if c in "(),":
            out.append(c)
            i += 1
            continue
        if c == "=":
            out.append("=")
            i += 1
            continue
        if c == '"':
            j = pred.find('"', i + 1)
            if j == -1:
                out.append(pred[i:])
                break
            out.append(pred[i : j + 1])
            i = j + 1
            continue
        j = i
        while j < n and (pred[j].isalnum() or pred[j] == "_"):
            j += 1
        if j == i:
            i += 1
            continue
        out.append(pred[i:j])
        i = j
    return out


def _cfg_leaf(name: str, value: str | None) -> str:
    """Three-valued answer for a leaf predicate.

    "F" — cannot hold in a production build (test-only).
    "U" — unknown: it may hold in a production build (a platform gate, a
    shipping feature, a debug build). Kept, so the guards still see the
    production surface.
    """
    if name == "test":
        return "F"
    if name == "feature" and value == _TEST_ONLY_FEATURE:
        return "F"
    return "U"


def _cfg_and(vals: list[str]) -> str:
    if "F" in vals:
        return "F"
    if all(v == "T" for v in vals):
        return "T"
    return "U"


def _cfg_or(vals: list[str]) -> str:
    if "T" in vals:
        return "T"
    if all(v == "F" for v in vals):
        return "F"
    return "U"


def _cfg_parse(toks: list[str], i: int) -> tuple[str, int]:
    tok = toks[i]
    if tok in ("all", "any"):
        i += 1
        if i >= len(toks) or toks[i] != "(":
            raise ValueError("expected (")
        i += 1
        vals: list[str] = []
        while True:
            v, i = _cfg_parse(toks, i)
            vals.append(v)
            if i < len(toks) and toks[i] == ",":
                i += 1
                continue
            break
        if i >= len(toks) or toks[i] != ")":
            raise ValueError("expected )")
        i += 1
        return (_cfg_and(vals) if tok == "all" else _cfg_or(vals)), i
    if tok == "not":
        i += 1
        if i >= len(toks) or toks[i] != "(":
            raise ValueError("expected (")
        v, i = _cfg_parse(toks, i + 1)
        if i >= len(toks) or toks[i] != ")":
            raise ValueError("expected )")
        return ("T" if v == "F" else "F" if v == "T" else "U"), i + 1
    name = tok
    value = None
    if i + 2 < len(toks) and toks[i + 1] == "=":
        value = toks[i + 2].strip('"')
        i += 3
    else:
        i += 1
    return _cfg_leaf(name, value), i


def _cfg_production(pred: str) -> bool:
    """True when the `cfg` predicate CAN hold in a production build.

    The guards read production code, so an item that a shipping build does
    not compile must not be read as production code. The shapes this repo
    uses for that are `cfg(test)`, `cfg(feature = "test-helpers")`,
    `cfg(any(test, feature = "test-helpers"))` and
    `cfg(all(test, ...))`.

    A platform gate (`cfg(windows)`, `cfg(target_os = "macos")`,
    `cfg(not(unix))`) is PRODUCTION code: it ships in a build for that
    platform, and hiding it would hide real severances. An unknown
    predicate is treated the same way (conservative: it may still ship).
    """
    toks = _cfg_tokens(pred)
    if not toks:
        return True
    try:
        value, _ = _cfg_parse(toks, 0)
    except (ValueError, IndexError):
        return True
    return value != "F"


def test_only_module_names(text: str) -> set[str]:
    """Names of modules declared behind a test-only `cfg`."""
    out: set[str] = set()
    for m in _CFG_ATTR_RE.finditer(text):
        if _cfg_production(m.group("pred")):
            continue
        decl = _MOD_DECL_RE.match(text, m.end())
        if decl:
            out.add(decl.group(1))
    return out


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
    """`text` with every item a PRODUCTION build would not compile removed.

    An item is dropped when its `cfg` predicate cannot hold in a production
    build: `cfg(test)`, `cfg(feature = "test-helpers")`,
    `cfg(any(test, ...))` and `cfg(all(test, ...))`. A `cfg` that CAN hold in
    production keeps its item (and its attribute) — the guards must see the
    production surface, not a guess.
    """
    out: list[str] = []
    pos = 0
    while True:
        m = _CFG_ATTR_RE.search(text, pos)
        if not m:
            out.append(text[pos:])
            return "".join(out)
        out.append(text[pos : m.start()])
        if _cfg_production(m.group("pred")):
            out.append(m.group(0))
            pos = m.end()
            continue
        pos = _skip_item(text, m.end())
