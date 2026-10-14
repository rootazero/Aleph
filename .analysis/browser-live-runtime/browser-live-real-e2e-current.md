# Browser-live real Gateway/Chrome selector run (current evidence)

Date: 2026-10-13 (worktree local run)
Worktree: `D:/Workspace/Aleph-worktrees/panel-browser-workspace-20261007-202532-13256`

## Boundary

The Gateway already running on `127.0.0.1:18790` was used; it was not restarted. Each successful selector received a fresh Gateway-issued grant from `real-issue.js`; no viewer URL was synthesized and no target was inferred. Playwright used the installed real Chromium browser. Credentials/URLs are intentionally not reproduced here.

## Fresh selector evidence

| Selector | Setup | Result |
|---|---|---|
| `stream` | fresh resource/grant | 1 passed (2.4s) |
| `control-all-entries` | fresh resource/grant | 1 passed (1.6s) |
| `standalone-same-target` | fresh resource/grant | 1 passed (1.5s) |
| `binding-switch` | two fresh Gateway-issued resources/grants | 1 passed (2.5s) |
| `grant-boundary` | fresh resource/grant | 1 passed (3.4s) |

A deliberate reuse of the already-consumed one-shot grant for `control-all-entries` failed after 30s with no frame; this confirms the harness requires a fresh grant per selector. It was not counted as a product regression, and the selector passed after fresh issuance.

Previously recorded real runs (see `task-23-acceptance-report.md`) also passed `revoke-live` (1 passed, 1.3s) and `navigate-fence` (1 passed, 1.0s) with Gateway-side drivers; those cases were not repeated in this run.

## Remaining boundary

`managed-companion` remains blocked because there is no production managed companion producer/health fixture and `CURRENT_CAPABILITIES` remains false. `reap`, target destruction, TTL expiry, Native/Tauri IPC, and full workspace/browser-live coverage were not claimed by this run. Overall Task #23 remains **PARTIAL / not production ready**.
