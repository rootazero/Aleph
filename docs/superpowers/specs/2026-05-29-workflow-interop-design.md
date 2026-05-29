# Workflow Interop 设计:Aleph Workflow ↔ Claude Code `.workflow.js` 双向互换

- **日期**: 2026-05-29
- **状态**: 设计已批准,待写实现计划
- **范围**: 为 Aleph 的 `workflow` 子系统反向指定一份与 Claude Code `.workflow.js` 工程文件兼容的声明式格式,并实现双向转换(导入 / 导出)+ 正式规范文档。
- **执行核心改动**: 无(`WorkflowDef` / `compile.rs` / `TeamDispatcher` 零改动)。

---

## 1. 背景与问题

Aleph 已有 Workflow 功能(merged `cd8ebc14c`):声明式静态 DAG 模板,经 Kahn 拓扑校验后编译进 `coord_tasks`,由 `TeamDispatcher` 执行;以 R8 工具 `workflow`(save/list/describe/delete/run)暴露。它是**薄执行器**——零调度、零推理(R7/R10)。

现有 `WorkflowDef` 格式:
```rust
WorkflowDef { name, description, steps: Vec<WorkflowStepDef> }
WorkflowStepDef { id, agent, prompt, depends_on: Vec<String> }
```

Claude Code 的 Workflow "工程文件"(`.workflow.js`)是另一种形态:`export const meta = {…}` + **命令式 JS 脚本体**(`agent()` / `parallel()` / `pipeline()` / `phase()` / `log()`,每次调用可带 `schema` / `label` / `model` / `phase`),支持循环、条件、`budget` 驱动 fan-out、嵌套 `workflow()`。

**核心张力**:`.workflow.js` 是命令式脚本引擎;Aleph 是声明式薄执行器。控制流(routing / evaluator-optimizer)是 Aleph 故意的 R7/R10 deferral,**不得**外接到这一层。因此"兼容格式"只能覆盖 `.workflow.js` 的**声明式、红线安全子集**,而非"运行任意 JS"。

**目标**:让两种格式在声明式层面同构、可双向无损往返,且 Aleph 执行核心零改动。

---

## 2. 架构落点(已定:方案 A — Manifest 超集,WorkflowDef 不动)

- 新增独立 `src/workflow/interop/` 层,定义声明式 interchange manifest(AWI)作为规范唯一真相。
- manifest 承载全部 Claude-Code 兼容元数据(`whenToUse`、`phases[]`、每步 `label/model/phase/schema`)。
- 导入 Aleph 时,只把**可执行内核**映射进现有 `WorkflowDef`;其余元数据原样存于导出 `.workflow.js` 的内嵌注释块,供导出无损重建。
- `WorkflowDef`、`compile.rs`、dispatcher **一行不改** → 零死配置(R10),无损往返。

被否决的方案:
- **B. 增强 WorkflowDef**:`phase`/`schema` 在 Aleph 执行层无消费者 → 死配置 / 踩 R10。
- **C. 纯文件互转**:丢失 save→run 闭环,不属于 Workflow 功能。

导入边界(已定):**声明式清单为唯一真相**。不引入 JS 引擎(swc/boa),不写完整 JS-子集解析器 → R3 安全。

---

## 3. 互换格式规范 (AWI — Aleph Workflow Interchange)

声明式 JSON manifest,是 `.workflow.js` `meta` 块 + 声明式步骤元数据的纯数据投影。`.workflow.js` 是它的渲染视图。

```jsonc
{
  "name": "research-report",
  "description": "...",
  "whenToUse": "...",                                  // ← meta.whenToUse
  "phases": [{ "title": "Gather", "detail": "..." }],  // ← meta.phases
  "steps": [
    {
      "id": "gather",
      "agent": "researcher",
      "prompt": "research {input}",
      "dependsOn": [],
      "label": "audit:gather",         // ← agent() opts.label   (可选)
      "model": "haiku",                // ← agent() opts.model   (可选)
      "phase": "Gather",               // ← agent() opts.phase   (可选)
      "schema": { "type": "object" }   // ← agent() opts.schema  (可选,原样透传)
    }
  ]
}
```

字段约定:
- 顶层 `name` / `description` / `steps` 必填;`whenToUse` / `phases` 可选(缺省空)。
- 步骤 `id` / `agent` / `prompt` 必填;`dependsOn` 缺省 `[]`;`label` / `model` / `phase` / `schema` 可选。
- `schema` 原样透传(任意 JSON 对象),Aleph 不解释其内容。
- **JSON 键采用 camelCase**(`dependsOn` / `whenToUse`)以贴合 `.workflow.js` 习惯;manifest 类型用 `#[serde(rename_all = "camelCase")]`。注意这与 `WorkflowDef` 的 `depends_on`(snake_case)是两套 serde 表示,互转在 `manifest.rs` 显式搬运。

### 3.1 与 WorkflowDef 的映射

| AWI manifest | WorkflowDef | 备注 |
|---|---|---|
| `name` / `description` | `name` / `description` | 直通 |
| `steps[].{id,agent,prompt}` | `steps[].{id,agent,prompt}` | 直通 |
| `steps[].dependsOn` | `steps[].depends_on` | 仅大小写/命名差异 |
| `whenToUse` / `phases` | —(不入) | 仅存内嵌块 |
| `steps[].{label,model,phase,schema}` | —(不入) | 仅存内嵌块 |

`WorkflowManifest::to_def()` 丢弃额外字段产出可执行 `WorkflowDef`;`WorkflowManifest::from_def()` 产出仅含内核的 manifest(额外字段空)。

---

## 4. 新模块 `src/workflow/interop/`(纯数据层)

| 文件 | 职责 |
|---|---|
| `mod.rs` | 重导出 `WorkflowManifest` / `render_workflow_js` / `parse_workflow_js`。 |
| `manifest.rs` | `WorkflowManifest` / `WorkflowManifestStep` 类型(serde + JsonSchema)+ `from_def(&WorkflowDef)` / `to_def() -> WorkflowDef`。纯字段搬运,零推理。 |
| `export.rs` | `render_workflow_js(&WorkflowManifest) -> String`。 |
| `import.rs` | `parse_workflow_js(src: &str) -> Result<ImportOutcome>`,`ImportOutcome { manifest, dropped: Vec<String> }`。 |

### 4.1 导出 `render_workflow_js`

1. 顶部写无损往返块:`/* @aleph-workflow {<manifest-json-单行>} */`。
2. 写 `export const meta = { name, description, whenToUse, phases: [...] }`(pure literal)。
3. 脚本体由 `depends_on` 跑 Kahn **分层**(BFS 按 indegree 逐层弹出 —— 与 `WorkflowDef::topo_order` 同算法,但收集"每一层"而非扁平顺序;在 `export.rs` 内写一个小 helper,不改 `def.rs`)生成:
   - 每个拓扑层:若该层 >1 个步骤 → 渲染 `await parallel([ () => agent(...), ... ])`;若 =1 → 渲染 `await agent(...)`。
   - 步骤渲染为 `agent(<prompt-字符串字面量>, { label?, phase?, model?, schema? })`。
   - 若步骤带 `phase`,在该层前插 `phase('<title>')`。
4. 字符串字面量转义:用单引号包裹并转义,或多行用数组 `.join('\n')`(参考工程文件 gotcha:**模板字面量内不得含裸反引号**)。导出器统一用 JSON.stringify 风格的双引号字符串以避免转义陷阱。

> 导出产物是**可读 + 可被 Claude Code 运行的声明式骨架**;命令式控制流不生成。`pipeline`/`budget`/循环不会出现在导出结果中(Aleph 源本就没有)。

### 4.2 导入 `parse_workflow_js`

- **① 有内嵌块**:正则/扫描提取 `/* @aleph-workflow {…} */` 内的 JSON → `serde_json` 解析为 `WorkflowManifest` → **无损**,`dropped` 为空。
- **② 无内嵌块(裸 `.workflow.js`)**:
  - 提取 `export const meta = { … }`(规范保证 pure literal)→ 取 `name`/`description`/`whenToUse`/`phases`。
  - 尽力识别声明式骨架:顺序 `await agent(...)` → 线性链;`parallel([...])` → 同层兄弟;`agent()` 的 `opts` 中 `label`/`phase`/`model`/`schema` 尽力提取。步骤 `id` 缺失时按出现序合成(如 `step_1`)。
  - 遇变量绑定、循环、条件、`budget`、`pipeline(items,...)`(item 列表运行时才知)、嵌套 `workflow()` 等命令式构造 → **不解析**,把该构造的简述记入 `dropped[]` 并跳过。
- 解析后 `manifest.to_def()` 再 `validate()`;失败则返回 Err(连同 `dropped` 上下文)。

**零新依赖**(R3):仅用 `serde_json`(已有)+ 手写字符串扫描;不引入 swc/boa。轻量扫描的局限性是**设计选择**,通过 `dropped[]` 对用户透明,而非假装全解析。

---

## 5. `.workflow.js` ↔ DAG 映射表(规范)

| Claude Code 构造 | Aleph 方向 | 说明 |
|---|---|---|
| `meta.{name,description,whenToUse,phases}` | ↔ 无损 | manifest 顶层 |
| 顺序 `await agent()` 链 | ↔ | 线性 `depends_on` 链 |
| `parallel([agent, agent])` | ↔ | 同层、彼此无 `depends_on` 的兄弟步骤 |
| `agent()` fan-in(一步用多个上游结果) | ↔ | 一个步骤 `depends_on` 多个上游 |
| `opts.{label,model,phase,schema}` | ↔ 无损(经内嵌块) | 存 manifest,不入 WorkflowDef |
| `pipeline(items, s1, s2)` | → 导入近似 | 无运行时 item 列表 → 记 `dropped` 提示,按阶段链近似;导出不生成 |
| 循环 / 条件 / `budget` / 嵌套 `workflow()` | ✗ **故意不支持** | 属 Think→Act 循环职责;导入记 `dropped[]`(R7/R10:不外接控制流) |

---

## 6. 工具表面(R8,沿用现有 `workflow` 工具风格)

`WorkflowArgs` 新增两个动作(`#[serde(rename_all="snake_case", tag="action")]` 不变):

```rust
/// Render a saved template into a Claude-Code-compatible `.workflow.js`.
Export {
    name: String,
    /// Also write it to `$ALEPH_HOME/workflows/<name>.workflow.js`.
    #[serde(default)]
    write_file: bool,
},
/// Parse a `.workflow.js` (or AWI manifest JSON) into a WorkflowDef.
Import {
    /// Raw `.workflow.js` text or AWI manifest JSON.
    source: String,
    /// Also persist the parsed template via the store.
    #[serde(default)]
    save: bool,
},
```

`WorkflowToolOutput` 新增两个字段(沿用 `skip_serializing_if = "Option::is_none"`):

```rust
/// Populated by `export` — the rendered `.workflow.js` text.
#[serde(skip_serializing_if = "Option::is_none")]
pub rendered: Option<String>,
/// Populated by `import` — imperative constructs that could not be mapped.
#[serde(skip_serializing_if = "Option::is_none")]
pub dropped: Option<Vec<String>>,
```

行为:
- `export`:`store::load(name)` → `WorkflowManifest::from_def` → `render_workflow_js`,填 `rendered`;`write_file=true` 时写盘(复用 `store` 的 atomic temp+rename 模式,扩展名 `.workflow.js`)。
- `import`:`parse_workflow_js(source)` → `to_def()` → `validate()`;填 `definition` + `dropped`;`save=true` 时 `store::save`。

`examples()` 增补 export/import 两条;工具 `DESCRIPTION` 末尾追加一句说明 export/import。

> 注:`WorkflowTool` 现有字段(`coord_store` / `dispatch_signal`)足够,export/import 不需要新依赖注入 → `constructor.rs` 构造点不改。

---

## 7. 规范文档

新增 `docs/reference/WORKFLOW_INTEROP.md`:
- AWI manifest schema(§3)。
- `.workflow.js` ↔ DAG 映射表(§5)。
- 无损往返机制(内嵌 `/* @aleph-workflow … */` 块)。
- **哪些 imperative 特性故意不支持及 R7/R10 理由**。
- 挂进 `CLAUDE.md` 文档索引表。

---

## 8. 测试计划

`src/workflow/interop/*.rs` 内 `#[cfg(test)]`:

- **manifest.rs**:`from_def → to_def` 往返保内核;额外字段在 `to_def` 被丢弃;camelCase serde 形状(`dependsOn`/`whenToUse`)。
- **export.rs**:线性 def → 顺序 `await agent` 序列;菱形 def → 含 `parallel([...])`;内嵌 `/* @aleph-workflow … */` 块存在且可被 import 还原(round-trip);带 `phase` 的步骤渲染 `phase('…')`;prompt 含特殊字符的转义安全。
- **import.rs**:带内嵌块 → 无损、`dropped` 空;裸 meta → 提取 name/description/whenToUse/phases;含循环/`pipeline`/嵌套 `workflow()` → 进 `dropped`;解析后 `validate` 失败路径返回 Err。
- **workflow_tool.rs**:`export` 填 `rendered` 不填 task_ids;`import(save=false)` 填 `definition`+`dropped` 不写盘;`import(save=true)` 后 `list` 可见;往返(save → export → import → describe 相等)。复用现有 `ENV_GUARD` + TempDir `ALEPH_HOME` 模式。

验证:`cargo check -p alephcore` + `cargo test -p alephcore --lib workflow`(覆盖 `workflow::` 与 `builtin_tools::workflow_tool::`)。**绝不** `cargo fmt`。

---

## 9. 红线自检

| 红线 | 结论 |
|---|---|
| R3 核心轻量化 | 零新重依赖(serde_json 已有 + 手写扫描),不引入 JS 引擎 ✅ |
| R7 LLM 主权 | interop 纯数据变换,无意图识别/完成度判断;imperative 控制流明确拒绝 ✅ |
| R8 工具即一切 | export/import 作为 `workflow` 工具动作,自然语言可驱动 ✅ |
| R10 薄 Harness / 笨循环 | 不进 `src/harness/`;`WorkflowDef`/`compile`/dispatcher 零改动;无新调度/推理 ✅ |
| P6 KISS/YAGNI | 轻量扫描局限通过 `dropped[]` 透明,不过度造全解析器 ✅ |

---

## 10. 改动清单(预计)

- **新增**:`src/workflow/interop/{mod,manifest,export,import}.rs`
- **新增**:`docs/reference/WORKFLOW_INTEROP.md` + `CLAUDE.md` 文档索引一行
- **改**:`src/workflow/mod.rs`(挂 `pub mod interop;` + 重导出)
- **改**:`src/builtin_tools/workflow_tool.rs`(`WorkflowArgs` +2 动作、`WorkflowToolOutput` +2 字段、`call` +2 分支、`examples`/`DESCRIPTION` 增补、测试)
- **不改**:`def.rs` / `compile.rs` / `store.rs` 核心(store 可能复用其 atomic-write helper,若需要再小幅 `pub(crate)` 暴露)、`constructor.rs` / `registry.rs` / `definitions.rs` / `groups.rs`(工具已注册,新增动作走同一 enum)。

## 11. 明确不做(YAGNI / 后续 PR)

- gateway RPC `workflow.export/import`(R4/R6 I/O 表面)——延后,需要时镜像 `gateway/handlers/teams.rs`。
- CLI `aleph workflow export/import`——延后。
- Panel UI——延后。
- 完整 JS-子集解析器——拒绝(R3);裸文件解析的覆盖面以 `dropped[]` 透明界定。
