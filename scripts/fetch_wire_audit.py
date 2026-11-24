#!/usr/bin/env python3
"""Fetch wire-seam guard - invariants the 2026-11-22 src/fetch audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/fetch_wire_audit.py

Exit non-zero when any check fails.
"""
import sys
from pathlib import Path

try:
    from wiring_strip import strip_tests as _repo_strip_tests
except ImportError:  # pragma: no cover - standalone use
    _repo_strip_tests = None

ROOT = Path(__file__).resolve().parent.parent
BAD = []


def read(p):
    return (ROOT / p).read_text(encoding="utf-8")


def prod(p):
    text = read(p)
    if _repo_strip_tests is not None:
        return _repo_strip_tests(text)
    return text.split("#[cfg(test)]")[0]


def bad(check, detail):
    BAD.append(f"{check}: {detail}")


def absent(check, path, needle, note):
    if needle in read(path):
        bad(check, f"{path} still carries {needle!r} - {note}")


def absent_prod(check, path, needle, note):
    if needle in prod(path):
        bad(check, f"{path} still carries {needle!r} in production code - {note}")


def present(check, path, needle, note):
    if needle not in read(path):
        bad(check, f"{path} no longer carries {needle!r} - {note}")


def present_prod(check, path, needle, note):
    if needle not in prod(path):
        bad(check, f"{path} no longer carries {needle!r} in production code - {note}")


# ---------------------------------------------------------------------------
# CUT: the inert routing knobs stay cut
# ---------------------------------------------------------------------------


def check_config_knobs_stay_cut():
    """`[fetch].enabled` / `default_provider` / `fallback_providers` stay cut.

    Nothing read them: the agent-facing `web_fetch` path never routes
    through a fetch provider (BT-D-R4-22), so the knobs only ever told the
    operator that a routing decision was being made. The section now holds
    backends only, and the boot warn reports the backends that exist."""
    body = read("src/config/types/fetch.rs")
    # Scope the section-level check to `FetchConfigInternal`: the per-backend
    # `enabled` gate (FetchBackendConfig) is live and must stay.
    section = body[body.find("pub struct FetchConfigInternal"):]
    section = section[: section.find("\n}")]
    for field in ("pub enabled:", "pub default_provider:", "pub fallback_providers:"):
        if field in section:
            bad("fetch/cut", f"src/config/types/fetch.rs still declares {field!r} on the section "
                             f"- the knob has no reader")
    for field in ("pub default_provider:", "pub fallback_providers:"):
        if field in body:
            bad("fetch/cut", f"src/config/types/fetch.rs still declares {field!r} - the knob has "
                             f"no reader")
    absent("fetch/cut", "src/config/structs.rs", "default_provider",
           "the migration must not re-introduce the routing knob")
    dto = read("src/gateway/handlers/fetch_config.rs")
    for field in ("pub enabled:", "pub default_provider:"):
        if field in dto:
            bad("fetch/cut", f"src/gateway/handlers/fetch_config.rs still declares {field!r} in "
                             f"the DTO - the Panel would show a toggle that changes nothing")


def check_fetch_section_still_backends():
    """The section must keep its only real content: the backends."""
    present("fetch/cut", "src/config/types/fetch.rs", "pub backends:",
            "the backend map is the section's real content")
    present("fetch/cut", "src/config/types/fetch.rs", "pub verified_shared:",
            "the verified side-channel is the section's real content")


# ---------------------------------------------------------------------------
# LIVE: the reconnected wires stay connected
# ---------------------------------------------------------------------------


def check_boot_warn_live():
    """The inert-backend warn must keep its producer.

    With the routing knobs gone, the operator learns that a backend is
    configured from the boot warn - the warn is the only remaining signal
    that a `[fetch]` section exists."""
    present_prod("fetch/live", "src/executor/builtin_registry/builder/constructor/mod.rs",
                 "fn inert_fetch_backend_names",
                 "the boot warn's helper must stay")
    present_prod("fetch/live", "src/executor/builtin_registry/builder/constructor/mod.rs",
                 "inert_fetch_backend_names(fetch_cfg",
                 "the boot warn must still be called")


def check_test_surface_live():
    """`fetch_config.test` must stay the operator's connection test."""
    present_prod("fetch/live", "src/gateway/handlers/fetch_config.rs", "pub async fn handle_test(",
                 "the connection test must stay reachable")
    present("fetch/live", "src/gateway/handlers/fetch_config.rs", '"crawl4ai"',
            "the crawl4ai backend must stay testable")
    present("fetch/live", "src/gateway/handlers/fetch_config.rs", '"firecrawl"',
            "the firecrawl backend must stay testable")


# ---------------------------------------------------------------------------
# DARK: the deliberately dark wire stays as it is
# ---------------------------------------------------------------------------


def check_provider_wiring_stays_dark():
    """The fetch providers stay out of the agent-facing `web_fetch` path.

    A provider resolves the target URL from its own network position, so the
    SSRF DNS pin cannot be enforced on the fetch that actually happens. The
    module documents this (BT-D-R4-22) and the marker must stay, so a later
    audit does not wire it up by accident."""
    doc = read("src/fetch/mod.rs")
    if "BT-D-R4-22" not in doc:
        bad("fetch/dark", "src/fetch/mod.rs lost its BT-D-R4-22 marker - the deliberate dark wire "
                         "must stay documented")
    if "These providers are **not** wired into `web_fetch`" not in doc:
        bad("fetch/dark", "src/fetch/mod.rs no longer states that the providers are not wired "
                         "into web_fetch")


def main():
    check_config_knobs_stay_cut()
    check_fetch_section_still_backends()
    check_boot_warn_live()
    check_test_surface_live()
    check_provider_wiring_stays_dark()
    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: fetch wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
