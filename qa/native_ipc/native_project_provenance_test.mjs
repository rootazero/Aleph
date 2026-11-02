import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

import { createNativeProvenance, prepareNativeProvenance, readNativeProvenance, validateNativeProvenance } from "./native_project_provenance.mjs";

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), "native-provenance-test-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const files = Object.fromEntries(["shell", "helper", "reaper", "wasm", "panelManifest", "tauri"].map((name) => [name, join(root, name)]));
  for (const [name, file] of Object.entries(files)) writeFileSync(file, `${name}-bytes`);
  return { root, files };
}

test("preparation records independent digests, accepts unchanged bytes, and cannot overwrite its snapshot", (t) => {
  const { root, files } = fixture(t);
  const manifest = createNativeProvenance({ files, revision: "a".repeat(40) });
  const file = join(root, "native-provenance.json");
  prepareNativeProvenance({ outputPath: file, manifest });
  assert.deepEqual(readNativeProvenance(file), manifest);
  assert.deepEqual(validateNativeProvenance({ manifest, files, revision: "a".repeat(40) }), { ok: true });
  assert.throws(() => prepareNativeProvenance({ outputPath: file, manifest }), /EEXIST/);
});

for (const [name, reason] of Object.entries({ shell: "artifact_hash_mismatch_shell", helper: "artifact_hash_mismatch_helper", reaper: "artifact_hash_mismatch_reaper", wasm: "wasm_hash_mismatch", panelManifest: "panel_manifest_hash_mismatch", tauri: "tauri_config_hash_mismatch" })) {
  test(`mutation of ${name} after preparation is rejected without rewriting the manifest`, (t) => {
    const { root, files } = fixture(t);
    const manifest = createNativeProvenance({ files, revision: "b".repeat(40) });
    const file = join(root, "native-provenance.json");
    prepareNativeProvenance({ outputPath: file, manifest });
    const before = readFileSync(file, "utf8");
    writeFileSync(files[name], "replaced-bytes");
    assert.deepEqual(validateNativeProvenance({ manifest: readNativeProvenance(file), files, revision: "b".repeat(40) }), { ok: false, reason });
    assert.equal(readFileSync(file, "utf8"), before);
  });
}

test("missing, malformed, stale, or secret-bearing manifests fail closed", (t) => {
  const { root, files } = fixture(t);
  const file = join(root, "native-provenance.json");
  assert.throws(() => readNativeProvenance(file), /provenance_manifest_missing/);
  writeFileSync(file, "{");
  assert.throws(() => readNativeProvenance(file), /provenance_manifest_invalid/);
  const manifest = createNativeProvenance({ files, revision: "a".repeat(40) });
  writeFileSync(file, JSON.stringify({ ...manifest, token: "must-not-be-accepted" }));
  assert.throws(() => readNativeProvenance(file), /provenance_manifest_invalid/);
  assert.equal(validateNativeProvenance({ manifest: null, files, revision: "c".repeat(40) }).ok, false);
  assert.deepEqual(validateNativeProvenance({ manifest, files, revision: "c".repeat(40) }), { ok: false, reason: "revision_mismatch" });
});

test("standalone preparation CLI runs on Windows paths and returns bounded NOT_RUN for missing artifacts", (t) => {
  const { root } = fixture(t);
  const result = spawnSync(process.execPath, [join(import.meta.dirname, "native_project_provenance.mjs"), "--repo-root", root, "--output", join(root, "manifest.json")], { encoding: "utf8" });
  assert.equal(result.status, 4);
  assert.equal(JSON.parse(result.stdout).status, "NOT_RUN");
  assert.equal(result.stdout.includes(root), false);
});
