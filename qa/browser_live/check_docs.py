#!/usr/bin/env python3
"""Doc entry-point gate for the browser live viewer (fail-closed, stdlib only).

Usage:
  python qa/browser_live/check_docs.py --self-test   # pure fixtures, touches no repo file
  python qa/browser_live/check_docs.py               # check the real tree; exit 0 pass, 1 fail, 2 error

What it proves: each of the seven entry files carries exactly one marked
`browser-live-docs` block that links to docs/reference/BROWSER_LIVE_VIEWER.md,
holds the shared boundary terms plus its own assigned terms, cites only exact
repo paths that exist, and makes no un-negated production claim.
What it does NOT prove: that the viewer works. It never runs the network or cargo.

`qa/browser_live/run.sh` is documented as NOT implemented; if it appears while the
docs still say so, this gate fails so the docs are updated in the same change.
"""
import argparse
import os
import re
import sys

BEGIN = "<!-- browser-live-docs:begin -->"
END = "<!-- browser-live-docs:end -->"
VIEWER = "docs/reference/BROWSER_LIVE_VIEWER.md"
RUN_SH = "qa/browser_live/run.sh"
SELECTORS = [
    "stream", "control-all-entries", "standalone-same-target", "binding-switch",
    "managed-companion", "grant-boundary", "reap", "revoke-live", "navigate-fence",
    "target-destroy", "ttl-expiry",
]
SELECTOR_TERMS = ["`%s`" % s for s in SELECTORS]

# Terms every block must carry (the shared boundary).
COMMON = [
    "BROWSER_LIVE_VIEWER.md", "Gateway owns CDP", "I/O only", "browser_viewer_origin",
    "deny-only", "`Safe`", "`Pending`", "browser.viewer.resource.decide", "revision CAS",
    "binding", "owner", "generation", "#27", "blocked", "NetworkContextGrant",
    "not production ready",
]

R = "docs/reference/"
# file -> (extra terms, exact repo paths that must appear in the block AND exist)
SPEC = {
    R + "FEATURE_LOCATOR.md": (
        ["browser.viewer.", "browser.live.resource", "check_docs.py",
         "check_memory.py", RUN_SH],
        ["src/browser/live/", "src/gateway/handlers/browser_viewer.rs",
         "src/gateway/server/browser_live_runtime.rs", "shared/protocol/src/browser_live.rs",
         "interfaces/tui/src/viewer_launcher.rs", "qa/browser_live/check_memory.py",
         "qa/browser_live/check_docs.py"],
    ),
    R + "ARCHITECTURE.md": (
        ["Panel/TUI", "R2/R4", "disabled-by-default"],
        ["src/browser/", "crates/aleph-cdp", "src/gateway/server/browser_live_runtime.rs",
         "src/gateway/handlers/browser_viewer.rs", "interfaces/tui/src/viewer_launcher.rs"],
    ),
    R + "TOOL_SYSTEM.md": (
        ["browser_open", "browser_navigate", "BrowserLiveProducer", "PreviewResource",
         "browser.live.resource", "browser.live.closed", "Refused"],
        ["src/builtin_tools/browser_tools/present.rs",
         "src/builtin_tools/browser_tools/operation_publish.rs"],
    ),
    R + "SESSION_KNOBS.md": (
        ["not a session knob", "browser_session_id", "target_id", "binding-switch", RUN_SH],
        ["src/browser/live/owner_registry.rs"],
    ),
    R + "SECURITY.md": (
        ["Frames-only", "fragment", "Refused -> Revoked", "one-shot", "BrowserContext"],
        ["src/browser/live/grant.rs"],
    ),
    R + "DEVELOPMENT.md": (
        ["check_memory.py", "check_docs.py", "4294967296", "4 GiB", "--self-test",
         "STATUS_DLL_INIT_FAILED", "fail closed", RUN_SH] + SELECTOR_TERMS,
        ["qa/browser_live/check_memory.py", "qa/browser_live/check_docs.py"],
    ),
    "qa/README.md": (
        ["check_docs.py", "check_memory.py", "--self-test", "real Playwright runner", "fail closed",
         "4294967296", "../docs/reference/BROWSER_LIVE_VIEWER.md", RUN_SH] + SELECTOR_TERMS,
        ["qa/browser_live/check_memory.py", "qa/browser_live/check_docs.py"],
    ),
}

_PROD = re.compile(r"production[ -]ready|生产就绪|生产可用", re.IGNORECASE)
_NEG = re.compile(r"(not|never|non|no|isn't|非|不能|不得|不是|未|不)[\s\-:*`]*$", re.IGNORECASE)
_LINK = re.compile(r"\]\(([^)#\s]+)(?:#[^)]*)?\)")


def norm(text):
    return text.replace("\r\n", "\n").replace("\r", "\n")


def extract_block(text):
    """Return the single marked block, or None; raises ValueError on duplicate/odd markers."""
    t = norm(text)
    if t.count(BEGIN) != 1 or t.count(END) != 1:
        raise ValueError("expected exactly one %s ... %s pair (found %d/%d)"
                         % (BEGIN, END, t.count(BEGIN), t.count(END)))
    i, j = t.index(BEGIN), t.index(END)
    if j < i:
        raise ValueError("end marker precedes begin marker")
    return t[i + len(BEGIN):j]


def un_negated_claims(block):
    bad = []
    for m in _PROD.finditer(block):
        before = block[max(0, m.start() - 12):m.start()]
        if not _NEG.search(before):
            bad.append(block[max(0, m.start() - 20):m.end() + 5].replace("\n", " "))
    return bad


def check_texts(texts, exists):
    """texts: relpath -> file text; exists: relpath -> bool. Returns a list of error strings."""
    errs = []
    if not exists(VIEWER):
        errs.append("missing link target %s" % VIEWER)
    run_sh_present = exists(RUN_SH)
    for rel, (extra, paths) in SPEC.items():
        if rel not in texts:
            errs.append("%s: file text unavailable (fail closed)" % rel)
            continue
        try:
            block = extract_block(texts[rel])
        except ValueError as e:
            errs.append("%s: %s" % (rel, e))
            continue
        for term in COMMON + extra:
            if term not in block:
                errs.append("%s: block lacks required term %r" % (rel, term))
        for p in paths:
            if p not in block:
                errs.append("%s: block does not cite exact path %s" % (rel, p))
            elif not exists(p):
                errs.append("%s: cited path does not exist: %s" % (rel, p))
        for claim in un_negated_claims(block):
            errs.append("%s: un-negated production claim: ...%s..." % (rel, claim))
        base = os.path.dirname(rel)
        found_viewer_link = False
        for target in _LINK.findall(block):
            if re.match(r"[a-z][a-z0-9+.-]*:", target):
                continue
            resolved = os.path.normpath(os.path.join(base, target)).replace("\\", "/")
            if not exists(resolved):
                errs.append("%s: relative link does not resolve: %s" % (rel, target))
            if resolved == VIEWER:
                found_viewer_link = True
        if not found_viewer_link:
            errs.append("%s: block has no resolvable link to %s" % (rel, VIEWER))
        if run_sh_present and "not implemented" in block:
            errs.append("%s: says run.sh is not implemented but %s exists; update the docs" % (rel, RUN_SH))
    return errs


def check_repo(root):
    texts = {}
    for rel in SPEC:
        try:
            with open(os.path.join(root, rel), "rb") as f:
                texts[rel] = f.read().decode("utf-8")
        except (OSError, UnicodeDecodeError) as e:
            raise RuntimeError("cannot read %s: %s" % (rel, e))
    return check_texts(texts, lambda p: os.path.exists(os.path.join(root, p)))


# ---------------------------------------------------------------- self-test

def _fixture_block(rel, extra, paths):
    pre = os.path.dirname(rel)
    link = os.path.relpath(VIEWER, pre or ".").replace("\\", "/")
    body = " ".join(COMMON + extra + paths)
    return "intro\r\n%s\r\n[v](%s)\r\n%s\r\n%s\r\n" % (BEGIN, link, body, END)


def _good_fixture():
    texts = {}
    for rel, (extra, paths) in SPEC.items():
        texts[rel] = _fixture_block(rel, extra, paths)
    return texts


def self_test():
    all_paths = {VIEWER, RUN_SH}
    for _, paths in SPEC.values():
        all_paths.update(paths)
    ex = lambda p: p in all_paths  # noqa: E731
    good = _good_fixture()
    assert check_texts(good, ex) == [], check_texts(good, ex)

    def mutated(rel, fn):
        t = dict(good)
        t[rel] = fn(t[rel])
        return t

    tgt = R + "SECURITY.md"
    # RED: each of these must fail
    assert check_texts(mutated(tgt, lambda s: s.replace(BEGIN, "")), ex), "missing begin marker"
    assert check_texts(mutated(tgt, lambda s: s + s), ex), "duplicate block"
    assert check_texts(mutated(tgt, lambda s: s.replace(END, "").replace(BEGIN, END + BEGIN)), ex), "reversed markers"
    assert check_texts(mutated(tgt, lambda s: s.replace("revision CAS", "")), ex), "missing common term"
    assert check_texts(mutated(tgt, lambda s: s.replace("Frames-only", "")), ex), "missing assigned term"
    assert check_texts(mutated(tgt, lambda s: s.replace(END, "It is production ready.\r\n" + END)), ex), "positive claim"
    assert check_texts(mutated(tgt, lambda s: s.replace(END, "is production-ready\r\n" + END)), ex), "hyphen claim"
    assert check_texts(mutated(tgt, lambda s: s.replace(END, "非生产就绪\r\n" + END)), ex) == [], "negated zh claim ok"
    assert check_texts(mutated(tgt, lambda s: s.replace(END, "also 生产可用\r\n" + END)), ex), "zh positive"
    assert check_texts(mutated(tgt, lambda s: s.replace("src/browser/live/grant.rs", "")), ex), "path not cited"
    broken_link = mutated(tgt, lambda s: re.sub(r"\]\([^)]*\)", "](./nope.md)", s))
    assert check_texts(broken_link, ex), "broken link"
    assert check_texts({k: v for k, v in good.items() if k != tgt}, ex), "unavailable file"
    assert check_texts(good, lambda p: p != "src/browser/live/grant.rs" and ex(p)), "cited path missing on disk"
    assert check_texts(good, lambda p: p != VIEWER and ex(p)), "viewer missing"
    # The runner is now part of the checked fixture; docs must describe the real
    # fail-closed Playwright entry instead of a missing runner.
    assert check_texts(good, ex) == [], check_texts(good, ex)
    # CRLF and LF both accepted
    assert check_texts({k: norm(v) for k, v in good.items()}, ex) == [], "LF fixture"
    # un_negated_claims unit cases
    assert un_negated_claims("Production ready now") != []
    assert un_negated_claims("this is not production ready") == []
    assert un_negated_claims("never production-ready") == []
    assert un_negated_claims("**not production ready**") == []
    print("check_docs self-test: OK")


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--self-test", action="store_true", help="run pure fixtures only")
    ap.add_argument("--root", default=None, help="repo root (default: derived from this file)")
    args = ap.parse_args(argv)
    try:
        if args.self_test:
            self_test()
            return 0
        root = args.root or os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
        errs = check_repo(root)
    except AssertionError as e:
        print("check_docs self-test FAILED: %s" % e, file=sys.stderr)
        return 1
    except Exception as e:  # fail closed: any unexpected condition is an error, never a pass
        print("check_docs error: %s" % e, file=sys.stderr)
        return 2
    if errs:
        for e in errs:
            print("FAIL: " + e)
        print("check_docs: %d problem(s)" % len(errs))
        return 1
    print("check_docs: OK (%d files)" % len(SPEC))
    return 0


if __name__ == "__main__":
    sys.exit(main())
