# Workflow Interop — `.workflow.js` 双向互换

Aleph 的 `workflow` 子系统(声明式静态 DAG)与 Claude Code `.workflow.js` 工程文件
(命令式编排脚本)之间的兼容桥。实现见 `src/workflow/interop/`——`manifest.rs`(AWI 类型 + `validate`)、
`export.rs`(渲染 + `partial_fan_in_notes` 有损披露)、`consts.rs`(有界 JS 数据字面量归一化器)、
`import/{mod,lexer,opts,scan}.rs`(2026-09-03 从单文件 `import.rs` 拆分:`lexer` 读字面量、`opts` 读 agent opts、
`scan` 出 `ScanEvent`、`mod` 保留 `parse_workflow_js`/`extract_embedded`/`scan_bare`;纯搬运,只用 `pub(super)`)。

## 设计原则

- **声明式 manifest (AWI) 是唯一真相**。`.workflow.js` 是它的渲染视图。
- **执行核心零改动**:`WorkflowDef` / `compile` / dispatcher 不变;额外元数据不入执行
  schema(避免死配置,R10)。
- **无 JS 引擎**(R3):导入裸 `.workflow.js` 用手写轻量扫描;schema 用 `consts.rs` 的
  **有界数据字面量归一化器**(只认纯数据形状、遇表达式即弃权,从不求值/插值)。无法映射
  的命令式构造与未解析 schema、动态 prompt 皆经 `dropped[]` 透明上报,绝不静默吞掉。
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
- **`tolerateFailedDeps`(2026-09-03 起,bare literal opt)**:同族的第四个可执行扩展——
  置 `true` 的步骤在它**直接依赖**的上游 `Failed`/`Cancelled` 时仍然运行,而不是留在
  `Unsatisfiable`(用于 synthesis / 报告 / 收尾这类少一个输入仍有活干的步骤)。进 `WorkflowDef`
  (`tolerate_failed_deps`,别名接受 snake_case),`materialize` **只在置位时**盖
  `TOLERATE_FAILED_DEPS_METADATA_KEY`(未置位的行字节不变);export 渲染 `tolerateFailedDeps: true`,
  import 经 `opts.rs::assign_bare_opt` 还原,`false` 不上 wire。`validate()` 拒绝 clarify 步骤带它
  (没有 agent run 可以容错)。语义与任务存储侧的判定见
  [MULTI_AGENT_SYSTEM.md](MULTI_AGENT_SYSTEM.md) 的 *Tolerant fan-in*。
- 顶层 manifest 与步骤**都**带 `deny_unknown_fields`(2026-09-03 补齐顶层):此前同一个类型的两半对
  「遇到不认识的键怎么办」给了两个答案——步骤上的错键响亮拒绝,顶层的 `whenToUsed` 静默丢成
  「没有 whenToUse」。store 是单写者、无历史 fixture,所以响亮拒绝是安全方向(P7 fail-closed);
  `WorkflowPhase` / `WorkflowStepDef` 刻意不动。
- 只有 `name` / `description` / `steps{id,agent,prompt,dependsOn,review,timeoutSecs,maxRetries,tolerateFailedDeps}`
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
| `opts.{review,requireGrounding,timeoutSecs,maxRetries,tolerateFailedDeps}`(bare literal) | ↔ 无损(bare 路径亦对称) | **可执行核心**:进 `WorkflowDef`,materialize 盖任务元数据。`requireGrounding`(2026-08-03)让声明式路径也能要求复审**碰一次现实**——`workflow_step_review.approve` 缺 `grounding` 即 bounce,证据词表与 loop_graph 锚点同一套(exit_code/numeric/line_count) |
| `const NAME_SCHEMA = { … }` + `schema: NAME_SCHEMA` | → 导入解析 | 工程格式把 schema hoist 成顶层 `const` 再按名引用;裸扫描经 `interop/consts.rs` 的**有界数据字面量归一化器**解析 hoisted const(及 inline schema),把 JS-lax 写法(裸键 / 单引号 / 尾逗号)归一为 JSON;遇任何表达式值(标识符 / 函数调用 / 模板串 / 计算键)整个 schema **弃权**并记 `dropped`(R3:非 JS 引擎,只认纯数据) |
| `agent(buildPrompt(u))` / `agent(promptVar)` 动态 prompt | → 计数入 `dropped` | 非字面量 prompt 不可静态导入(R7/R10);裸扫描计数并报 "N agent()/clarify() call(s) with dynamic prompts not imported",全动态时空步骤错误也带计数 |
| `pipeline(items, s1, s2)` | → 导入近似 | 运行时 item 列表未知 → 记 `dropped`;导出不生成 |
| `TARGETS.forEach(() => agent(…))` / `items.map(…)` 数组扇出 | → 记 `dropped` | **2026-09-03 补**:`contains_call_like_keyword("for", …)` 的前边界规则刻意拒 `forEach(`(防 `iffy(`/`switcher` 误报),而动态 prompt 计数器只抓非字面量 prompt——于是**字面量 prompt 的扇出**(`TARGETS.forEach(() => agent("audit this target"))`)曾导入成**一个**步骤且 `dropped: []`,即一次被报告为无损的 N 路塌缩。现由一根独立的、带前导 `.` 的 needle(**不进**控制流关键字表,以免破坏前边界规则)推一条 `array fan-out (.forEach/.map) — runtime item list not statically known`。**刻意吵**:一个用来打日志的 `.map(` 也会触发它 |
| 循环 / 条件 / `budget` / 嵌套 `workflow()` | ✗ 故意不支持 | 导入记 `dropped`(R7/R10) |

## ⚠️ 保真度的三个前提（2026-08-03 补，此前只有前两条是真的）

上表说的"无损"依赖三件事，其中两件曾经不成立：

1. **`save` 不得剥掉 extras**。`WorkflowManifest::from_def` 按定义清空 `whenToUse`/`phases` 与每步的
   `model`/`effort`/`label`/`phase`/`schema`/`isolation`/`agentType`，而 `store::save` 是纯 replace ——
   所以 `import(save=true)` → `describe` → 改一行 → `save` 会**销毁 per-step 的 model / effort**，
   而这两个是**可执行**的（`run` 读它们盖 `WORKFLOW_MODEL_KEY`/`WORKFLOW_EFFORT_KEY`）。更糟的是
   import 自己的 `dropped` 提示就是在教用户这么做（"先 edit + save 把 agent 换成真实成员"）。
   现 `save` 走**读-改-写**：`WorkflowManifest::with_core_from(&def)` 按 step id 保留 extras。
   **推论**：任何新增的"只承载核心"的写入面，都要问一句「它会不会覆盖掉核心表达不了的东西」。
2. **裸路径必须先剥注释**。`scan_events` / `strip_string_literals` / `collect_consts` 三个扫描器
   原本只认引号：`// don't forget…` 里的撇号开出幽灵字符串，吞掉其后整份文件（用户看到的是
   "no agent() calls found"）；`// await agent('old pass')` 则被当成活步骤导入。现 `blank_comments`
   一趟前置（字符串感知、保留换行）。注意顺序：它只在裸路径跑，因为 `@aleph-workflow` 内嵌头
   本身就是块注释，`extract_embedded` 必须先拿到机会。
3. **`meta` 从解析出来的对象读，不要 grep 原文**。`scan_meta_field` 原本在整份原文里找第一个
   `"<field>:"`，于是工程格式自己的惯例（schema const 提到 meta 之前）会让 `name:` 落到 schema
   属性上、整个 import 被拒；反向是 schema 里的 `description:` 冒名顶替。现优先读
   `collect_consts` 已解析的 `meta`，仅当它不是纯数据字面量时才回落旧扫描。

另两条边界订正：`WorkflowManifestStep` 现接受 `timeout_seconds` / `max_retries` / `require_grounding`
别名（`describe` 返回的 `definition` 直接喂回 `import` 是文档明许的入口，此前会**无声**丢掉这几个
可执行字段）；带引号的 opts 键不再让整个 opts 对象弃权（那会连 `review` 安全门一起丢），
`timeoutSecs: 0` 也不再被裸路径静默改写成"用全局默认"——由共享 `validate()` 出唯一那句错误。

## 无损往返机制

导出在文件首行写入:

```js
/* @aleph-workflow {<完整 manifest 的单行 JSON>} */
```

导入优先读此块 → 精确还原(`dropped` 为空)。无此块的裸文件则走轻量扫描:提取
`meta.{name,description,whenToUse,phases}` + 各 `agent()` 的 prompt 实参为步骤(拓扑层),
并把识别到的命令式构造写入 `dropped`。

⚠️ **裸路径的边重建是有损的,而 export 现在自陈这件事**(2026-09-03):body 只把拓扑**层**渲染成
`parallel([...])`,裸扫描能反推的唯一边规则是「依赖整个前一层」。凡有步骤的真实 `dependsOn` 是前一层的
**真子集**(partial fan-in),或有边**跨过**前一层(skip edge),`export::partial_fan_in_notes` 就逐个点名它们——
文件里落一段 `//` 披露块,`workflow(action='export')` 的**工具消息**复述同一句(同一个谓词、两张脸、一次派生)。
结论一句:**要无损重进来就别剥掉 `/* @aleph-workflow … */` 头**。

裸扫描的保真处理:

- **多行 prompt 数组**:`agent()` 实参支持两种声明式形态——单行字面量,或
  `[ "l1","l2" ].join("sep")` 数组(工程格式的主力惯用法);后者把各字符串元素按
  `.join` 分隔符拼回。元素含标识符(如 `GROUND_TRUTH`)或拼接(`'a' + x`)即视为动态,
  整条 `agent()` 弃权不导入(R7/R10),与 `.map(...)` 一样属"故意不静态化"。导出端对
  含 `\n` 的 prompt 也按此惯用法渲染,故导出/导入即便脱去内嵌头亦完全对称。
  **动态 prompt 不再静默丢失**:非字面量 prompt 的 `agent()`/`clarify()` 由 `scan_events`
  记 `DynamicPrompt`,`scan_bare` 计数并入 `dropped`(全动态→空步骤时错误消息也带计数,P7)。
- **schema 常量与 JS-lax 归一化**(2026-07-25):工程文件把 schema hoist 成顶层
  `const NAME_SCHEMA = { … }` 再 `schema: NAME_SCHEMA` 引用,且写成 JS 对象字面量
  (裸键 / 单引号 / 尾逗号)——非合法 JSON。`interop/consts.rs` 的**有界数据字面量
  归一化器** `parse_js_data` 只认 `object/array/string/number/bool/null` 纯数据形状,
  把 JS-lax 归一为 `serde_json::Value`,inline schema 与 hoisted-const 引用两路皆解析
  (`collect_consts` 建顶层 const 符号表,字符串感知、首声明胜)。**遇任何表达式值
  (标识符 / 函数调用 / 模板串 / 计算键)整个 schema 弃权**并记 `dropped`——是归一化器
  不是 JS 引擎(R3):从不求值、不插值、不解析成员访问。未解析 / 非数据 schema 皆经
  `AgentOpts::schema_dropped` 上浮 `dropped`,绝不静默丢。JSON 是被接受语法的子集,故
  Aleph 自导出的 inline-JSON schema 在裸路径原样回读。
- **命令式 needle 只扫代码骨架**:检测 `for`/`if`/`pipeline(`/`parallel(` 等构造前,
  先剥离所有字符串字面量内容(保留引号定界符),因此 prompt 文本里的
  "search **for** files" 不会误报成 `for` 循环。
- **`meta.phases` 是 phase 计划的权威**(2026-09-03):`scan_bare` 此前只从 body 的 `phase()` 标记重建
  相位(`detail: ""`、`model: None`),完全未被引用的条目整条消失,而 `dropped` 一个字不说。现
  `import/mod.rs::phase_plan(meta_obj, markers)`——解析出来的 `meta.phases` 说了算(title/detail/model),
  body 标记里它没声明的追加在后(手写文件可以只有 `phase()` 没有 `meta.phases`);`meta` 不是纯数据
  字面量时仍以标记为唯一来源。这就是上文第 3 条「meta 权威,**包括对它没说的那些键**」的逐源规则
  应用到 phases。
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
- ~~**`meta.phases` 的 `detail`/`model` 在 header-stripped 往返中丢失**~~ —— **已修(2026-09-03)**:
  `phase_plan` 让解析出来的 `meta.phases` 成为 phase 计划的权威,body 标记只追加它没声明的标题
  (见上文「无损往返机制」)。
- **裸路径的 DAG 重建仍会造出并不存在的扇入边,但它不再是静默的**(2026-08-03 发现,2026-09-03
  **改为自陈**,边行为本身未变)——export 只把拓扑**层**编码成 `await parallel([...])`,不编码单条边;
  import 反过来让每一步依赖**整个**前一层,于是 `a`/`b` 独立、`c` 只依赖 `a` 的模板重进来 `c` 也等 `b`
  (`b` 一失败 `c` 就 `Unsatisfiable`)。此前 `dropped: []` 且 import 里那句注释还写着这是 export 的
  "exact inverse"。现 `export::partial_fan_in_notes` 在**文件的 `//` 块**与**工具的 export 消息**里同时
  点名受影响的步骤(partial fan-in 与 skip edge 两种形状),注释订正,**并明说 `@aleph-workflow` 头是
  无损重导入的前提**。真正的正解仍是让 body 渲染真实 `dependsOn`(bare opts,与 `review`/`timeoutSecs`
  已有的做法一致)并顺带修掉 step id 重新编号——那是 **wire 格式变更**,值得独立一轮,本轮刻意不做。
