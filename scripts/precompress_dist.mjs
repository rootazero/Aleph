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
import { pathToFileURL } from 'node:url';

// Below one TCP initial window there is nothing to win, and a `.br` larger than
// its source is pure loss. The criterion is size plus measured benefit — NOT an
// extension allow-list, which would silently miss the next asset type.
export const MIN_BYTES = 4096;

// scripts/check_panel_dist.mjs imports MIN_BYTES above as the single source
// of truth for the threshold (never retype it). A plain ESM import runs
// 100% of a module's top-level code, so without this guard, importing this
// file for one constant would silently re-run the whole compression pass —
// every dist file, brotli quality 11, tens of seconds — as a side effect of
// what is supposed to be a read-only check. Worse, that side effect heals a
// corrupt or missing sibling before the guard's own assertions ever run,
// which makes them structurally unfalsifiable. Gate the executable body
// behind an entry-point check so importing for MIN_BYTES is inert, while
// `node scripts/precompress_dist.mjs` (direct invocation, including via
// `just wasm`) still runs it. `pathToFileURL(...).href` (not a raw string
// compare against process.argv[1]) is required for this to hold on Windows,
// where argv[1] uses backslashes and import.meta.url does not.
const isMain = import.meta.url === pathToFileURL(process.argv[1]).href;

if (isMain) {
  const dir = process.argv[2] || 'interfaces/webchat/dist';

  const failures = [];
  let written = 0;
  let skipped = 0;

  // A read failure (missing dir), a mid-loop fs error (deleted/permission-denied
  // file), or a compressor error would otherwise surface as a raw Node stack
  // trace that names no file. Every fs/zlib call below is caught and turned
  // into a named `name: message` entry instead (Node's own error messages
  // already carry the syscall and path, e.g. "ENOENT: ... open 'X'").
  let entries;
  try {
    entries = readdirSync(dir).sort();
  } catch (err) {
    console.error(`✗ ${dir}: could not read directory — ${err.message}`);
    process.exit(1);
  }

  for (const name of entries) {
    if (name.endsWith('.br')) continue;
    const path = `${dir}/${name}`;
    const brPath = `${path}.br`;

    try {
      const st = statSync(path);
      if (!st.isFile()) continue;

      if (st.size < MIN_BYTES) {
        // Remove a stale sibling if the file shrank below the threshold, so the
        // bidirectional guard in check_panel_dist.mjs cannot trip on a leftover.
        try { unlinkSync(brPath); } catch { /* nothing to remove */ }
        console.log(`  ${name}: below ${MIN_BYTES}-byte threshold (${st.size} < ${MIN_BYTES}), skipped`);
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

      // Incompressible source: emit nothing and remove any stale sibling. A
      // `.br` larger than its source is strictly worse on every axis — more
      // bytes on the wire, plus decode work at the other end.
      //
      // NOTE — this branch and `check_panel_dist.mjs`'s direction 2 disagree by
      // construction, and the guard's failure message points here. The guard
      // requires a sibling for every source over MIN_BYTES; this skips one when
      // brotli does not win. Today nothing in dist/ hits it: all four assets are
      // text or wasm and all four compress by 65-88%. The first already-
      // compressed asset over 4 KiB — a .png, a .woff2, a .zip — trips the guard
      // instead of shipping silently unguarded, which is the safe direction, but
      // re-running this script will NOT clear it. Extend both together: teach
      // the guard to accept a missing sibling when re-compressing the source
      // reproduces this same "not smaller" verdict. Do not simply exempt an
      // extension list — that stops being true the day someone commits an
      // uncompressed .png.
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
        // Same invariant as the two skip branches above: never leave a `.br`
        // on disk that doesn't match its source. A stale sibling here would be
        // read by rust_embed (debug builds) and served as the real asset.
        try { unlinkSync(brPath); } catch { /* nothing to remove */ }
        failures.push(`${name}: brotli round-trip did not reproduce the source`);
        continue;
      }

      writeFileSync(brPath, compressed);
      const pct = ((1 - compressed.length / source.length) * 100).toFixed(1);
      console.log(`  ${name}: ${source.length} -> ${compressed.length} (-${pct}%)`);
      written++;
    } catch (err) {
      failures.push(`${name}: ${err.message}`);
    }
  }

  if (failures.length) {
    console.error(failures.map((f) => `✗ ${f}`).join('\n'));
    process.exit(1);
  }
  console.log(`✓ precompressed ${written} file(s), skipped ${skipped}`);
}
