#!/usr/bin/env python3
"""Discovery wire-seam guard - invariants the 2026-11-21 discovery audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/discovery_wire_audit.py

Exit non-zero when any check fails.

Two checks live here:

* PARITY - the manifest names DISCOVERY advertises, the names the manifest
           ADAPTERS detect and the names the LOADER advertises must describe
           the same world. A name that exists on one side only is either a
           plugin nobody can find or a directory that is claimed but never
           parsed - both are silent failures.
* SWALLOW - a `read_dir` failure during a PLUGIN scan must reach
           `PluginDiscovery::unreadable` (the "unknown, not absent" channel)
           or carry an explicit `// allow:` justification. A swallowed failure
           is reported downstream as "no plugin here", which is a lie.
"""
import re
import sys
from pathlib import Path

try:  # the repo's brace-matching test stripper is more precise than a cut
    from wiring_strip import strip_tests as _repo_strip_tests
except ImportError:  # pragma: no cover - standalone use
    _repo_strip_tests = None

ROOT = Path(__file__).resolve().parent.parent
BAD = []

# ===========================================================================
# Deferred manifest formats.
#
# A name may live on ONE side of the parity check only when it is listed here
# with a one-line justification. The allowlist is the ratchet: every entry
# names a KNOWN divergence the audit deliberately did not repair, so the
# check passes today and fails on any NEW divergence. An entry that is no
# longer needed is itself a finding - the justification has to be revisited
# instead of decaying into a rubber stamp.
# ===========================================================================
ALLOWLIST = {
    "aleph.plugin.json": (
        "the V1 JSON manifest: discovery advertises it so a plugin shipping "
        "only aleph.plugin.json is still FOUND (dropping the entry would "
        "hide it), but no adapter parses it yet (audit D1-03/D3-01)"
    ),
    ".lsp.json": (
        "the loader advertises it in COMPONENT_DIRS, but no LSP adapter, "
        "plugin kind or capability exists and discovery does not look for it "
        "(audit D1-04/D3-05)"
    ),
}


def read(p):
    return (ROOT / p).read_text(encoding="utf-8")


def prod(p):
    """Production code only: every `#[cfg(test)]` item removed."""
    text = read(p)
    if _repo_strip_tests is not None:
        return _repo_strip_tests(text)
    return text.split("#[cfg(test)]")[0]


def bad(check, detail):
    BAD.append(f"{check}: {detail}")


# ---------------------------------------------------------------------------
# Tiny Rust source readers (the audit facts live in consts and fn bodies)
# ---------------------------------------------------------------------------


def _skip_string(text, i):
    """Index just past the string/char literal starting at `i`."""
    quote = text[i]
    i += 1
    while i < len(text):
        if text[i] == "\\":
            i += 2
            continue
        if text[i] == quote:
            return i + 1
        i += 1
    return i


def _match_pair(text, i, open_ch, close_ch):
    """Index of the closer matching the opener at `i` (strings skipped)."""
    depth = 0
    while i < len(text):
        c = text[i]
        if c == '"' or (c == "r" and i + 1 < len(text) and text[i + 1] in '#"'):
            i = _skip_string(text, i) if c == '"' else _skip_raw_string(text, i)
            continue
        if c == open_ch:
            depth += 1
        elif c == close_ch:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def _skip_raw_string(text, i):
    hashes = 0
    j = i + 1
    while j < len(text) and text[j] == "#":
        hashes += 1
        j += 1
    end = text.find('"' + "#" * hashes, j)
    return len(text) if end < 0 else end + 1 + hashes


def brace_block(text, open_idx):
    """The `{ ... }` block starting at `open_idx`, braces in strings ignored."""
    end = _match_pair(text, open_idx, "{", "}")
    return text[open_idx:] if end < 0 else text[open_idx:end + 1]


def strip_comments(text):
    """Rust source with `//` and `/* */` comments removed (strings kept)."""
    out = []
    i = 0
    while i < len(text):
        c = text[i]
        if c == '"':
            j = _skip_string(text, i)
            out.append(text[i:j])
            i = j
            continue
        if c == "/" and i + 1 < len(text) and text[i + 1] == "/":
            nl = text.find("\n", i)
            i = len(text) if nl < 0 else nl
            continue
        if c == "/" and i + 1 < len(text) and text[i + 1] == "*":
            end = text.find("*/", i + 2)
            i = len(text) if end < 0 else end + 2
            continue
        out.append(c)
        i += 1
    return "".join(out)


def fn_body(text, name):
    """Body of the first `fn <name>` that has one."""
    for m in re.finditer(r"\bfn\s+" + re.escape(name) + r"\s*\(", text):
        open_paren = m.end() - 1
        close_paren = _match_pair(text, open_paren, "(", ")")
        if close_paren < 0:
            continue
        j = close_paren + 1
        while j < len(text):
            c = text[j]
            if c == '"':
                j = _skip_string(text, j)
                continue
            if c == "{":
                return brace_block(text, j)
            if c == ";":
                return None  # a declaration, not a definition
            j += 1
    return None


def const_map(rel_paths):
    """`NAME -> "&str"` for every `const NAME: &str = "..."` in the given files."""
    out = {}
    for rel in rel_paths:
        for m in re.finditer(
            r"\bconst\s+([A-Z_][A-Z0-9_]*)\s*:\s*&str\s*=\s*\"([^\"]+)\"", prod(rel)
        ):
            out.setdefault(m.group(1), m.group(2))
    return out


def joined_names(body, consts):
    """The path fragments a Rust body looks for via `dir.join(...)`."""
    names = []
    for lit, ident in re.findall(r"\.join\(\s*\"([^\"]+)\"\s*\)|\.join\(\s*([A-Z_][A-Z0-9_]*)\s*\)", body):
        if lit:
            names.append(lit)
        elif ident in consts:
            names.append(consts[ident])
        else:
            names.append(ident)  # unresolved const: reported by the caller
    return names


# ---------------------------------------------------------------------------
# The three name universes
# ---------------------------------------------------------------------------


def discovery_names():
    """(files, dirs) discovery advertises as "this directory is a plugin"."""
    text = prod("src/discovery/scanner.rs")
    consts = const_map(
        sorted(str(p.relative_to(ROOT)) for p in (ROOT / "src" / "discovery").rglob("*.rs"))
    )
    m = re.search(r"const\s+MANIFEST_REL_PATHS[^=]*=\s*&\[(.*?)\];", text, re.S)
    if not m:
        return None, None
    files = []
    block = strip_comments(m.group(1))
    for lit, ident in re.findall(r"\"([^\"]+)\"|([A-Z_][A-Z0-9_]*)", block):
        if lit:
            files.append(lit)
        elif ident in consts:
            files.append(consts[ident])
        else:
            files.append(ident)  # unresolved const name
    dirs = re.findall(r"is_existing_dir_no_follow\(\s*&path\.join\(\s*\"([^\"]+)\"\s*\)\s*\)", text)
    return files, dirs


def adapter_names():
    """Every path fragment a manifest adapter detects."""
    consts = const_map(
        sorted(
            str(p.relative_to(ROOT))
            for p in (ROOT / "src" / "extension" / "manifest").rglob("*.rs")
        )
    )
    names = set()
    for path in sorted((ROOT / "src" / "extension" / "manifest").rglob("*.rs")):
        body = fn_body(prod(str(path.relative_to(ROOT))), "detect")
        if body:
            names.update(joined_names(body, consts))
    return names


def loader_names():
    """The component locations the loader advertises."""
    text = prod("src/extension/manifest/mod.rs")
    m = re.search(r"const\s+COMPONENT_DIRS[^=]*=\s*&\[(.*?)\];", text, re.S)
    if not m:
        return None
    return re.findall(r"\(\s*\"([^\"]+)\"\s*,", m.group(1))


# ---------------------------------------------------------------------------
# Checks
# ---------------------------------------------------------------------------


def check_manifest_name_parity():
    """Discovery, the adapters and the loader must name the same formats.

    A manifest name that only discovery knows makes a plugin visible but
    unparseable; a name only the adapters know makes a directory invisible to
    the whole pipeline (the 2026-11-21 audit's `hooks.json` case).
    """
    files, dirs = discovery_names()
    if files is None:
        bad("discovery/parity", "src/discovery/scanner.rs no longer declares MANIFEST_REL_PATHS")
        return
    adapters = adapter_names()
    loader = loader_names()
    if loader is None:
        bad("discovery/parity", "src/extension/manifest/mod.rs no longer declares COMPONENT_DIRS")
        return

    known = set(files) | set(dirs)

    for name in files:
        if name not in adapters and name not in ALLOWLIST:
            bad(
                "discovery/parity",
                f"discovery advertises {name!r} but no adapter parses it "
                f"(add an adapter, or a documented allowlist entry)",
            )

    for name in sorted(adapters):
        if name not in known and name not in ALLOWLIST:
            bad(
                "discovery/parity",
                f"an adapter detects {name!r} but discovery never looks for it "
                f"(the plugin stays invisible)",
            )

    for name in loader:
        first = name.split("/")[0]
        if name not in known and first not in known and name not in ALLOWLIST:
            bad(
                "discovery/parity",
                f"the loader advertises {name!r} but discovery never looks for it",
            )

    for name, why in ALLOWLIST.items():
        if not why.strip():
            bad("discovery/parity", f"the allowlist entry {name!r} has no justification")
        elif name in known and name in adapters:
            bad(
                "discovery/parity",
                f"the allowlist entry {name!r} is no longer a divergence - "
                f"remove it and let the check police the format again",
            )


def check_no_swallowed_scan_failure():
    """A plugin scan that cannot read a directory must say so.

    `PluginDiscovery::unreadable` is the "unknown, not absent" channel the
    extension registry and the usage report already consume. A `read_dir`
    failure that only reaches a debug log is reported as "no plugin here".
    """
    text = prod("src/discovery/scanner.rs")
    for m in re.finditer(r"read_dir\(", text):
        window = text[m.end():m.end() + 800]
        err = re.search(r"Err\(", window)
        if not err:
            continue
        arrow = window.find("=>", err.end())
        if arrow < 0:
            continue
        brace = window.find("{", arrow)
        if brace < 0:
            continue
        body = brace_block(window, brace)
        if "unreadable" in body:
            continue
        allow = re.search(r"//\s*allow:(.*)", body)
        if allow:
            if len(allow.group(1).strip()) < 40:
                bad(
                    "discovery/swallow",
                    f"src/discovery/scanner.rs: {m.start()} the `// allow:` "
                    f"justification is too short to be a justification",
                )
            continue
        bad(
            "discovery/swallow",
            f"src/discovery/scanner.rs: {m.start()} a read_dir failure is "
            f"swallowed into a debug log - it must reach "
            f"`PluginDiscovery::unreadable` or carry a `// allow:` comment",
        )


CHECKS = [
    ("manifest-name parity", check_manifest_name_parity),
    ("no silently swallowed scan failure", check_no_swallowed_scan_failure),
]


def main():
    results = []
    for name, fn in CHECKS:
        before = len(BAD)
        fn()
        results.append((name, len(BAD) == before))

    print("discovery wire audit")
    for name, ok in results:
        print(f"  {name:.<45} {'PASS' if ok else 'FAIL'}")

    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: discovery wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
