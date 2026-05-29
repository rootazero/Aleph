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
  "phases": [{ "title": "Gather", "detail": "..." }],
  "steps": [
    {
      "id": "gather",
      "agent": "researcher",
      "prompt": "research {input}",
      "dependsOn": [],
      "label": "audit:gather",
      "model": "haiku",
      "phase": "Gather",
      "schema": { "type": "object" }
    }
  ]
}
```

- 必填:顶层 `name` / `steps`;步骤 `id` / `agent` / `prompt`。
- 可选:`description` / `whenToUse` / `phases`;步骤 `dependsOn`(缺省 `[]`)/ `label` /
  `model` / `phase` / `schema`(原样透传,Aleph 不解释)。
- 只有 `name` / `description` / `steps{id,agent,prompt,dependsOn}` 映射进 `WorkflowDef`;
  其余字段仅在导出的 `.workflow.js` 头部内嵌块中保留。

## `.workflow.js` ↔ DAG 映射

| Claude Code 构造 | 方向 | 说明 |
|---|---|---|
| `meta.{name,description,whenToUse,phases}` | ↔ 无损 | manifest 顶层 |
| 顺序 `await agent()` 链 | ↔ | 线性 `dependsOn` 链 |
| `parallel([agent, agent])` | ↔ | 同拓扑层、彼此无 `dependsOn` 的兄弟步骤 |
| `agent()` fan-in | ↔ | 一步 `dependsOn` 多个上游 |
| `opts.{label,model,phase,schema}` | ↔ 无损(经内嵌块) | 存 manifest,不入 `WorkflowDef` |
| `pipeline(items, s1, s2)` | → 导入近似 | 运行时 item 列表未知 → 记 `dropped`;导出不生成 |
| 循环 / 条件 / `budget` / 嵌套 `workflow()` | ✗ 故意不支持 | 导入记 `dropped`(R7/R10) |

## 无损往返机制

导出在文件首行写入:

```js
/* @aleph-workflow {<完整 manifest 的单行 JSON>} */
```

导入优先读此块 → 精确还原(`dropped` 为空)。无此块的裸文件则走轻量扫描:提取
`meta.{name,description,whenToUse}` + 各 `agent()` 的首个字符串字面量为步骤(线性链),
并把识别到的命令式构造写入 `dropped`。

## 工具用法(R8)

```
workflow(action='export', name='research-report')                 # 渲染为 .workflow.js 文本
workflow(action='export', name='research-report', write_file=true) # 同时写盘
workflow(action='import', source='<.workflow.js 或 manifest JSON>', save=true)
```

- `export` 输出填 `rendered`(渲染文本);`write_file=true` 时写
  `$ALEPH_HOME/workflows/<name>.workflow.js`。
- `import` 输出填 `definition`(解析出的 `WorkflowDef`)+ `dropped`(被丢弃的命令式构造);
  `save=true` 时入库。

## 明确不做(YAGNI / 后续 PR)

- gateway RPC `workflow.export/import`(R4/R6 I/O 表面)。
- CLI `aleph workflow export/import`。
- Panel UI。
- 完整 JS-子集解析器(R3 拒绝;裸文件覆盖面以 `dropped[]` 透明界定)。
