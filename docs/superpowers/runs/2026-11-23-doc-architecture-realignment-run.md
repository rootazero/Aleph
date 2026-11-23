# 文档架构对齐运行回执（2026-11-23）

> **状态：历史文档（point-in-time）** — 本回执记录 2026-11-23 这一轮文档校对的执行结果与验收证据。
> 当前状态以代码为准；文档分层契约见 [`docs/reference/EVIDENCE_LAYER.md`](../../reference/EVIDENCE_LAYER.md)。

## 目标

Aleph 在 Tauri → NSH 桌面迁移后，文档出现大面积滞后：入口层仍把桌面宿主写成 Tauri 独占，参考层 46 份文档里的
路径锚点指向重构前的树。本轮把文档重新校到**当前代码**上，并留下一条防漂移的守卫。

计划：[`docs/superpowers/plans/2026-11-23-doc-architecture-realignment.md`](../plans/2026-11-23-doc-architecture-realignment.md)

## 代码事实基线（核对来源）

| 事实 | 代码位置 |
|---|---|
| 发版中的桌面宿主仍是 Tauri | `.github/workflows/aleph-app-release.yml:148`（`cd desktop/shell && cargo tauri build`）、`justfile` 的 `shell-build` / `shell-build-lite` |
| NSH 宿主存在但未进发版流水线 | `desktop/nsh-shell/`（`aleph-nsh-shell`，nsh 0.6.0）；`justfile` 无 nsh 目标；CI 只 check/clippy `aleph-desktop-shell`（`.github/workflows/aleph-core-ci.yml:586-599`） |
| Panel 只有一份 | `interfaces/webchat`（`aleph-panel`，Leptos 0.8 CSR/WASM），由 `src/gateway/control_plane/assets.rs` 以 `rust_embed` 内嵌进 daemon |
| 宿主 ↔ Panel 已类型化 | `shared/protocol/src/panel_shell.rs`；宿主侧 `desktop/nsh-shell/src/panel_bridge.rs`；Panel 侧 `interfaces/webchat/src/app/shell_bridge.rs` |
| 残留 Tauri 内部接口 | `interfaces/webchat/src/api/native_credentials.rs`、`interfaces/webchat/src/platform/wide/views/voice/audio.rs` |
| 本地 Panel 端点 | `desktop/host/src/local_server.rs:11`（`DAEMON_PORT=18790`）、`shared/client/src/connection_target.rs:5`（`DEFAULT_PORT=18790`） |
| 迁移门禁 | Phase 11 P11-A…P11-H：三平台证据 + ≥14 天双轨运行；计划 `docs/superpowers/plans/2026-11-21-phase11-nsh-cutover.md` |

## 本轮改动

| 类别 | 内容 |
|---|---|
| L1 入口层 | `AGENTS.md` / `CLAUDE.md` 新增「桌面宿主：双轨现状（2026-11）」章节并同步；`README.md` / `README_CN.md` 改架构图与说明；`CONTRIBUTING.md` R2 条目改写 |
| L2 参考层 | 46 份文档路径重锚（372 处补 `src/` 前缀 + 636 处唯一后缀解析）；`GATEWAY.md` 修回被写坏的 MCP 通知名 `notifications/tools/list_changed`；`ARCHITECTURE.md` / `PRODUCT_TOPOLOGY.md` / `SERVER_DEVELOPMENT.md` / `REDLINES.md` 改双轨表述；7 份时点文档加历史横幅 |
| L3 证据层 | 新增 `docs/README.md`（文档地图）与 `docs/reference/EVIDENCE_LAYER.md`（三层契约）；入口层「文档纪律」条目指向二者 |
| 守卫 | `scripts/doc_wiring_audit.py`（11 项检查 + baseline 棘轮）、`scripts/doc_wiring_audit_baseline.json`、`justfile` 的 `check-doc-wiring`（已并入 `just check-wiring`） |

## 关键发现

1. **滞后集中在入口层，不在参考层**。`docs/reference/` 早已有 NSH 表述（`DESKTOP_SHELL.md`、`DESKTOP_BRIDGE.md`、
   `PRODUCT_TOPOLOGY.md`、`LINUX_DESKTOP.md` 各自带 2026-11 的 NSH 横幅）；真正把"桌面壳 Tauri"当现状写死的只有
   `AGENTS.md` / `CLAUDE.md` / 两份 README / `CONTRIBUTING.md` / `skills/architecture/SKILL.md`。而入口层恰恰是
   每个 agent 的第一份读物——**错的入口比错的附录危害大**。
2. **路径锚点是第二类腐化**。首轮守卫只检查"文档点名的路径是否存在"，漏掉了两种形状：
   文档漏写 `src/` 前缀（372 处）、文件扩展名后粘多余片段（`notifications/src/tools/registry.rs_changed`，
   实际应为 JSON-RPC 通知名 `notifications/tools/list_changed`）。加入 `ROOTS` 与 `MANGLED` 两项检查后暴露。
3. **写坏的路径藏在"路径不存在"里**。原守卫遇到"首段目录在仓库里不存在"的 token 会直接跳过，于是拼错的路径
   反而不可见——检测盲区本身就是一类 bug。

## 验收

```
$ python3 scripts/doc_wiring_audit.py
doc wiring audit: 0 failure(s), 9 advisory, 34 known stale path(s) on the backlog (77 canonical documents)
```

- 9 条 advisory：6 条 CRATES（形似 crate 名的普通词，如 `aleph-official`）、2 条 PORT（`BROWSER_LIVE_VIEWER.md` 里的
  历史端口 58682）、1 条 ROOTS。
- 34 条 backlog 全部位于时点文档，已登记在 `scripts/doc_wiring_audit_baseline.json`。
- `AGENTS.md` ≡ `CLAUDE.md`（守卫 `SYNC` 通过）。
- 5 个临时 worktree（`/tmp/aleph-doc-{A..E}`）与分支 `docs/anchor-sweep-{A..E}` 已删除。

## 未做

- 未跑 `cargo build/test`：本轮只改文档与 Python 守卫脚本，无生产代码改动。
- `skills/architecture/SKILL.md` 的改动**在 submodule（Aleph-skills）内单独提交**（`6c90f1b`），尚未 push，主仓指针未更新——
  更新主仓指针需要先 push submodule，否则 CI 检出的 pinned submodule 会指向不存在的提交。
- 证据层 2400+ 份文件**没有逐份加横幅**（原因与替代方案见计划 §7.2）。
- `docs/reference/` 里 34 处历史路径锚点未逐条修复，登记为 backlog。
