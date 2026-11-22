# NSH Panel Parity Slice Plan / NSH Panel Parity 切片计划

**Status / 状态:** approved execution plan; Task5 human gate accepted in `m01055`
**Input / 输入:** [NSH 0.5 migration inventory](../runs/2026-11-15-nsh05-migration-inventory.md), [upstream evidence](../runs/2026-11-15-nsh05-upstream-evidence.md), [parity spec](../specs/2026-11-15-nsh-panel-parity-slice.md), [R2 decision proposal](../specs/2026-11-15-nsh-r2-native-ui-decision.md)

## Step 1 — freeze the v0.5 compatibility surface / 固定 v0.5 兼容面

- Use only release commit `f4d68f7f9f31eb7b3ecc8a4cbc31096d5c6238a1` and the relative paths in the upstream evidence manifest.
- Record exact CLI/crate/runtime/platform versions, generated `nsh/desktop` declarations, `Window`/`WindowRef` construction, AppView lifecycle, host channel, optional Tokio handle, media grants and shutdown hooks.
- Create `desktop/nsh-shell/tests/nsh_v05_window_compat.rs`; keep the current shell unchanged until the pinned symbols are mapped.

**Gate:** `window_kind_and_ref_follow_pinned_v05` and `close_and_exit_settle_owned_host_work` have a version-matched API table and no v0.4 placeholder.

## Step 2 — settle the R2/host authority before code / 在编码前确定 R2/Host 权威

- Record one approved choice from `docs/superpowers/specs/2026-11-15-nsh-r2-native-ui-decision.md`.
- For the recommended choice, NSH owns native windows, platform lifecycle, transport and OS/API I/O while Panel/Leptos/WASM remains business UI authority; native host composition is optimized without changing business facts.
- Define ownership for the client, event receiver, UI worker and v0.5 host channel. The detached `aleph-server --daemon start` remains externally managed and outside this slice.

**Gate:** startup, reconnect, close and error terminal states have named owners and observable settlement receipts.

## Step 3 — build the fixed Chat route / 建立固定 Chat 路由

- Create `desktop/nsh-shell/tests/nsh_panel_chat_slice.rs` around `PanelMode::Chat`, path `/`, and `WorkspacePanel` body `Artifacts`.
- Project `system.hello`, `events.subscribe`, `sessions.list` and `chat.send` through existing shared contracts. The fixed state/data path is: session list/history snapshot → canonical session/message state; `run.queued`/`stream.run_queued` with `run_id`, `session_key`, `ahead` → queued state; internal run topics → existing `stream.*` wire methods → shared client `StreamEvent`; `run.accepted` → active state; `agent.run.complete`/`agent.run.error` → terminal state; `interfaces/webchat/src/api/artifacts.rs:131` `artifacts.list` → populated Artifacts; `interfaces/webchat/src/components/artifacts/mod.rs:250-257` session-change refetch → stale marker followed by `artifacts.list`; `:271-279` handles the `session.artifact` invalidation ping. Record the session/run association and reconnect replacement-snapshot/dedup rule in the test receipt. A raw internal topic or fabricated shared event is not sufficient.
- Project the above into connection/session/messages/queued/streaming/active-run/refusal/shutdown state. Reconnect must re-handshake, replace the receiver, rebuild from canonical snapshots, and only then accept new deltas. Preserve queued→accepted→complete/error transitions; do not collapse `run.queued` into generic streaming.
- Reproduce sidebar Chat selection and the Artifacts empty/loading/populated state. Classify differences Preserve/NSH redesign/Deferred; keep the Tauri sources as comparison fixtures.
- Remove the mock path from the slice's product evidence. Local `SessionManager` UUIDs and fabricated assistant responses cannot satisfy the gate.

**Gate:** connected, reconnecting, streaming, refusal/error and owned shutdown states are driven by real inputs, with no duplicate local session.

## Step 4 — prove parity and performance / 证明 parity 与性能

- Create the proposed measurement harness `qa/nsh_panel_parity.py`, schema `qa/nsh_panel_parity.schema.json`, and protocol README `qa/nsh_panel_parity.md` in the same change set as the first performance evidence. The first supported profile is `debug-qa` because `desktop/shell/src/main.rs` intentionally rejects `qa-owned-root` in release builds and rejects `qa-owned-root` with the default `embedded-core` feature. The bounded launcher preparation first allocates shell-safe task variables, for example `PARITY_TARGET="$(mktemp -d)"`, then runs from the repository root: `python3 qa/browser_live/check_memory.py --required-bytes 4294967296 && CARGO_TARGET_DIR="$PARITY_TARGET" cargo build -p aleph-desktop-shell -p aleph-nsh-shell --no-default-features --features aleph-desktop-shell/qa-owned-root --offline --locked --jobs 1`; a nonzero memory-gate result forbids the build. This debug-qa comparison is a relative integration/performance gate, not a release-performance claim. A future release-performance gate requires a separately reviewed release-safe QA confinement design.
- The harness itself must prepare the existing Tauri owned-root protocol before launch: create one direct child of `tempfile.gettempdir()` named `aleph-shell-qa-` plus a fresh suffix, create it without following links, generate a 32-byte lowercase hexadecimal nonce, write exactly `nonce + "\n"` to `owned_root / ".qa-owned"`, and pass the absolute root and nonce as separate argv values. It must reject/recreate the root if the prefix, direct-child, canonical-path or nonce-file conditions are not satisfied. The NSH process receives no Tauri QA root because its first slice has no owned-root contract yet; its fixture and cleanup boundaries are recorded separately.
- The harness then launches the already-built binaries without Cargo using argv arrays and `shell=False`: Tauri executable `Path(PARITY_TARGET) / "debug" / "aleph-desktop-shell"` with argv array `["--qa-root", str(owned_root), "--qa-nonce", nonce]` and cwd `desktop/shell`; NSH executable `Path(PARITY_TARGET) / "debug" / "aleph-nsh-shell"` with cwd `desktop/nsh-shell`. The NSH binary is a proposed post-approval launcher; the current v0.4 shell is not v0.5 proof. The receipt records argv as an array, cwd and fixture identifier, so paths containing spaces cannot be split.
- As part of that future QA-only change set, modify `desktop/shell/src/main.rs` and `desktop/nsh-shell/src/main.rs` so each launcher emits `ALEPH_PANEL_APPVIEW_START` immediately before its Tauri `app.run`/NSH `run_app` or AppView launch. The harness starts the process but begins startup timing only at that application-level marker, then uses the first-frame marker for the end; Cargo launcher overhead is excluded. It records process ids for the shell and renderer/engine children, samples RSS/thread count, and writes one JSON object per scenario and implementation. The command argv, cwd and fixture identifier are recorded in the receipt.
- The receipt schema has `scenario`, `implementation`, `argv`, `cwd`, `fixture`, `profile`, `warmup_runs`, `measured_runs`, `timing_start`, `timing_end`, `samples`, `latencies_ms`, `median_ms`, `p95_ms`, `peak_rss_bytes`, `max_threads`, `processes`, `valid`, `invalid_reason`, `threshold_status`, `baseline`, `thresholds`, and `acceptance_reason`; `threshold_status` is `ACCEPTED` or `NOT_ACCEPTED` for a valid measurement, while `invalid_reason` is used only when `valid` is false. The baseline and thresholds record the paired Tauri values and +10%/+20%/+10%/+2 rules. p95 is sorted rank `ceil(0.95 * 30) = 29` one-based. The launcher records first-frame, route, input, reconnect and close markers using the timing definitions in the spec. It excludes the externally managed daemon from process measurements. Validate the generated receipt before comparison with `python3 qa/nsh_panel_parity.py --validate-only --schema qa/nsh_panel_parity.schema.json --input docs/superpowers/runs/2026-11-15-nsh-panel-parity-execution-receipt.json`.
- Exit contract: `0` means schema-valid paired measurements and thresholds accepted; `2` means an invalid pair or missing controlled receipt and must be rerun after reset; `3` means valid measurements with `threshold_status=NOT_ACCEPTED` and must be recorded as `NOT_ACCEPTED`; `4` means launcher/schema/protocol failure and is a planning/test failure. The README records the same codes and reset procedure.
- Run Tauri first and NSH second for each paired scenario. Before every scenario reset the canonical server/session fixture, clear selected-session/reconnect state, close prior windows, and record that no stale receiver or artifact cache was reused. Discard the first 10 warm-up runs; collect 30 measured runs per implementation. If a provider/server receipt differs, invalidate the pair, reset both implementations and rerun it.
- Apply the proposed slice rule: median ≤ Tauri +10%, p95 ≤ +20%, RSS ≤ +10%, threads ≤ +2. If a scenario is invalid or misses a threshold, record NOT_ACCEPTED with its receipt; do not convert it into a release speedup claim.

**Gate:** every visible difference has a reason and classification; all performance values have reproducible receipts.

## Step 5 — platform and removal gates / 平台与移除门槛

- macOS 14+ is the first execution target.
- Windows 10 1809+ and Linux GTK 4.14+/WebKitGTK 6.0 (2.40+) remain NOT_PROVEN until real target-platform evidence exists.
- Do not remove Tauri, WebChat or workspace members until Phase4 parity is complete and a two-week dual run is reviewed.
- Keep Swift media features, i18n and PluginRuntime separately tracked.

## Validation discipline / 验证纪律

Every Cargo command requires the 4 GiB memory check, explicit isolated `CARGO_TARGET_DIR`, `--offline --locked --jobs 1`, bounded owned processes and natural terminal receipts. Source exploration must first run Graphify. No global install, lockfile update, remote push or publication is part of this plan.

## Rejection boundaries / 拒绝边界

Reject the slice if it uses local fabricated chat data, claims v0.5 from the v0.4 checkout, silently overrides R2, claims detached daemon shutdown, leaves close-owned work detached, substitutes a second client/registry, or declares Phase4/Phase6 complete from a single route. This plan remains review-only until human approval.
