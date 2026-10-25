#!/usr/bin/env python3
"""Archive the pre-removal Terminal agent/MoA closure before deletion.

The archive is source-only: tracked repository files selected by explicit seeds and
stable deletion symbols are copied with repository-relative names.  It intentionally
never reads target/, databases, credentials, or generated archive output.
"""
from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import subprocess
import tarfile
from pathlib import Path
from typing import Iterable

ARCHIVE_ROOT = Path("archive/removed-terminal-agent-moa")
ARCHIVE_PREFIX = "aleph-terminal-agent-moa-pre-removal-"

EXPLICIT_SEEDS = {
    "crates/agent-detect",
    "src/gateway/pty/foreground.rs",
    "src/gateway/pty/runtime.rs",
    "src/gateway/runtime",
    "src/gateway/handlers/runtime.rs",
    "src/builtin_tools/terminal/capabilities.rs",
    "src/builtin_tools/terminal/tests.rs",
    "shared/protocol/src/runtime.rs",
    "interfaces/webchat/src/api/runtime_agents.rs",
    "interfaces/webchat/src/components/sidebar/agent_panel.rs",
    "shared/ui_logic/src/state/agent_panel.rs",
    "shared/ui_logic/src/state/agent_panel_parity.rs",
    "interfaces/tui/src/tui/widgets/agent_panel.rs",
}

DYNAMIC_TOKENS = (
    b"RuntimeAgents",
    b"RuntimeAgentState",
    b"terminal_sessions_list",
    b"terminal_sessions_read",
    b"terminal_sessions_status",
    b"terminal_sessions_wait",
    b"terminal_sessions_explain",
    b"terminal_sessions_attach",
    b"normalize_terminal_compat_call",
    b"DetectionInputs",
    b"agent-detect",
    b"runtime.agents.changed",
    b"runtime_agents",
    b"AgentPanel",
    b"foreground::",
)

DYNAMIC_ROOTS = (
    "Cargo.toml",
    "Cargo.lock",
    "src/",
    "shared/protocol/",
    "shared/ui_logic/",
    "interfaces/webchat/",
    "interfaces/tui/",
    "qa/",
    "docs/reference/",
)

EXCLUDED_GENERATED_PREFIXES = (
    "interfaces/webchat/dist/",
    "interfaces/tui/dist/",
    "interfaces/webchat/node_modules/",
    "interfaces/tui/node_modules/",
)


def run_git(root: Path, *args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=root, text=True).strip()


def tracked_files(root: Path) -> list[str]:
    raw = subprocess.check_output(["git", "ls-files", "-z"], cwd=root)
    return [item.decode() for item in raw.split(b"\0") if item]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def is_dynamic_candidate(path: str) -> bool:
    return not path.startswith(EXCLUDED_GENERATED_PREFIXES) and any(
        path == root or path.startswith(root) for root in DYNAMIC_ROOTS
    )


def category(path: str) -> str:
    if path.startswith("crates/"):
        return "crate"
    if path.startswith("src/"):
        return "production-source"
    if path.startswith("shared/"):
        return "shared-contract-or-ui"
    if path.startswith("interfaces/"):
        return "interface-ui"
    if path.startswith("qa/"):
        return "qa"
    if path.startswith("docs/"):
        return "reference-document"
    return "workspace-manifest"


def production_or_test(path: str) -> str:
    lowered = path.lower()
    if "/tests" in lowered or lowered.endswith("_test.rs") or "/test" in lowered:
        return "test"
    if path.startswith("qa/"):
        return "qa"
    if path.startswith("docs/"):
        return "documentation"
    return "production"


def select_files(root: Path) -> tuple[list[str], dict[str, list[str]], list[str]]:
    tracked = tracked_files(root)
    selected: set[str] = set()
    reasons: dict[str, list[str]] = {}
    seed_matches: list[str] = []

    for path in tracked:
        if path == "crates/agent-detect" or path.startswith("crates/agent-detect/"):
            selected.add(path)
            reasons.setdefault(path, []).append("complete crates/agent-detect snapshot")

    for seed in EXPLICIT_SEEDS:
        matches = [p for p in tracked if p == seed or p.startswith(seed + "/")]
        for path in matches:
            selected.add(path)
            reasons.setdefault(path, []).append(f"explicit seed: {seed}")
            if path not in seed_matches:
                seed_matches.append(path)

    for path in tracked:
        if not is_dynamic_candidate(path) or path in selected:
            continue
        file_path = root / path
        try:
            data = file_path.read_bytes()
        except OSError:
            continue
        hits = [token.decode("ascii") for token in DYNAMIC_TOKENS if token in data]
        if hits:
            selected.add(path)
            reasons.setdefault(path, []).append("dynamic deletion reference: " + ", ".join(hits))

    return sorted(selected), reasons, sorted(seed_matches)


def deterministic_add(tar: tarfile.TarFile, path: Path, arcname: str) -> None:
    info = tar.gettarinfo(str(path), arcname=arcname)
    info.mtime = 0
    info.uid = 0
    info.gid = 0
    info.uname = ""
    info.gname = ""
    if info.isreg():
        with path.open("rb") as handle:
            tar.addfile(info, handle)
    else:
        tar.addfile(info)


def manifest_for(root: Path, files: Iterable[str], reasons: dict[str, list[str]], head: str, date: str) -> dict:
    entries = []
    for path in files:
        absolute = root / path
        entries.append(
            {
                "path": path,
                "category": category(path),
                "sha256": sha256(absolute),
                "reason": reasons[path],
                "complete_snapshot": True,
                "production_or_test": production_or_test(path),
            }
        )
    return {
        "format": 1,
        "archive": "removed-terminal-agent-moa",
        "source_head": head,
        "source_date_utc": date,
        "selection": {
            "seed": "tracked files under crates/agent-detect plus explicit mixed-module seeds",
            "dynamic_references": [token.decode("ascii") for token in DYNAMIC_TOKENS],
            "dynamic_reference_decision": "Include tracked source, protocol, UI/TUI, QA, and current reference files containing deletion symbols; preserve complete snapshots of mixed files.",
            "excluded": ["target/", "databases", "credentials", "temporary files", "archive sidecars"],
        },
        "counts": {
            "files": len(entries),
            "production": sum(item["production_or_test"] == "production" for item in entries),
            "test": sum(item["production_or_test"] == "test" for item in entries),
            "qa": sum(item["production_or_test"] == "qa" for item in entries),
            "documentation": sum(item["production_or_test"] == "documentation" for item in entries),
            "bytes": sum((root / item["path"]).stat().st_size for item in entries),
        },
        "files": entries,
    }


def verify(output: Path, root: Path) -> None:
    manifest_path = output / "MANIFEST.json"
    tar_path = next(output.glob("*.tar.gz"))
    readme_path = output / "README.md"
    checksum_path = output / "SHA256SUMS"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    expected = sorted(item["path"] for item in manifest["files"])
    with tarfile.open(tar_path, "r:gz") as archive:
        actual = sorted(member.name for member in archive.getmembers() if member.isfile())
        if actual != expected:
            raise SystemExit(f"archive member mismatch: expected {len(expected)}, got {len(actual)}")
        if "crates/agent-detect/NOTICE" not in actual:
            raise SystemExit("crates/agent-detect/NOTICE is missing from archive")
        for item in manifest["files"]:
            member = archive.extractfile(item["path"])
            if member is None:
                raise SystemExit(f"missing archive member: {item['path']}")
            digest = hashlib.sha256(member.read()).hexdigest()
            if digest != item["sha256"]:
                raise SystemExit(f"hash mismatch in archive: {item['path']}")
    checksum_lines = {}
    for line in checksum_path.read_text(encoding="utf-8").splitlines():
        digest, name = line.split("  ", 1)
        checksum_lines[name] = digest
    for path in (tar_path, manifest_path, readme_path):
        if checksum_lines.get(path.name) != sha256(path):
            raise SystemExit(f"checksum mismatch: {path.name}")
    if set(checksum_lines) != {tar_path.name, manifest_path.name, readme_path.name}:
        raise SystemExit("SHA256SUMS must cover exactly tar, manifest, and README")
    print(json.dumps({"verified": True, "files": len(expected), "bytes": manifest["counts"]["bytes"], "archive": str(tar_path)}, ensure_ascii=False))


def generate(root: Path, date: str) -> Path:
    output = root / ARCHIVE_ROOT
    output.mkdir(parents=True, exist_ok=True)
    head = run_git(root, "rev-parse", "HEAD")
    files, reasons, seed_matches = select_files(root)
    if "crates/agent-detect/NOTICE" not in files:
        raise SystemExit("selection error: agent-detect NOTICE was not selected")
    manifest = manifest_for(root, files, reasons, head, date)
    tar_path = output / f"{ARCHIVE_PREFIX}{date}.tar.gz"
    manifest_path = output / "MANIFEST.json"
    readme_path = output / "README.md"
    checksum_path = output / "SHA256SUMS"
    for path in (tar_path, manifest_path, readme_path, checksum_path):
        if path.exists():
            path.unlink()
    with tarfile.open(tar_path, "w:gz") as archive:
        for path in files:
            deterministic_add(archive, root / path, path)
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    readme_path.write_text(
        f"# Removed Terminal Agent MoA archive\n\n"
        f"This is the pre-removal source snapshot for the Aleph Terminal herdr-like agent detection, runtime panel, and terminal observation capability removal.\n\n"
        f"- Source HEAD: `{head}`\n"
        f"- Archive date (UTC): `{date}`\n"
        f"- Tar members: `{manifest['counts']['files']}` tracked files, `{manifest['counts']['bytes']}` source bytes\n"
        f"- Selection: complete `crates/agent-detect/**` plus explicit mixed-module seeds and tracked dynamic references listed in `MANIFEST.json`.\n"
        f"- `crates/agent-detect/NOTICE` is required and verified.\n"
        f"- The archive contains repository-relative paths only; sidecars are outside the tar to avoid checksum recursion.\n\n"
        f"Historical/pre-removal documentation remains in the repository where needed and is marked as such. Current references must describe Terminal as pure PTY/VT/session/attach/title/cwd/ownership.\n",
        encoding="utf-8",
    )
    checksum_path.write_text(
        "".join(f"{sha256(path)}  {path.name}\n" for path in (tar_path, manifest_path, readme_path)),
        encoding="utf-8",
    )
    verify(output, root)
    print(json.dumps({"source_head": head, "seed_matches": len(seed_matches), "manifest": str(manifest_path)}, ensure_ascii=False))
    return output


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--date", default=dt.datetime.now(dt.timezone.utc).date().isoformat())
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    root = Path(run_git(Path.cwd(), "rev-parse", "--show-toplevel"))
    output = root / ARCHIVE_ROOT
    if args.verify:
        verify(output, root)
    else:
        generate(root, args.date)


if __name__ == "__main__":
    main()
