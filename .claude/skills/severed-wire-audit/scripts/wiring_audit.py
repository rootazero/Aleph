#!/usr/bin/env python3
"""Generic "severed wire" guard — DEFINED - CONSUMED symbol parity at a seam.

A wire is reachable only when a symbol is BOTH defined (produced/registered on
side A) AND consumed (dispatched/read/subscribed on side B). A symbol present on
A but absent from B is a severed wire: the code exists and may have tests, but
its far end is never reached. `dead_code` lints can't see this (the producer
isn't dead; the consumer is a stub / another crate / an inert read).

This is the Aleph tool-registration guard generalized. Configure one SeamSpec
per seam (registration, RPC call-vs-handler, config-reader, event emit-vs-sub),
then `run_audit([...])`. Language-agnostic: only the regexes are syntax-specific.

    SEVERED = DEFINED - CONSUMED
    exit non-zero when  SEVERED - known_severed  is non-empty.

`known_severed` is the triaged baseline (intentionally severed / deferred). It
only ever SHRINKS: fix a wire, delete its baseline entry, and a NEW severance
turns the check red. A guard is a DETECTOR, not a judge — it says a wire is
severed, not whether to reconnect or delete it (that's a human/read decision).
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

_TEST_MOD_RE = re.compile(r"\n\s*#\[cfg\(test\)\]")


@dataclass
class SeamSpec:
    """One seam to audit.

    name           human label for the report.
    defined_glob   glob (relative to root) for files declaring produced symbols.
    defined_re     regex with ONE capture group = the produced symbol name.
    consumed_files explicit file list (relative to root) for the consumer side.
    consumed_glob  OR a glob for the consumer side (use one of files/glob).
    consumed_re    regex with ONE capture group = the consumed symbol name.
    known_severed  triaged baseline of intentionally-severed names (shrinks only).
    strip_tests    cut each file at the first `#[cfg(test)]` before scanning, so
                   a test-only registration can't mask a production severance.
                   Set False for non-Rust or when test code is irrelevant.
    """

    name: str
    defined_glob: str
    defined_re: str
    consumed_re: str
    consumed_files: list[str] = field(default_factory=list)
    consumed_glob: str = ""
    known_severed: set[str] = field(default_factory=set)
    strip_tests: bool = True

    def __post_init__(self) -> None:
        # Fail-fast on the two footguns of adapting this template to a new seam:
        # (1) forgetting / doubling the consumer source, (2) a regex whose capture
        # group count != 1 (we always read m.group(1)).
        if bool(self.consumed_files) == bool(self.consumed_glob):
            raise ValueError(
                f"SeamSpec '{self.name}': set exactly ONE of consumed_files / consumed_glob"
            )
        for label, pat in (("defined_re", self.defined_re), ("consumed_re", self.consumed_re)):
            if re.compile(pat).groups != 1:
                raise ValueError(
                    f"SeamSpec '{self.name}': {label} must have exactly one capture group "
                    "(the symbol name)"
                )


def _strip_tests(text: str, enabled: bool) -> str:
    if not enabled:
        return text
    m = _TEST_MOD_RE.search(text)
    return text[: m.start()] if m else text


def _scan(files: list[Path], pat: re.Pattern[str], root: Path, strip: bool) -> dict[str, str]:
    """Return {symbol -> 'relpath:line' of first occurrence}."""
    found: dict[str, str] = {}
    for f in files:
        try:
            text = _strip_tests(f.read_text(encoding="utf-8", errors="replace"), strip)
        except OSError:
            continue
        rel = f.relative_to(root).as_posix()
        for m in pat.finditer(text):
            line = text.count("\n", 0, m.start()) + 1
            found.setdefault(m.group(1), f"{rel}:{line}")
    return found


def _consumer_files(spec: SeamSpec, root: Path) -> list[Path]:
    if spec.consumed_glob:
        return sorted(root.glob(spec.consumed_glob))
    return [root / f for f in spec.consumed_files]


def audit_seam(spec: SeamSpec, root: Path) -> tuple[list[str], dict[str, str]]:
    """Return (unexpected_severed_names, defined_map)."""
    defined = _scan(
        sorted(root.glob(spec.defined_glob)),
        re.compile(spec.defined_re),
        root,
        spec.strip_tests,
    )
    consumed = set(
        _scan(
            _consumer_files(spec, root),
            re.compile(spec.consumed_re, re.MULTILINE),
            root,
            spec.strip_tests,
        )
    )
    severed = sorted(set(defined) - consumed)
    unexpected = sorted(set(severed) - spec.known_severed)

    print(f"\n=== [{spec.name}] DEFINED {len(defined)} | CONSUMED {len(consumed)} "
          f"| severed {len(severed)} ({len(unexpected)} new) ===")
    for name in severed:
        tag = "  [known]" if name in spec.known_severed else "  [NEW!] "
        print(f"{tag}{name}  {defined[name]}")
    return unexpected, defined


def run_audit(specs: list[SeamSpec], root: str = ".") -> int:
    """Run every seam; exit non-zero if any has a severance beyond its baseline."""
    root_path = Path(root).resolve()
    failed: list[tuple[str, list[str], dict[str, str]]] = []
    for spec in specs:
        unexpected, defined = audit_seam(spec, root_path)
        if unexpected:
            failed.append((spec.name, unexpected, defined))

    if failed:
        print("\nFAIL: new severed wire(s) beyond the triaged baseline:")
        for seam_name, names, defined in failed:
            for n in names:
                print(f"  [{seam_name}] {n}  {defined[n]}")
        return 1
    print("\nOK: no severed wires beyond the triaged baseline.")
    return 0


# --- Self-test: run `python3 wiring_audit.py --selftest` to verify the engine ---
if __name__ == "__main__":
    if "--selftest" in sys.argv:
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            (root / "producers").mkdir()
            # alpha + beta defined; only alpha dispatched; gamma dispatched-but-undefined.
            (root / "producers" / "a.rs").write_text(
                'const NAME: &str = "alpha";\n'
                'const NAME: &str = "beta";\n'
                '#[cfg(test)]\nconst NAME: &str = "test_only";\n'
            )
            (root / "dispatch.rs").write_text('    "alpha" => run(),\n    "gamma" => run(),\n')
            spec = SeamSpec(
                name="selftest",
                defined_glob="producers/**/*.rs",
                defined_re=r'const NAME:\s*&(?:\'static\s+)?str\s*=\s*"([^"]+)"',
                consumed_files=["dispatch.rs"],
                consumed_re=r'^\s*"([a-z][a-zA-Z0-9_]*)"\s*=>',
            )
            unexpected, _ = audit_seam(spec, root)
            assert unexpected == ["beta"], f"expected ['beta'], got {unexpected}"
            # baseline suppresses it:
            spec.known_severed = {"beta"}
            unexpected2, _ = audit_seam(spec, root)
            assert unexpected2 == [], f"expected [], got {unexpected2}"

            # __post_init__ fail-fast: neither consumer source, both sources, bad group count.
            def _expect_valueerror(label: str, **kw) -> None:
                base = dict(name="x", defined_glob="**/*.rs",
                            defined_re=r'"([^"]+)"', consumed_re=r'"([^"]+)"')
                try:
                    SeamSpec(**{**base, **kw})
                except ValueError:
                    return
                raise AssertionError(f"expected ValueError for {label}")

            _expect_valueerror("neither consumer source")  # no files, no glob
            _expect_valueerror("both consumer sources", consumed_files=["d.rs"], consumed_glob="d/*")
            _expect_valueerror("zero-group defined_re", consumed_files=["d.rs"], defined_re=r'"[^"]+"')
            _expect_valueerror("two-group consumed_re", consumed_files=["d.rs"], consumed_re=r'"(a)(b)"')

            print("\nselftest OK: severed+baseline+test-strip correct; SeamSpec validation fail-fast.")
        sys.exit(0)

    print(__doc__)
    print("Import this module and call run_audit([SeamSpec(...)]). "
          "Run with --selftest to verify the engine.")
    sys.exit(0)
