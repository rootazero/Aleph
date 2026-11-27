# Memory Wiki evidence-chain verification / Memory Wiki 证据链验收

2026-11-26 · local branch `refactor/memory-wiki` · base `0386bdfff`

[Design / 设计](../specs/2026-11-26-memory-wiki-evidence-chain-design.md) · [Plan / 计划](../plans/2026-11-26-memory-wiki-evidence-chain.md) · [Current contract / 当前契约](../../reference/MEMORY_SYSTEM.md#12-provenance-chain-evidence-drill-down--证据链)

## Delivered / 交付

- `a5b3f0c79`, `bc8956d20`: shared trace DTOs and compatible RPC addressing. The Gateway still checks visibility; exact mode does not grant access. / 共享契约；精确寻址不绕过可见性检查，旧请求保留原 scope。
- `ef41c4cc5`, `a5075f7b0`: Core follows bounded, same-partition Markdown citations and exposes omissions separately from unavailable evidence. Synthesis records only the sorted, nonblank previews it actually sent to the model. / 按同分区 Markdown 引用有界追溯；合成只落盘实际输入，区分截断与来源缺失。
- `281b2d15e`, `bcde1d8a5`: the existing drawer renders note sources, fact annotations and raw evidence from the shared DTO; the rebuilt JavaScript/WASM/CSS and Brotli siblings are checked in for the embedding workflow. `2099798de` separately repairs the Galaxy disposal-safe reads and error classification required by the Panel suite. / 原抽屉消费共享事实，更新内嵌资源；星系旧守卫问题单独提交。
- Removed the Panel's duplicate trace wire DTOs and evidence-length heuristic, the Core `fact_origins` accessor superseded by `facts_with_origin`, and the unbounded session-to-raw-body lookup superseded by a scoped ID prefix. / 删除重复 DTO、长度猜测与被替代的 Core 读取路径。

## Recorded regressions and final gates / 复现与验收

| Gate / 门禁 | Observed / 实测 |
|---|---|
| Exact Gateway partition (red → green) | A bare `main` in exact mode returned `alice exercise evidence` instead of `org exercise evidence` before the fix. The final Core run includes both addressing modes and the admitted-runtime RPC with real Markdown. |
| Markdown citations (red → green) | The old walker returned `[]` for two expected source-note edges. The final run includes typed and untyped references, reverse matches, pruning, partitions, and omission budgets. |
| Fact-only Panel response (red → green) | The old decoder lost `transcript-17`; the shared decoder retains the source ID. Native tests also exercise edge states and `(raw_id, via_note)` expansion identity. |
| `cargo test --locked -p aleph-protocol` | 449 passed, zero failures; two ignored doctests. Includes two trace-wire cases. |
| `cargo test --locked -p alephcore --lib -- builtin_tools::memory_trace gateway::handlers::memory::trace_tests memory::store::sqlite::notes::tests memory::store::markdown::raw_memory memory::dreaming::stages::note_synthesis` | 91 passed, zero failures. Includes production `NoteSynthesisStage` writing actual Markdown and a trace replay through the persisted summary, `preference/02`, and `raw-1`. The no-rewrite case also passed; no behavioral red run was recorded for the synthesis tests. |
| `cargo test --locked -p aleph-panel --lib` | 1,563 passed, zero failures. Two pre-existing Galaxy source guards failed on the first run and passed after `2099798de`. |
| `just wasm` | Passed with worktree-local `wasm-bindgen-cli 0.2.129` matching the lockfile and the existing Tailwind deps. The bundled JS resolved all 39 WASM references against 42 exports; WebView baseline and Brotli checks passed. |
| Chromium, built `/memory?view=table` with a loopback JSON-RPC fixture | Opened `synthesis/wiki-smoke` and expanded the raw row. Accessibility capture showed the fact and source ID, unavailable note, full fixture excerpt, and both independent notices; no page errors. Fixture received `agent_id: "main@browser-fixture"`, `exact_partition: true`, `kind: "note"`, `target: "synthesis/wiki-smoke"`, `max_results: 20`. |
| Independent review | Final Kimi Pro review `2716727c-dcbe-48c8-963b-e9ab30df9b99` reported no correctness findings in the scoped walk, Gateway trace entry, synthesis path and Panel drawer. It did not rerun tests or inspect protocol serde, scope internals, store implementations, or WASM. An earlier review timed out with no verdict. |
| Source/doc checks | `rustfmt --check` on changed Rust files, `git diff --check`, and added reference links and anchor verified. |

浏览器验收使用真实构建后的 Panel、模拟 RPC；真实存储、Dreaming 写入与 Gateway 授权入口由独立的 Core 集成用例验证。未运行生产网关加浏览器的一体化场景、全量 Core 测试或三平台宿主 QA。No historical citation backfill, new Capability, new dependency, or Dreaming rollback was added. No merge, push, or production change was performed.
