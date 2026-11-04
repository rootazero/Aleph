#!/usr/bin/env python3
"""Classify repository-relative Markdown links under docs/.

The checker deliberately treats links in fenced or inline code as examples,
not as repository link contracts. External links and machine-local paths are
reported separately so historical evidence is not silently rewritten.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
INLINE_CODE = re.compile(r"`[^`]*`")
PROTOCOLS = ("http://", "https://", "mailto:", "file:", "vscode:")
PLACEHOLDERS = {"url", "about:blank"}
PRIVATE_PREFIXES = ("/", "~", "T:", "C:", "D:")


@dataclass(frozen=True)
class Link:
    path: Path
    line: int
    target: str
    category: str


def strip_inline_code(line: str) -> str:
    return INLINE_CODE.sub("", line)


def classify_target(root: Path, source: Path, target: str) -> str:
    target = target.strip().split(None, 1)[0].strip("<>")
    if not target:
        return "placeholder"
    if target.startswith(PROTOCOLS):
        return "external_or_protocol"
    if target in PLACEHOLDERS:
        return "placeholder"
    if target.startswith(PRIVATE_PREFIXES):
        return "absolute_or_private"
    if target.startswith("#"):
        return "valid"

    path = target.split("#", 1)[0].split("?", 1)[0]
    if not path:
        return "valid"
    if path.startswith(("docs/", "src/", "qa/", ".github/")):
        candidate = root / path
    else:
        candidate = source.parent / path
    return "valid" if candidate.exists() else "missing_repository_targets"


def scan(root: Path) -> list[Link]:
    links: list[Link] = []
    for source in sorted((root / "docs").rglob("*.md")):
        in_fence = False
        for number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
            if line.strip().startswith("```"):
                in_fence = not in_fence
                continue
            if in_fence:
                links.extend(
                    Link(source, number, match.group(1).strip(), "code_or_example")
                    for match in LINK.finditer(line)
                )
                continue
            for code in INLINE_CODE.findall(line):
                links.extend(
                    Link(source, number, match.group(1).strip(), "code_or_example")
                    for match in LINK.finditer(code)
                )
            for match in LINK.finditer(strip_inline_code(line)):
                target = match.group(1).strip().split(None, 1)[0].strip("<>")
                links.append(Link(source, number, target, classify_target(root, source, target)))
    return links


def self_test() -> None:
    root = Path("/tmp/aleph-link-check-self-test")
    (root / "docs").mkdir(parents=True, exist_ok=True)
    (root / "docs/target.md").write_text("# target\n", encoding="utf-8")
    source = root / "docs/source.md"
    source.write_text(
        "[valid](target.md) [external](https://example.com) [private](~/.cache/x) "
        "[placeholder](url)\n"
        "`[inline](missing.md)`\n"
        "```md\n[code](missing.md)\n```\n"
        "[missing](missing.md)\n",
        encoding="utf-8",
    )
    links = scan(root)
    categories = Counter(link.category for link in links)
    assert categories == Counter(
        {
            "valid": 1,
            "external_or_protocol": 1,
            "absolute_or_private": 1,
            "placeholder": 1,
            "code_or_example": 2,
            "missing_repository_targets": 1,
        }
    ), categories
    print("self-test: PASS")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return 0

    root = args.root.resolve()
    links = scan(root)
    counts = Counter(link.category for link in links)
    doc_files = len(list((root / "docs").rglob("*.md")))
    print(f"doc_files={doc_files}")
    for category in (
        "valid",
        "external_or_protocol",
        "absolute_or_private",
        "placeholder",
        "code_or_example",
        "missing_repository_targets",
    ):
        print(f"{category}={counts[category]}")
    for link in links:
        if link.category == "missing_repository_targets":
            print(f"missing: {link.path}:{link.line}: {link.target}")
    return 1 if counts["missing_repository_targets"] else 0


if __name__ == "__main__":
    sys.exit(main())
