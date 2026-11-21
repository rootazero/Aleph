#!/usr/bin/env python3
"""Event wire-seam guard for the invariants repaired in the 2026-11 audit.

Run from the repo root:

    python3 scripts/event_wire_audit.py

Every AlephEvent variant needs a production producer and subscriber, and no
struct-variant field may be empty at every production producer. The repaired
cuts and the boot/transition wiring are pinned as LIVE/CUT checks as well.
"""
import re
import sys
from pathlib import Path

try:
    from wiring_strip import strip_tests
except ImportError:  # pragma: no cover - standalone use
    strip_tests = None

ROOT = Path(__file__).resolve().parent.parent
BAD = []


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def production(path):
    text = (ROOT / path).read_text(encoding="utf-8")
    if strip_tests is not None:
        text = strip_tests(text)
    else:  # Conservative fallback; the repository normally uses wiring_strip.
        text = text.split("#[cfg(test)]")[0]
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("//")
    )


def bad(check, detail):
    BAD.append(f"{check}: {detail}")


def matching(text, opening):
    pairs = {"{": "}", "(": ")", "[": "]"}
    close = pairs[text[opening]]
    depth = 0
    for pos in range(opening, len(text)):
        char = text[pos]
        if char == text[opening]:
            depth += 1
        elif char == close:
            depth -= 1
            if depth == 0:
                return pos
    return len(text) - 1


def event_shape():
    text = read("src/event/types.rs")
    start = text.index("pub enum AlephEvent")
    opening = text.index("{", start)
    body = text[opening + 1 : matching(text, opening)]
    variants = []
    for match in re.finditer(r"(?m)^    ([A-Z][A-Za-z0-9_]*)(\s*\{|\s*\()", body):
        name = match.group(1)
        is_struct = match.group(2).strip() == "{"
        fields = []
        if is_struct:
            field_open = body.index("{", match.start())
            field_body = body[field_open + 1 : matching(body, field_open)]
            fields = re.findall(r"(?m)^        ([a-z][a-z0-9_]*):", field_body)
        variants.append((name, fields))
    return variants


def production_sources():
    for path in sorted((ROOT / "src").rglob("*.rs")):
        relative = path.relative_to(ROOT)
        if "tests" in relative.parts or path.stem.endswith("tests"):
            continue
        yield str(relative), production(relative)


def check_parity_and_empty_fields():
    variants = event_shape()
    if not variants:
        bad("event/parity", "could not parse any AlephEvent variants")
        return
    producers = {name: set() for name, _ in variants}
    subscribers = {name: set() for name, _ in variants}
    constructions = {name: [] for name, _ in variants}

    for path, text in production_sources():
        for name, _ in variants:
            if re.search(rf"\bEventType::{name}\b", text):
                subscribers[name].add(path)
            for match in re.finditer(rf"\bAlephEvent::{name}\s*([{{(])", text):
                opening = match.end() - 1
                closing = matching(text, opening)
                suffix = text[closing + 1 :].lstrip()
                if suffix.startswith("=>"):
                    subscribers[name].add(path)
                    continue
                # A constructor, unlike a match arm, is not followed by `=>`.
                producers[name].add(path)
                constructions[name].append((path, text[opening + 1 : closing]))

    for name, fields in variants:
        if not producers[name]:
            bad("event/parity", f"{name} has no production producer")
        if not subscribers[name]:
            bad("event/parity", f"{name} has no production subscriber")
        sites = constructions[name]
        for field in fields:
            empty = re.compile(
                rf"(?m)^\s*{field}\s*:\s*"
                rf"(?:None|vec!\[\s*\]|Vec::new\(\s*\)|"
                rf"HashMap::new\(\s*\)|HashSet::new\(\s*\))\s*,"
            )
            if sites and all(empty.search(body) for _, body in sites):
                bad(
                    "event/no-always-empty-field",
                    f"{name}.{field} is None or an empty collection at every producer",
                )


def check_cuts_and_live_wires():
    types = read("src/event/types.rs")
    bus = read("src/event/global_bus.rs")
    filter_rs = read("src/event/filter.rs")
    for needle, note in (
        ("pub const fn name(", "AlephEvent::name had no consumer"),
        ("EventType::All", "the wildcard pseudo-variant must stay removed"),
        ("    All,", "the wildcard pseudo-variant must stay removed"),
        ("progress: Option<f32>", "TeamTaskUpdated.progress had only None producers"),
        ("member_ids: Vec<String>", "TeamCreated.member_ids had only empty producers"),
    ):
        if needle in types:
            bad("event/cut", f"src/event/types.rs still carries {needle!r}: {note}")
    if "fn unsubscribe(" in bus:
        bad("event/cut", "GlobalBus::unsubscribe must stay removed")
    if "pub fn all(" in filter_rs or "EventType::All" in filter_rs:
        bad("event/cut", "EventFilter::all and wildcard matching must stay removed")

    store = read("src/agents/swarm/tasks/store/mod.rs")
    notifier = read("src/teams/notifier.rs")
    if "previous_status" not in types or "previous_status" not in store:
        bad("event/live", "TeamTaskUpdated must carry the pre-update status")
    if 'previous_status.as_deref() != Some("waiting_review")' not in notifier:
        bad("event/live", "waiting_review notification must require a real transition")
    if "owner_changed" not in store or "TeamTaskAssigned" not in store:
        bad("event/live", "owner changes must emit TeamTaskAssigned")

    start = read("src/bin/aleph-server/commands/start/mod.rs")
    logger = start.find("Team event handlers registered (TeamEventLogger)")
    notifier_done = start.find("Team notifier registered (TeamNotifier)")
    dispatch = start.find("dispatcher.spawn_loop()")
    if not (0 <= logger < notifier_done < dispatch):
        bad(
            "event/boot-order",
            "TeamEventLogger and TeamNotifier must subscribe before the first dispatch",
        )
    notifier_block = start[start.find("// Register TeamNotifier") : dispatch]
    if "\n            tokio::spawn(async move {" in notifier_block:
        bad("event/boot-order", "TeamNotifier subscription itself must be awaited, not spawned")


def run_check(name, function):
    before = len(BAD)
    function()
    status = "PASS" if len(BAD) == before else "FAIL"
    print(f"{status:<4} {name}")


def main():
    run_check("emit/subscribe parity + populated fields", check_parity_and_empty_fields)
    run_check("event cuts and live transition/boot wiring", check_cuts_and_live_wires)
    if BAD:
        print("\nEvent wiring audit FAILED:", file=sys.stderr)
        for item in BAD:
            print(f"  - {item}", file=sys.stderr)
        return 1
    print("All event wiring checks passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
