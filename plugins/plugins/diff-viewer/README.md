# Diff Viewer Plugin

WASM plugin for Aleph that provides code diff comparison and change analysis.

## Tools

- **diff_text** — Compare two text strings, outputting unified diff, inline diff, or statistics.
- **diff_files** — Compare two files by path (requires host workspace capability; currently returns an error directing callers to use `diff_text`).
- **diff_summary** — Compute change statistics (lines added, removed, unchanged, change ratio).

## Build

```bash
cargo build --target wasm32-wasip1 --release
```
