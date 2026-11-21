#!/usr/bin/env python3
"""Cluster wire-seam guard - invariants the 2026-11-20 severed-wire audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/cluster_wire_audit.py

Exit non-zero when any check fails.
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


def const_ms(text, name):
    m = re.search(name + r": u64 = ([\d_]+)", text)
    return int(m.group(1).replace("_", "")) if m else None


def check_shape():
    """The command SCHEMA stays on the wire and out of the stored shape.

    `CommandDeclaration` is the connect-frame shape: its `schema` key is
    required, so a command entry without it still makes the catalog malformed
    (fail-closed, and old centers still parse it). `CommandDescriptor` is what
    the registry stores and ships: nothing ever read the schema back, so
    keeping it there only cloned bytes into every `environments.list`.
    """
    reg = prod("src/cluster/registry.rs")
    decl = re.search(r"pub struct CommandDeclaration \{(.*?)\n\}", reg, re.S)
    desc = re.search(r"pub struct CommandDescriptor \{(.*?)\n\}", reg, re.S)
    if not decl or "pub schema" not in decl.group(1):
        bad("shape", "CommandDeclaration must keep the wire `schema` field")
    if desc and "pub schema" in desc.group(1):
        bad("shape", "CommandDescriptor must not carry `schema` - no consumer reads it")


def check_timing():
    """The node tool window must ENCLOSE the center's approval window.

    A gated command parks the node on the operator; the center's approval
    driver waits `DEFAULT_APPROVAL_TIMEOUT_MS` before it answers. If the
    reverse-RPC window is not longer than that, the verdict loses the race and
    the model is told "timed out" while the node is still parked.
    """
    window = const_ms(prod("src/exec/manager.rs"), "DEFAULT_APPROVAL_TIMEOUT_MS")
    if window is None:
        bad("timing", "DEFAULT_APPROVAL_TIMEOUT_MS not found in src/exec/manager.rs")
        return
    for tool in ("node_invoke", "node_invoke_many", "node_file"):
        got = const_ms(prod(f"src/builtin_tools/{tool}.rs"), "DEFAULT_TIMEOUT_MS")
        if got is None:
            bad("timing", f"{tool}: DEFAULT_TIMEOUT_MS not found")
        elif got <= window:
            bad("timing", f"{tool} window {got}ms must exceed the approval window {window}ms")
    park = const_ms(prod("src/cluster/node_approval.rs"), "NODE_APPROVAL_TIMEOUT_MS")
    if park is None:
        bad("timing", "NODE_APPROVAL_TIMEOUT_MS not found")
    elif park <= window:
        bad("timing", f"node approval park {park}ms must exceed the approval window {window}ms")


def check_liveness():
    """The node session must bind a close signal, and the production AuthContext
    must carry the gateway event bus.

    Without a close signal the node's reverse-RPC channel cannot be torn down
    when the center stops reading, so the node stays "online" while every
    command to it times out. Without the event bus the operator-initiated
    `node.disconnected` event is never published.
    """
    node = prod("src/bin/aleph-server/commands/node.rs")
    if "ReverseRpcChannel::with_close(" not in node:
        bad("liveness", "the node session channel must bind a close signal")
    if "ReverseRpcChannel::new(" in node:
        bad("liveness", "the node session channel must not be built without a close signal")
    sub = prod("src/bin/aleph-server/commands/start/builder/subsystems.rs")
    if "event_bus: None" in sub:
        bad("bus", "the production AuthContext must carry the gateway event bus")
    start = prod("src/bin/aleph-server/commands/start/mod.rs")
    if "server.event_bus()" not in start:
        bad("bus", "start must hand the gateway event bus to initialize_vault")


def main():
    for check in (check_shape, check_timing, check_liveness):
        try:
            check()
        except Exception as exc:  # a missing file IS a failure
            bad(check.__name__, f"guard could not run: {exc}")
    for line in BAD:
        print(f"FAIL {line}")
    if BAD:
        print(f"cluster_wire_audit: FAIL ({len(BAD)} finding(s))")
        return 1
    print("cluster_wire_audit: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
