#!/usr/bin/env python3
"""Export wire-seam guard - invariants the 2026-11-21 src/export audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo
root:

    python3 scripts/export_wire_audit.py

Exit non-zero when any check fails.

Three kinds of check live here:

* CUT   - a severed wire that was removed must stay removed, or the cut is
           undone by a later edit and the dead surface grows back.
* LIVE  - a reconnected wire must stay connected: the producer, the consumer
           and the call that joins them.
* DARK  - a deliberately dark wire (a duplicate that waits for a product
           ruling) must stay as it is, so a later audit does not "fix" it into
           a lie - and so a NEW copy cannot hide behind it.
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

# The export document's policy. ONE owner: `src/export/mod.rs`. The gateway
# derives the policy it serves from the same literal, so a second copy is a
# second policy - and the two had already drifted apart.
EXPORT_CSP = "default-src 'none'; img-src data:; style-src 'unsafe-inline'"


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


def code_only(text):
    """Text with the line comments dropped (doc comments included)."""
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("//")
    )


def absent(check, path, needle, note):
    """A severed wire must stay severed."""
    if needle in read(path):
        bad(check, f"{path} still carries {needle!r} - {note}")


def absent_prod(check, path, needle, note):
    """A severed wire must stay severed (test code may keep a local helper)."""
    if needle in prod(path):
        bad(check, f"{path} still carries {needle!r} in production code - {note}")


def present(check, path, needle, note):
    """A reconnected wire must stay connected."""
    if needle not in read(path):
        bad(check, f"{path} no longer carries {needle!r} - {note}")


def present_prod(check, path, needle, note):
    """A reconnected wire must stay connected in production code."""
    if needle not in prod(path):
        bad(check, f"{path} no longer carries {needle!r} in production code - {note}")


# ---------------------------------------------------------------------------
# CUT: the severed wires stay severed
# ---------------------------------------------------------------------------


def check_human_size_cut():
    """`page::human_size` had no consumer outside its own file.

    It was `pub(super)` with one internal caller; the other two copies of the
    same helper live in their own modules. A private helper is the whole
    point - a widened one is the beginning of a third copy."""
    absent_prod("export/cut", "src/export/page.rs", "pub(super) fn human_size",
                "the helper has no consumer outside its own file")


# ---------------------------------------------------------------------------
# LIVE: the reconnected wires stay connected
# ---------------------------------------------------------------------------


def check_csp_single_owner():
    """ONE policy literal, ONE owner.

    The exporter and the gateway each held a copy of the export document's
    policy, the two had already drifted apart (one had `sandbox`, the other
    did not), and both module docs claimed there was only one. A new copy is
    the same defect with a new address."""
    owners = []
    for path in sorted((ROOT / "src").rglob("*.rs")):
        body = code_only(prod(path.relative_to(ROOT)))
        if EXPORT_CSP in body:
            owners.append(str(path.relative_to(ROOT)))
    if len(owners) != 1:
        bad("export/live", f"the export policy literal must live in exactly one production file, "
                           f"found {owners}")
    elif owners[0] != "src/export/mod.rs":
        bad("export/live", f"the export policy literal moved to {owners[0]} - its owner is "
                           f"src/export/mod.rs")


def check_csp_derivation():
    """The gateway DERIVES its policy; it does not spell it out."""
    present_prod("export/live", "src/gateway/server/artifact_route.rs",
                 "crate::export::content_security_policy!()",
                 "the served policy must be derived from the export module's literal")
    absent_prod("export/live", "src/gateway/server/artifact_route.rs", EXPORT_CSP,
                "the gateway must not spell the policy out again")


def check_sandbox_downloads():
    """The served document's sandbox allows downloads.

    The export renders every non-image attachment as a `download` link; under
    `sandbox` without `allow-downloads` the browser blocks the download and
    the reader gets a dead row."""
    route = prod("src/gateway/server/artifact_route.rs")
    if "sandbox allow-downloads" not in route:
        bad("export/live", "the served document policy no longer allows downloads - every "
                          "attachment row in an export would be dead in the browser")


def check_zero_timestamp():
    """A zero timestamp renders as absent, not as 1970.

    The write boundary already treats `0` as "the producer did not say when";
    the read boundary did not, so a legacy row rendered as 1970-01-01. The
    ORDERING path still resolves the epoch, because a row with no stamp must
    still sort somewhere."""
    types = prod("src/gateway/session_store/types.rs")
    rfc = types.find("pub fn rfc3339(&self) -> String {")
    if rfc == -1:
        bad("export/live", "src/gateway/session_store/types.rs no longer has MessageRecord::rfc3339")
        return
    body = code_only(types[rfc : rfc + 400])
    if "self.timestamp == 0" not in body:
        bad("export/live", "rfc3339 no longer treats the zero timestamp as absent - a legacy row "
                          "renders as 1970-01-01 in every export")
    instant = types.find("pub fn instant(&self)")
    if instant == -1:
        bad("export/live", "src/gateway/session_store/types.rs no longer has MessageRecord::instant")
        return
    if "== 0" in code_only(types[instant : instant + 300]):
        bad("export/live", "instant() now drops the zero timestamp - the ordering path needs the "
                          "epoch to sort a stamp-less row")


def check_dropped_figures_reported():
    """A figure the budget dropped is reported to the model.

    The export document says "not embedded"; the model was told nothing, so it
    believed every figure it asked for was in the document."""
    present_prod("export/live", "src/builtin_tools/artifact_publish.rs",
                 "collect_artifacts_with_drops",
                 "the publish path must ask for the drops, not only the survivors")
    publish = prod("src/builtin_tools/artifact_publish.rs")
    if "drop.reason.as_str()" not in publish:
        bad("export/live", "the publish path no longer reports WHY a figure was dropped")


def check_remote_images_become_links():
    """A remote image becomes a link.

    The export policy allows `img-src data:` and nothing else, so a remote
    `<img>` can never load - and it would be a fetch the document promises it
    does not make. The image is rewritten into a link instead."""
    present_prod("export/live", "src/export/markdown.rs", "fn sanitize_events",
                 "the markdown event rewrite must stay")
    markdown = prod("src/export/markdown.rs")
    if "Event::Start(Tag::Image" not in markdown:
        bad("export/live", "the markdown rewrite no longer handles image events - a remote image "
                          "would render as an empty frame")
    if "https:" in code_only(markdown).split("fn markdown_options")[0]:
        bad("export/live", "the markdown renderer now allows a remote source")


def check_export_rpc_idempotency():
    """The Panel's export call carries an idempotency key.

    `session.export_html` is a Mutate-lane RPC: with `require_idempotency_key`
    on, a mutating call without a key is rejected."""
    present("export/live", "interfaces/webchat/src/api/artifacts.rs",
            'encode_write("session.export_html"',
            "the Panel's export call must carry an idempotency key")


def check_artifact_ids_reach_the_model():
    """The published artifact's id reaches the model.

    Without it the model cannot reference what it just published, and the
    harvest result was only logged."""
    present_prod("export/live", "src/tools/scoped/artifact_harvest.rs",
                 'STORED_FIELD: &str = "_media_artifacts"',
                 "the stored-artifact annotation must stay")
    present_prod("export/live", "src/tools/scoped/dispatch.rs",
                 "annotate_media_stored",
                 "the dispatch path must annotate the stored artifacts into the tool value")


# ---------------------------------------------------------------------------
# DARK: the deliberately dark wires stay as they are
# ---------------------------------------------------------------------------


def check_headers_test_duplicate_dark():
    """The security-header test keeps its own copy of the strict policy.

    It is duplicated on purpose, so the test keeps guarding the behaviour
    even if the artifact route moves. A NEW copy elsewhere is the defect this
    guard exists for; this one stays."""
    headers = prod("src/security/headers.rs")
    if EXPORT_CSP in headers:
        bad("export/dark", "src/security/headers.rs now carries the export policy in production "
                          "code - the copy there is a deliberate test duplicate")


def main():
    check_human_size_cut()
    check_csp_single_owner()
    check_csp_derivation()
    check_sandbox_downloads()
    check_zero_timestamp()
    check_dropped_figures_reported()
    check_remote_images_become_links()
    check_export_rpc_idempotency()
    check_artifact_ids_reach_the_model()
    check_headers_test_duplicate_dark()
    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: export wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
