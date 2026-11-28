#!/usr/bin/env python3
"""Defined-vs-consumed sweep (v2) for the three audited modules.

For every `pub`/`pub(crate)` item exported by the audited module trees, count
how many NON-TEST files elsewhere in the repo mention the identifier. Zero
mentions outside the defining file = "no consumer" (Form 1) candidate.

v2 fixes two bugs in sweep.py:
  * the definition regex now runs with re.MULTILINE (v1 matched only the
    first line of the file and therefore found NOTHING - a green but blind
    guard);
  * the scan root is a CLI argument, so the same sweep can be run against the
    main checkout and against the fix worktree.
"""
import argparse
import collections
import os
import re

SKIP_DIRS = {
    ".git",
    "target",
    "node_modules",
    "graphify-out",
    ".worktrees",
    "dist",
    "build",
    ".superpowers",
    ".analysis",
}

DEF_RE = re.compile(
    r"^[ \t]*pub(?:\(crate\))?[ \t]+"
    r"(?:async[ \t]+|const[ \t]+|unsafe[ \t]+|extern[ \t]+)*"
    r"(fn|struct|enum|trait|type|const|static|mod|union)[ \t]+"
    r"([A-Za-z_][A-Za-z0-9_]*)",
    re.MULTILINE,
)
TOKEN_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
PATH_QUAL_RE = re.compile(r"::([A-Za-z_][A-Za-z0-9_]*)")
IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def strip_test_blocks(text: str) -> str:
    """Blank out `#[cfg(test)]` blocks so test-only uses do not count."""
    out = []
    i, n = 0, len(text)
    while i < n:
        idx = text.find("#[cfg(test)]", i)
        if idx == -1:
            out.append(text[i:])
            break
        out.append(text[i:idx])
        j = text.find("{", idx)
        if j == -1:
            break
        depth, k = 0, j
        while k < n:
            if text[k] == "{":
                depth += 1
            elif text[k] == "}":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        out.append(" " * (k - idx))
        i = k + 1
    return "".join(out)


def load(root: str):
    files = {}
    for base, dirs, names in os.walk(root):
        dirs[:] = [d for d in dirs if d not in SKIP_DIRS]
        for name in names:
            if name.endswith(".rs"):
                path = os.path.join(base, name)
                rel = os.path.relpath(path, root)
                with open(path, encoding="utf-8", errors="ignore") as handle:
                    files[rel] = handle.read()
    return files


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default="/home/zou/data/workspace/Aleph")
    parser.add_argument(
        "--modules",
        default="src/generation,src/goal,src/group_chat",
        help="comma-separated module prefixes to sweep",
    )
    args = parser.parse_args()

    modules = tuple(m.strip() for m in args.modules.split(",") if m.strip())
    files = load(args.root)

    token_files: dict[str, set[str]] = collections.defaultdict(set)
    path_files: dict[str, set[str]] = collections.defaultdict(set)
    for rel, text in files.items():
        code = strip_test_blocks(text)
        for token in set(TOKEN_RE.findall(code)):
            token_files[token].add(rel)
        for token in set(PATH_QUAL_RE.findall(code)):
            path_files[token].add(rel)

    for module in modules:
        definitions: dict[str, set[str]] = collections.defaultdict(set)
        kinds: dict[str, str] = {}
        for rel, text in files.items():
            if not rel.startswith(module + "/"):
                continue
            for kind, name in DEF_RE.findall(text):
                definitions[name].add(rel)
                kinds.setdefault(name, kind)
        print(f"=== {module}: exported symbols with NO production mention outside their file ===")
        found = 0
        for name, owners in sorted(definitions.items()):
            users = (token_files.get(name, set()) | path_files.get(name, set())) - owners
            if users:
                continue
            own = sum(len(IDENT_RE.findall(files[rel])) and files[rel].count(name) for rel in owners)
            print(f"  {name:44s} {kinds[name]:7s} owners={sorted(owners)} own_mentions={own}")
            found += 1
        print(f"  ({found} candidates)")


if __name__ == "__main__":
    main()
