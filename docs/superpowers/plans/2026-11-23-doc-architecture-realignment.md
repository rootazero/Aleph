# Doc architecture realignment after the Tauri → NSH desktop migration
# 文档架构对齐（Tauri → NSH 桌面迁移后）

> **Scope / 范围：** re-verify the repository's documentation against the *actual* code, so that stale documents stop mis-guiding future work. Historical evidence is preserved and labelled, not rewritten.
> **Status / 状态：** Active. Baseline commit `f13417f39` (2026-11-23).
> **Why now / 起因：** the desktop host moved from a single Tauri shell to a two-track layout (legacy Tauri host + NSH host), and the Panel↔host IPC moved from `__TAURI_INTERNALS__` to a typed contract. The entry documents still describe the pre-migration world.

---

## 1. The problem / 问题

`AGENTS.md:53` and `CLAUDE.md:53` still declare the stack as "Panel Leptos/WASM · 桌面壳 Tauri".
`README.md:100` draws the desktop box as "Native Desktop App (Tauri)". `docs/reference/PRODUCT_TOPOLOGY.md`
and `docs/reference/DESKTOP_SHELL.md` describe the desktop host as a Tauri v2 application.

Two opposite errors are equally misleading:

| Wrong claim | Why it misleads |
|---|---|
| "The desktop shell **is** Tauri" | Points new work at `desktop/shell/`, the host scheduled for retirement after Phase 11 gate P11-G. |
| "Tauri **is gone**, NSH replaced it" | False today: the release workflow and every `just shell-*` target still build `desktop/shell/`. An agent that believes NSH ships would break the release pipeline. |

The truth is a **two-track desktop host**:

```
                  ┌──────────────────── one Panel, shared ────────────────────┐
                  │  interfaces/webchat  (aleph_panel, Leptos 0.8 CSR + WASM) │
                  │  embedded into aleph-server via rust_embed, served :18790 │
                  └───────────────────────────────────────────────────────────┘
                            ▲                                        ▲
              webview host  │                                        │  webview host
                            │                                        │
        desktop/shell (aleph-desktop-shell)          desktop/nsh-shell (aleph-nsh-shell)
        Tauri v2 — LEGACY, still the shipped build    NSH 0.6.0 — forward path, Phase 11
```

---

## 2. Code fact baseline / 代码事实基线

Every row below was read out of the code at `f13417f39`. This table is the *source of truth* the
document corrections are written against.

| Fact | Where the code says so |
|---|---|
| Workspace package list (18 members) | root `Cargo.toml` `[workspace] members` |
| Core crate is the root package `alephcore` (`src/`) | root `Cargo.toml` `[package]` |
| Binaries: `aleph-server`, `aleph-migrate`, `aleph-panel-native-qa-gateway` | `src/bin/` |
| Panel = `interfaces/webchat` → `aleph-panel` (Leptos 0.8 `csr`, `wasm-bindgen`, cdylib) | `interfaces/webchat/Cargo.toml` |
| Panel assets embedded into the daemon by `rust_embed` | `src/gateway/control_plane/assets.rs` |
| Local Panel / Gateway endpoint `http://127.0.0.1:18790` | `desktop/host/src/local_server.rs:11` (`DAEMON_PORT`), `shared/client/src/connection_target.rs:5` (`DEFAULT_PORT`) |
| Legacy Tauri host = `desktop/shell` → `aleph-desktop-shell` (Tauri v2) | `desktop/shell/Cargo.toml`, `desktop/shell/tauri*.conf.json` |
| NSH host = `desktop/nsh-shell` → `aleph-nsh-shell` (nsh-core/engine 0.6.0) | `desktop/nsh-shell/Cargo.toml` |
| The **shipped** desktop app is still the Tauri host | `.github/workflows/aleph-app-release.yml:148` (`cd desktop/shell && cargo tauri build`), `justfile` `shell-build` / `shell-build-lite` / `shell-dev` |
| There is **no** NSH release or `just` target yet | `justfile` has no `nsh` recipe; `.github/workflows/` has no NSH build |
| NSH host scope today: window + webview, tray, global shortcut, notification permission, read-only autostart status, typed Panel bridge, daemon supervisor | `desktop/nsh-shell/src/{main,shell_host,platform,autostart,panel_bridge,supervisor}.rs` |
| NSH host does **not** yet implement the Tauri host's updater, `aleph://` deeplink, macOS menu or service control | `desktop/nsh-shell/src/` has no `update.rs` / `deeplink.rs` / `menu.rs` (contrast `desktop/shell/src/`) |
| Panel↔host IPC is a typed contract, not `__TAURI_INTERNALS__` | `shared/protocol/src/panel_shell.rs` (`ShellRequest`, `ShellReply`, `PanelReply`, `CredentialCall`, `AutostartStatus`, `valid_document`, `valid_notification`) |
| Bridge implementations | host side `desktop/nsh-shell/src/panel_bridge.rs` (+ `panel_bridge.js`); Panel side `interfaces/webchat/src/app/shell_bridge.rs` |
| Two unmigrated legacy consumers remain | `interfaces/webchat/src/api/native_credentials.rs`, `interfaces/webchat/src/platform/wide/views/voice/audio.rs` |
| Phase 11 gates P11-A…P11-H: inventory, three platforms, contracts/security, runtime, default/recovery, **≥14 calendar days dual run**, removal, handoff | `docs/superpowers/plans/2026-11-21-phase11-nsh-cutover.md`, `docs/superpowers/runs/2026-11-2*-phase11-*.json` |
| Linux Phase 11 evidence is scoped only (production Panel run NOT RUN) | `docs/superpowers/runs/2026-11-23-phase11-linux-native-runner-receipt.json` |

---

## 3. Layered strategy / 分层策略

The repository holds ~3400 markdown files. Rewriting history is both expensive and destructive to the
audit trail, so the work is split into three layers with different rules.

| Layer | Files | Rule |
|---|---|---|
| **L1 Entry** | `AGENTS.md`, `CLAUDE.md`, `README.md`, `README_CN.md`, `CONTRIBUTING.md`, `skills/architecture/SKILL.md` | Corrected to the current architecture. These are read on every session. |
| **L2 Reference** | `docs/reference/*.md` (74) | Corrected where they describe current state; historical design notes get an explicit status banner instead of a rewrite. `docs/reference/FEATURE_LOCATOR.md` keeps its existing "current-state section first" convention. |
| **L3 Evidence** | `docs/superpowers/` (3338), `docs/reports/` (80), `docs/engineering-reports/` (124), `docs/plans/` (43), `docs/research/` (38), `docs/audits/` (11), `docs/archive/` (3) | **Content preserved.** A directory-level status banner states that these are point-in-time evidence, never current architecture. |

Supporting rule written into L1: *evidence layer ≠ current state; when a historical document and the
code disagree, the code wins and the historical document is only an evidence trail.*

---

## 4. Roadmap / 路线图

| Phase | Work | Output | Gate |
|---|---|---|---|
| **P0** | Code fact baseline | §2 table above | every row cites a code location |
| **P1** | L1 entry corrections | 6 files corrected | `AGENTS.md` ≡ `CLAUDE.md` (except the documented deltas) |
| **P2** | L2 reference corrections | 74 docs triaged: corrected / banner-marked | no canonical doc asserts a single-track desktop host |
| **P3** | L3 evidence banners | 7 directory banners + L1 rule | no historical document is reachable as "current" |
| **P4** | Drift guard | `scripts/doc_wiring_audit.py` + `just check-wiring` | guard passes on the corrected tree |
| **P5** | Acceptance | guard run, link check, diff check, staged commits | — |

**Ordering rationale.** The guard (P4) is written *before* the L2 sweep (P2), so the sweep is driven by
a detector instead of by reading 74 documents hoping to spot the error. The guard is a **detector, not
a judge** — it reports what it can prove, and it never rewrites a document.

---

## 5. Guard design / 守卫设计

`scripts/doc_wiring_audit.py` follows the existing `scripts/*_wire_audit.py` convention.

| Check | Kind | What it proves |
|---|---|---|
| `LINKS` | LIVE | every local markdown link in the canonical set resolves to a file that exists |
| `PATHS` | LIVE | every backticked repo path in the canonical set exists on disk |
| `CRATES` | LIVE | every backticked `aleph-*` name in the canonical set is a real package or binary |
| `JUST` | LIVE | every `just <target>` in the canonical set exists in the `justfile` |
| `PORT` | LIVE | the canonical set names the local endpoint as `127.0.0.1:18790` |
| `DUAL` | LIVE | the canonical set keeps the two-track desktop host visible (NSH *and* legacy Tauri) |
| `STALE` | CUT | banned single-track assertions stay out of the canonical set |
| `SYNC` | LIVE | `AGENTS.md` and `CLAUDE.md` remain identical except their documented deltas |

Advisory (reported, non-failing): reference documents that are reachable from no index.

---

## 6. Explicitly out of scope / 明确不做

- Rewriting, deleting or re-dating any L3 evidence document.
- Removing the Tauri host, its crates, its CI jobs or its release workflow — that is Phase 11's job
  behind gates P11-A…P11-H, not a documentation task.
- Claiming the NSH host is feature-complete; it currently lacks the updater, deeplink, macOS menu and
  service control that the Tauri host has.
- Editing `docs/research/NSH_ADR-*` or the Phase 11 plan/receipts.
- Any change to production code.
