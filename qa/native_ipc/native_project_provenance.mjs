import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

export const PROVENANCE_CONTRACT = "aleph-native-project-provenance-v1";
const DIGEST_RE = /^[0-9a-f]{64}$/;
const REVISION_RE = /^[0-9a-f]{40}$/;
const ARTIFACT_NAMES = ["shell", "helper", "reaper"];
const DIGEST_NAMES = ["wasm_sha256", "panel_manifest_sha256", "tauri_config_sha256"];

function digestFile(file) {
  if (!existsSync(file)) throw new Error("provenance_artifact_missing");
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

export function nativeArtifactPaths(repoRoot) {
  return {
    shell: path.join(repoRoot, "target", "debug", "aleph-desktop-shell.exe"),
    helper: path.join(repoRoot, "target", "debug", "aleph-panel-native-qa-gateway.exe"),
    reaper: path.join(repoRoot, "target", "debug", "examples", "qa_credential_reaper.exe"),
    wasm: path.join(repoRoot, "interfaces", "webchat", ".panel-live", "shell", "aleph_panel_bg.wasm"),
    panelManifest: path.join(repoRoot, "interfaces", "webchat", ".panel-live", "build.json"),
    tauri: path.join(repoRoot, "desktop", "shell", "tauri.qa.conf.json"),
  };
}

function validateManifestShape(manifest) {
  if (manifest === null || typeof manifest !== "object" || Array.isArray(manifest)) return false;
  const keys = ["contract", "revision", "artifact_sha256", ...DIGEST_NAMES];
  if (Object.keys(manifest).length !== keys.length || Object.keys(manifest).some((key) => !keys.includes(key))) return false;
  if (manifest.contract !== PROVENANCE_CONTRACT || typeof manifest.revision !== "string" || !REVISION_RE.test(manifest.revision)) return false;
  if (manifest.artifact_sha256 === null || typeof manifest.artifact_sha256 !== "object" || Array.isArray(manifest.artifact_sha256)) return false;
  if (Object.keys(manifest.artifact_sha256).length !== ARTIFACT_NAMES.length || Object.keys(manifest.artifact_sha256).some((key) => !ARTIFACT_NAMES.includes(key))) return false;
  if (ARTIFACT_NAMES.some((key) => typeof manifest.artifact_sha256[key] !== "string" || !DIGEST_RE.test(manifest.artifact_sha256[key]))) return false;
  return DIGEST_NAMES.every((key) => typeof manifest[key] === "string" && DIGEST_RE.test(manifest[key]));
}

export function createNativeProvenance({ files, revision }) {
  if (typeof revision !== "string" || !REVISION_RE.test(revision)) throw new Error("provenance_revision_invalid");
  return {
    contract: PROVENANCE_CONTRACT,
    revision,
    artifact_sha256: Object.fromEntries(ARTIFACT_NAMES.map((name) => [name, digestFile(files[name])])),
    wasm_sha256: digestFile(files.wasm),
    panel_manifest_sha256: digestFile(files.panelManifest),
    tauri_config_sha256: digestFile(files.tauri),
  };
}

// Preparation is a separate command. Execution never calls this writer.
export function prepareNativeProvenance({ repoRoot, outputPath, manifest, files, revision } = {}) {
  const value = manifest ?? createNativeProvenance({ files: files ?? nativeArtifactPaths(repoRoot), revision: revision ?? currentRevision(repoRoot) });
  if (typeof outputPath !== "string" || !validateManifestShape(value)) throw new Error("provenance_manifest_invalid");
  writeFileSync(outputPath, `${JSON.stringify(value, null, 2)}\n`, { flag: "wx" });
  return outputPath;
}

export function readNativeProvenance(file) {
  if (!existsSync(file)) throw new Error("provenance_manifest_missing");
  let manifest;
  try { manifest = JSON.parse(readFileSync(file, "utf8")); }
  catch { throw new Error("provenance_manifest_invalid"); }
  if (!validateManifestShape(manifest)) throw new Error("provenance_manifest_invalid");
  return manifest;
}

export function validateNativeProvenance({ manifest, files, revision }) {
  if (!validateManifestShape(manifest)) return { ok: false, reason: "provenance_manifest_invalid" };
  if (manifest.revision !== revision) return { ok: false, reason: "revision_mismatch" };
  try {
    for (const name of ARTIFACT_NAMES) if (digestFile(files[name]) !== manifest.artifact_sha256[name]) return { ok: false, reason: `artifact_hash_mismatch_${name}` };
    if (digestFile(files.wasm) !== manifest.wasm_sha256) return { ok: false, reason: "wasm_hash_mismatch" };
    if (digestFile(files.panelManifest) !== manifest.panel_manifest_sha256) return { ok: false, reason: "panel_manifest_hash_mismatch" };
    if (digestFile(files.tauri) !== manifest.tauri_config_sha256) return { ok: false, reason: "tauri_config_hash_mismatch" };
  } catch { return { ok: false, reason: "provenance_artifact_missing" }; }
  return { ok: true };
}

export function currentRevision(repoRoot) {
  try {
    return execFileSync("git", ["-C", repoRoot, "rev-parse", "HEAD"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch { throw new Error("provenance_revision_unavailable"); }
}

export function inspectNativeProvenance(repoRoot) {
  try {
    const manifest = readNativeProvenance(path.join(repoRoot, "interfaces", "webchat", ".panel-live", "native-provenance.json"));
    const checked = validateNativeProvenance({ manifest, files: nativeArtifactPaths(repoRoot), revision: currentRevision(repoRoot) });
    return checked.ok ? { ok: true, manifest } : checked;
  } catch (error) {
    return { ok: false, reason: ["provenance_manifest_missing", "provenance_manifest_invalid", "provenance_revision_unavailable"].includes(error.message) ? error.message : "provenance_manifest_invalid" };
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const valid = args.length === 4 && args[0] === "--repo-root" && args[2] === "--output" && !args[1].startsWith("--") && !args[3].startsWith("--");
  try {
    if (!valid) throw new Error("invalid_args");
    prepareNativeProvenance({ repoRoot: path.resolve(args[1]), outputPath: path.resolve(args[3]) });
    process.stdout.write(`${JSON.stringify({ contract: PROVENANCE_CONTRACT, status: "prepared" })}\n`);
  } catch (error) {
    const reason = ["invalid_args", "provenance_artifact_missing", "provenance_revision_unavailable"].includes(error.message) ? error.message : error.code === "EEXIST" ? "provenance_manifest_exists" : "provenance_manifest_invalid";
    process.stdout.write(`${JSON.stringify({ contract: PROVENANCE_CONTRACT, status: "NOT_RUN", reason })}\n`);
    process.exitCode = 4;
  }
}
