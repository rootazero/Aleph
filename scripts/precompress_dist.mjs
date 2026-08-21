#!/usr/bin/env node
// Produce brotli siblings for the committed panel dist.
//
// Why build-time and not runtime: the gateway currently gzips on every ETag
// miss, which means zstd-decompressing ~22 MB out of the binary and then
// gzip-compressing it again to send ~5 MB. Precompressing moves that work to
// `just wasm` (paid once) and drops the wire payload by roughly a third.
//
// Why the outputs are COMMITTED: interfaces/webchat/dist/ is a git-tracked
// build output — the release workflow embeds it verbatim and no job owns a
// WASM toolchain (see the header of scripts/check_panel_dist.mjs). A `.br`
// that is out of sync with its source would serve WRONG BYTES, which is worse
// than the problem this fixes, so check_panel_dist.mjs pairs them in both
// directions.
//
// Usage: node scripts/precompress_dist.mjs [dist-dir]
import { readdirSync, readFileSync, writeFileSync, statSync, unlinkSync } from 'node:fs';
import { brotliCompressSync, brotliDecompressSync, constants } from 'node:zlib';

const dir = process.argv[2] || 'interfaces/webchat/dist';

// Below one TCP initial window there is nothing to win, and a `.br` larger than
// its source is pure loss. The criterion is size plus measured benefit — NOT an
// extension allow-list, which would silently miss the next asset type.
export const MIN_BYTES = 4096;

const failures = [];
let written = 0;
let skipped = 0;

for (const name of readdirSync(dir).sort()) {
  if (name.endsWith('.br')) continue;
  const path = `${dir}/${name}`;
  const st = statSync(path);
  if (!st.isFile()) continue;

  const brPath = `${path}.br`;

  if (st.size < MIN_BYTES) {
    // Remove a stale sibling if the file shrank below the threshold, so the
    // bidirectional guard in check_panel_dist.mjs cannot trip on a leftover.
    try { unlinkSync(brPath); } catch { /* nothing to remove */ }
    skipped++;
    continue;
  }

  const source = readFileSync(path);
  const compressed = brotliCompressSync(source, {
    params: {
      [constants.BROTLI_PARAM_QUALITY]: 11,
      // 24 is the maximum window for STANDARD brotli. Anything above it is the
      // Large-Window extension, which `Content-Encoding: br` does not cover.
      [constants.BROTLI_PARAM_LGWIN]: 24,
      [constants.BROTLI_PARAM_SIZE_HINT]: source.length,
    },
  });

  if (compressed.length >= source.length) {
    try { unlinkSync(brPath); } catch { /* nothing to remove */ }
    console.log(`  ${name}: brotli is not smaller (${compressed.length} >= ${source.length}), skipped`);
    skipped++;
    continue;
  }

  // Round-trip immediately. A corrupt sibling is the one failure mode that
  // reaches users as wrong bytes rather than as an error.
  const back = brotliDecompressSync(compressed);
  if (!back.equals(source)) {
    failures.push(`${name}: brotli round-trip did not reproduce the source`);
    continue;
  }

  writeFileSync(brPath, compressed);
  const pct = ((1 - compressed.length / source.length) * 100).toFixed(1);
  console.log(`  ${name}: ${source.length} -> ${compressed.length} (-${pct}%)`);
  written++;
}

if (failures.length) {
  console.error(failures.map((f) => `✗ ${f}`).join('\n'));
  process.exit(1);
}
console.log(`✓ precompressed ${written} file(s), skipped ${skipped}`);
