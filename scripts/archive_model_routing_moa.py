#!/usr/bin/env python3
"""Create a deterministic pre-removal archive for model routing and MoA code.

The archive is intentionally source-only.  It records the source closure used to
select files, the dynamic-reference review decisions, and enough repository
metadata to identify the exact pre-removal tree.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import re
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path, PurePosixPath
from typing import Iterable, Mapping, Sequence


TOOL_NAME = "archive_model_routing_moa"
TOOL_VERSION = "1.0"
ARCHIVE_FILENAME = "aleph-model-routing-moa-pre-removal-2026-10-19.tar.gz"

DELETED_PATHS = (
    "src/providers/moa/activation.rs",
    "src/providers/moa/advisor_health.rs",
    "src/providers/moa/advisory_view.rs",
    "src/providers/moa/config_handle.rs",
    "src/providers/moa/fan_out.rs",
    "src/providers/moa/mod.rs",
    "src/providers/moa/preset_store.rs",
    "src/providers/moa/prompts.rs",
    "src/providers/moa/provider.rs",
    "src/providers/session_moa_handle.rs",
    "src/config/types/moa.rs",
    "src/builtin_tools/moa_manage.rs",
    "src/gateway/handlers/moa.rs",
    "interfaces/webchat/src/api/moa.rs",
    "interfaces/webchat/src/platform/wide/views/settings/moa/mod.rs",
    "interfaces/webchat/src/platform/wide/views/settings/moa/options.rs",
    "interfaces/webchat/src/platform/wide/views/settings/moa/preset_editor.rs",
    "src/routing/recall.rs",
    "src/routing/experience_store.rs",
    "src/routing/observer.rs",
    "src/memory/store/sqlite/routing_experience.rs",
)

REQUIRED_SNAPSHOT_PATHS = (
    "shared/protocol/src/events.rs",
    "shared/protocol/src/trace_presentation.rs",
    "shared/protocol/src/trace_replay.rs",
)

SEED_SYMBOLS = (
    "MoaProvider",
    "MoaManageTool",
    "session_moa_handle",
    "RoutingRecall",
    "RoutingExperienceStore",
    "OutcomeObserver",
    "routing_experience",
    "model_hint",
    "provider_hint",
    "proposer_models",
    "moa_synthesis",
    "moa_completed",
    "moa_synthesis_failed",
    "moa_no_proposals",
    "moa.*",
    "protocol == \"moa\"",
    "MoaAdvisor",
    "MoaAggregating",
    "MoaAdvisorSpend",
    "MoaTurnTrace",
    "effective_model_directive",
)

SOURCE_SUFFIXES = {
    ".rs",
    ".toml",
    ".json",
    ".md",
    ".yaml",
    ".yml",
    ".py",
    ".js",
    ".jsx",
    ".ts",
    ".tsx",
    ".sh",
}

DYNAMIC_REFERENCE_PATTERN = re.compile(
    r"(?:format!|concat!|include!|env!|option_env!|serde|rename|register|registry|rpc|mcp|acp|slash|command|cfg\s*\()",
    re.IGNORECASE,
)
CLOSURE_TOKEN_PATTERN = re.compile(
    r"\b(?:Moa[A-Za-z0-9_]*|Routing[A-Za-z0-9_]*|Outcome[A-Za-z0-9_]*|"
    r"[a-z][a-z0-9_]*(?:_hint|_experience|_synthesis|_proposal|_proposals|_moa|moa_[a-z0-9_]*)|"
    r"effective_model_directive)\b"
)


class ArchiveError(RuntimeError):
    """Raised when the archive cannot be safely produced."""


class ChecksumMismatch(ArchiveError):
    """Raised when a manifest entry does not match the source bytes."""


def validate_relative_path(value: object, field: str, root: Path | None = None) -> str:
    """Validate and canonicalize a repository-relative archive path."""
    if not isinstance(value, str) or not value:
        raise ArchiveError(f"{field} must be a non-empty relative path")
    if "\x00" in value:
        raise ArchiveError(f"{field} contains a NUL byte: {value!r}")

    normalized = value.replace("\\", "/")
    if normalized.startswith("/") or normalized.startswith("//") or re.match(r"^[A-Za-z]:", normalized):
        raise ArchiveError(f"{field} must be relative: {value!r}")
    parts = normalized.split("/")
    if any(part in {"", ".", ".."} for part in parts):
        raise ArchiveError(f"{field} contains an unsafe path component: {value!r}")
    canonical = str(PurePosixPath(*parts))
    if not canonical or canonical == ".":
        raise ArchiveError(f"{field} must name a file below the repository root")

    if root is not None:
        root_resolved = root.resolve()
        candidate = (root_resolved / Path(*parts)).resolve(strict=False)
        try:
            candidate.relative_to(root_resolved)
        except ValueError as error:
            raise ArchiveError(f"{field} escapes root {root_resolved}: {value!r}") from error
    return canonical


def safe_path(root: Path, value: object, field: str) -> tuple[Path, str]:
    """Return a validated path and reject symlink escapes from root."""
    canonical = validate_relative_path(value, field, root)
    root_resolved = root.resolve()
    candidate = (root_resolved / Path(*canonical.split("/"))).resolve(strict=False)
    try:
        candidate.relative_to(root_resolved)
    except ValueError as error:
        raise ArchiveError(f"{field} escapes root {root_resolved}: {value!r}") from error
    return candidate, canonical


def validated_manifest_entries(
    root: Path, manifest: Mapping[str, object] | Sequence[Mapping[str, object]]
) -> list[Mapping[str, object]]:
    """Validate manifest shape, paths, hashes, and uniqueness before I/O."""
    raw_entries: object = manifest["entries"] if isinstance(manifest, Mapping) else manifest
    if not isinstance(raw_entries, Sequence) or isinstance(raw_entries, (str, bytes)):
        raise ArchiveError("manifest entries must be a sequence")

    entries: list[Mapping[str, object]] = []
    original_paths: set[str] = set()
    archive_paths: set[str] = set()
    for index, raw_entry in enumerate(raw_entries):
        if not isinstance(raw_entry, Mapping):
            raise ArchiveError(f"manifest entry {index} is not an object")
        try:
            original_path = validate_relative_path(
                raw_entry["original_relative_path"],
                f"manifest entry {index} original_relative_path",
                root,
            )
            archive_path = validate_relative_path(
                raw_entry["archive_path"],
                f"manifest entry {index} archive_path",
                root,
            )
            expected_hash = raw_entry["sha256"]
        except KeyError as error:
            raise ArchiveError(f"manifest entry {index} is missing {error.args[0]}") from error
        if not isinstance(expected_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_hash):
            raise ArchiveError(f"manifest entry {index} has an invalid sha256")
        if original_path in original_paths:
            raise ArchiveError(f"duplicate original_relative_path in manifest: {original_path}")
        if archive_path in archive_paths:
            raise ArchiveError(f"duplicate archive_path in manifest: {archive_path}")
        original_paths.add(original_path)
        archive_paths.add(archive_path)
        entries.append(raw_entry)
    return entries


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def run_git(repo_root: Path, *args: str) -> str:
    completed = subprocess.run(
        ["git", *args],
        cwd=repo_root,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise ArchiveError(
            f"git {' '.join(args)} failed ({completed.returncode}): "
            f"{completed.stderr.strip()}"
        )
    return completed.stdout.strip()


def require_clean_worktree(repo_root: Path) -> None:
    status = run_git(repo_root, "status", "--porcelain=v1", "--untracked-files=all")
    if status:
        raise ArchiveError(
            "refusing dirty worktree; commit or use a clean detached worktree before "
            "creating the pre-removal archive:\n" + status
        )


def tracked_paths(repo_root: Path) -> list[str]:
    output = run_git(repo_root, "ls-files", "-z")
    return sorted(path for path in output.split("\0") if path)


def is_scan_candidate(path: str) -> bool:
    return Path(path).suffix.lower() in SOURCE_SUFFIXES


def read_text(repo_root: Path, relative_path: str) -> str:
    return (repo_root / relative_path).read_text(encoding="utf-8", errors="replace")


def seed_pattern() -> re.Pattern[str]:
    alternatives = [re.escape(symbol) for symbol in SEED_SYMBOLS if symbol != "moa.*"]
    alternatives.append(r"\bmoa(?:[.:_-]|\b)")
    return re.compile("(?:" + "|".join(alternatives) + ")", re.IGNORECASE)


def candidate_texts(repo_root: Path, paths: Sequence[str]) -> dict[str, str]:
    return {
        path: read_text(repo_root, path)
        for path in paths
        if is_scan_candidate(path)
    }


def collect_source_closure(repo_root: Path, paths: Sequence[str]) -> list[str]:
    """Find a fixed point of model-routing/MoA source references."""
    available = set(paths)
    texts = candidate_texts(repo_root, paths)
    pattern = seed_pattern()
    selected = {path for path in DELETED_PATHS if path in available}
    selected.update(path for path in REQUIRED_SNAPSHOT_PATHS if path in available)
    selected.update(path for path, text in texts.items() if pattern.search(text))

    token_index: dict[str, set[str]] = {}
    for path, text in texts.items():
        for token in set(CLOSURE_TOKEN_PATTERN.findall(text)):
            token_index.setdefault(token, set()).add(path)

    changed = True
    while changed:
        discovered: set[str] = set()
        for path in selected:
            discovered.update(CLOSURE_TOKEN_PATTERN.findall(texts.get(path, "")))
        additions = set().union(*(token_index.get(token, set()) for token in discovered)) - selected
        selected.update(additions)
        changed = bool(additions)

    missing = [path for path in (*DELETED_PATHS, *REQUIRED_SNAPSHOT_PATHS) if path not in available]
    if missing:
        raise ArchiveError(
            "pre-removal source is incomplete; required archive paths are missing: "
            + ", ".join(missing)
        )
    return sorted(selected)


def discover_dynamic_references(
    repo_root: Path, paths: Sequence[str], selected_paths: Iterable[str]
) -> list[dict[str, object]]:
    """Return dynamic registration/reference lines and their review decisions."""
    selected = set(selected_paths)
    pattern = seed_pattern()
    records: list[dict[str, object]] = []
    for path, text in sorted(candidate_texts(repo_root, paths).items()):
        for line_number, line in enumerate(text.splitlines(), 1):
            if pattern.search(line) and DYNAMIC_REFERENCE_PATTERN.search(line):
                records.append(
                    {
                        "path": path,
                        "line": line_number,
                        "excerpt": line.strip()[:240],
                        "decision": "included_source_snapshot" if path in selected else None,
                    }
                )
    return sorted(records, key=lambda item: (str(item["path"]), int(item["line"]), str(item["excerpt"])))


def unresolved_dynamic_references(records: Iterable[Mapping[str, object]]) -> list[Mapping[str, object]]:
    return [record for record in records if not str(record.get("decision") or "").strip()]


def validate_dynamic_references(records: Iterable[Mapping[str, object]]) -> None:
    unresolved = unresolved_dynamic_references(records)
    if unresolved:
        rendered = ", ".join(f"{item['path']}:{item['line']}" for item in unresolved)
        raise ArchiveError("unresolved dynamic references lack manifest decisions: " + rendered)


def category_for_path(path: str) -> tuple[str, str]:
    normalized = path.replace("\\", "/")
    lower = normalized.lower()
    if normalized in DELETED_PATHS:
        return "deleted_source", "remove the obsolete model-routing or MoA implementation"
    if lower.startswith("docs/"):
        return "documentation", "preserve pre-removal design and migration context"
    if lower.startswith("scripts/"):
        return "archive_tool", "preserve the source-closure and archive verification helper"
    if "/tests/" in lower or lower.endswith("_test.rs") or "test" in Path(lower).name:
        return "test", "preserve pre-removal coverage and fixtures"
    if "/locales/" in lower or lower.endswith("locales.json"):
        return "locale", "preserve pre-removal user-visible strings"
    return "modified_source_snapshot", "preserve the complete pre-removal file containing a removal reference"


def build_manifest_entries(repo_root: Path, selected_paths: Sequence[str]) -> list[dict[str, object]]:
    entries: list[dict[str, object]] = []
    for raw_relative_path in sorted(set(selected_paths)):
        source, relative_path = safe_path(repo_root, raw_relative_path, "original_relative_path")
        if not source.is_file():
            raise ArchiveError(f"selected archive path is not a regular file: {relative_path}")
        archive_path = validate_relative_path(relative_path, "archive_path", repo_root)
        category, reason = category_for_path(relative_path)
        is_test = category == "test"
        entries.append(
            {
                "original_relative_path": relative_path,
                "archive_path": archive_path,
                "category": category,
                "sha256": sha256_file(source),
                "snapshot_kind": "deleted_file" if relative_path in DELETED_PATHS else "complete_file_snapshot",
                "deletion_reason": reason,
                "production_test": "test" if is_test else "production",
                "size_bytes": source.stat().st_size,
            }
        )
    return entries


def repository_metadata(repo_root: Path, recorded_root: Path | None = None) -> dict[str, object]:
    cargo_lock = repo_root / "Cargo.lock"
    branch = run_git(repo_root, "branch", "--show-current")
    if not branch:
        containing = run_git(repo_root, "for-each-ref", "--contains", "HEAD", "--format=%(refname:short)")
        branch = sorted(line for line in containing.splitlines() if line)[0] if containing else "DETACHED"
    return {
        "commit": run_git(repo_root, "rev-parse", "HEAD"),
        "branch": branch,
        "repository_root": str(recorded_root) if recorded_root is not None else ".",
        "cargo_lock_sha256": sha256_file(cargo_lock) if cargo_lock.is_file() else None,
        "tool": {"name": TOOL_NAME, "version": TOOL_VERSION},
    }


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def build_manifest(
    repo_root: Path,
    selected_paths: Sequence[str],
    all_paths: Sequence[str],
    recorded_root: Path | None = None,
) -> dict[str, object]:
    entries = build_manifest_entries(repo_root, selected_paths)
    dynamic_references = discover_dynamic_references(repo_root, all_paths, selected_paths)
    validate_dynamic_references(dynamic_references)
    return {
        "format_version": 1,
        "archive": ARCHIVE_FILENAME,
        "source": repository_metadata(repo_root, recorded_root),
        "seed_symbols": sorted(SEED_SYMBOLS),
        "source_closure": {
            "algorithm": "seed symbols plus recursively discovered model-routing/MoA identifier references to a fixed point",
            "entry_count": len(entries),
            "paths": [entry["original_relative_path"] for entry in entries],
        },
        "dynamic_reference_review": dynamic_references,
        "unresolved_dynamic_reference_count": len(unresolved_dynamic_references(dynamic_references)),
        "entries": entries,
    }


def create_tarball(repo_root: Path, archive_path: Path, entries: Sequence[Mapping[str, object]]) -> None:
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    validated_entries = validated_manifest_entries(repo_root, entries)
    archive_names = [str(entry["archive_path"]) for entry in validated_entries]
    if len(archive_names) != len(set(archive_names)):
        raise ArchiveError("manifest contains duplicate archive paths")
    with archive_path.open("wb") as output:
        with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                for entry in sorted(validated_entries, key=lambda item: str(item["archive_path"])):
                    source, relative_path = safe_path(repo_root, entry["original_relative_path"], "original_relative_path")
                    archive_name = validate_relative_path(entry["archive_path"], "archive_path", repo_root)
                    if not source.is_file():
                        raise ArchiveError(f"selected archive path is not a regular file: {relative_path}")
                    data = source.read_bytes()
                    info = tarfile.TarInfo(name=archive_name)
                    info.size = len(data)
                    info.mode = 0o644
                    info.mtime = 0
                    info.uid = 0
                    info.gid = 0
                    info.uname = ""
                    info.gname = ""
                    archive.addfile(info, io.BytesIO(data))


def verify_manifest_hashes(
    root: Path,
    manifest: Mapping[str, object] | Sequence[Mapping[str, object]],
    path_key: str = "original_relative_path",
) -> None:
    entries = validated_manifest_entries(root, manifest)
    if path_key not in {"original_relative_path", "archive_path"}:
        raise ArchiveError(f"unsupported manifest path key: {path_key}")
    for entry in entries:
        relative_path = validate_relative_path(entry[path_key], path_key, root)
        source, _ = safe_path(root, relative_path, path_key)
        if not source.is_file():
            raise ChecksumMismatch(f"manifest path is missing: {relative_path}")
        actual = sha256_file(source)
        expected = str(entry["sha256"])
        if actual != expected:
            raise ChecksumMismatch(f"checksum mismatch for {relative_path}: expected {expected}, got {actual}")


def extract_and_verify_archive(archive_path: Path, manifest: Mapping[str, object], destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    entries = validated_manifest_entries(destination, manifest)
    expected_names = [validate_relative_path(entry["archive_path"], "archive_path", destination) for entry in entries]
    expected_paths = set(expected_names)
    with tarfile.open(archive_path, mode="r:gz") as archive:
        members = archive.getmembers()
        member_names = [member.name for member in members]
        if len(members) != len(expected_names) or len(member_names) != len(set(member_names)):
            raise ArchiveError("archive contains duplicate or unexpected member count")
        if set(member_names) != expected_paths:
            raise ArchiveError(f"archive path set mismatch: expected {len(expected_paths)}, got {len(set(member_names))}")
        for member in members:
            if not member.isfile():
                raise ArchiveError(f"unsafe or non-file archive member: {member.name}")
            member_name = validate_relative_path(member.name, "archive member", destination)
            target, _ = safe_path(destination, member_name, "archive member")
            if target.is_symlink():
                raise ArchiveError(f"archive target is a symlink: {member.name}")
            target.parent.mkdir(parents=True, exist_ok=True)
            extracted = archive.extractfile(member)
            if extracted is None:
                raise ArchiveError(f"cannot read archive member: {member.name}")
            target.write_bytes(extracted.read())
    verify_manifest_hashes(destination, manifest, path_key="archive_path")


def write_readme(path: Path, manifest: Mapping[str, object]) -> None:
    source = manifest["source"]  # type: ignore[index]
    entries = manifest["entries"]  # type: ignore[index]
    dynamic = manifest["dynamic_reference_review"]  # type: ignore[index]
    path.write_text(
        "# Removed model routing and MoA pre-removal archive\n\n"
        "This archive captures the source tree before the approved model-routing and MoA removal. "
        "It is historical evidence, not a backup crate and must not be added to the workspace.\n\n"
        f"- Source commit: `{source['commit']}`\n"
        f"- Source branch: `{source['branch']}`\n"
        f"- Cargo.lock SHA-256: `{source['cargo_lock_sha256'] or 'missing'}`\n"
        f"- Archive entries: `{len(entries)}`\n"
        f"- Dynamic references reviewed: `{len(dynamic)}`\n"
        f"- Unresolved dynamic references: `{manifest['unresolved_dynamic_reference_count']}`\n\n"
        "Every archive member keeps its original repository-relative path. `MANIFEST.json` records "
        "the closure decision, category, snapshot kind, removal reason, classification, and source hash. "
        "`SHA256SUMS` covers this README, the manifest, and the compressed archive.\n\n"
        "Restoring any file requires re-reviewing the current provider, session, wire, and security "
        "contracts. The archive deliberately excludes target directories, build output, credentials, "
        "user configuration, databases, logs, and temporary files.\n",
        encoding="utf-8",
    )


def write_checksums(output_dir: Path) -> None:
    names = [ARCHIVE_FILENAME, "MANIFEST.json", "README.md"]
    lines = [f"{sha256_file(output_dir / name)}  {name}" for name in sorted(names)]
    (output_dir / "SHA256SUMS").write_text("\n".join(lines) + "\n", encoding="utf-8")


def create_archive(
    repo_root: Path,
    output_dir: Path,
    recorded_root: Path | None = None,
) -> dict[str, object]:
    repo_root = repo_root.resolve()
    require_clean_worktree(repo_root)
    paths = tracked_paths(repo_root)
    selected_paths = collect_source_closure(repo_root, paths)
    manifest = build_manifest(repo_root, selected_paths, paths, recorded_root)
    output_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = output_dir / "MANIFEST.json"
    write_json(manifest_path, manifest)
    write_readme(output_dir / "README.md", manifest)
    create_tarball(repo_root, output_dir / ARCHIVE_FILENAME, manifest["entries"])  # type: ignore[arg-type]
    write_checksums(output_dir)
    return manifest


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "archive/removed-model-routing-moa",
    )
    parser.add_argument(
        "--recorded-root",
        type=Path,
        help="record this logical repository root when the clean source tree is a detached worktree",
    )
    args = parser.parse_args(argv)
    try:
        manifest = create_archive(args.repo_root, args.output_dir, args.recorded_root)
    except ArchiveError as error:
        print(f"archive failed: {error}", file=sys.stderr)
        return 1
    print(
        f"archived {manifest['source_closure']['entry_count']} files to "
        f"{(args.output_dir / ARCHIVE_FILENAME).resolve()}"
    )
    return 0


class ArchiveHelperTests(unittest.TestCase):
    def git(self, root: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=root, check=True, capture_output=True)

    def make_repo(self) -> Path:
        root = Path(tempfile.mkdtemp(prefix="archive-helper-test-"))
        self.git(root, "init", "-q")
        self.git(root, "config", "user.email", "test@example.invalid")
        self.git(root, "config", "user.name", "Archive Test")
        (root / "src/nested").mkdir(parents=True)
        (root / "src/nested/z.rs").write_bytes(b"MoaProvider\n")
        (root / "src/a.rs").write_text("effective_model_directive\n", encoding="utf-8")
        self.git(root, "add", ".")
        self.git(root, "commit", "-qm", "fixture")
        return root

    def make_dynamic_repo(self) -> Path:
        root = self.make_repo()
        for relative_path in (*DELETED_PATHS, *REQUIRED_SNAPSHOT_PATHS):
            path = root / relative_path
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('registry.register("moa");\n', encoding="utf-8")
        (root / "src/registry.rs").write_text('registry.register("moa");\n', encoding="utf-8")
        self.git(root, "add", ".")
        self.git(root, "commit", "-qm", "dynamic registry fixture")
        return root

    def test_repository_metadata_uses_stable_relative_root(self) -> None:
        root = self.make_repo()
        metadata = repository_metadata(root)
        self.assertEqual(metadata["repository_root"], ".")
        self.assertNotIn(str(root), json.dumps(metadata))

    def test_path_validation_is_fail_closed(self) -> None:
        root = self.make_repo()
        for invalid_path in ("", "/tmp/outside", "../outside", "src/../../outside", r"..\\outside"):
            with self.subTest(invalid_path=invalid_path):
                with self.assertRaises(ArchiveError):
                    build_manifest_entries(root, [invalid_path])

        entries = build_manifest_entries(root, ["src/a.rs"])
        for invalid_path in ("", "/tmp/outside", "../outside", "src/../../outside"):
            with self.subTest(manifest_path=invalid_path):
                unsafe_entries = [dict(entries[0], archive_path=invalid_path)]
                with self.assertRaises(ArchiveError):
                    create_tarball(root, root / "unsafe.tar.gz", unsafe_entries)
                with self.assertRaises(ArchiveError):
                    verify_manifest_hashes(root, unsafe_entries)

    def test_duplicate_tar_members_are_rejected(self) -> None:
        root = self.make_repo()
        entries = build_manifest_entries(root, ["src/a.rs"])
        archive_path = root / "duplicate.tar.gz"
        data = (root / "src/a.rs").read_bytes()
        with archive_path.open("wb") as output:
            with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as archive:
                    for _ in range(2):
                        info = tarfile.TarInfo(name="src/a.rs")
                        info.size = len(data)
                        archive.addfile(info, io.BytesIO(data))
        with self.assertRaisesRegex(ArchiveError, "duplicate"):
            extract_and_verify_archive(archive_path, {"entries": entries}, root / "extracted")

    def test_dynamic_registration_requires_selected_snapshot(self) -> None:
        root = self.make_dynamic_repo()
        paths = tracked_paths(root)
        unresolved = discover_dynamic_references(root, paths, [])
        self.assertTrue(unresolved)
        self.assertEqual(unresolved_dynamic_references(unresolved), unresolved)
        with self.assertRaisesRegex(ArchiveError, "unresolved dynamic references"):
            validate_dynamic_references(unresolved)

    def test_dynamic_registration_in_normal_closure_is_included_snapshot(self) -> None:
        root = self.make_dynamic_repo()
        paths = tracked_paths(root)
        selected = collect_source_closure(root, paths)
        records = discover_dynamic_references(root, paths, selected)
        self.assertTrue(records)
        self.assertTrue(all(record["decision"] == "included_source_snapshot" for record in records))
        validate_dynamic_references(records)

    def test_dirty_worktree_refusal(self) -> None:
        root = self.make_repo()
        (root / "src/a.rs").write_text("dirty\n", encoding="utf-8")
        with self.assertRaisesRegex(ArchiveError, "dirty worktree"):
            require_clean_worktree(root)

    def test_deterministic_manifest_ordering(self) -> None:
        root = self.make_repo()
        entries = build_manifest_entries(root, ["src/nested/z.rs", "src/a.rs"])
        self.assertEqual([entry["original_relative_path"] for entry in entries], ["src/a.rs", "src/nested/z.rs"])
        self.assertEqual(entries, build_manifest_entries(root, ["src/nested/z.rs", "src/a.rs"]))

    def test_original_path_is_preserved_in_tarball(self) -> None:
        root = self.make_repo()
        entries = build_manifest_entries(root, ["src/nested/z.rs"])
        archive_path = root / "archive.tar.gz"
        create_tarball(root, archive_path, entries)
        with tarfile.open(archive_path, "r:gz") as archive:
            self.assertEqual(archive.getnames(), ["src/nested/z.rs"])
            self.assertEqual(archive.extractfile("src/nested/z.rs").read(), b"MoaProvider\n")

    def test_checksum_mismatch_is_reported(self) -> None:
        root = self.make_repo()
        entries = build_manifest_entries(root, ["src/a.rs"])
        (root / "src/a.rs").write_text("changed\n", encoding="utf-8")
        with self.assertRaisesRegex(ChecksumMismatch, "checksum mismatch"):
            verify_manifest_hashes(root, entries)

    def test_unresolved_dynamic_reference_is_reported(self) -> None:
        records = [{"path": "src/registry.rs", "line": 9, "excerpt": "register(moa)", "decision": None}]
        self.assertEqual(unresolved_dynamic_references(records), records)
        with self.assertRaisesRegex(ArchiveError, "unresolved dynamic references"):
            validate_dynamic_references(records)


if __name__ == "__main__":
    raise SystemExit(main() if any(argument.startswith("--") for argument in sys.argv[1:]) else unittest.main())
