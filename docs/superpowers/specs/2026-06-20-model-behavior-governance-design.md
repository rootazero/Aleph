# Model Behavior Governance — 模型行为治理（连线 + 启发 + 回退 + 默认预设 一体）

> Design spec · 2026-06-20 · sub-project #1 of the harness-layer 深度治理 series
>
> 目标：让 harness 真正**管控**得住不听话的模型、又能优雅**回退收手**不把弱模型管死、还能**启发**模型榨取其能力——三者全部活在 prompt 与结构化脚手架里，零循环认知，严守 R3/R7/R9/R10。

---

## 0. 背景与三目标的 R10 自洽

用户的诉求横跨整个 harness 层（重构/增强/打磨/修 bug/连线），对单个 spec 太大，已切分为子项目。**本 spec = 子项目 #1「模型行为治理」**，覆盖三个目标中的全部三个，但只取**连线/修 bug/启发文案 + Kimi/Minimax 默认预设**这一刀（不含能力级 failover、不含大文件拆分、不含防御性输出修复——见 §7 边界）。

| 用户目标 | R10-自洽的落点 |
|---|---|
| 管控不听话的模型 | 结构化看门狗（已有 verifier/ToolLoop，脚手架）+ **per-model prompt 教练**（R9 智慧在 prompt） |
| 充分回退收手、别把弱模型管死 | grace turn / 软着陆 / 部分交付（已有大半，本轮补缺口） |
| 启发、榨取模型能力 | **按能力的 prompt 启发文案**（L2），赋能而非强制（R7） |

关键论点：「更强管控 + 充分回退 + 启发」与「薄 harness、笨循环、信任模型」**不冲突**——只要三件事都在 prompt 和结构化计数里，不进循环做语义判断。用户自己说的「不仅硬管，还要启发」恰指向同一条路。

---

## 1. 纠正后的问题框架（亲读码纠正了勘察子代理的一处错判）

**错判**：勘察子代理报告称「per-family 教练从未到达模型，`src/thinker/layers/` 无 `ModelBehaviorLayer`」。**实际不然**——存在**两套互不相通的 per-family 教练系统，key 在不同标识上，且都看不见弱 OSS 模型**：

| | 活的路径 | 死的路径 |
|---|---|---|
| 谁 | `ProviderGuidanceLayer`（`src/thinker/layers/provider_guidance.rs`, prio 810） | `model_behaviors/*.md` + `load_model_behavior` |
| 内容 | 硬编码 Rust const（`TOOL_USE_ENFORCEMENT` / `TOOL_PERSISTENCE_DOCTRINE` / `OPENAI_EXECUTION_DISCIPLINE_TAIL` / `GOOGLE_OPERATIONAL_DIRECTIVES`） | markdown，`~/.aleph/model_behaviors/{name}.md` 可覆盖 |
| key | 原始 `provider_protocol`（anthropic/openai/gemini/ollama/custom） | 解析后的 behavior name |
| 状态 | ✅ 到达模型 | ❌ `run_loop/inner.rs` 加载 → `info!` log → **直接丢弃** |

由此推出 **WS1 的 5 个真实缺口**：

1. **键错靶（核心 bug）**：教练层 key 在原始 `protocol` 上，而 `model_behavior_override()` 与（新的）model-id 档**只**驱动 robustness 阈值、**steer 不到教练层**。结果：Kimi-over-openai 拿到与 GPT-4 **一字不差**的教练——"管控不听话模型"没打中靶心。
   - **更糟的真实场景**（用户反馈）：Kimi / Minimax **同时支持 openai 与 anthropic 协议**（不同协议走不同域名后缀），而用户**习惯配 anthropic 协议**。于是 Kimi-over-anthropic 当前会被 `protocol_to_behavior("anthropic") → "anthropic"` 归类为"强指令跟随者"，拿到**最松的缰绳**（Claude 档：`steer_max 12`、`novelty_min 0.35`、几乎无教练）——与弱模型实际所需**完全相反**。这正是必须按 **model id**（而非协议）识别的铁证。
2. **无 model-id 档**：共用 openai/anthropic 协议的弱 OSS 模型（Kimi/Minimax/DeepSeek/Qwen/GLM）既无法在教练上、也无法在 robustness 上与 GPT/Claude 区分（按协议各归 `openai`/`anthropic` 档）。
3. **死的 .md 重复体**：同一概念两个真相源，一个还是 loaded-then-discarded（`src/gateway/execution_engine/run_loop/inner.rs` ~543-554）。
4. **缺 L2 启发内容**：现有教练层只有"管控"（工具强制/持久化），无按能力的 thinking/规划/推理力度"启发"。
5. **解析逻辑重复**：`src/orchestrator/harness_bridge/runner_impl.rs:250`（robustness）与 `run_loop/inner.rs`（被丢弃的 content）各解析一遍同一身份。

---

## 2. 目标架构（一处收口，三处受益）

```
              ┌─ resolve_behavior(override, model_id, base_url, protocol) ─┐
  model_behavior_override()  (最高，显式 provider 配置)                       │
    → vendor_identity(model_id, base_url)  (新增，数据表 id∪域名，config 可覆盖; │
        Kimi/Minimax/DeepSeek/Qwen/GLM → "strict")                          │
    → protocol_to_behavior(protocol)  (现有兜底)                             │
    → "unknown"  (= conservative 阈值 + 非-anthropic 基线教练)                │
                  └────────────────── behavior name ─────────────────────┘
                               │                          │
                ┌──────────────┘                          └──────────────┐
        robustness_profile                          ProviderGuidanceLayer
        for_behavior(name)  (已有)                  改 key: protocol → behavior name
        + 新增 "strict" 档                          + "strict" 分支 + L2 启发内容(.md)
```

- 把 `runner_impl.rs` + `inner.rs` 两份解析**收口成一个** `resolve_behavior()`（置于 `src/orchestrator/harness_bridge/`），同一 behavior name 同时驱动 robustness 阈值与教练层 → 彻底消除键错靶。
- 教练层改成 key 在 behavior name 上 → `model_behavior_override` 与 model-id strict 档**真正 steer 教练内容**。
- 删掉 `inner.rs` 的 loaded-then-discarded 死块。

---

## 3. WS1 详细设计

### 3a. 单一解析器 `resolve_behavior`
新增 `src/orchestrator/harness_bridge/` 内的 helper（建议 `behavior_resolve.rs`）：

```text
fn resolve_behavior(override_: Option<&str>, model_id: &str, base_url: Option<&str>, protocol: &str) -> Cow<'static, str>
  优先级: override_  →  vendor_identity(model_id, base_url)  →  protocol_to_behavior(protocol)  →  "unknown"
```

- `runner_impl.rs:250`（robustness）与 `prompt_build.rs:440`（教练层 protocol 注入）均改调它。
- 删除 `run_loop/inner.rs` 的 discarded content 块。
- **vendor-identity 必须先于 protocol**：这是处理 Kimi/Minimax 双协议（用户习惯配 anthropic 协议）的关键——身份按 vendor 定，缰绳就不会被协议带偏。
- **协议与行为彻底解耦**：`protocol()` 继续管**线路/native tool_use 格式**（`supports_native_tools`、wire），`behavior name` 管**教练 + robustness 阈值**。于是 Kimi-over-anthropic 可以**既用 anthropic 原生 tool_use 线路、又拿 strict 教练 + 紧阈值**——这正是正确的分离。
- ⚠️ **待确认的连线点**：`AiProvider` trait 仅 `protocol()` / `model_behavior_override()`，**无 `model_id()` / `base_url()`**。两者在 orchestrator 请求边界（`resolved.model` + provider 配置的 endpoint）可得，从请求/配置层取，不走 provider trait。计划阶段定位精确取值点；若边界拿不到，则 vendor-identity 退化为 no-op（解析仍正确，只是 strict 档不触发），不阻塞其余部分。

### 3b. vendor-identity 表（新，数据驱动 + config 可覆盖）
`src/providers/model_behaviors/mod.rs` 新增 `vendor_identity(model_id, base_url) -> Option<&'static str>`：**小写后子串匹配机器稳定标识**（model id 或 base_url 域名，命中任一即可；P8 允许，非自然语言）。

```text
信号(model-id 子串 ∪ 域名子串)                         → behavior
moonshot.cn | moonshot | kimi                          → "strict"   # Kimi（platform.moonshot.cn）
minimaxi.com | minimax | abab                          → "strict"   # Minimax（api.minimaxi.com/anthropic，model id 常为 abab*/MiniMax-* 不自带厂名 → 必须靠域名）
deepseek                                               → "strict"   # api.deepseek.com
dashscope.aliyuncs.com | qwen | qwq                    → "strict"   # Qwen/Dashscope
open.bigmodel.cn | glm | chatglm                       → "strict"   # GLM/Zhipu
```

- **双信号的必要性**（用户反馈）：Minimax 的 model id 常是 `abab6.5*` / `MiniMax-M1`，**不含 "minimax" 子串**——只匹配 model id 会漏；域名 `minimaxi.com` 才是可靠标识。故 model id ∪ 域名，命中任一即归档。
- config `[model_behaviors]` 表（信号子串 → behavior name）合并覆盖内置；缺字段回退内置。用户可为自己的端点/模型加信号或改档。
- 保守起步（只收公认弱指令跟随族），将来按三次法则再拆 bespoke 族。

### 3c. 新增 "strict" 行为档
- `src/providers/model_behaviors/strict.md`（内置 `include_str!`）：最严管控——一次一个工具、严格 tool-call/JSON 格式提醒、显式反循环（"同一调用失败两次就换法/换源"）、禁编造、简洁；**启发极克制**（弱模型被要求"详尽规划"会适得其反，仅给"一行说计划再执行"）。
- `model_behaviors/mod.rs::builtin_behavior` + `protocol_to_behavior` 无需改（strict 不对应协议，只经 id 表/override 进入）；新增 `BUILTIN_STRICT` const。
- `ModelRobustnessProfile::for_behavior("strict")` 新增最紧阈值（≈ ollama 或更紧：`repeat_threshold 3 / steer_max 6 / novelty_min 0.6 / silence_required false`），经 `clamped()` 守窗口不变量。

### 3d. 教练层改 key + 方向 X 内容外移（数据驱动）
- `LayerInput`（`src/thinker/prompt_layer.rs`）新增 `behavior_name: Option<&str>`，`with_behavior_name[_opt]` 构造器；`PromptBuilder` 持有 `behavior_name: Option<String>`，`prompt_build.rs` 按 `resolve_behavior` 结果线程进来（复用 `provider_protocol` 现有通路 `cache.rs:76`）。
- `ProviderGuidanceLayer::inject` 改按 `behavior_name` 分派（不再读 `provider_protocol`）。
- **内容分层（保 DRY，消歧义）**：
  - **共享块保留为层内 const**（不迁 .md）：`TOOL_PERSISTENCE_DOCTRINE`（**全族含 anthropic**）+ `TOOL_USE_ENFORCEMENT`（**除 anthropic 外全族**）。这两块跨族复用，留 const 即天然 DRY、零迁移、零跨文件重复。
  - **只有 per-behavior 增量迁 `.md`**：openai tail / google directives / ollama 严格度 / strict 控制 / anthropic 极简 + 各自的 L2 启发文案。
  - 层 = 组合 `[通用持久化 const] + [工具强制 const(非 anthropic)] + [per-behavior .md delta]`。即：跨族共享 = const，**逐族变化 = 数据（.md，`~/.aleph/` 可覆盖）**。这正是方向 X 的精确边界。
- **头号回归守卫**：Kimi-over-openai 现解析成 `strict` → 拿 strict 教练（此前拿 openai 教练）。
- `.md` 内容由 boundary 预加载（`load_model_behavior` 是 async I/O），以 `PromptConfig.model_behavior_delta: Option<String>` 一类字段线程进层（层本身保持 sync）。

### 3e. L2 启发内容（"榨取"的实体；R7 原则：赋能不强制）
- **强模型（anthropic / openai / gemini）**：复杂多步任务先思考/规划；支持的模型用足 extended reasoning；执行前分解、定稿前对照目标自检。措辞用 "you may / for complex tasks…"（赋能），不死板规定步骤。
- **弱模型（ollama / strict）**：启发最小化——保持在轨：一步一动、具体、别过度思考；至多"一行说计划再执行"。
- 注：是否"支持 thinking"理想上 key 在真实能力 flag（reasoning_effort / thinking 支持），而非家族；L2 以家族为代理，计划阶段若有现成 capability flag 可低成本替换则用之。

---

## 4. WS2 详细设计（稳妥兜底集，全 R10-safe 脚手架）

### 4a. timeout / stall → best-effort grace（CRITICAL）
- 现状：`HarnessError::StalledTurn`（turn_timeout, `agent.rs:505-519`）与 stall watchdog（`agent.rs:478-492`）→ 立即 `hit_limit` 退出，**无 grace、无部分交付**。
- 改：触发时 `fire_boundary_grace_turn`，新 `GraceReason::Timeout`（含 `nudge()` 文案 `GRACE_NUDGE_TIMEOUT`：「时间预算耗尽（某步可能卡住）。停止调用工具。一条短消息说明已完成什么、还剩什么、现在能交付的部分结果。」），配**专用短 grace 预算** + 完全 cancel-safe。
- 关键性质：provider 真挂起时 grace 调用也会超时 → **fail-soft**（现有 grace 已是：warning 后静默返回，不崩循环）。**最坏 = 与今天相同（无收尾），最好 = 拿到部分交付，严格只增不减。**
- grace 预算做成 config 旋钮（合理默认，如 `min(remaining, 30s)`），同 provider。

### 4b. 软着陆提示（pre-cap warning）
- `consecutive_failure_cap` 命中**前一轮**注入软提示（镜像 `max_iterations` 的 G1 `MAX_STEPS_HINT` 模式）：「连续多轮失败——先诊断根因，否则总结并停止」。给弱模型在硬 cap 前一次自纠机会。纯结构触发（计数阈值），R10-safe。

### 4c. consecutive_failure 计数修正
- 现状（`agent.rs:556-589`）：仅"全工具失败"轮 +1，**任一成功即清零** → `fail→succeed→fail` 循环永不累积、可无界 churn。
- 改：**纯结构化**规则——"多数失败轮"（failures > successes）+1，仅"零失败的干净轮"清零（**不引入"进展"语义判断**，避免 R10 越界）。精确规则计划阶段定，意图 = 一次穿插成功不抹掉 churning 失败连胜。

---

## 5. WS3 — Kimi/Minimax 默认预设（anthropic 为主，openai 次选）

仅动 `src/providers/presets/registry.rs`（数据驱动 + `src/config/presets_override.rs` 可覆盖），零 harness/thinker 侵入。源于用户反馈：「Kimi/Minimax 都是 anthropic 协议，建议作为默认配置、鼓励用户用 anthropic 协议」。

- **Minimax**：主预设 `minimax` 改为 anthropic 协议 + `https://api.minimaxi.com/anthropic`（用户推荐端点；`src/providers/protocols/anthropic/provider_policy.rs:29-33` `detect_anthropic_endpoint_class` 已识别该 Bearer 代理特例），默认 `MiniMax-M2.5` 沿用；原 openai 端点 `https://api.minimax.io/v1` 降为次选预设 `minimax-openai`。
- **Kimi/Moonshot**：主预设 `moonshot`（alias `kimi`）改为 anthropic 协议 + `https://api.moonshot.cn/anthropic`（用户给的 `platform.moonshot.cn` → CN 区；`provider_policy.rs:343` 已识别），默认模型沿用 `kimi-k2-*`；`temperature_policy = Omit`（Kimi 服务端管温度，同既有 `kimi-for-coding`）。原 openai 端点降为次选 `moonshot-openai`（保留 ai/cn 两区）。`kimi-for-coding`（anthropic coding 端点）保持不变。
- **与 WS1 的强耦合（关键护栏）**：主预设转 anthropic 后 `protocol()` 返 `"anthropic"` → 若无 WS1 的 vendor-identity，会错拿 Claude 松缰绳。vendor-identity 按域名（`minimaxi.com` / `moonshot.cn`）→ `strict`，正是这条的护栏。**WS3 必须与 WS1 同 spec 落地**，否则切 anthropic 反而削弱治理（这是把两者放进同一 spec 的根本原因）。
- **待确认（计划阶段）**：精确默认 model id；`canonical_provider_id` 对 `minimax-openai` / `moonshot-openai` 的 pricing 归一（`src/pricing.rs:571` MiniMax 费率按 `"minimax"` canonical 解析，次选预设须归一不漏价）。

---

## 6. R10 / 红线自检

- **不新增 harness 文件、不新增 thinker layer** —— 复用现有 `ProviderGuidanceLayer`（改 key + 外移内容）；解析器在 `harness_bridge`（orchestrator，**非 `src/harness/`**）；`strict.md` + vendor-identity 表是数据；WS2 全在现有 grace 机制内改；WS3 仅动 `presets/registry.rs` 数据 → **`src/harness/` 12 文件预算零侵入**。
- 教练 = prompt 文本（R9）；管控 = 结构化计数/阈值（脚手架，非认知）；零循环语义判断 → **R10-safe**。被移除中间件的"智慧"正是迁进 prompt 的 L2 启发文案（R9 落地）。
- 数据驱动 .md 外移 = R3（核心轻量化，文本→数据）+ R8（config 可覆盖）+ 复活已建好的基础设施（优于 YAGNI 删除）。
- model-id 子串匹配是机器稳定标识查表（P8 允许），**非**对自然语言做正则路由。

---

## 7. 范围边界（显式 OUT，后续子项目）

- **能力级 failover**（WS2-next）：模型持续畸形 tool call / 发不出合法调用 → 信号 FailoverProvider 按能力降级换模型。本轮不做（触及 failover 架构，风险中等）。
- **harness 大文件拆分**：`think.rs`(1900) / `agent.rs`(1519) / `act.rs`(1251) 超 ~4900 行红线——独立重构子项目，与"加内容"方向相反，单独权衡。
- **防御性输出修复**（deferred）：截断 tool-call JSON 修复（`text_tool_call` all-or-nothing）、UTF-8 surrogate 净化（Rust 下语义不同于 Python，需单独评估）。
- **per-family bespoke OSS 文案**：本轮只 1 个统一 `strict` 档（用户选择），按三次法则将来再拆。

---

## 8. 测试策略（TDD）

- **解析器优先级**：override > id-table > protocol > unknown。
- **id 表命中**：kimi/deepseek/qwen/glm → strict；gpt-* → openai；claude-* → anthropic（经协议）；未知 → unknown。
- **头号回归守卫**：Kimi-over-openai 现得 `strict` 教练而非 `openai`。
- **robustness**：`for_behavior("strict")` 最紧；`clamped()` 守窗口不变量；现有 anthropic/ollama/conservative 字节不变。
- **字节兼容**：现有 protocol→behavior（anthropic/openai/gemini/ollama）解析与教练输出对既有 model 不变（现有 `provider_guidance.rs` 测试改为按 behavior name 喂入但断言同输出）。
- **WS2**：timeout 触发 grace（新）；provider 挂起时 grace fail-soft 不崩；consecutive_failure 计数不被穿插成功清零；软着陆提示在 cap 前一轮注入。
- **WS3**：`minimax` / `moonshot` 主预设解析为 anthropic 协议；且 vendor-identity 仍把它们判为 `strict`（**回归守卫：切 anthropic 默认不得削弱治理**）；`minimax-openai` / `moonshot-openai` 次选预设存在且 pricing 归一。

---

## 9. 工程纪律

- **cargo 极度节制**（用户 standing 偏好）：开发期不跑全量；高风险合并至多一次 `cargo check -p alephcore --lib`。
- 提交规范：英文 commit，`<scope>: <desc>`。
- 单分支或 worktree 视 writing-plans/executing 阶段决定。

---

## 10. 关联

- 续 [[project-harness-multimodel-robustness]]（robustness_profile 本体）、[[project-failover-behavior-protocol-passthrough]]（behavior 透传修复）、[[project-cron-duplicate-fabrication-fix]]（Kimi 默认 conservative 暴露的痛点）。
- 参考项目坐标：Hermes（厚，错误分类法/quirks 表/tool-call 修复/sanitization）、Pi（薄，callback）、openclaw（薄，steering 队列）——Aleph 立于"中间偏厚"，本轮做连线/启发/兜底，不向任一极端靠拢。
