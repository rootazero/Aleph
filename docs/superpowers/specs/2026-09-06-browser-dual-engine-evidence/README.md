# Browser dual-engine — evidence (2026-09-06)

Throwaway scan reports and the obscura v0.2.2 re-measurement behind `../2026-09-06-browser-dual-engine-design.md`. Not tests; never enter `qa/`.

- Binary measured: obscura **v0.2.2** release `obscura-aarch64-macos.tar.gz` (sha256 `607471654d0c23799abd3bf45d1f4afd314a11fdbe1ee376e29018f32a2dfab9`, verified against the GitHub API asset `digest`); Chrome 152.0.7977.76 `--headless=new` for the side-by-side rows.
- Re-run: `obscura serve --port 9444 --allow-private-network`; `python3 -m http.server 18999` in `probes/` for `probe.html`; then `node probes/<m*.mjs> ws://127.0.0.1:9444/devtools/browser <url> …` (each script prints its own usage). `cdp.mjs` is the shared minimal CDP client (Node ≥ 22, native WebSocket).
- Network on the measuring machine goes through a fake-ip TUN proxy; navigation timings are network-dominated.
- Raw JSON/PNG outputs (69 files) were deliberately not committed; every number quoted in the spec is in `obscura-spike-v022.md` with its command.
- Source anchors in `aleph-browser-map.md` are at Aleph `c049ef1ed`; in `obscura-source-survey.md` at obscura `72c84ad` (3 version-bump commits behind the v0.2.2 tag).
