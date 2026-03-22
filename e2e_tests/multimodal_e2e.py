#!/usr/bin/env python3
"""Layer B — Multimodal Telegram E2E Monitor.

Monitors Aleph server log files for multimodal probe events while a human
performs test actions in Telegram. Validates probe sequences per scene.

Usage:
    Terminal 1: RUST_LOG=info,multimodal=info cargo run --bin aleph-server start
    Terminal 2: python e2e_tests/multimodal_e2e.py

Requires only Python stdlib (no pip dependencies).
"""

import glob
import os
import re
import sys
import time
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import List, Optional

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

LOG_DIR = Path.home() / ".aleph" / "logs"
TAIL_TIMEOUT_S = 60
POLL_INTERVAL_S = 0.3

# Probe line pattern:
#   <timestamp> <LEVEL> multimodal: <probe_tag> <kv_pairs> <message>
# Example:
#   2026-03-22T15:30:01Z INFO multimodal: P3_download run_id="abc" ... Media attachment resolved
PROBE_RE = re.compile(
    r"(?P<ts>\d{4}-\d{2}-\d{2}T[\d:.]+Z?)\s+"
    r"(?P<level>\w+)\s+"
    r"multimodal:\s+"
    r"(?P<tag>P\d+\w*)\s+"
    r"(?P<rest>.*)"
)

# Key-value pair extractor for structured fields
KV_RE = re.compile(r'(\w+)="([^"]*)"|\b(\w+)=(\S+)')


# ---------------------------------------------------------------------------
# Data types
# ---------------------------------------------------------------------------

@dataclass
class Probe:
    """A single parsed probe event."""
    timestamp: str
    level: str
    tag: str          # e.g. P1, P3_download, P4_native, P7
    fields: dict      # structured kv pairs
    message: str      # trailing free text
    raw: str          # original log line


@dataclass
class ProbeMatch:
    """Result of matching an expected tag against collected probes."""
    expected: str
    found: bool
    probe: Optional[Probe] = None


@dataclass
class SceneResult:
    """Outcome for one test scene."""
    name: str
    passed: bool
    matches: List[ProbeMatch] = field(default_factory=list)
    probes: List[Probe] = field(default_factory=list)
    error: Optional[str] = None


# ---------------------------------------------------------------------------
# ProbeCollector — tails log file and parses probe lines
# ---------------------------------------------------------------------------

class ProbeCollector:
    """Tails a log file from current position and collects multimodal probes."""

    def __init__(self, log_path: Path):
        self.log_path = log_path
        self._offset = log_path.stat().st_size if log_path.exists() else 0

    def collect(self, timeout_s: float = TAIL_TIMEOUT_S) -> List[Probe]:
        """Tail the log for up to timeout_s seconds, return all new probes."""
        probes: List[Probe] = []
        deadline = time.monotonic() + timeout_s

        print(f"  Tailing log for up to {timeout_s}s ... (press Ctrl-C to stop early)")
        try:
            while time.monotonic() < deadline:
                new_lines = self._read_new_lines()
                for line in new_lines:
                    probe = self._parse_line(line)
                    if probe:
                        probes.append(probe)
                        self._print_probe(probe)
                # Stop early once we've seen at least one probe and nothing
                # new in the last 5 seconds (gives time for reaction lifecycle).
                if probes and (time.monotonic() - deadline + timeout_s) > 10:
                    # We've been running >10s and have probes — check quiescence
                    pass
                time.sleep(POLL_INTERVAL_S)
        except KeyboardInterrupt:
            print("\n  (interrupted)")

        return probes

    def _read_new_lines(self) -> List[str]:
        """Read new lines since last offset."""
        try:
            size = self.log_path.stat().st_size
        except FileNotFoundError:
            return []

        if size <= self._offset:
            # File might have been rotated (smaller than before)
            if size < self._offset:
                self._offset = 0
            return []

        lines = []
        with open(self.log_path, "r", errors="replace") as f:
            f.seek(self._offset)
            for line in f:
                lines.append(line.rstrip("\n"))
            self._offset = f.tell()
        return lines

    @staticmethod
    def _parse_line(line: str) -> Optional[Probe]:
        m = PROBE_RE.search(line)
        if not m:
            return None

        rest = m.group("rest")
        fields = {}
        last_kv_end = 0
        for kv in KV_RE.finditer(rest):
            key = kv.group(1) or kv.group(3)
            val = kv.group(2) or kv.group(4)
            fields[key] = val
            last_kv_end = kv.end()

        message = rest[last_kv_end:].strip()

        return Probe(
            timestamp=m.group("ts"),
            level=m.group("level"),
            tag=m.group("tag"),
            fields=fields,
            message=message,
            raw=line,
        )

    @staticmethod
    def _print_probe(probe: Probe):
        kvs = " ".join(f"{k}={v}" for k, v in probe.fields.items())
        print(f"    [{probe.timestamp}] {probe.tag} {kvs} {probe.message}")


# ---------------------------------------------------------------------------
# Scene definitions
# ---------------------------------------------------------------------------

@dataclass
class Scene:
    number: int
    name: str
    instruction: str
    expected_probes: List[str]       # ordered tag prefixes, e.g. ["P1", "P3", "P4_native"]
    field_checks: dict = field(default_factory=dict)  # tag -> {field: expected_value}


SCENES = [
    Scene(
        number=1,
        name="Image Understanding",
        instruction=(
            'Send a PHOTO with caption "这是哪里？" to Aleph in Telegram.\n'
            "  (Pick any recognizable location photo)"
        ),
        expected_probes=["P1", "P3", "P4_native", "P5", "P7"],
        field_checks={
            "P4_native": {},
            "P5": {"has_images": "true"},
            "P7": {},
        },
    ),
    Scene(
        number=2,
        name="Voice Transcription",
        instruction=(
            "Send a VOICE MESSAGE to Aleph in Telegram.\n"
            "  (Record a short voice clip, any content)"
        ),
        expected_probes=["P1", "P3", "P4_transcribe", "P5", "P7"],
        field_checks={
            "P4_transcribe": {},
            "P5": {"has_transcripts": "true"},
        },
    ),
    Scene(
        number=3,
        name="Sticker",
        instruction="Send a STICKER to Aleph in Telegram.",
        expected_probes=["P1", "P3", "P4", "P5", "P7"],
    ),
    Scene(
        number=4,
        name="No-text Image",
        instruction=(
            "Send a PHOTO WITHOUT any caption to Aleph in Telegram.\n"
            "  (Just the image, no text)"
        ),
        expected_probes=["P1", "P3", "P4", "P5", "P7"],
    ),
    Scene(
        number=5,
        name="Image+Text Mix",
        instruction=(
            'Send a PHOTO with caption "翻译图中文字" to Aleph in Telegram.\n'
            "  (Use a photo that contains visible text)"
        ),
        expected_probes=["P1", "P3", "P4_native", "P5", "P7"],
        field_checks={
            "P4_native": {},
            "P5": {"has_images": "true"},
        },
    ),
    Scene(
        number=6,
        name="Reaction Lifecycle",
        instruction=(
            "Send any TEXT message to Aleph in Telegram.\n"
            "  (Watch for reaction changes: eyes -> thumbs up)"
        ),
        expected_probes=["P7", "P7"],
        field_checks={},
    ),
]


# ---------------------------------------------------------------------------
# Matching logic
# ---------------------------------------------------------------------------

def match_probes(expected: List[str], collected: List[Probe],
                 field_checks: dict) -> List[ProbeMatch]:
    """Match expected probe tag prefixes against collected probes in order.

    Each expected tag is matched against the earliest unmatched probe whose
    tag starts with the expected prefix.  Field checks are applied when present.
    """
    results: List[ProbeMatch] = []
    used = set()

    for exp in expected:
        found = False
        for i, p in enumerate(collected):
            if i in used:
                continue
            if not p.tag.startswith(exp.rstrip("_")):
                # Allow "P4_native" to match "P4_native" or "P4_native_vision"
                if not p.tag.startswith(exp):
                    continue

            # Check required fields if any
            checks = field_checks.get(exp, {})
            fields_ok = all(p.fields.get(k) == v for k, v in checks.items())
            if not fields_ok:
                continue

            used.add(i)
            results.append(ProbeMatch(expected=exp, found=True, probe=p))
            found = True
            break

        if not found:
            results.append(ProbeMatch(expected=exp, found=False))

    return results


# ---------------------------------------------------------------------------
# Log file finder
# ---------------------------------------------------------------------------

def find_latest_log() -> Path:
    """Find the most recent aleph-server log file."""
    pattern = str(LOG_DIR / "aleph-server.log*")
    candidates = glob.glob(pattern)
    if not candidates:
        print(f"ERROR: No log files found matching {pattern}")
        print("Make sure aleph-server is running with RUST_LOG=info,multimodal=info")
        sys.exit(1)

    # Sort by modification time, newest first
    candidates.sort(key=lambda p: os.path.getmtime(p), reverse=True)
    chosen = Path(candidates[0])
    print(f"Using log file: {chosen}")
    return chosen


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def run_scene(scene: Scene, collector: ProbeCollector) -> SceneResult:
    """Run a single test scene interactively."""
    print(f"\n{'='*60}")
    print(f"Scene {scene.number}: {scene.name}")
    print(f"{'='*60}")
    print(f"\n  ACTION REQUIRED:\n  {scene.instruction}\n")

    try:
        input("  Press ENTER when you have performed the action... ")
    except (EOFError, KeyboardInterrupt):
        return SceneResult(
            name=scene.name, passed=False, error="Skipped by user"
        )

    probes = collector.collect(timeout_s=TAIL_TIMEOUT_S)

    if not probes:
        return SceneResult(
            name=scene.name,
            passed=False,
            probes=probes,
            error="No probes detected within timeout",
        )

    matches = match_probes(scene.expected_probes, probes, scene.field_checks)
    all_found = all(m.found for m in matches)

    # Print match details
    print(f"\n  Expected sequence: {' -> '.join(scene.expected_probes)}")
    for m in matches:
        status = "FOUND" if m.found else "MISSING"
        detail = ""
        if m.probe:
            detail = f" @ {m.probe.timestamp}"
        print(f"    {status}: {m.expected}{detail}")

    result = SceneResult(
        name=scene.name,
        passed=all_found,
        matches=matches,
        probes=probes,
    )

    print(f"\n  Result: {'PASS' if result.passed else 'FAIL'}")
    return result


def main():
    print("=" * 60)
    print("  Aleph Layer B — Multimodal Telegram E2E Monitor")
    print("=" * 60)
    print()
    print("Prerequisites:")
    print("  1. aleph-server running with RUST_LOG=info,multimodal=info")
    print("  2. Telegram bot configured and connected")
    print("  3. A Telegram chat with the bot open on your phone/desktop")
    print()

    log_path = find_latest_log()
    collector = ProbeCollector(log_path)

    results: List[SceneResult] = []

    for scene in SCENES:
        result = run_scene(scene, collector)
        results.append(result)

    # Final summary
    print(f"\n{'='*60}")
    print("  FINAL SUMMARY")
    print(f"{'='*60}\n")

    passed = sum(1 for r in results if r.passed)
    failed = sum(1 for r in results if not r.passed)

    for r in results:
        status = "PASS" if r.passed else "FAIL"
        extra = f" ({r.error})" if r.error else ""
        probe_count = len(r.probes)
        print(f"  [{status}] {r.name} — {probe_count} probes collected{extra}")

    print(f"\n  Total: {passed} passed, {failed} failed, {len(results)} scenes")

    if failed == 0:
        print("\n  All scenes passed!")
    else:
        print(f"\n  {failed} scene(s) need investigation.")

    sys.exit(0 if failed == 0 else 1)


if __name__ == "__main__":
    main()
