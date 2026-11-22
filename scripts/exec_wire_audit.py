#!/usr/bin/env python3
"""Exec wire-seam guard - invariants the 2026-11-21 src/exec audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/exec_wire_audit.py

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


def absent_code(check, path, needle, note):
    """A severed wire must stay severed (comments are not code)."""
    if needle in code_only(read(path)):
        bad(check, f"{path} still carries {needle!r} - {note}")


def present(check, path, needle, note):
    """A reconnected wire must stay connected."""
    if needle not in read(path):
        bad(check, f"{path} no longer carries {needle!r} - {note}")


def present_prod(check, path, needle, note):
    """A reconnected wire must stay connected in production code."""
    if needle not in prod(path):
        bad(check, f"{path} no longer carries {needle!r} in production code - {note}")


def block(text, header):
    """The `{ ... }` block that follows `header` (brace matched)."""
    i = text.find(header)
    if i == -1:
        return None
    start = text.find("{", i)
    if start == -1:
        return None
    depth = 0
    for j in range(start, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[start : j + 1]
    return None


def block_prod(path, header):
    """The `{ ... }` block that follows `header` in production code."""
    return block(prod(path), header)


# ---------------------------------------------------------------------------
# CUT: the severed wires stay severed
# ---------------------------------------------------------------------------


def check_approval_record_fields_cut():
    """`ExecApprovalRecord` carried three write-only fields.

    `decision`, `resolved_by` and `resolved_at_ms` were stamped when the
    approval was resolved and never read back: the waiter gets the decision
    through the oneshot channel, and the audit hook takes the actor from a
    local. They were a second, unwritten copy of facts the record already
    carried."""
    body = block_prod("src/exec/manager.rs", "pub struct ExecApprovalRecord {")
    if body is None:
        bad("exec/cut", "src/exec/manager.rs no longer defines ExecApprovalRecord")
        return
    for field in ("pub resolved_at_ms:", "pub resolved_by:", "pub decision:"):
        if field in body:
            bad("exec/cut", f"ExecApprovalRecord still declares {field!r} - the write-only "
                            f"field is a severed wire nobody reads")


def check_record_originator_cut():
    """`ExecApprovalManager::record_originator` had no caller.

    The originator travelled to the UI through the approval request itself,
    so the setter only ever wrote a field nobody read back."""
    absent_prod("exec/cut", "src/exec/manager.rs", "fn record_originator",
                "the originator setter had no caller")
    absent("exec/cut", "src/gateway/mcp_face/session.rs", "record_originator",
           "the MCP face no longer documents or calls the removed setter")


def check_approval_receiver_try_recv_cut():
    """`RegisteredApprovalReceiver::try_recv` had no PRODUCTION caller.

    The receiver is awaited as a future; a non-blocking poll on it was a
    second way to read the same channel that no production code used. The
    audit's first pass deleted it outright and broke six test call sites
    (the poll is how the approval tests assert "no decision has been
    delivered yet"), so the poll now lives in a `#[cfg(test)]` inherent impl
    and the production surface stays bare."""
    absent_prod("exec/cut", "src/exec/manager.rs", "impl RegisteredApprovalReceiver",
                "the receiver's inherent impl must stay test-only - the poll has no production caller")
    present("exec/live", "src/exec/manager.rs", "#[cfg(test)]\nimpl RegisteredApprovalReceiver",
            "the test-only poll the six approval test call sites rely on must stay")


def check_cleanup_expired_cut():
    """`ExecApprovalManager::cleanup_expired` was dead - and `#[allow(dead_code)]`
    hid that fact from the compiler.

    The live sweep is `cleanup_expired_locked`, called while holding the
    registration lock. The wrapper had no caller at all."""
    absent_prod("exec/cut", "src/exec/manager.rs", "pub(crate) fn cleanup_expired(&self)",
                "the dead sweep wrapper came back (it was silenced with #[allow(dead_code)])")
    present_prod("exec/cut", "src/exec/manager.rs", "fn cleanup_expired_locked",
                "the live sweep that retires expired approvals must stay")


def check_analysis_fields_cut():
    """`CommandAnalysis::chains`, `CommandSegment::argv` and
    `CommandResolution::raw_executable` were write-only.

    The parser produced them and nothing read them: the segment list is what
    the callers consume, the argv is recomputed locally where it is needed,
    and the resolution carries the resolved path, not the raw string."""
    for struct_header, field in (
        ("pub struct CommandAnalysis {", "pub chains:"),
        ("pub struct CommandSegment {", "pub argv:"),
        ("pub struct CommandResolution {", "pub raw_executable:"),
    ):
        body = block_prod("src/exec/analysis.rs", struct_header)
        if body is None:
            bad("exec/cut", f"src/exec/analysis.rs no longer defines {struct_header}")
            continue
        if field in body:
            bad("exec/cut", f"{struct_header} still declares {field!r} - the write-only "
                            f"field is a severed wire nobody reads")


# ---------------------------------------------------------------------------
# LIVE: the reconnected wires stay connected
# ---------------------------------------------------------------------------


def check_custom_reason_live():
    """`CustomRiskPattern::reason` reaches the operator.

    The pattern's reason was dropped by the kernel, so an operator who wrote
    "this command touches the customer database" never saw it: the denial said
    only that a custom pattern matched."""
    present_prod("exec/live", "src/exec/kernel.rs", "pub fn assess_custom_with_reason",
                 "the kernel must expose the pattern's reason next to the verdict")
    present_prod("exec/live", "src/sandbox/security_kernel_hook.rs",
                 "assess_custom_with_reason",
                 "the security hook must ask for the reason, not the bare verdict")
    hook = read("src/sandbox/security_kernel_hook.rs")
    if "reasons.push(reason.clone())" not in hook:
        bad("exec/live", "src/sandbox/security_kernel_hook.rs no longer records the operator's "
                         "reason in the audit reason list")
    if "blocked by custom shell-security pattern" not in hook:
        bad("exec/live", "src/sandbox/security_kernel_hook.rs no longer names the custom "
                         "pattern in the denial text")


def check_grant_store_error_live():
    """`GrantStore` must not turn an unreadable registry into an empty one.

    `read_file` swallowed every read error, so a registry that could not be
    read looked like a registry with no grants - and the next write replaced
    it. `mutate` also published the cache after a failed write, so the caller
    was told the write happened."""
    grants = prod("src/sandbox/exec_approval/grants.rs")
    read_file = block(grants, "fn read_file(path: &Path)")
    if read_file is None:
        bad("exec/live", "src/sandbox/exec_approval/grants.rs no longer defines read_file")
    else:
        if "Err(e) => return Err(e)" not in read_file:
            bad("exec/live", "read_file swallows a read error again - an unreadable registry "
                             "must not look like an empty one")
        if "ErrorKind::NotFound" not in read_file:
            bad("exec/live", "read_file no longer treats a missing file as an empty registry")
    mutate = block(grants, "fn mutate<F>(")
    if mutate is None:
        bad("exec/live", "src/sandbox/exec_approval/grants.rs no longer defines mutate")
    else:
        if "if result.is_ok()" not in mutate:
            bad("exec/live", "mutate publishes the cache even when the write failed")
    if "keeping the cached grants" not in grants:
        bad("exec/live", "reload_if_stale no longer keeps the cached grants when the registry "
                         "cannot be read")


def check_secret_masker_catalog_live():
    """The exec masker consumes the vendor catalog.

    `secret_masker_patterns()` used to keep its own copy of the vendor
    patterns, so a vendor family added to the canonical table did not reach
    the masker. The exec module is now a CONSUMER of the one catalog."""
    present_prod("exec/live", "src/exec/secret_patterns.rs",
                 "crate::secrets::vendor_patterns::VENDOR_SECRET_PATTERNS",
                 "the exec masker must consume the canonical vendor catalog")
    present("exec/live", "src/exec/secret_patterns.rs", "sk-proj-",
            "the OpenAI project key family must stay in the masker patterns")


def check_leak_detector_families_live():
    """The byte scrubber knows the GitHub and Discord families.

    `default_patterns_bytes()` chains the secret sources and the vendor
    catalog; the GitHub and Discord families were missing from both, so a
    leaked `github_pat_…` or a Discord bot token travelled through the
    outbound scrubber."""
    detector = read("src/secrets/leak_detector.rs")
    for family in ("github_token", "github_fine_grained_pat", "discord_bot_token"):
        if family not in detector:
            bad("exec/live", f"src/secrets/leak_detector.rs no longer knows the {family} family")
    present("exec/live", "src/sandbox/scrub.rs", "github_token",
            "the outbound scrubber must be pinned against the GitHub family")


def check_mcp_redaction_live():
    """MCP redaction starts from the shared masker.

    The MCP redactor only stripped bearer/basic headers and `key=value`
    assignments, so an OAuth bearer or a vendor API key in a tool argument
    leaked into the MCP transcript."""
    redact = prod("src/mcp/redact.rs")
    if "crate::exec::SecretMasker::new().mask(text)" not in redact:
        bad("exec/live", "src/mcp/redact.rs no longer starts from the shared SecretMasker")
    if "fn conservative_redact(text: &str) -> String" not in redact:
        bad("exec/live", "src/mcp/redact.rs no longer defines conservative_redact")


# ---------------------------------------------------------------------------
# DARK: the deliberately dark wires stay as they are
# ---------------------------------------------------------------------------


def check_note_manage_embedder_dark():
    """The note_manage embedder claim stays dark until the memory side is fixed.

    `NoteManageTool` builds its own indexer with `embedder: None`, so the
    hybrid (vector + FTS) query never runs - while its doc claims the
    provider is "wired from the registry's config.embedder". The audit
    corrected the DOC and left the behaviour alone: fixing it means changing
    the memory runtime's retrieval, which is outside this audit's scope."""
    doc = read("src/builtin_tools/note_manage/mod.rs")
    if "Wired from the registry's `config.embedder`" in doc:
        bad("exec/dark", "src/builtin_tools/note_manage/mod.rs claims the embedder is wired "
                         "from the registry config - the field is gone and the claim is a lie")


def main():
    check_approval_record_fields_cut()
    check_record_originator_cut()
    check_approval_receiver_try_recv_cut()
    check_cleanup_expired_cut()
    check_analysis_fields_cut()
    check_custom_reason_live()
    check_grant_store_error_live()
    check_secret_masker_catalog_live()
    check_leak_detector_families_live()
    check_mcp_redaction_live()
    check_note_manage_embedder_dark()
    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: exec wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
