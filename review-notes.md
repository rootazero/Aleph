# Review: src/workflow

## Summary

- **Files**: 14 (5,668 LOC, ≈40% tests)
- **P0 / P1 / P2**: 0 / 4 / 3
- **整体评估**: 健康 (有少量需要关注的风险点)

模块是一份声明式工作流模板层,对外暴露 `WorkflowDef`/`WorkflowManifest` schema + 文件 store + 一个将模板编译到既有 `coord_tasks` DAG 的 `materialize`,以及 `.workflow.js` ↔ Aleph 双向互操作。代码防御深度好:
- 路径用 `sanitise_name` 防穿越 (store.rs:73, `json_canvas_io::sanitise_name`); 测试覆盖 (store.rs:434)。
- `WorkflowManifest`/`WorkflowManifestStep` 都加 `deny_unknown_fields` (manifest.rs:84, 100); legacy `snake_case` 通过 `alias` 接受。
- `WorkflowDef::validate` 对 step id、依赖、循环、clarify 与 agent 的不变量都做了硬校验 (def.rs:269-385)。
- import lexer 是 bounded hand-rolled state machine,无 regex、无 eval、`MAX_DEPTH=128`、对动态表达式一律 abstains (consts.rs:14); 注释里多次强调 R3/R7/R10 边界。
- 模板运行后 prompt 模板 `{input}`/`{{name}}` 用**单 pass** 替换,substituted value 永不 re-scan (def.rs:537),消除模板注入。
- 没有任何 `unsafe`,所有 production 代码 `unwrap_or_else(|e| {tracing::error/..})` 显式处理错误,`unwrap()`/`expect()` 全部在 `#[cfg(test)]` 模块。

---

## Findings

### [P1] store.rs:128-176 `save_at` — 同名并发写入的 lost-update race

`save_at` 用 `{pid}.{AtomicU64 seq}.tmp` 保证不同 writer 的 temp 文件不冲突,但 `fs::rename(&tmp_path, &final_path)` 是 POSIX-atomic「后写覆盖」。两个进程/线程同时对同一 `manifest.name` 调用 `save_at`,后写者覆盖前者,先写者拿到 `Ok(())`(rename 也成功)却不知道内容已被覆盖。

在 Aleph 的实际使用里:`save` 的语义是「保存我的模板」,workflow 名字由用户/AI 选取,同名并发冲突罕见但理论上可达(proposal 自动 accept 时若同名)。现状是静默丢失更新。

建议修复方向:
- 在 `save_at` 内做「先检查再 rename」的乐观锁:写完 temp 后 `read_link`/`fs::metadata` 检查 final 的 `mtime`,若已被覆盖则返回 `AlephError::conflict`。
- 或者:在 store 层加一个 in-process `Mutex<HashSet<String>>` 串行化同名 save(只保护同进程,跨进程只能靠 lockfile)。
- 至少在文档里写明「`save` 非并发安全,同名并发是用户错误」。

### [P1] import/mod.rs:60-69 `extract_embedded` — 手写 `.workflow.js` 嵌入块中的字面 `*/` 会过早截断

`find(EMBED_SUFFIX)` 不感知 JSON 字符串上下文,在 prefix 之后直接找第一个 `*/`。一个手写文件如下:

```js
/* @aleph-workflow {"description":"use the */ glob"} */
export const meta = ...
```

`find(EMBED_SUFFIX)` 会落在字符串内的第一个 `*/`,返回 `{` + 部分 description,`serde_json::from_str` 报 parse error。用户得到的反馈是 "embedded @aleph-workflow parse failed: ...",**没有任何提示**指出「字符串里有 `*/` 是元凶」。

export 侧 (export.rs:27) 通过 `.replace("*/", "*\\/")` 规避了这个问题,所以**导出 → 重新导入是安全的**(测试覆盖:import/mod.rs:1087);漏洞只发生在**用户手写**且不熟悉这一转义约定的场景。

建议修复方向:
- 在 `serde_json::from_str` 失败时,如果错误信息提到位置在 JSON 字符串内,给出 actionable hint: 「字符串内含字面 `*/` 会被嵌入块边界错误截断,请用 `*\/` 转义或避免」。
- 或者:写一个 JSON-aware 的 find-close-quote scanner(比 export 的全量替换更稳)。

### [P1] import/mod.rs + lexer.rs + scan.rs — import 路径缺少输入大小上限,可被超大文件 DoS

`parse_workflow_js(src)` 没有限制 `src.len()`。lexer 会在源串上做至少 4× 内存放大:
1. `chars: Vec<char>` = 4 字节/char (UTF-8 worst case)
2. `blanked: String` (blank_comments, lexer.rs:62) ≈ 1× src
3. `skeleton: String` (strip_string_literals, lexer.rs:355) ≈ 1× src
4. `collect_consts` 内部的 `ConstTable` HashMap

对一份 1 GiB 的恶意文件,峰值内存 ~4-5 GiB。`MAX_DEPTH=128` 只防 stack overflow,不防内存耗尽。

建议修复方向:
- 在 `parse_workflow_js` 入口加 `if src.len() > MAX_IMPORT_BYTES { return Err(invalid_input(...)) };`,常量例如 16 MiB(覆盖任何合理 `.workflow.js`)。
- 同样限制应用到 `store::load_at` / `read_to_string`(已有 `fs::read_to_string` 不限大小)。

### [P1] import/scan.rs:97-122 `scan_events` — `parallel_watch` 栈无界增长(不平衡括号)

`parallel_watch: Vec<i32>` 在每次识别到 `parallel(` 时 push 当前 `paren_depth`。如果恶意文件有大量 `parallel(` 但缺少对应 `)`,`parallel_watch` 单调增长;`Vec<i32>` 上限等于输入大小(每个 `parallel(` 占 4 字节),1 GiB 文件可堆出 ~1 GiB 辅助结构。这与 #3 共享输入大小根因,但单独触发条件更窄(必须 `parallel(` 开头),因此作为独立 P1。

修复方向:与 #3 合并,在入口处限制 src 长度即可一并解决。

### [P2] import/consts.rs:228-247 `read_string` — 非标准转义被「字面通过」,round-trip 有损

```rust
out.push(match esc {
    'n' => '\n', 't' => '\t', 'r' => '\r', '0' => '\0',
    other => other,  // \b→b, \f→f, \v→v, \xNN→xNN, \uNNNN→uNNNN
});
```

注释明确说"best-effort bare path;the `@aleph-workflow` embed header is the byte-exact round-trip guarantee"。一个手写 prompt 用 `\b`(backspace)或 `\u00A0`(non-breaking space)会被原样保留为 `b`/`u00A0`,在 agent 看到 prompt 时不是预期字符。

不是安全问题(没有注入面),是**手写工作流的诚实问题**。建议:在 lexer 顶端加 doc 警告「bare 路径只支持 `\n \t \r \0 \? \\\" \\\' \\\\`,其它转义请用嵌入块 round-trip」,或扩展 `read_string` 支持 `\b \f \v \xNN \uNNNN`。

### [P2] compile.rs:392-446 `cancel_partial` — 部分回滚的 race window,settle sweep 可能误发通知

`cancel_partial` 先 stamp `WORKFLOW_NOTIFIED_KEY` (anchor 上),再并发把 `ids` 全标 `Cancelled`。两个写之间没有事务。如果 stamp 成功、cancel 失败(磁盘错误),用户会收到「⚠️ Workflow finished」的 settle 通知,然后才发现任务实际被取消。注释自己承认了 "without this the settle sweep sees a fully-settled interactive run and pushes `⚠️ Workflow 'x' finished … cancelled` to the user, seconds after the tool already returned an error saying the run could not be started. Two contradictory messages about a run that never executed a step."

当前实现已经选了「先 stamp 抑制通知」的较优方向,但 **stamp 之后、cancel 之前的瞬时窗口** 仍然能让 dispatcher 进入 settle sweep。完全防住需要把 stamp 和 cancel 作为一个原子 batch,或放弃在 anchor 上单独 stamp(改成让 dispatcher 直接认 cancelled 状态、不读 notification key)。

属于 documented known limitation,作为 P2 列出供讨论。

### [P2] compile.rs:600 `WorkflowManifest::validate` 重复校验 (cosmetic)

`WorkflowManifest::validate` 末尾调用 `self.to_def().validate()`,`to_def().validate()` 内部又调用 `topo_order()`。`materialize` 入口已先调用一次 `def.validate()`(compile.rs:374),所以一次 materialise 触发三次 topo_order(O(V+E))。对 100 步以下 workflow 不可见,1000+ 步工作流可能拖慢启动。不阻塞。

修复:`materialize` 入口直接调 `def.topo_order()` 一次,跳过冗余 `validate()`;或者 `WorkflowManifest::validate` 只检查 manifest 独有字段(`effort`、`isolation` 词表),不去再校验 `to_def`。

---

## 违反红线的情况

无 hard 违反,但有 1 处 **gray area** 值得在 design review 里讨论:

### [P3 / gray] R8 vs import lexer 的「非机器格式」解析

**R8 原文**:"LLM 处理 intent/routing。Regex 只用于机器格式 (JSON, URL)。"

**实际情况**:
- import lexer (`consts.rs` + `lexer.rs` + `opts.rs` + `scan.rs` ≈ 1500 LOC) 实现了 JS 数据字面量子集 (bare keys / 单引号 / trailing commas) 的 hand-rolled 解析。**它不是 regex**,是 state machine,所以 R8 字面上没违反。
- 但 R8 的精神是「结构化解析保持极简,语义理解交给 LLM」。这个 lexer 把工程格式完整解析成 manifest,涉及非平凡的 JS-lax 子集(嵌套对象/数组、转义、`.join` 链、`[...].join("...")` 数组 prompt、hoisted const 引用)。这是「R8 反对的范式」—— 程序在替代 LLM 解析结构化文本。

**为什么还这么做**:R3/R7 明确要求 module 自洽、不依赖外部 JS 解析器;且 import 路径的输入是「声明式模板」而非「自然语言 intent」。这与 R8 的目标(避免用 regex 替代 LLM 理解自然语言)并不重合。

**建议**:
1. **在 AGENTS.md 增补一行**:明确「R8 排除 import 的机器化声明式格式解析,适用 .workflow.js / 嵌入 manifest JSON / legacy def JSON」,把 lexer 显式豁免出 R8。
2. 或者:**保留 lexer 但在文件顶端加更长 doc**,写清楚「此处放弃 R8 的精神以换取 R3 独立性,边界 = 数据字面量,任何动态表达式 abstains」。
3. **不要尝试替换为「让 LLM 解析」**——会让 import 路径需要在线 LLM,违反 R3/R7。

### 其他红线

| 红线 | 状态 |
|---|---|
| **R1** Core 不调用平台 API | ✅ 满足。模块只用 `std::fs / serde / tracing / uuid / futures`。 |
| **R2** 复杂业务 UI 在 Leptos/WASM | ✅ 不涉及(纯后端 schema/compiler/store)。 |
| **R3** Core 最小化,无重 dep | ✅ 满足。lexer 是 hand-rolled 状态机,无 `serde_yaml`/`quick-xml`/`boa_engine` 之类的重型解析器。 |
| **R4** Interface 层 = pure I/O | ✅ store.rs 是 pure file I/O,无业务逻辑。 |
| **R5** Menu bar first | N/A (后端模块)。 |
| **R6** AI comes to you | N/A。 |
| **R7** 一个 core 多个 shell | ✅ 模块是 declarative schema + 编译器,无平台特定代码;clarify 通过 channel-agnostic metadata 工作。 |
| **R8** regex 只用于机器格式 | gray (见上) — 字面未违反,精神上有张力。 |
| **R9** 可配置项暴露为 tool | ✅ workflow tool 的 `run/describe/status/list/delete/cancel/accept_proposal/import/export` 都通过 MCP 暴露。 |
| **R10** 智能在 prompt | ✅ 没有 middleware tax:store/compile/import/export 都是确定性映射;所有「意图」(选哪个 step、跑什么 prompt)都留给 LLM 在 tool args 里。 |

---

## 建议但不阻塞 (P3)

1. **def.rs:537 `scan_prompt` — `{input}` 与 `{{input}}` 是不同语义,易混淆**:文档明确,但 prompt author 很可能不知道。是否需要在 `validate` / `missing_vars` 里 warn?

2. **def.rs:683 `referenced_vars` 把 `{{name}}` 收集为 `BTreeSet<String>`,但不收集 `{input}`**:正确(因为 `{input}` 由 `RunInputs::from_input` 显式提供),但与 `missing_vars` 的实现耦合,任何一边修改都会另一侧失效。建议加测试覆盖「only `{input}` no `{{...}}`」和「only `{{...}}` no `{input}`」两种边界。

3. **proposal.rs:100-150 `skeleton_from_chain` — `chain.len() < 2` 与 sanitization 后只剩 1 step 是两阶段检查**:`if steps.len() < 2 { return None; }` 已经覆盖,但中间 trace(警告日志)只在「完全塌缩」时打。两阶段都在调用前的「塌缩 → 中间步骤 → 重建」链路里,proptest 友好,但建议加 property test 保证「任何 chain 经过 `skeleton_from_chain` 后一定通过 `validate`」(目前已经手写测试覆盖 collision case,但 prop-test 更稳)。

5. **compile.rs:600 `with_tolerate_failed_deps / with_task_timeout / with_max_retries` 链式调用**:三次独立 JSON 序列化/反序列化(每次都 round-trip 一份 metadata),对大 metadata 是 O(n) × 3。建议:这三个 helper 是否能合并为一次 round-trip,或只 patch 必要的 key?

6. **interop/export.rs:96 `partial_fan_in_disclosure` 注释注释很丰富,但 `// NOTE - the body below is NOT a lossless encoding ...` 注释块超过 15 行**:手工维护的注释比代码长,未来 export 修改时容易漂移。建议:把 disclosure 内容做成 const string,写一个 `disclosure_block_for(manifest) -> Option<String>` 单测覆盖每一个 graph shape,让注释成为 code 而非 docs。

7. **store.rs:60 `aleph_home()` fallback to `.`**:当 `get_config_dir()` 失败时,落到 CWD,在 systemd / sandbox / 非交互 daemon 下可能落到一个意外目录(甚至不可写)。当前是隐式行为,应改为至少 `tracing::warn!` 一下 + 返回 error 而不是 fallback。

---

## 不做的事 (State of Negative)

- **未跑测试**:`cargo test -p alephcore --lib workflow` 没执行。审查是纯静态,所有断言基于代码 + 注释 + 测试代码的覆盖面。
- **未检查协作模块**:`CoordTaskStore`、`SqliteCoordTaskStore`、`dispatcher`、`inbound_router`、`strategy::Strategy` 等模块没有通读,只通过 workflow 的 use 语句和注释理解它们的行为。P1 级别的 race 问题可能与这些模块的并发模型有关,需要单独审查。
- **未 fuzz import lexer**:lexer 的字符串解析、转义处理、hoisted const 引用都手工写了测试,但缺少 proptest。proptest-regressions 目录的存在说明项目有 fuzz 基础设施,建议给 lexer 加 `proptest!` 覆盖。
- **未审 `WITH_CORE_FROM` 的幂等性**:`with_core_from` 是新代码,有测试,但「连续编辑两次再 save」这种场景的测试没看到(只在 `with_core_from_keeps_executable_extras_of_surviving_steps` 测试了一次合并)。建议加 property test:对任意 manifest + 任意编辑后 def,`with_core_from` 仍能保留 extras。
- **未审 docstring 与代码一致性**:许多 file-top doc 注释非常长(> 100 行),可能有 doc 描述与 code 不一致的地方(如 R8 红线解释)。本次审查以代码为准,doc 不一致未单独列出。
- **未审 Cargo.toml / build script**:没有检查 workflow 模块在 `Cargo.toml` 里的 feature flag、依赖、可选编译路径。