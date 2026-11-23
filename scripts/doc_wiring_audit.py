#!/usr/bin/env python3
"""Documentation wiring guard - keeps the docs and the code from drifting apart.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break: a document that points
at a file which no longer exists, at a crate which was renamed, or at a `just`
target which was deleted. Run from the repo root:

    python3 scripts/doc_wiring_audit.py

Exit non-zero when any check fails.

The canonical set is the documentation an agent reads to learn the current
architecture:

    AGENTS.md, CLAUDE.md, README.md, README_CN.md, CONTRIBUTING.md,
    skills/architecture/SKILL.md, docs/reference/**/*.md

The evidence layer (docs/superpowers/, docs/reports/, ...) is deliberately out
of scope: it is point-in-time history, not current state.

Kinds of check:

* LIVE  - a reconnected wire must stay connected: the doc names the file, the
          crate, the recipe or the port that the code actually has.
* CUT   - a severed wire must stay severed: the pre-NSH single-track claims
          about the desktop host must not come back.

PATHS and SYMBOLS are ratchets: the misses that already exist are recorded in
scripts/doc_wiring_audit_baseline.json, and only a new miss fails. Re-record
the baseline with --update-baseline after a sweep repairs entries.
"""
import difflib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BAD = []
ADVISORY = []
# Every backticked path token that does not exist, per document. The baseline
# file is a snapshot of this map: it shrinks when a sweep repairs a path and
# grows when a document starts naming a path that never existed.
STALE_PATHS = {}
# Every backticked code symbol that occurs nowhere in the tracked source, per
# document. Recorded into the same baseline file under "SYMBOLS".
STALE_SYMBOLS = {}
# Files whose identifiers are about the docs, not the code: the baseline lists
# stale names, so reading it would make every baselined name look current.
SYMBOL_SOURCE_EXCLUDED = {
    "scripts/doc_wiring_audit_baseline.json",
    "scripts/doc_wiring_audit.py",
}

ENTRY_DOCS = [
    "AGENTS.md",
    "CLAUDE.md",
    "README.md",
    "README_CN.md",
    "CONTRIBUTING.md",
    "docs/README.md",
]
# `skills/` is a separate repository (the Aleph-skills submodule). Its copy of
# the architecture skill is versioned there, so this guard does not police it.
REFERENCE_DIR = "docs/reference"

# JSON-RPC method and notification names. They contain a slash and look like
# paths, but they are protocol identifiers, not files in this repository.
# A file name whose extension is followed by extra characters: the shape a
# botched rewrite leaves behind when it pastes a path over a method name.
MANGLED_RE = re.compile(r"^.+\.(rs|ts|tsx|js|py|json|toml|md|sh|yml|yaml)[_\-][\w\-]*$")

KNOWN_NON_PATHS = {
    "notifications/tools/list_changed",
    "tools/call",
    "tools/list",
    "tools/list_changed",
    "tools/truncate",
}

# Phrases that assert a single-track (Tauri-only) desktop host. They were true
# before the NSH migration and became false after it.
CUT_PHRASES = [
    ("桌面壳 Tauri", "the desktop host is not Tauri-only any more"),
    ("Native Desktop App (Tauri)", "the desktop host is not Tauri-only any more"),
    ("桌面端 App（Tauri）", "the desktop host is not Tauri-only any more"),
    ("桌面端 App (Tauri)", "the desktop host is not Tauri-only any more"),
]

# Identifiers that look like a crate but are not one.
CRATE_ALIASES = {    "aleph-shell-v1",
    "aleph",
}

# Sections of AGENTS.md / CLAUDE.md that are allowed to differ.
SYNC_ALLOWED_DIFFS = {
    "Agent skills (Claude Code plugin)",
    "Skill routing",
}

CODE_RE = re.compile(r"`([^`\n]+)`")
LINK_RE = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
JUST_RE = re.compile(r"\bjust\s+([a-zA-Z0-9_-]+)")
CRATE_RE = re.compile(r"^aleph-[a-z0-9]+(?:-[a-z0-9]+)*$")
LOCAL_URL_RE = re.compile(r"(?:127\.0\.0\.1|localhost):(\d+)")
DATED_DOC_RE = re.compile(r"^\d{4}-\d{2}-\d{2}-")
# A backticked Rust/TS path such as `Foo::bar`, `run()` or `snake_case_name`.
SYMBOL_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*)(?:\(\))?$")
CAMEL_RE = re.compile(r"^[A-Z][a-z0-9]+(?:[A-Z][a-z0-9]+)+$")
IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
# Source files whose identifiers count as "the code has this name".
SOURCE_RE = re.compile(r"(\.(rs|ts|tsx|js|mjs|cjs|py|swift|sh|toml|json|yml|yaml)|justfile|/SKILL\.md)$")
# A heading that marks its section as history: symbols under it are allowed to
# be gone from the code.
HISTORICAL_HEADING_RE = re.compile(r"Historical|Superseded|历史记录")


def bad(check, detail):
    BAD.append(f"{check}: {detail}")


def advisory(check, detail):
    ADVISORY.append(f"{check}: {detail}")


def read(rel):
    return (ROOT / rel).read_text(encoding="utf-8")


def canonical_docs():
    """The (relative path, text) pairs the hard checks run over."""
    docs = []
    for rel in ENTRY_DOCS:
        path = ROOT / rel
        if path.is_file():
            docs.append((rel, path.read_text(encoding="utf-8")))
    ref_root = ROOT / REFERENCE_DIR
    if ref_root.is_dir():
        for path in sorted(ref_root.rglob("*.md")):
            # A dated file name marks a point-in-time design note: it is evidence,
            # not a description of the current architecture.
            if DATED_DOC_RE.match(path.name):
                continue
            rel = path.relative_to(ROOT).as_posix()
            docs.append((rel, path.read_text(encoding="utf-8")))
    return docs


def known_crates():
    """Every crate the workspace actually builds, plus its binary targets."""
    names = set()
    for manifest in ROOT.rglob("Cargo.toml"):
        if "target" in manifest.parts or ".git" in manifest.parts or "node_modules" in manifest.parts:
            continue
        for line in manifest.read_text(encoding="utf-8").splitlines():
            stripped = line.strip()
            if stripped.startswith("name") and "=" in stripped:
                value = stripped.split("=", 1)[1].strip().strip('"')
                if value:
                    names.add(value)
                    names.add(value.replace("-", "_"))
    bin_dir = ROOT / "src" / "bin"
    if bin_dir.is_dir():
        for path in bin_dir.iterdir():
            if path.suffix == ".rs":
                names.add(path.stem)
                names.add(path.stem.replace("-", "_"))
    return names


def known_just_targets():
    targets = set()
    justfile = ROOT / "justfile"
    if not justfile.is_file():
        return targets
    for line in justfile.read_text(encoding="utf-8").splitlines():
        # A recipe may take parameters: "release version:" is still `just release`.
        match = re.match(r"^([a-zA-Z0-9_-]+)(?:\s+[a-zA-Z0-9_$-]+)*\s*:", line)
        if match:
            targets.add(match.group(1))
        alias = re.match(r"^alias\s+([a-zA-Z0-9_-]+)\s*:=", line)
        if alias:
            targets.add(alias.group(1))
    return targets


def code_daemon_port():
    """The port the daemon and the Panel are actually served on."""
    candidates = []
    host = ROOT / "desktop/host/src/local_server.rs"
    if host.is_file():
        match = re.search(r"DAEMON_PORT\s*:\s*u\d+\s*=\s*(\d+)", host.read_text(encoding="utf-8"))
        if match:
            candidates.append(match.group(1))
    client = ROOT / "shared/client/src/connection_target.rs"
    if client.is_file():
        match = re.search(r"DEFAULT_PORT\s*:\s*u\d+\s*=\s*(\d+)", client.read_text(encoding="utf-8"))
        if match:
            candidates.append(match.group(1))
    if not candidates:
        return None
    if len(set(candidates)) > 1:
        bad("PORT", f"the code disagrees about the daemon port: {sorted(set(candidates))}")
    return candidates[0]


def path_candidates(text):
    """Backticked tokens that look like a path inside this repository."""
    for token in CODE_RE.findall(text):
        token = token.strip()
        if " " in token or not token:
            continue
        if any(ch in token for ch in "<>{}()[]=→"):
            continue
        token = token.rstrip(".,;:")
        token = re.sub(r":\d+$", "", token)
        token = token.split("::", 1)[0]
        if "…" in token or "..." in token:
            continue
        if "/" not in token:
            continue
        if token.startswith("/"):
            continue
        head = token.split("/", 1)[0]
        if not head or not (ROOT / head).exists():
            continue
        if head.startswith("."):
            # Hidden directories (.git/, .pi/, .cargo/) are not part of the
            # tracked tree; a document may name them as environment context.
            continue
        yield token


def path_exists(token):
    if "*" in token or "?" in token:
        return bool(list(ROOT.glob(token)))
    return (ROOT / token).exists()


def check_links(rel, text):
    for target in LINK_RE.findall(text):
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", target):
            continue
        target = target.split("#", 1)[0]
        if not target:
            continue
        if "/" not in target and not target.endswith((".md", ".json", ".rs", ".toml", ".yaml", ".yml")):
            continue
        if target.startswith("/"):
            candidate = ROOT / target.lstrip("/")
        else:
            candidate = (ROOT / rel).parent / target
        if not candidate.exists():
            bad("LINKS", f"{rel} links to a missing target: {target}")


def check_unknown_roots(rel, text):
    """Catch a path that was mangled into something that no longer reads as one.

    `path_candidates` only inspects tokens whose first segment is a real
    directory here, so two shapes slipped through unnoticed:

    * MANGLED - a real file name with junk glued onto its extension, e.g. the
      JSON-RPC method `notifications/tools/list_changed` rewritten into
      `notifications/src/tools/registry.rs_changed`.
    * ROOTS  - a path that exists only once the missing `src/` prefix is put
      back, e.g. `identity/artifact.rs` for `src/identity/artifact.rs`.
    """
    for token in CODE_RE.findall(text):
        token = token.strip().rstrip(".,;:")
        if " " in token or not token or "/" not in token:
            continue
        if any(ch in token for ch in "<>{}()[]=→"):
            continue
        if "://" in token or token.startswith(("/", "~")) or ":" in token:
            continue
        token = re.sub(r":\d+$", "", token)
        token = token.split("::", 1)[0]
        if "…" in token or "..." in token:
            continue
        if token in KNOWN_NON_PATHS:
            continue
        head = token.split("/", 1)[0]
        if not head or head.startswith("."):
            continue
        if (ROOT / head).exists():
            continue
        last = token.rsplit("/", 1)[-1]
        if MANGLED_RE.match(last):
            bad(
                "MANGLED",
                f"{rel} names a mangled path whose file name carries junk: {token}",
            )
            continue
        if path_exists(f"src/{token}"):
            advisory(
                "ROOTS",
                f"{rel} names a path that only exists under src/: {token}",
            )


def check_paths(rel, text, backlog):
    """A path a document names must exist, unless it is on the recorded backlog."""
    for token in path_candidates(text):
        if path_exists(token):
            continue
        STALE_PATHS.setdefault(rel, [])
        if token not in STALE_PATHS[rel]:
            STALE_PATHS[rel].append(token)
        if token in backlog.get(rel, []):
            continue
        bad("PATHS", f"{rel} names a path that does not exist: {token}")


def known_symbols():
    """Every identifier that occurs in a tracked source file, submodules included."""
    listed = subprocess.run(
        ["git", "-C", str(ROOT), "ls-files", "--recurse-submodules"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout.splitlines()
    names = set()
    for rel in listed:
        if not SOURCE_RE.search(rel) or rel in SYMBOL_SOURCE_EXCLUDED:
            continue
        path = ROOT / rel
        if not path.is_file():
            continue
        names.update(IDENT_RE.findall(path.read_text(encoding="utf-8", errors="ignore")))
    return names


def symbol_candidates(text):
    """Backticked tokens shaped like code symbols, outside historical sections.

    Plain words stay out: a token counts only when it has a `::` path, a call
    `()`, an underscore in a lower-case name, or at least two CamelCase humps.
    """
    historical_level = None
    for line in text.splitlines():
        heading = re.match(r"^(#+)\s", line)
        if heading:
            level = len(heading.group(1))
            if historical_level is not None and level <= historical_level:
                historical_level = None
            if historical_level is None and HISTORICAL_HEADING_RE.search(line):
                historical_level = level
            continue
        if historical_level is not None:
            continue
        for token in CODE_RE.findall(line):
            token = token.strip()
            match = SYMBOL_RE.match(token)
            if not match:
                continue
            segments = match.group(1).split("::")
            last = segments[-1]
            snake = "_" in last and last == last.lower() and len(last) > 4
            if "::" in token or token.endswith("()") or snake or CAMEL_RE.match(last):
                yield token, segments


def check_symbols(rel, text, backlog, symbols):
    """A code symbol a document names must exist, unless it is on the recorded backlog."""
    for token, segments in symbol_candidates(text):
        missing = [s for s in segments if s not in symbols and s not in ("crate", "self", "super", "std")]
        if not missing:
            continue
        STALE_SYMBOLS.setdefault(rel, [])
        if token not in STALE_SYMBOLS[rel]:
            STALE_SYMBOLS[rel].append(token)
        if token in backlog.get(rel, []):
            continue
        bad("SYMBOLS", f"{rel} names a code symbol that does not exist: {token}")


def check_crates(rel, text, crates):
    """A document that calls a name a crate must name a real crate."""
    for span in CODE_RE.finditer(text):
        token = span.group(1).strip().rstrip(".,;:")
        if not CRATE_RE.match(token):
            continue
        if token in crates or token in CRATE_ALIASES:
            continue
        window = text[span.end(): span.end() + 120].lower()
        mentions_crate = any(
            word in window
            for word in ("crate", "package", "workspace member", "workspace crate")
        )
        cargo_command = re.search(rf"cargo\b[^\n]*\b{re.escape(token)}\b", text)
        if mentions_crate or cargo_command:
            bad("CRATES", f"{rel} names an unknown crate: {token}")
        else:
            advisory("CRATES", f"{rel} uses the crate-shaped name {token!r} without claiming a crate")


def check_just(rel, text, targets):
    """A `just <target>` command must name a recipe the justfile has."""
    for span in CODE_RE.finditer(text):
        for target in JUST_RE.findall(span.group(1)):
            if target not in targets:
                bad("JUST", f"{rel} names a `just {target}` target that does not exist")


def check_ports(rel, text, daemon_port):
    """Advisory: a local URL in prose may be a QA fixture port, not the product one."""
    if daemon_port is None:
        return
    for match in LOCAL_URL_RE.finditer(text):
        port = match.group(1)
        if port == daemon_port:
            continue
        window = text[max(0, match.start() - 160): match.end() + 160].lower()
        if "panel" in window or "daemon" in window or "gateway" in window:
            advisory(
                "PORT",
                f"{rel} points a local URL at port {port}, the code serves the Panel on {daemon_port}",
            )


def check_dual_track(rel, text):
    """The entry documents must keep both desktop hosts visible."""
    lowered = text.lower()
    has_nsh = "nsh" in lowered
    has_tauri = "tauri" in lowered
    if not has_nsh:
        bad("DUAL", f"{rel} never mentions the NSH desktop host")
    if not has_tauri:
        bad("DUAL", f"{rel} never mentions the legacy Tauri desktop host")


def check_cut_phrases(rel, text):
    for phrase, note in CUT_PHRASES:
        if phrase in text:
            bad("STALE", f"{rel} still asserts {phrase!r} - {note}")


def normalize_constitution(text):
    """Strip the deltas the two constitution copies are allowed to have."""
    lines = text.splitlines()
    kept = []
    skip = False
    for line in lines:
        title = re.match(r"^##\s+(.*)$", line)
        if title:
            name = title.group(1).strip()
            skip = any(allowed in name for allowed in SYNC_ALLOWED_DIFFS)
            if skip:
                continue
        if skip:
            continue
        if line.startswith("# ") and "Constitution" in line:
            kept.append("# <constitution title>")
            continue
        if line.startswith("> **入口分工**"):
            kept.append("> <entrance paragraph>")
            continue
        if line.strip() == "---" or re.match(r"^\*Last updated:.*\*$", line.strip()):
            # Separators and the timestamp may sit in different places in the two
            # copies; they carry no architectural claim.
            continue
        if not line.strip():
            # Blank lines drift when a section is inserted; content is what matters.
            continue
        kept.append(line)
    return "\n".join(kept)


def sections(text):
    """Split a markdown document into (title, body) sections."""
    out = []
    title = "(preamble)"
    body = []
    for line in text.splitlines():
        match = re.match(r"^##\s+(.*)$", line)
        if match:
            out.append((title, "\n".join(body)))
            title = match.group(1).strip()
            body = []
        else:
            body.append(line)
    out.append((title, "\n".join(body)))
    return out


def check_constitution_sync():
    """AGENTS.md and CLAUDE.md must stay identical except their known deltas."""
    agents = normalize_constitution(read("AGENTS.md"))
    claude = normalize_constitution(read("CLAUDE.md"))
    diff = list(
        difflib.unified_diff(
            agents.splitlines(), claude.splitlines(), "AGENTS.md", "CLAUDE.md", lineterm="", n=1
        )
    )
    if diff:
        detail = "\n".join(diff[:20])
        bad("SYNC", f"AGENTS.md and CLAUDE.md drifted apart:\n{detail}")


def check_reachability(docs):
    """Advisory: reference documents that no index points at."""
    linked = set()
    for rel, text in docs:
        for target in LINK_RE.findall(text):
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            target = target.split("#", 1)[0]
            if not target:
                continue
            try:
                resolved = ((ROOT / rel).parent / target).resolve()
                resolved = resolved.relative_to(ROOT)
            except (ValueError, OSError):
                continue
            linked.add(resolved.as_posix())
    for rel, _ in docs:
        if rel.startswith(REFERENCE_DIR) and rel not in linked:
            advisory("REACH", f"{rel} is not linked from any other canonical document")


def load_backlog():
    """The recorded paths and symbols that are already known to be stale.

    A ratchet, not a silence: a path that is already listed here is reported as
    known backlog, a path that appears for the first time fails the guard.
    """
    path = ROOT / "scripts/doc_wiring_audit_baseline.json"
    if not path.is_file():
        return {}, {}
    data = json.loads(path.read_text(encoding="utf-8"))
    return data.get("PATHS", {}), data.get("SYMBOLS", {})


def main():
    docs = canonical_docs()
    if not docs:
        bad("SETUP", "the canonical documentation set is empty")
        print("doc wiring audit: FAIL (no canonical documents found)")
        return 1

    crates = known_crates()
    targets = known_just_targets()
    daemon_port = code_daemon_port()
    backlog, symbol_backlog = load_backlog()
    symbols = known_symbols()

    for rel, text in docs:
        check_links(rel, text)
        check_paths(rel, text, backlog)
        check_symbols(rel, text, symbol_backlog, symbols)
        check_unknown_roots(rel, text)
        check_crates(rel, text, crates)
        check_just(rel, text, targets)
        check_ports(rel, text, daemon_port)
        check_cut_phrases(rel, text)
    for rel in ENTRY_DOCS:
        path = ROOT / rel
        if path.is_file():
            check_dual_track(rel, path.read_text(encoding="utf-8"))
    check_constitution_sync()
    check_reachability(docs)

    report = {
        "canonical_documents": len(docs),
        "known_crates": len(crates),
        "known_just_targets": len(targets),
        "daemon_port": daemon_port,
        "failures": BAD,
        "advisory": ADVISORY,
    }
    known_backlog = sum(len(v) for v in backlog.values())
    known_symbols_backlog = sum(len(v) for v in symbol_backlog.values())
    if "--update-baseline" in sys.argv:
        # Re-record the ratchet: a path that was repaired leaves the backlog,
        # a path that is still stale stays on it.
        payload = {
            "PATHS": {
                rel: sorted(set(tokens)) for rel, tokens in sorted(STALE_PATHS.items())
            },
            "SYMBOLS": {
                rel: sorted(set(tokens)) for rel, tokens in sorted(STALE_SYMBOLS.items())
            },
        }
        target = ROOT / "scripts/doc_wiring_audit_baseline.json"
        target.write_text(
            json.dumps(payload, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        total = sum(len(v) for v in payload["PATHS"].values())
        total_symbols = sum(len(v) for v in payload["SYMBOLS"].values())
        print(
            f"doc wiring audit: baseline re-recorded, {total} stale path(s) "
            f"across {len(payload['PATHS'])} document(s), {total_symbols} stale "
            f"symbol(s) across {len(payload['SYMBOLS'])} document(s)"
        )
        return 1 if BAD else 0
    if "--json" in sys.argv:
        print(json.dumps(report, indent=2, ensure_ascii=False))
    else:
        for line in BAD:
            print(f"  {line}")
        for line in ADVISORY:
            print(f"  advisory {line}")
        print(
            f"doc wiring audit: {len(BAD)} failure(s), {len(ADVISORY)} advisory, "
            f"{known_backlog} known stale path(s) and {known_symbols_backlog} "
            f"known stale symbol(s) on the backlog "
            f"({len(docs)} canonical documents)"
        )
    return 1 if BAD else 0


if __name__ == "__main__":
    sys.exit(main())
