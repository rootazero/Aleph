# Workflow Interop — `.workflow.js` 双向互换

Aleph 的 `workflow` 子系统(声明式静态 DAG)与 Claude Code `.workflow.js` 工程文件
(命令式编排脚本)之间的兼容桥。实现见 `src/workflow/interop/`。

## 设计原则

- **声明式 manifest (AWI) 是唯一真相**。`.workflow.js` 是它的渲染视图。
- **执行核心零改动**:`WorkflowDef` / `compile` / dispatcher 不变;额外元数据不入执行
  schema(避免死配置,R10)。
- **无 JS 引擎**(R3):导入裸 `.workflow.js` 用手写轻量扫描;无法映射的命令式构造经
  `dropped[]` 透明上报,绝不静默吞掉。
- **命令式控制流故意不支持**(R7/R10):routing / 循环 / 条件 / evaluator-optimizer
  属 Think→Act 主循环的职责,不外接到这一声明式层。

## AWI manifest schema

JSON,camelCase 键(贴合 `.workflow.js` 的 `meta`)。

```jsonc
{
  "name": "research-report",
  "description": "...",
  "whenToUse": "...",
  "phases": [{ "title": "Gather", "detail": "...", "model": "opus" }],
  "steps": [
    {
      "id": "gather",
      "agent": "researcher",
      "prompt": "research {input}",
      "dependsOn": [],
      "label": "audit:gather",
      "model": "haiku",
      "phase": "Gather",
      "schema": { "type": "object" },
      "isolation": "worktree",
      "agentType": "Explore",
      "effort": "high"
    }
  ]
}
```

- 必填:顶层 `name` / `steps`;步骤 `id` / `agent` / `prompt`。
- 可选:`description` / `whenToUse` / `phases`(条目可选 `model` 相位级模型覆盖,
  interchange-only);步骤 `dependsOn`(缺省 `[]`)/ `label` / `phase` / `schema` /
  `isolation` / `agentType`(原样透传,Aleph 不解释、不执行,仅为忠实导出
  `.workflow.js`)。
- **步骤 `model` 与 `effort` 是可执行覆盖**(2026-07-24 起 `effort` 兑现旧文
  "留作后续 PR"):两者都**不进** `WorkflowDef`,而在 `run` 时由 workflow 工具从
  manifest 取出、经 `materialize` 盖进任务元数据(`WORKFLOW_MODEL_KEY` /
  `WORKFLOW_EFFORT_KEY`,byte-identical-when-absent 同款盖章模式),dispatcher
  转成成员 run 的 `model_override` / `think_level`。`effort` 取值
  `low`/`medium`/`high`/`xhigh`/`max`(贴合动态 workflow 的 `agent(..,{effort})`),
  经活表 `normalize_think_level` 归一——`max`≡High 与全仓一致(不为 workflow
  fork 档位词表);`validate()` 在 save/import 边界拒未知 effort 值。
- **可执行扩展(2026-07-16 起)**:步骤 `review`(lead 审查门)/ `timeoutSecs`(每步运行
  超时秒)/ `maxRetries`(每步重试上限,`0`=首败即终)——三者进 `WorkflowDef` 可执行核心,
  materialize 时盖进任务元数据由 dispatcher 现有消费者执行;`.workflow.js` 侧渲染/解析为
  agent() 的 bare-literal opts(非字符串),header-stripped 的 bare 路径同样往返。
- 只有 `name` / `description` / `steps{id,agent,prompt,dependsOn,review,timeoutSecs,maxRetries}`
  映射进 `WorkflowDef`;其余字段存 manifest(导出时进 `.workflow.js` 头部内嵌块;
  `model`/`effort` 另在 run 时盖任务元数据,见上)。

## `.workflow.js` ↔ DAG 映射

| Claude Code 构造 | 方向 | 说明 |
|---|---|---|
| `meta.{name,description,whenToUse,phases}`(含 `phase.model`) | ↔ 无损 | manifest 顶层 |
| 顺序 `await agent()` 链 | ↔ | 线性 `dependsOn` 链 |
| `agent("prompt")` 单行 prompt | ↔ 无损 | 字符串字面量 ↔ `prompt` |
| `agent([ "l1","l2" ].join("\n"))` 多行 prompt | ↔ 无损 | 工程格式签名惯用法:导出按 `\n` 拆行渲染数组,导入按 `.join` 分隔符还原(转义正确解码),裸路径亦对称 |
| `parallel([agent, agent])` | ↔ | 同拓扑层、彼此无 `dependsOn` 的兄弟步骤 |
| `agent()` fan-in | ↔ | 一步 `dependsOn` 多个上游 |
| `opts.{label,model,phase,schema,isolation,agentType,effort}` | ↔ 无损(经内嵌块 + bare 路径) | 存 manifest,不入 `WorkflowDef`;其中 `model`/`effort` 在 `run` 时盖任务元数据成为**可执行覆盖**(见上);`effort` 亦渲染为 bare-scan 可还原的 `effort: "…"` |
| `opts.{review,timeoutSecs,maxRetries}`(bare literal) | ↔ 无损(bare 路径亦对称) | **可执行核心**:进 `WorkflowDef`,materialize 盖任务元数据 |
| `pipeline(items, s1, s2)` | → 导入近似 | 运行时 item 列表未知 → 记 `dropped`;导出不生成 |
| 循环 / 条件 / `budget` / 嵌套 `workflow()` | ✗ 故意不支持 | 导入记 `dropped`(R7/R10) |

## 无损往返机制

导出在文件首行写入:

```js
/* @aleph-workflow {<完整 manifest 的单行 JSON>} */
```

导入优先读此块 → 精确还原(`dropped` 为空)。无此块的裸文件则走轻量扫描:提取
`meta.{name,description,whenToUse}` + 各 `agent()` 的 prompt 实参为步骤(线性链),
并把识别到的命令式构造写入 `dropped`。

裸扫描的三点保真处理:

- **多行 prompt 数组**:`agent()` 实参支持两种声明式形态——单行字面量,或
  `[ "l1","l2" ].join("sep")` 数组(工程格式的主力惯用法);后者把各字符串元素按
  `.join` 分隔符拼回。元素含标识符(如 `GROUND_TRUTH`)或拼接(`'a' + x`)即视为动态,
  整条 `agent()` 弃权不导入(R7/R10),与 `.map(...)` 一样属"故意不静态化"。导出端对
  含 `\n` 的 prompt 也按此惯用法渲染,故导出/导入即便脱去内嵌头亦完全对称。
- **命令式 needle 只扫代码骨架**:检测 `for`/`if`/`pipeline(`/`parallel(` 等构造前,
  先剥离所有字符串字面量内容(保留引号定界符),因此 prompt 文本里的
  "search **for** files" 不会误报成 `for` 循环。
- **导入校验失败保留 `dropped` 诊断**:若扫描结果丢弃了命令式构造且随后 `validate()`
  失败,错误信息会并入被丢弃的构造清单,而非静默吞掉——用户能看到导入是有损的。

## 工具用法(R8)

```
workflow(action='export', name='research-report')                 # 渲染为动态 workflow 文本
workflow(action='export', name='research-report', write_file=true) # 同时写盘
workflow(action='import', source='<.mjs / .workflow.js 或 manifest JSON>', save=true)
```

- `export` 输出填 `rendered`(渲染文本);`write_file=true` 时写
  `$ALEPH_HOME/workflows/<name>.mjs`——`.mjs` 是 Claude Code workflow 菜单 /
  `~/.claude/workflows` 加载器识别的动态 workflow 扩展名(参考工程文件即 `*.mjs`);
  渲染正文与内嵌头不变,只是落盘扩展名从 Aleph 旧的 `.workflow.js` 迁到 `.mjs`。
- `import` 接受 `.mjs` / `.js` / `.workflow.js` / manifest JSON 任意文本(`source` 是裸文本而非路径,
  无目录 glob 依赖 → 扩展名迁移非破坏);输出填 `definition`(解析出的 `WorkflowDef`)+
  `dropped`(被丢弃的命令式构造);`save=true` 时入库。

## 明确不做(YAGNI / 后续 PR)

- gateway RPC `workflow.export/import`(R4/R6 I/O 表面)。
- CLI `aleph workflow export/import`。
- Panel UI。
- 完整 JS-子集解析器(R3 拒绝;裸文件覆盖面以 `dropped[]` 透明界定)。
