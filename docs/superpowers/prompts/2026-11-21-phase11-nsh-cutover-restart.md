# Phase11 restart prompt — start implementation on local main
# Phase11 重启提示词——从本地 main 开始实施

Copy the prompt below into the next coding session at this repository. / 将以下提示词交给下一会话。

---

请从**本地 `main`**继续 Aleph Phase11，按已提交的 [Phase11 计划](../plans/2026-11-21-phase11-nsh-cutover.md)开始实施，而不是再次只输出一份计划。

目标是：**三平台桌面替换为 NSH，保留共享 Leptos/WASM Panel，在至少两周真实双轨验证通过后，删除 Tauri 旧壳、运行时依赖和活动构建/发布路径。** 本目标不是原生 Chat UI 重写，也不是删除 `interfaces/webchat` 或 Swift/其他平台能力。

## 已确认决策 / Confirmed decisions

1. 用户明确要求 Phase11 做到切换 NSH 桌面和删除 Tauri。
2. 用户明确保留总纲的**至少两周双轨运行**门槛。未满 14 个日历日、没有真实活动记录时，不得删除旧壳或宣称 Phase11 全部完成。
3. 用户明确要求 **macOS、Windows、Linux 全部替代后才能全删 Tauri**，不允许悄悄停发 Windows/Linux。当前非 macOS 入口不是合格替代。
4. 延续 R2 choice A：现有 Panel 是唯一业务 UI。总纲历史 Phase6 删除 `interfaces/webchat` 的条款已被这一决定取代。原生 UI/Plugin Runtime 的旧阶段不能被标成完成。
5. 本会话任务是实施，不需为已经明确的常规本地工作重复征求许可。按计划闸门通过后执行已批准的默认切换；未获授权的发布、远程 push、新签名信任/密钥变更和破坏性凭据迁移，仍须准备具体可核验结果后按适用权限处理。

## 首先执行 / First actions

1. 用 `git rev-parse --show-toplevel`、`git branch --show-current`、`git status --short`、`git log -5 --oneline`核对当前检出；必须从本地 `main` 的已提交交付开始。若有用户新改动，保留并识别，不覆盖。下方“交付身份”记录本次 Phase10 提交；计划提交可能是它的后继。
2. 读项目 `AGENTS.md`，先查询现有 Graphify 图，再定位源码。读完整 [Phase11 计划](../plans/2026-11-21-phase11-nsh-cutover.md)、完整 [NSH 总纲](../specs/2026-11-05-nsh-desktop-refactor-design.md)、[Phase9 R2 决策](../runs/2026-11-16-phase9-nsh-r2-boundary.md)、[官方 Phase10 规格](../specs/2026-11-20-phase10-official-nsh.md)、[Phase10 收尾](../runs/2026-11-21-phase10-closeout.md)、[完整性回执](../runs/2026-11-21-phase10-closeout-integrity.json)、[门槛修订说明](../runs/2026-11-21-phase10-acceptance-amendment.md)和对应 JSON。
3. 按计划 Task1 读路由所需 reference（`SECURITY`、`TOOL_SYSTEM`、`DESKTOP_SHELL`、`DESKTOP_BRIDGE`、`PRODUCT_TOPOLOGY`、`WINDOWS_RUNTIME`、`LINUX_DESKTOP`、`RELEASE`、`DEVELOPMENT`），核对现有 API、凭据/服务/更新所有权和 CI。先形成具体迁移规格及逐调用点清单并完成 SPEC→QUALITY，再写新的边界代码。不要重复询问已确定的两周与三平台决策。
4. 创建并维护 Todos：契约/平台可行性 → Panel IPC与安全迁移 → 连接与服务 → 三平台 OS 集成 → updater/打包 → 回归/基线 → 默认切换/回退 → 双轨 → 删除 → 收尾。一次一个 in_progress，未通过的闸门保持未完成。
5. 优先用小范围官方 API 实验确认 Windows/Linux 可行性，再实施。平台能力/机器不足则记录具体缺口并继续可独立完成的本地任务，不把空入口、交叉编译或 macOS 回执当作跨平台实测。不要转回 0.5 fork 或静默改 toolkit 源码。

## 起点事实与不可混淆之处 / Starting facts

- Phase10 官方基线是已安装 `nsh 0.6.0 (5541637)`及 crates.io `=0.6.0`，不是旧 fork。
- Phase10 在批准的本机范围下完成。其原始矩阵 `.phase10-evidence/g7-official-matrix.Ji0duOcq/matrix.json` SHA-256 为 `fbf5f1272be769ae554ba53df065ef171badb6bbe942bb90e254168664e5f7c2`，仍是 `VALID / NOT_ACCEPTED`。
- 用户分别批准仅把 Quit/`close_ms` 的 median、p95 上限改为 `1.30×`；[独立修订评估](../runs/2026-11-21-phase10-acceptance-amendment.json)据原完整 40+40 数据接受修订合同。其他延迟门槛仍 `1.10×/1.20×`，RSS median `1.10×`，threads median delta `2`。这是接受取舍，不是性能改进或新跑分，不可全局放宽或篡改原回执。
- G6 物理多屏拓扑为用户暂缓、**NOT TESTED**；通知 feature-disable 开关未定义/实现，权限拒绝不等于零资源禁用。Phase11 更大范围应明确处置这些缺口。
- 已撤回“JS GC 导致主要耗时”“NSWindow.close 必定阻塞 200–400ms”等未测量归因，不得作为架构事实重用。
- 现有 NSH 生产入口只验收 macOS，默认本地 Panel；`HostSettings` 的非默认 endpoint+nonce 是 debug QA，并非生产远程连接支持。Tauri 仍拥有 full/lite、连接凭据、服务启动/监管、updater、autostart及部分原生操作，必须迁移后再删。
- 当前共享边界在 `shared/protocol/src/panel_shell.rs`、`interfaces/webchat/src/app/shell_bridge.rs`、`desktop/nsh-shell/src/panel_bridge.{rs,js}`；Panel `api/native_credentials.rs`已有 `BridgeIpc`、凭据 lease 与 attempt 防竞态，复用它们，不造假 `__TAURI_INTERNALS__` 兼容层或任意 invoke 隧道。
- `desktop/macos`、`desktop/windows`、`desktop/linux`、`desktop/shared`和共享 client/protocol 不因删除 Tauri 自动退役。先查真实依赖。

## 工作与验证纪律 / Execution discipline

- 不 reset、不 pull、不 rebase 到 `origin/dev`，不删除既有 worktree、历史证据或用户资产。本项目交付点为本地 `main`。
- 不读写 `.claude/settings.json`、根 `config.lock`、根 `config.toml`。不输出 token、凭据或原始拒绝 URL。
- Cargo 前运行 `python3 qa/browser_live/check_memory.py --required-bytes 4294967296`，只有 `ALLOW` 后串行执行 `--offline --locked --jobs 1`。定时 native QA 不与编译/资产构建竞争。
- 复用现有 source owner、Capability/tool契约、类型与 reducer，先设计防错，再 lint/type/自动检查，自行跑验证；人工核验只用于确实无法自动完成的项目。不得用审核“通过”替代实际效果。
- 保持原采样、预热、p95 排名、进程结算与自然退出边界；不挑样本、不对未变候选重跑赌通过。新平台性能合同必须在计时前冻结。
- 任一重大平台/安全缺口先提供源证据与具体选项，不静默改变信任或所有权。常规可逆修复继续做，不停在重复状态描述。
- 双轨期间记录三平台实际默认/回退、开始/结束身份和活动；共同合格期从最后一个平台开始计算。时间未到或硬件不可用时留真实未完成项和下一可执行动作，不伪造日期、不忙等十四天。
- 删除阶段必须先通过计划 P11-A–F。保留可运行的 Tauri 基线/回退包与历史解析器，最后移除 `desktop/shell`、活动 Tauri依赖/IPC/构建链，锁文件由 Cargo 正常生成。保留历史文档字符串不能被误报为运行时残留；零字符串也不是依赖闭合证明。
- 计划要求的最后结果是 P11-A–H 证据闭合，包含默认切换、回退演练、真实两周双轨、三平台替代及移除后回归。下一次提交用 conventional message，默认只提交本地，不 push/tag/release。

## 完整参考清单 / Complete reference manifest

以下清单覆盖本计划所依据的参考文档、实现、QA 和参考项目；不是让下一会话盲读所有日志。**先读 A/B，再按 Task 读取 C/D，E 中的项目先定位、核对版本再引用。** 新实施产生的 spec/平台回执另纳入 Task1 清单。仓库路径均相对实际 git root，不将本机绝对路径写入可移植交付。

### A. 必须完整阅读：决定与验收权威 / Required authority

| 文件 / File | 用途与限界 / Purpose and limits |
| --- | --- |
| [AGENTS.md](../../../AGENTS.md) / [CLAUDE.md](../../../CLAUDE.md) | 项目宪法双入口；按当前 agent 入口执行，若修改跨项目约束须同步两份 |
| [Phase11 plan](../plans/2026-11-21-phase11-nsh-cutover.md) | 本期目标、P11-A–H、三平台/14天门槛与有序任务 |
| [NSH master](../specs/2026-11-05-nsh-desktop-refactor-design.md) | 完整历史总纲，v0.4/原生UI路线须按新 R2 决策解释 |
| [NSH ADR-001–009](../../research/NSH_ADR-001~009.md) | 原架构、server、插件与信任决策；不把 proposed 插件ADR当作本期实施许可 |
| [ADR-001 Web amendment](../../research/NSH_ADR-001_Amendment_Web_Target.md) / [NSH protocol V1](../../research/NSH_PROTOCOL_V1.md) | 历史 Web/传输设计参考；当前 wire 类型和 Panel owner 才是实现事实源 |
| [Phase9 R2 boundary](../runs/2026-11-16-phase9-nsh-r2-boundary.md) / [Panel parity](../specs/2026-11-15-nsh-panel-parity-slice.md) | choice A、Panel唯一业务UI与共享状态契约 |
| [Phase10 official NSH spec](../specs/2026-11-20-phase10-official-nsh.md) / [Phase10 plan](../plans/2026-11-18-phase10-nsh-shell-integration.md) | 官方0.6契约和G1–G7边界 |
| [Phase10 closeout](../runs/2026-11-21-phase10-closeout.md) / [closeout integrity](../runs/2026-11-21-phase10-closeout-integrity.json) | 已完成范围、检查结果、未测和未交付项 |
| [Quit amendment](../runs/2026-11-21-phase10-acceptance-amendment.md) / [machine receipt](../runs/2026-11-21-phase10-acceptance-amendment.json) | 两项独立授权、仅Quit1.30、原失败与修订接受分开 |
| [Official native ledger](../runs/2026-11-20-phase10-official-nsh-checkpoint.md) / [candidate identity index](../runs/2026-11-20-phase10-official-candidate-integrity.json) | 每个原生闸门与本地证据哈希，保留反例/失败历史 |

### B. 仓库参考：按架构与任务路由 / Repository references

- 架构红线：[REDLINES](../../reference/REDLINES.md)、[ARCHITECTURE](../../reference/ARCHITECTURE.md)、[CODE_ORGANIZATION](../../reference/CODE_ORGANIZATION.md)、[DESIGN_PATTERNS](../../reference/DESIGN_PATTERNS.md)、[DOMAIN_MODELING](../../reference/DOMAIN_MODELING.md)、[HARNESS_PHILOSOPHY](../../reference/HARNESS_PHILOSOPHY.md)。本期不扩 Core harness。
- 权限/工具：[SECURITY](../../reference/SECURITY.md)、[TOOL_SYSTEM](../../reference/TOOL_SYSTEM.md)、[PLUGIN_SYSTEM](../../reference/PLUGIN_SYSTEM.md)。沿用当前 Capability/权限权威，不另造。
- 桌面/产品：[DESKTOP_SHELL](../../reference/DESKTOP_SHELL.md)、[DESKTOP_BRIDGE](../../reference/DESKTOP_BRIDGE.md)、[PRODUCT_TOPOLOGY](../../reference/PRODUCT_TOPOLOGY.md)、[WINDOWS_RUNTIME](../../reference/WINDOWS_RUNTIME.md)、[LINUX_DESKTOP](../../reference/LINUX_DESKTOP.md)。Swift Bridge 与 NSH宿主不是同一个系统。
- 定位/工作纪律：[FEATURE_LOCATOR](../../reference/FEATURE_LOCATOR.md)、[SUBSYSTEM_ROUTING](../../reference/SUBSYSTEM_ROUTING.md)、[WORKSPACE](../../reference/WORKSPACE.md)、[DEVELOPMENT](../../reference/DEVELOPMENT.md)、[STYLE](../../reference/STYLE.md)、[RELEASE](../../reference/RELEASE.md)、[PROCESS_MANAGEMENT](../../reference/PROCESS_MANAGEMENT.md)、[SERVER_DEVELOPMENT](../../reference/SERVER_DEVELOPMENT.md)。大文件按定位章节读，不反复整份注入。
- 历史延续：[Phase8 handoff](../runs/2026-11-16-phase8-main-handoff.md)、[Phase9 runtime spec](../specs/2026-11-15-phase9-runtime-proof-and-dependency-closure.md)、[Phase9 closeout](../runs/2026-11-16-phase9-closeout.md)、[execution receipt](../runs/2026-11-16-phase9-execution-receipt.md)、[final integrity](../runs/2026-11-16-phase9-final-integrity.json)、[accepted verdict](../runs/2026-11-18-phase9-acceptance.json)、[source manifest](../runs/2026-11-18-phase9-source-manifest.json)、[default assets](../runs/2026-11-18-phase9-default-manifest.json)、[parity receipt](../runs/2026-11-18-phase9-parity-receipt.json)。这些是历史身份，不是Phase11新回归结果。
- 早期 API/重构背景：[Phase0 spike](../plans/2026-11-05-nsh-phase0-spike.md)、[API verification](../plans/2026-11-05-nsh-api-verification.md)、[Phase1 protocol/transport](../plans/2026-11-05-nsh-desktop-phase1-protocol-transport.md)、[Phase2 connection](../plans/2026-11-05-nsh-desktop-phase2-connection.md)、[Phase3 technical debt](../plans/2026-11-05-nsh-desktop-phase3-technical-debt.md)、[Phase3 UI core](../plans/2026-11-05-nsh-desktop-phase3-ui-core.md)、[Phase4 capability alignment](../plans/2026-11-05-nsh-desktop-phase4-capability-alignment.md)、[input limitations](../plans/2026-11-05-nsh-input-limitations.md)。v0.4/0.5 API和旧状态不得覆盖0.6生产事实。
- 历史Phase10起点：[entry brief](../runs/2026-11-18-phase10-entry-brief.md)、[baseline](../runs/2026-11-18-phase10-baseline.md)、[API inventory](../runs/2026-11-18-phase10-nsh-api-inventory.md)、[old restart prompt](2026-11-18-phase10-nsh-shell-restart.md)。其中0.5 pin/原门槛/旧范围以官方规格、批准修订及本Phase11计划为准。

### C. 代码参考：旧契约与新实现逐点对照 / Source manifest

以下是起始入口，不是允许保留未挂载代码的理由。先以 `rg --files`和工作区/运行入口核对当前实际拥有者，Task1追加每个真实调用点。

| 范围 / Area | 完整路径 / Paths |
| --- | --- |
| 工作区/构建 | `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `build.rs`, `justfile` |
| 当前NSH宿主 | `desktop/nsh-shell/Cargo.toml`, `desktop/nsh-shell/src/main.rs`, `desktop/nsh-shell/src/shell_host.rs`, `desktop/nsh-shell/src/panel_bridge.rs`, `desktop/nsh-shell/src/panel_bridge.js`, `desktop/nsh-shell/tests/nsh_window_compat.rs` |
| 本机包参考 | `desktop/nsh-shell/macos/Info.plist`, `desktop/nsh-shell/macos/bundle-local.sh`, `desktop/nsh-shell/macos/test_bundle_local.py`；仅本机签名打包，非发布更新器 |
| 旧壳真实契约 | `desktop/shell/Cargo.toml`, `desktop/shell/src/main.rs`, `desktop/shell/src/daemon.rs`, `desktop/shell/src/connection.rs`, `desktop/shell/src/connect_setup.rs`, `desktop/shell/src/gateway_probe.rs`, `desktop/shell/src/keyring_bridge.rs`, `desktop/shell/src/update.rs`, `desktop/shell/src/tray.rs`, `desktop/shell/src/hotkey.rs`, `desktop/shell/src/deeplink.rs`, `desktop/shell/src/notify.rs`, `desktop/shell/src/menu.rs`, `desktop/shell/src/external_link.rs`, `desktop/shell/src/webview_perms.rs`, `desktop/shell/src/perm_monitor.rs`, `desktop/shell/src/phase9_qa.rs` |
| 旧壳包与权限 | `desktop/shell/tauri.conf.json`, `desktop/shell/tauri.lite.conf.json`, `desktop/shell/tauri.macos.conf.json`, `desktop/shell/capabilities/`；其余`tauri*.json`按实际文件继续普查 |
| 类型与Panel宿主 | `shared/protocol/src/panel_shell.rs`, `shared/protocol/src/lib.rs`, `shared/client/`, `interfaces/webchat/src/app.rs`, `interfaces/webchat/src/app/shell_bridge.rs`, `interfaces/webchat/src/platform_host.rs`, `interfaces/webchat/src/context.rs` |
| 凭据/文件边界 | `interfaces/webchat/src/api/native_credentials.rs`, `interfaces/webchat/src/api/fs.rs`；保留BridgeIpc/lease/attempt安全语义 |
| Panel直接消费者 | `interfaces/webchat/src/components/boot_check_gate.rs`, `interfaces/webchat/src/components/command_palette.rs`, `interfaces/webchat/src/components/directory_browser.rs`, `interfaces/webchat/src/components/layout_toggle.rs`, `interfaces/webchat/src/components/mode_sidebar.rs`, `interfaces/webchat/src/components/notification_center.rs`, `interfaces/webchat/src/components/team_task_strip.rs`, `interfaces/webchat/src/platform/wide/views/chat/project_menu.rs`, `interfaces/webchat/src/platform/wide/views/settings/desktop_autostart.rs`, `interfaces/webchat/src/platform/wide/views/voice/audio.rs`, `interfaces/webchat/styles/tailwind.css`, `interfaces/webchat/webview-baseline.json` |
| Core边界 | `src/bin/aleph-server/commands/service/`, `src/bin/aleph-server/commands/start/`, `src/bin/aleph-server/commands/update.rs`, `src/gateway/control_plane/assets.rs`, `src/gateway/surface/desktop.rs`, `src/gateway/server/connection/off_loop.rs`, `src/capability/` |
| 保留平台/原生能力 | `desktop/macos/`, `desktop/windows/`, `desktop/linux/`, `desktop/shared/`, `tests/bridge_acceptance/`；按真实调用查Swift/IPC桥，不将其误归Tauri独占 |
| 发布/依赖链 | `.github/workflows/aleph-app-release.yml`, `.github/workflows/aleph-core-ci.yml`, `.github/workflows/rust-doctor.yml`, `scripts/stage_shell_placeholders.sh`, `scripts/check_webview_baseline.mjs`；同时普查其他活动脚本/README/贡献指南 |

### D. 验证与原始证据 / Verification and raw evidence

- QA入口：`qa/README.md`、`qa/phase9_runtime.md`、`qa/nsh_panel_launcher.py`、`qa/nsh_panel_matrix.py`、`qa/nsh_panel_parity.py`、`qa/nsh_panel_runtime.schema.json`、`qa/nsh_panel_driver.js`、`qa/nsh_panel_gateway.py`、`qa/nsh_panel_provider.py`、`qa/test_nsh_panel_launcher.py`、`qa/native_ipc/`、`qa/webview_compat/`、`qa/browser_live/check_memory.py`、`qa/docs/check_markdown_links.py`。
- Phase10共享资产清单：`.phase10-evidence/official-canonical.mYMCixUf/sha256.json`；矩阵目录`.phase10-evidence/g7-official-matrix.Ji0duOcq/`内`matrix.json`、`source-identity.json`、`binaries.sha256`、`matrix-output/parity-receipt.json`及每轮原始文件。不能只看汇总PASS。
- 保留评估器：`.phase10-evidence/phase10-amended-acceptance.py`、`.phase10-evidence/g7-tray-candidate.IThQBKIR/audit-matrix.py`、`restored-candidate-audit.json`（后两者位于同一目录）。历史审计可能断言当时HEAD；新commit后应核对源哈希与祖先关系，不把HEAD演进误当源码变化，也不篡改原回执。
- 审查回执：`.phase10-evidence/closeout-20261121/independent-matrix-audit.md`、`shell-source-audit.md`、`review-QUALITY-retry.md`，及`review-QUALITY.md`的超时记录；`review-SPEC-summary.md`若存在是摘要而非完整原文。具体可用性以索引/文件核查为准。
- 其余G1–G7、反例、旧fork和失败矩阵，从candidate integrity/native ledger逐项定位，保留而不改写；不在prompt重复几百轮日志。完整性索引已列明每项路径和哈希，本清单通过索引包含它们。

### E. 参考项目及版本定位 / Reference projects and version discovery

| 项目 / Project | 用途、版本和定位 / Role, version and discovery |
| --- | --- |
| [Aleph canonical repository](https://github.com/rootazero/Aleph) | 唯一编辑落点是当前`git rev-parse --show-toplevel`得到的本地main；远端页面不替代未push的本地交付 |
| [Official NSH](https://github.com/longbridge/nsh) | 官方API/示例/平台后端参考。Phase10固定0.6.0，CLI标识5541637；按`Cargo.lock`解析的registry source为编译事实源，不把最新HEAD或旧0.5 fork当同版本实现 |
| NSH本机参考检出`nsh-phase10` | 本机定位为git root的同级`../nsh-phase10`，2026-11-21核查remote为官方NSH，HEAD `644a31f0cd1cc35855b6241aedec90a2eda9f222`，且有工作区改动。它是历史研究/实验检出，不能自动链接为生产依赖，也不能覆盖/清理其改动。换机先找再核对 |
| Aleph历史工作检出`aleph-phase10-local` | 本机同级`../aleph-phase10-local`，核查HEAD `0e4e1d4ebaa6e6dee8d6af9928f969d2e90f42b7`、remote为Aleph。只用于历史对照，禁止跨检出误编辑、reset或删除 |
| [Tauri](https://github.com/tauri-apps/tauri) / [plugins workspace](https://github.com/tauri-apps/plugins-workspace) | 被替代的框架语义参考，实际业务契约仍以Aleph旧壳为准。当前lock: `tauri 2.12.4`、`tauri-runtime-wry 2.12.2`、`tauri-plugin-updater 2.14.0`、`tauri-plugin-autostart 2.7.0`；查当前resolver确认再引用 |
| [WRY](https://github.com/tauri-apps/wry) / [TAO](https://github.com/tauri-apps/tao) | 仅在WebView/事件循环行为差异需追根时查；当前lock为`wry 0.57.0`、`tao 0.37.1`。它们不是另一个获准替代框架，不因此引入新依赖 |

官方资料入口：[installation](https://nsh.dev/docs/installation/)、[Rust integration](https://nsh.dev/docs/rust/)、[macOS runner](https://nsh.dev/docs/macos/)、[WebView messaging](https://nsh.dev/docs/web-view-and-video/#messaging-between-the-page-and-the-app)、[desktop/single instance](https://nsh.dev/modules/nsh-desktop)、[URL schemes/notifications](https://nsh.dev/modules/nsh)、[Tauri v2 guides](https://v2.tauri.app/)。这些链接是已知参考入口，本次规划未重新抓取所有外站；网站可能领先锁定版本，签名以同版本源码为准。

定位源码：先读取当前lock里的包名/版本；通过当前`CARGO_HOME`（未设置时`~/.cargo`）下`registry/src/*/<crate>-<version>/`查安装源，再核查官方检出的remote/HEAD/status。重点NSH路径：`nsh-core-0.6.0/src/{desktop.rs,desktop/,element/,notifications.rs,url_scheme.rs,single_instance.rs}`、`nsh-engine-0.6.0/src/{lib.rs,host_channel.rs}`、`nsh-platform-macos-0.6.0/src/{app.rs,surface.rs,window.rs,window_state.rs,tray.rs,global_shortcut.rs,system.rs}`，以及匹配官方版本的Windows/Linux backend、examples和docs。本次lock尚无Windows/Linux NSH平台包；必须验证而不能猜测存在可用发布版。历史git cache和兄弟实验检出不能替代registry baseline。

不要求克隆无关参考项目来“补齐清单”；若实施新增参考项目，先记录URL、版本、具体借鉴的契约和不适用处。以上同级目录是本机定位记录，不是跨机器固定地址或缺失即不可用的判断。

## 交付身份 / Handoff identity

Phase10 commit on local `main`: `576c723591e40c139728f7198d9dd1630e7706e6`（`feat(nsh): complete phase10 macOS shell integration`）。历史测量 HEAD `0e4e1d4ebaa6e6dee8d6af9928f969d2e90f42b7`仅用于旧回执身份，不是待 reset 的目标。下一会话应验证交付提交是当前 `main` 的祖先，并从最新本地 `main`继续。

本机 `.phase10-evidence/`、`.phase10-target/`及相关优化目标目录是保留的未跟踪本地证据/构建树，不是待全部 add 的文件。若换机器，先核对交付回执所指原始证据可用性；缺失时如实记为未能重核，不重造历史 PASS。

---

**本提示词未执行 / Not performed here:** Phase11实现、系统默认切换、Windows/Linux验收、双轨14天、发布、Tauri删除。下一会话从 Task1 开始实施。
