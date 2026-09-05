# Browser live view — spike evidence (throwaway)

These are throwaway spike artifacts from 2026-09-05, kept only so the obscura re-evaluation in the spec's §8 can be re-run against a newer obscura; they are not tests and never enter `qa/`.
Obscura probes: `obscura serve --port 9333 --allow-private-network` (omit the flag for `probe.mjs block`), serve `probe.html` on `http://127.0.0.1:18999`, then `node probe.mjs {block|full} 9333`, `node probe2.mjs 9333`, `node probe3.mjs 9333`.
Chrome probe: launch Chrome with `--headless=new --remote-debugging-port=0 --user-data-dir=<dir>`, read `<dir>/DevToolsActivePort` into `ws://127.0.0.1:<port><path>`, then `node probe-chrome.mjs <wsUrl> http://127.0.0.1:18999`.
Readings and verbatim errors are in `obscura-spike-findings.md` and `chrome-spike-findings.md`; the spec is `../2026-09-05-browser-live-view-design.md`.
