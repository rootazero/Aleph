# MoA 第三轮优化 — 修复与连线 (Round 3: Fixes & Wiring)

- **Date**: 2026-07-06
- **Status**: Design approved, ready for planning
- **Predecessors**: [`2026-07-05-moa-continuous-advisory-port-design.md`](2026-07-05-moa-continuous-advisory-port-design.md)（第一轮移植）、[`2026-07-05-moa-round2-optimization-design.md`](2026-07-05-moa-round2-optimization-design.md)（第二轮优化）
- **Focus**: 修复 + 连线（bug fixes & wiring）。**非**大重构、**非**新功能面。

---

## 1. 背景与动机 (Context)

MoA 连续咨询经两轮开发已过端到端运行时 QA（`moa` 工具真执行、真实顾问咨询、面板渲染、select_model 互斥槽位、开销归并桶均验证通过）。Config 层（3 层递归守卫）、核心 `process()`（advisory view → 并行扇出 → 超时降级 → 尾部注入 → 聚合器执行）、账务诚实、4 事件 schema、面板/TUI 渲染均已成熟。

本轮**不动成熟核心**，只收敛前两轮留下的**真实连线缺口与已知取舍**。审查中发现的核心问题：**MoA 会话激活（arm）逻辑分散在 4 处独立实现，且其中 2 处缺 preset 校验**，导致「用户选了 MoA / 不存在的 preset → 静默无效、零报错」的 UX 缺口。

### 1.1 关键发现：arm 站点有 4 处，校验不对称

| # | 站点 | 文件 | 校验 preset? | arm 类型 |
|---|---|---|---|---|
| 1 | `MoaManageTool::activate`（工具 `on`/`once`） | `src/builtin_tools/moa_manage.rs:232` | ✅ `resolve_preset` | sticky / one-shot |
| 2 | `SelectModelTool`（`moa:<preset>` 选模） | `src/builtin_tools/select_model.rs:92` | ✅ `resolve_preset` | sticky |
| 3 | `apply_moa_selector_semantics`（`chat.send`/`agent.run` 的 `model_override{provider:"moa"}`） | `src/gateway/handlers/agent.rs:526` | ❌ **裸 arm** | sticky |
| 4 | `/moa <prompt>` one-shot（Panel/CLI + channel 两份） | `src/gateway/execution_engine/execute.rs:262` + `slash_command.rs:156` | ❌ **裸 arm** | one-shot |

- 站点 1、2 arm 前会 `resolve_preset` 校验，不存在则上游拒绝并给「use moa tool list」提示。
- 站点 3、4 **直接 arm 无校验** → 靠 run-build 时 `try_build_for_run` fail-soft 回退到普通模型 → **用户毫无感知地失去 MoA，也没有任何报错**。
- 四处都各自内联 `set_session_moa(...) + clear_session_model(...)` 的 set-then-clear 序列（站点 3/4 注释里都写着「mirror moa_manage::activate()」），是真实的**漂移风险**：任何一处改了序列或语义，其余三处不会自动跟上。

---

## 2. 本轮范围 (Scope)

四项，均为「修复 / 连线」性质，无新用户功能面：

| 项 | 一句话 | 价值 | 风险 |
|---|---|---|---|
| **F1** | 单一 arm/disarm 真源 + 闭合站点 3/4 的静默无效 | 高 | 低 |
| **F2** | `/moa` one-shot 结束后恢复被它覆盖的 sticky 激活 | 中 | 低 |
| **F3** | `/moa` slash 路径加 caller-role operator 门（对齐 `moa` 工具） | 中 | 低 |
| **F4** | 一行文档注释锁定 VESR「归因给聚合器」是有意为之 | 文档 | 零 |

### 非目标 (Non-Goals)

- 不拆 `provider.rs`（生产代码仅 ~413 行，其余是测试）或大改模块边界。
- 不动核心 `process()` / 扇出 / 缓存 / 账务 / 事件 schema。
- 不加聚合器流式、渐进 advisor emit 等新功能面（那是"增强优先"路线，本轮不做）。
- 不改 `moa` 工具自身的 operator 门（`method_authz` 已保护，无需动）。

---

## 3. 设计 (Design)

### F1 — 单一 arm/disarm 真源 + 闭合静默无效

**新增** `src/providers/moa/activation.rs`，作为**唯一** arm/disarm 逻辑真源（内聚一处，`mod.rs` 保持 re-export 枢纽）。三个自由函数：

```rust
/// Resolve + validate the preset, then arm sticky MoA for the session and
/// clear any per-session model pick (selector-slot exclusivity). Returns the
/// resolved preset name (for user-facing messages) or an error string
/// ("preset 'X' not found — ...") when the preset does not resolve.
pub fn arm_sticky(session_key: &str, preset: Option<String>) -> Result<String, String>;

/// Same resolution/exclusivity as `arm_sticky`, but arms a ONE-SHOT pref.
/// If an existing STICKY pref occupies the slot, it is stashed into the new
/// pref's `restore` field (F2) so it can be reinstated after the one-shot run.
pub fn arm_one_shot(session_key: &str, preset: Option<String>) -> Result<String, String>;

/// Clear the session's MoA sticky (selector-slot exclusivity: called by the
/// "normal model pick" path before it sets the model handle). Idempotent.
pub fn disarm(session_key: &str);
```

- 校验通过 `crate::providers::moa::get_moa_config().resolve_preset(...)` 完成——与站点 1/2 同一路径，行为一致。
- `session_moa_handle.rs` 保留底层原语（`set_session_moa`/`get_session_moa`/`take_for_run`/`clear_session_moa`/`restore_one_shot`）；新助手**组合**它们，状态模块保持纯粹（无 config 依赖）。

**四处站点改造**：

- **站点 1**（`moa_manage::activate`）：`resolve_preset` + `set_session_moa` + `clear_session_model` → 改为 `arm_sticky` / `arm_one_shot`（按 `one_shot` 参数），据返回的 `Result` 生成工具消息。
- **站点 2**（`select_model.rs`）：`moa:` 分支改调 `arm_sticky`；normal-pick 分支的 `clear_session_moa` 改调 `disarm`。错误消息复用返回的 `Err` 串。
- **站点 3**（`apply_moa_selector_semantics`）：**闭合静默无效的关键**。该函数返回 `Option<ModelOverride>`，无用户错误通道。改造：
  - `Qualified{provider:"moa", model}` → 调 `arm_sticky(key, Some(model))`。
    - `Ok(name)` → 返回 `None`（吞掉 override，MoA 已 arm）。
    - `Err(msg)` → **不 arm、不清**，用站点 2 同款 `notify_tool_result` 通道以 `"moa"` 为标签（该 arm 由 model_override 触发而非 `select_model` 工具调用，故用中性标签而非 `"select_model"`）推一句报错通知，返回 `None`（不 arm 就近降级为普通模型链，但用户看到了错误——不再静默）。
  - `Some(other)` → 调 `disarm(key)`，返回 `Some(other)`（passthrough，同今日）。
  - `None` → 返回 `None`（同今日）。
- **站点 4**（`execute.rs` + `slash_command.rs` 的 `/moa` one-shot）：`set_session_moa(&key, None, true) + clear_session_model` 改调 `arm_one_shot(&key, None)`。one-shot 恒用默认 preset（`None`）；若无任何可用 preset，`arm_one_shot` 返回 `Err` → 该路径记录/忽略错误并当普通轮跑（见 F3 的降级 UX 复用同一剥前缀路径）。

**备选（不采纳）**：把 arm 逻辑包成 `MoaSelector` struct/enum——过度设计，与模块现有自由函数风格不符（P6 KISS）。

### F2 — `/moa` one-shot 结束后恢复 sticky

**问题**：`session_moa_handle` 是**单槽**（每 session 一个 `SessionMoaPref`）。`/moa <prompt>` 的 one-shot 直接 overwrite 掉已激活的 sticky；one-shot 被 `take_for_run` 消费后槽变空 → 原 sticky **永久丢失**。用户「常驻 MoA + 偶尔插一句 /moa」的心智模型被破坏。

**方案**：给 `SessionMoaPref` 加一个可选的**压栈**字段：

```rust
pub struct SessionMoaPref {
    pub preset: Option<String>,
    pub one_shot: bool,
    /// A sticky pref displaced by a one-shot arm, to be reinstated when this
    /// one-shot is consumed by `take_for_run`. `None` for sticky prefs and for
    /// one-shots armed over an empty/one-shot slot. Boxed to keep the struct
    /// small (self-referential Option).
    pub restore: Option<Box<SessionMoaPref>>,
}
```

- `arm_one_shot`（F1）：写 one-shot 前读现有槽；若现有是 **sticky**（`one_shot == false`）→ 把它装进新 pref 的 `restore`。若现有是空 / one-shot → `restore = None`（one-shot 本就短命，无需保）。
- `take_for_run`：消费 one-shot 时（`pref.one_shot == true`），若 `pref.restore` 存在 → **回填 `*restore`**（reinstate sticky）而非 `remove`；否则 `remove`（同今日）。保住「单一恢复点」原子性——仍在同一 write-lock section 内完成读+改。
- 边界：
  - `moa off` / `disarm` → `clear_session_moa` 清整槽（连 stash 一起清）= 显式 off 胜，正确。
  - one-shot 叠 one-shot → 第二个覆盖第一个，`restore` 不继承（第一个 one-shot 本就该消失）。
  - run 构建失败的 `restore_one_shot` 回填路径不变（empty-slot-only 语义），但与 F2 的交互按覆盖对象分两种终态（均 benign）：
    - **覆盖空槽**的 one-shot（常见 `/moa` 路径）：`take_for_run` `remove` 后槽空 → `restore_one_shot` 回填该 one-shot → 下一轮重试，同今日。
    - **覆盖 sticky** 的 one-shot：`take_for_run` 已即时 reinstate sticky（槽非空）→ `restore_one_shot`（`entry().or_insert` 仅填空槽）成 **no-op** → one-shot **不**回填、**sticky 保留**。终态无害且更符合心智模型：MoA 保持激活、build 失败经 `count==0` 顾问事件呈现（非静默）、下一轮用 sticky preset 而非重试 one-shot 的默认 preset。要让 one-shot 在此情形也重试需 `take_for_run` 延后 reinstate（两阶段协议）＝破坏「单一原子 restore 点」核心设计，收益边际，故不做（KISS/YAGNI）。

### F3 — `/moa` slash 路径查 caller role

**问题**：`moa` 工具已被 `src/gateway/method_authz.rs`（`"moa"` ∈ operator-required 集）保护；但 `/moa <prompt>` slash 路径**绕过工具**直接 arm。Chat-tier channel 的 guest 可打 `/moa ...` 触发多模型顾问、烧 advisor token（前提：operator 已配过 preset）。

**方案**：两个 one-shot arm 点（`execute.rs` Panel/CLI 路径、`slash_command.rs` channel 路径）读 `request.metadata.get("caller_role")`（已证实可达，见 `run_loop/inner.rs:464`），复用 `method_authz` 判定 operator 的谓词：

- **operator**（loopback / Panel 授权 / CLI 本地）→ 正常 arm（同今日）。
- **非 operator**（channel guest）→ **不 arm**，剥掉 `/moa` 前缀当普通轮跑（guest 得正常回答，不泄露 `/moa` 进 LLM），并推一句一行提示「MoA advisory requires operator; running normally」。
  - Panel/CLI 路径（`execute.rs`）：input 无论如何都被重写为剥前缀的 `prompt`（现有 line 267），只需把 `set_session_moa` 块包进 operator 判断。
  - channel 路径（`slash_command.rs`）：现有已 `Fallthrough` + `moa_fallthrough_input` 剥前缀，只需把 `set_session_moa` 块的 `if` 条件加上 operator 判断。

即 F3 是在两处 arm 块外**加一层 role 判断**，非结构改动。

### F4 — VESR 归因文档注释

**现状（已正确）**：round-2 B8 已把 VESR 归因从「pre-MoA override 模型」改为**聚合器身份**（`runner_impl.rs:405-412`）——聚合器是本轮真正 serve token 的模型，归因正确。

**剩余 nuance**：MoA 辅助下的成功被归因给「单独聚合器模型」，未来 router recall 单跑该模型未必复现同等质量。此为低价值、偏哲学问题，**不改行为**。

**方案**：在 `runner_impl.rs` 归因处（~line 404 的 B8 注释旁）加一行注释，锁定「MoA active 时归因给聚合器模型是有意为之——它是本轮实际执行者；MoA 辅助增益不单独建模，属已知取舍」。零行为改变。

---

## 4. 验证策略 (Testing)

尊重「极度节制 cargo」：每项配**定向单元测试**，收尾**至多一次** `cargo check --lib`。

- **F1**：
  - `activation.rs`：`arm_sticky` 解析成功写 sticky + 清 model handle；`arm_sticky` 对不存在 preset 返回 `Err`（不写状态）；`arm_one_shot` 写 one-shot；`disarm` 幂等。
  - 站点 3：`apply_moa_selector_semantics` 对 `provider:"moa", model:"ghost"` 返回 `None` **且不 arm**（校验拒绝路径），对合法 preset 返回 `None` 且 arm。复用现有测试骨架。
  - 回归：站点 1/2 现有测试仍绿（行为等价）。
- **F2**：`session_moa_handle` 新测试——`arm_one_shot` 覆盖 sticky 后 `take_for_run` 消费 one-shot → sticky reinstated；覆盖空槽/one-shot → 消费后为空；`moa off` 清整槽含 stash。
- **F3**：两处 arm 点——operator caller_role 正常 arm；非 operator caller_role 不 arm 且 input 剥前缀。
- **F4**：无（纯注释）。

---

## 5. 红线合规 (Redline Compliance)

- **R10（薄 harness）**：零改动进 `src/harness/`。F1/F2 在 `src/providers/`，F3 在 `src/gateway/execution_engine/`，F4 在 `src/orchestrator/`（归因逻辑本就在此）。无 harness LOC 增长。
- **P2（高内聚）/ P6（KISS）**：新 `activation.rs` 把分散的 arm 逻辑收拢一处；自由函数而非新抽象层。
- **P7（防御性）**：所有 lock 用 `unwrap_or_else(|e| e.into_inner())`（沿用模块现有 poison 处理）。
- **R7/R9**：无确定性代码替代 LLM 推理；纯状态/权限连线。

---

## 6. 交付形态 (Deliverables)

- 新文件：`src/providers/moa/activation.rs`
- 改动文件：`src/providers/moa/mod.rs`（re-export）、`src/providers/session_moa_handle.rs`（F2 字段 + `take_for_run`）、`src/builtin_tools/moa_manage.rs`、`src/builtin_tools/select_model.rs`、`src/gateway/handlers/agent.rs`、`src/gateway/execution_engine/execute.rs`、`src/gateway/execution_engine/slash_command.rs`、`src/orchestrator/harness_bridge/runner_impl.rs`（F4 注释）
- 文档：`docs/reference/FEATURE_LOCATOR.md` §4.9 追加第三轮说明（收尾）
- 合入本地 main（推送由用户决定，沿前两轮惯例）
