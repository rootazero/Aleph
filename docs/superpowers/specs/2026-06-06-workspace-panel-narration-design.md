# 设计:让工作区面板回显出现"边做边说"的 LLM 旁白

- **日期**: 2026-06-06
- **状态**: 已批准,待实现
- **作者**: AI (brainstorming session)

## 问题

Aleph chat 窗口的工作区面板(右侧 `WorkspacePanel`)流式回显里**全是工具调用**——
`skill_list COMPLETED·110ms` / `#1` / `ctx_search COMPLETED·63ms` / `#2` …——
**看不到任何 LLM 自然语言反馈**。

对照基线 `/Volumes/TBU4/goal.md`(一段 Claude Code transcript):Claude 每发一批工具前
都先写一句话说明在干什么、为什么(「先定位日志位置…」「噪音太多。聚焦 harness 循环模块本身」
「关键证据出现了」),并在拿到关键证据时做阶段性小结、任务收尾时给 `※ recap`。
用户要的就是这种"边做边说"的条理感。

## 根因(已取证)

回显缺旁白**不是 UI 渲染 bug,而是模型在中间轮次没产出文本**。客户端渲染链路早已完整:

```
模型每轮文本 → think.rs:920 emit TextEmitted{Final, text}
            → events.rs:101 "text_emitted"
            → chat.set_step_text(run_id, iteration, text)
            → ChatMessage.content
            → workspace_panel.rs:44 timeline_groups() narration
            → workspace_panel.rs:159 StepCard 渲染 {narration}
```

`think.rs:920` 对**每个 iteration** 都会 emit `TextEmitted`——只要那一轮模型产出了文本。
所以唯一缺的环节是:**模型在中间轮次只发工具调用、文本为空**。

三个 prompt/guard 因素合谋压制了旁白:

1. **`src/thinker/layers/guidelines.rs` 规则 17** 明令「Narrate only when it earns its place
   …do not prefix every call with "now I'll…"」——直接禁止用户现在想要的逐步旁白。
2. **`src/verification/tool_loop_verifier.rs:132-138`** 的 `has_text` 放行:那一轮只要有思考
   文本就 `Continue`。于是"有旁白"反而成了躲过"同工具空转"检测的手段——这正是规则 17 当初
   存在的理由(记忆 `project_agent_convergence_guideline` #15:A 股 run 靠旁白躲过 guard)。
3. (次要,仅 Google/Gemini 系生效) **`src/thinker/layers/provider_guidance.rs:180`**
   `GOOGLE_OPERATIONAL_DIRECTIVES` 的「Actions and results beat narration」。

## 关键约束:旁白 vs 防空转的真实矛盾

`tool_loop_verifier` 的 `trailing_repeat_run`(`tool_loop_verifier.rs:89-98`)只把
**`name` 且 `args_hash` 完全相同**的连发计入重复 run。因此:

- **合法变化探索**(同工具不同参数,如 `file_read` 读 10 个不同 path → `args_hash` 不同)
  天然 run=1,**本来就不会触发** guard。goal.md 里 Claude Code 的合法探索正是这种"变化工具/
  变化参数",与 guard 无冲突。
- **病态循环**(逐字相同的 `tool`+`args` 刷 N 次)才是 guard 的目标,而这种循环
  **无论有无旁白都是空转**。

结论:`has_text` 放行对"逐字相同调用"的检测是**多余且有害**的——它是唯一让这种循环在
有旁白时蒙混过关的口子。删掉它对合法探索零误伤(被 `args_hash` 相等条件挡在外面),
却堵上了规则 17 当初要补的洞,从而让"鼓励旁白"变得安全。

## 修复(四块,互相印证)

### ① Prompt 主杠杆 —— 改写规则 17:禁止 → 规定

文件:`src/thinker/layers/guidelines.rs`(GuidelinesLayer,priority 1300)

把规则 17 从"少旁白"反转为"实质性旁白":

- 每发一个工具(或一批)前,先写一句自然语言说明**做什么 + 为什么**。
- 拿到关键证据时用一句话小结(对齐 goal.md 的阶段性证据梳理)。
- 收敛/收尾时给一句 recap。
- 对齐 goal.md 里 Claude Code 的 `⏺` 节奏:旁白以"步骤"为粒度,不是每个工具一条流水账。

**护栏必须保留**:旁白必须携带发现 / 决策 / 理由;空喊"现在我去做 X"却始终不产出 X
仍是规则 15 的反模式(保留对规则 15 的交叉引用)——让"交付的结果",而非"宣告",成为进度。

同步更新该文件 `#[cfg(test)]` 里对规则 17 文案的断言。

### ② Guard 硬化 —— 移除 `has_text` 放行

文件:`src/verification/tool_loop_verifier.rs:132-138`

删除 `has_text` 早退分支。删除后:逐字相同的 `tool`+`args` 连发达到阈值 →
**即使每轮都有旁白也照样 veto / halt**。

- 因 `trailing_repeat_run` 已要求 `name`+`args_hash` 完全相同,合法变化探索零误伤。
- 同步更新文件头注释(删去检测前提里「current turn's `final_text` is empty/None」那一条)。
- 同步更新测试 `src/verification/tests/tool_loop_verifier.rs`:断言"逐字相同循环 + 有旁白"
  现在产生 Veto/Halt(而非旧的 Continue);"变化参数"仍 `Continue`。

### ③ UI 润色 —— 旁白升为步骤卡主角

文件:`interfaces/webchat/src/components/workspace_panel.rs`(`StepCard`,~114-176)

- 旁白样式从 `text-xs text-text-secondary`(小号次要灰)提升为
  `text-sm text-text-primary`,并按 **markdown** 渲染(与聊天气泡 `MessageBubble` 一致,
  让加粗 / 列表 / recap 行排版正确)。
- 版面顺序:`#iteration` 标签 → 旁白(主线)→ 工具列表(明细)。旁白成为"这一步在干什么"
  的视觉主角,工具行退为支撑细节。
- **不动数据链路**(链路已完整,见根因)。

### ④ provider_guidance.rs 轻改(Google/Gemini 系)

文件:`src/thinker/layers/provider_guidance.rs:179-180`

将「Conciseness: keep explanatory text brief — a few sentences, not paragraphs.
**Actions and results beat narration.**」中压制旁白的半句轻改为不与新方向冲突的措辞:
保留"brief / 不写长段落"的本意,但去掉"results beat narration"这种"别旁白"的暗示,
改为鼓励"简短但实质的步骤旁白"。仅此一句,family 微调,不扩大改动面。

## 红线自检

- **R10(薄 Harness)**:`tool_loop_verifier` 位于 `src/verification/`(12 模块法定归属),
  **非** `src/harness/` —— harness 零增行;硬化属循环正确性脚手架,非认知。✓
- **R7 / R9(LLM 主权 / 智慧在 prompt)**:旁白是 prompt 驱动的**模型行为**,
  不是确定性代码伪造旁白文本。✓
- **P3 / 外科手术**:数据链路不动,只反转一条规则(17)、删一个放行(has_text)、
  提一处样式(StepCard)、轻改一句(Google 系)。每行改动都直接追溯到本需求。✓

## 测试 / 验证计划

1. `cargo test -p alephcore --lib`:
   - `guidelines.rs` 单测断言新规则 17 文案。
   - `tool_loop_verifier` 测试:逐字相同循环(含旁白)→ Veto/Halt;变化参数 → Continue。
2. `cargo check -p alephcore --bin aleph-server` 干净。
3. wasm32 panel 构建(`just wasm` 或 `cargo build -p ... --target wasm32-...`)通过。
4. Live e2e(手动):重发一个多步真实任务(如 goal.md 那类"搜索+做报告"),确认:
   - 工作区面板**每步出现旁白**,markdown 渲染正常;
   - 对一个逐字相同调用的人造循环,guard 仍能 veto→halt。
5. 部署生效需走 CLAUDE.md 的 Panel↔Daemon 嵌入链:`just wasm` → 重编 binary → 热替换
   → supervisor relaunch(仅本地验证用,是否部署由用户决定)。

## 不做(超出本次范围)

- 不改 `text_emitted` / `TextEmitted` 协议结构(链路已完整)。
- 不改聊天气泡(左栏)渲染——它本就显示旁白;本次只补工作区面板(右栏)。
- 不引入新的确定性"旁白生成"代码(违反 R7)。
- 不做 recap 的特殊结构化事件——recap 就是模型产出的普通文本,作为旁白渲染即可。
