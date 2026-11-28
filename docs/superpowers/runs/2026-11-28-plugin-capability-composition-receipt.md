# 插件组合验收 / Scoped Plugin Composition Verification

设计与计划 / Design and implementation plan: [设计 / Design](../specs/2026-11-27-plugin-capability-composition-design.md) · [实施计划 / Implementation plan](../plans/2026-11-28-plugin-capability-composition.md)

## 验收边界 / Acceptance scope

所有改动在 `plugin-capability-composition` 上以 `438227060` 为基线。验证主机为 Apple M4、16 GB 内存的 macOS，使用 Rust 1.99.1、真实 Gateway 和 CLI 以及隔离的 `HOME` 与 `ALEPH_HOME`。模拟模型端记录实际请求。MCP stdio 返回内容和 Hook 回调均从生产调用路径观察。

The isolated `plugin-capability-composition` branch starts at `438227060`. Verification used macOS on Apple M4 with 16 GB RAM, Rust 1.99.1, real Gateway and CLI binaries, and an isolated `HOME` and `ALEPH_HOME`. The mock model recorded actual provider requests. The production paths supplied the observed MCP stdio result and hook callback.

| 区域 / Area | 可观察结果 / Observed result |
| --- | --- |
| Claude Code | 已安装技能和 Agent 进入真实模型请求，Agent 发出子请求，MCP 工具结果返回模型，经 CLI 审批的 Hook 写入回调标记。 / Skills and agents reached provider requests, the agent made a child request, the MCP result returned to the model, and the approved hook wrote a callback marker. |
| Pi | 选中的平铺技能可列出和读取，排除项不可选。提示词保留位置、引号和默认参数语义，编辑和重载后模型收到新内容。 / The selected flat skill was listed and read, while the excluded skill stayed unavailable. Prompt argument rendering and updated content were observed in model requests. |
| DeepSeek Harness | 暂存安装保留字面量 stdio 与 Streamable HTTP 声明。stdio 工具返回实际结果，loopback HTTP 在发布工具前被现有 SSRF 策略拒绝。导入诊断定位 `!!js` 与 Cordis 专属配置。 / Staged installation retained literal stdio and Streamable HTTP declarations. A stdio tool call returned a real result. Existing SSRF policy refused loopback HTTP before tool publication. Import diagnostics located `!!js` and Cordis-specific configuration. |
| 实例与世代 / Instances and generations | 相同 ID 的两个项目和全局安装各自发布。禁用项目实例后裸别名继续遮挡，其他作用域保留。WASM guest 和 Capability facade 验证旧 lease 失效，MCP 退役客户端无法重新发布。 / Two projects and Global published independent same-ID instances. Disabling the project winner preserved package shadowing and the sibling owners. The real WASM guest and facade exercised stale lease rejection, and the retired MCP client could no longer republish. |

## 冷发现度量 / Cold discovery measurements

在同一台主机上重复运行 144 个本地包的固定样本。样本含 6 种布局，每种 24 个包。两个版本各启动 5 个新进程，计时只涵盖发现与解析。单独用 macOS `open(2)` 计数器记录清单的只读打开次数。

Both revisions ran the same 144-package local fixture, with 24 packages in each of six layouts. Each revision ran five fresh processes. Timings cover discovery and parsing. A separate macOS `open(2)` trace counted read-only manifest opens.

| 代表布局 / Manifest layout | 基线 / Baseline | 整合版 / Integrated |
| --- | ---: | ---: |
| Claude TOML | 72 | 24 |
| Claude JSON | 96 | 24 |
| Aleph TOML | 72 | 24 |
| Codex | 24 | 24 |
| Cursor | 48 | 24 |

冷发现基线中位数为 **19.3 ms**，范围 18.4–25.6 ms。整合版避开编译负载后复测，中位数为 **23.5 ms**，范围 23.2–121.5 ms，包含首次启动的 121.5 ms 样本。整合版首次测量与编译并行，中位数为 32.9 ms，范围 26.0–47.8 ms。字节内容的打开次数下降，快照验证另有文件元数据检查。

The baseline median was **19.3 ms** with an 18.4–25.6 ms range. A repeat of the integrated measurement without concurrent compilation had a **23.5 ms** median and a 23.2–121.5 ms range, including the first 121.5 ms launch. The initial integrated run overlapped compilation and recorded a 32.9 ms median and a 26.0–47.8 ms range. Manifest content opens decreased. Snapshot validation performs separate metadata checks.

## 验证结果 / Verification results

同一份最新 Gateway 二进制通过 11 个隔离场景。`composition`、`command`、`manifest`、`scope`、`visibility`、`trust`、`cc-cache`、`scaffold`、`marketplaces`、`subagent`、`dsh` 均返回 0。DeepSeek Harness 的 HTTP 用例覆盖配置导入和默认 SSRF 策略拒绝 loopback，成功调用覆盖 stdio。

The same final Gateway binary passed 11 isolated scenarios. Each of `composition`, `command`, `manifest`, `scope`, `visibility`, `trust`, `cc-cache`, `scaffold`, `marketplaces`, `subagent`, and `dsh` returned 0. The DeepSeek Harness HTTP case verified configuration import and loopback refusal under the default SSRF policy. The stdio case verified a successful call.

19 组定向 Rust 测试全部通过，共 856 项通过，1 项忽略。Hook 组最终复跑 218 项通过。另有 14 项技能预处理测试通过，覆盖子进程为全局实例和两个项目实例收到不同的数据目录。

All 19 focused Rust groups passed, with 856 tests passed and one ignored. The final Hook rerun passed all 218 tests. Another 14 skill preprocessing tests passed, including a subprocess check that one global and two project instances each received a distinct data directory.

整合检查通过 `cargo check -p alephcore --all-targets` 和 `cargo clippy -p alephcore --lib`。Clippy 报告了 15 条警告。随后的审阅修复用最终 Gateway 构建与定向测试覆盖。

The integrated check passed `cargo check -p alephcore --all-targets` and `cargo clippy -p alephcore --lib`. Clippy reported 15 warnings. The final Gateway build and focused tests covered the subsequent review fixes.

## 独立审阅 / Independent review

独立 gpt-sol 审阅发现，部分执行入口仍用清单 ID 校验项目运行时 owner，导致 Hook、命令和技能无法一致解析实例数据目录。修复复用了 `PluginVars` 的 owner 检查与精确技能发布，保留原有审批标签。复审确认项目技能索引继续经由 `parse_skill_file`，保留 1 MiB 读取上限、内容筛查和伴随文件筛查。

The independent gpt-sol review found that some execution paths applied manifest ID validation to project runtime owners, which kept hooks, commands, and skills from consistently resolving instance data directories. The fixes share owner validation in `PluginVars` and use the selected skill publication while retaining established consent labels. Follow-up review confirmed that the project skill index still uses `parse_skill_file` with its 1 MiB read cap, content screening, and companion-file screening.

能力范围与格式说明 / Capabilities and format reference: [Plugin System](../../reference/PLUGIN_SYSTEM.md)
