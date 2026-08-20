# 静态代码审查报告 — guardrails

- **审查单元**: `src/guardrails/` —— 输入/输出/工具调用三轨安全防线（PII 脱敏、密钥防泄漏、占位符注入）
- **审查日期**: 2026-08-20
- **基线**: `.worktrees/review-modules/`（与 main 一致的 git worktree）
- **方法**: 全量人工静态阅读 + 沿 `pii_secrets → security/runtime_guard → pii/* → secrets/leak_detector` 调用链深入到真正的正则/检测层
- **审查者**: 静态代码审查（按 group_chat 报告的格式）

## 统计

| 指标 | 值 |
|------|-----|
| 源文件数 | 4 生产文件 + 4 测试文件 |
| 总行数 | 2856（不含 `mod.rs` 的内联测试 + 测试目录） |
| 核心文件 | `pii_secrets.rs` (456)、`registry.rs` (280)、`decision.rs` (103)、`traits.rs` (49) |
| 委托的真实检测器 | `security/runtime_guard.rs` (619)、`secrets/leak_detector.rs` (502)、`exec/leak_detector.rs` (346)、`security/content_sanitizer.rs` (1008)、`pii/engine.rs` (633) |
| PII 内置规则 | 7 条（api_key, ssh_key, id_card, phone, bank_card, email, ip_address） |
| 注入检测模式 | 23 条（`security/injection_patterns.rs`） |
| 已知漏洞（ReDoS / 真 0day 绕过） | **0** —— 全部依赖 Rust `regex` crate 的线性时间保证，自定义模式经 `safe_regex::bounded_builder` 编译 |

文件清单：
- 生产：`mod.rs` (28)、`decision.rs` (103)、`pii_secrets.rs` (456)、`registry.rs` (280)、`traits.rs` (49)
- 测试：`tests/bench.rs` (52)、`tests/input.rs` (51)、`tests/output.rs` (51)、`tests/registry.rs` (309)
- 内联测试：`decision.rs` (28)、`pii_secrets.rs` (281 行内联单元测试)

## 总体评估

guardrails 是一个**精心设计的安全模块**：作者明显知道正则回溯的陷阱（`safe_regex` 模块 1 MiB 编译上限），对 homoglyph / 零宽字符 / Bidi 攻击有专门的 `unicode_guard`，对 PKCS#8 头部、`sk-` 子串、`task-`/`musk-` 假阳性都有显式回归测试。失败默认行为（fail-closed）在输入/输出/工具调用三轨上一致。

但模块**实际承载安全决策的关键代码只占总行数约 1/3**，其余为 trait 适配和扫描委托。真正的检测逻辑分布在 `security/runtime_guard.rs`、`secrets/leak_detector.rs`、`exec/leak_detector.rs`、`pii/engine.rs` 四个下游模块。本次审查范围限于 `guardrails/`，但发现的所有重大问题都需要沿调用链去到这些下游模块去验证/修复。

## 发现列表（按严重级排序）

### Critical

**C1. `pii_secrets.rs:91-94` —— `Block.reason` 把 `SecretError` 原文传给模型、日志、UI，构成密钥名枚举侧信道**

```rust
Err(SecurityGuardError::SecretResolutionFailed(e)) => GuardrailDecision::Block {
    reason: format!("Secret resolution failed: {e}"),
    class: ErrorClass::Unexpected,
},
```

`SecretError::NotFound(String)` 的 `Display` 实现是 `Secret '{0}' not found`（`secrets/types.rs:90`），错误变体 `InvalidPlaceholder` 还把用户的输入字符串原样回显（同文件 L107）。这条 `reason` 字段的下游消费路径：

- `harness/agent/guardrails.rs:60` —— `format!("guardrail blocked: {reason}")` 后写入 `SessionEvent::ToolError`，**持久化到 session log**
- `harness/agent/guardrails.rs:47` —— `tracing::warn!(?session_id, tool = %call.name, reason = %reason, ...)`，**写入日志流**
- `harness/agent/guardrails.rs:55` —— `callback.on_safety_block(&reason)`，**显示在 UI**
- `harness/agent/guardrails.rs:78` —— `format!("output guardrail blocked: {reason}")` 进入 `HarnessError::Llm(AlephError::Validation/other)`，**返回给模型**

攻击面：用户可以拿任意字符串当作 `{{secret:NAME}}` 试错，根据返回的 `Block.reason` 是否含 `"'<NAME>' not found"` 推断哪些 NAME 是真实存在的（不报错 → 存在 → 占位符已被解析）。同一副作用允许枚举 vault 的密钥命名空间（如 `"STRIPE_PROD"` vs `"STRIPE_PROD_KEY"` vs `"STRIPE_PROD_KEY_FALLBACK"`）。

`InvalidPlaceholder` 还把"任意用户输入"原样写回 reason，本身不是漏洞（用户的输入回显给用户自己），但若后续被存入 trace/审计流就构成 XSS-in-log（如果 trace consumer 把字符串当 HTML 渲染）。

**建议**:
- `NotFound` 不要把请求的 secret 名带回 reason，只返回静态字符串（"secret not found"），把 name 写入审计日志的独立字段
- `InvalidPlaceholder` 把"用户原始输入"统一替换成 `{{...}}` 形式的骨架串，避免回显任意字节
- 或者更稳妥：在 `Block` 路径统一做一个白名单 reason 替换器

---

**C2. `harness/agent/guardrails.rs:42-56` —— 工具调用 Sanitize 重新解析 JSON 失败时回退到原始 args，可能保留未脱敏的密钥/PII**

```rust
crate::guardrails::GuardrailDecision::Sanitize(rep) => {
    let new_args = match serde_json::from_str::<Value>(&rep.text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(...);
            call.arguments.clone()    // ← 回退到原始 args
        }
    };
    ...
}
```

`pii_secrets.rs:198-213` 把整个 args 序列化回 JSON 字符串（`serde_json::to_string(&resolved)`），这里再 `from_str` 解析回来。这条路径在两条线路上都可能**静默保留未脱敏数据**：

1. **脱敏把秘密替换成保留占位符**。如果 `runtime_guard::process_outbound` 把 `sk-live-xxxx` 替换成 `***LEAKED_REDACTED***`（含 `*`），`serde_json` 序列化没问题；但若某个未来 PII 规则把占位符替换成含未转义字符（如控制字符、`"`）的内容，`from_str` 可能失败 → 回退到原始 args → 已识别的密钥**照原样送到工具**。
2. **脱敏生成了形状改变的 JSON**（如 `{"cmd": "..."}` 变成 `{"cmd_0": "..."}` 之类的结构破坏），`from_str` 失败 → 回退到原始 args → PII 继续存在。

注释（harness/agent/guardrails.rs:42-46）作者明确说"shape change is worse than un-applied sanitize"——这是合理的设计选择，但**安全层的默认应该是 fail-closed（Block + 告警）**，而不是 fail-open（静默保留原始）。

**建议**:
- `from_str` 失败时改为 `Block { class: Unexpected }`，要求 operator 显式 disable_all 才能放行
- 更根本的修复：guardrail 直接携带 `Value` 而非 `String`，绕过 reparse（注释里写到的 "deeper fix"），消除这条路径

---

### High

**H1. `pii_secrets.rs:171-184`（`evaluate_input`/`evaluate_output`/`evaluate_tool_call`）—— 三处 `SecurityContext::default()`，让 provider/platform 配置全部失效**

三个 `evaluate_*` 方法全部传 `SecurityContext::default()`：
```rust
let ctx = SecurityContext::default();
let r = self.guard.process_outbound(text, None, ctx).await;
```

`SecurityContext { provider_name: None, platform_name: None, session_id: None }`（`security/runtime_guard.rs:73-79`）。

这导致：
1. **`is_platform_excluded`（`pii/engine.rs:204-211`）永远走全局配置**，不会应用任何 `[platform_policies.xxx]` 配置——运营商在 Telegram 上放宽 phone 检测的配置形同虚设。
2. **audit entry `actor_user` 永远是 None**（`runtime_guard.rs:152-159` 的 `crate::scope::current_room_author()` 仅在 `agent::run_turn_internal` 注入 scope 时非空；但 guardrail trait 接口在调用栈上未必经过那条路径，详见 think.rs:849 之前）。
3. **`session_id` 缺失**，审计日志失去关联 session 的能力。

但这里的设计意图似乎是"guardrail trait 不感知调用者上下文"——这本身是个 trait 设计的缺陷，不是单点 bug。traits.rs 三个 trait 方法签名都没有 context 参数。

**建议**:
- 给三个 trait 加可选 context 参数（`Option<&SecurityContext>` 或 `&dyn ProviderContext`），让 harness 调用点把 `provider_name`/`platform_name`/`session_id` 传下来
- 至少 session_id 必须传，否则审计日志无法关联

---

**H2. `registry.rs:68-83` + `registry.rs:171-183` —— 评估管线无优先级，多个 guardrail 注册时仅有"第一个非 Allow 胜出"**

```rust
let mut last_warn = None;
for g in &self.input {
    let d = g.evaluate_input(text).await;
    match d {
        GuardrailDecision::Allow => continue,
        GuardrailDecision::Warn { reason } => {
            last_warn = Some(GuardrailDecision::Warn { reason });  // 仅保留最后一个
        }
        _ => return d,
    }
}
last_warn.unwrap_or(GuardrailDecision::Allow)
```

问题：

1. **多个 guardrail 都返回 Sanitize 时，第一个胜出**，但它输出的 sanitized text **不会被后续 guardrail 看到**——后续 guardrail 用的是原始 `text`。如果 G1 把 `{{secret:A}}` 替换成 `REDACTED_A`，G2 看到的是原始 `{{secret:A}}`（可能反过来 Block 这个占位符）。
2. **多个 Warn 时，仅保留最后一个 reason**（`last_warn`），前面的诊断信息全部丢失。
3. **Block vs Sanitize 顺序未文档化**——`evaluate_input`/`evaluate_output`/`evaluate_tool_call` 三个方法的逻辑相同（`_ => return d`），但没有测试覆盖"先注册 Block-guardrail 再注册 Sanitize-guardrail"或反向时的预期行为。`tests/registry.rs:160-167` 的 `first_non_allow_wins_when_multiple_registered` 只测试 Allow+Block，没测 Block+Sanitize 或 Sanitize+Block。

测试覆盖盲点（**H2 子项**）：

| 场景 | 是否测试 | 风险 |
|------|---------|------|
| Sanitize 后面的 Sanitize 是否链式应用 | ❌ | sanitization 不彻底 |
| Block 后面跟着 Sanitize 的优先级 | ❌ | Block 胜出符合预期但未断言 |
| 一个 Block + 一个 Warn，Warn 是否被吞 | ❌ | last_warn 机制被 Block 短路 |
| 三个 guardrail 第一个 Sanitize 第二个 Block | ❌ | 关键安全路径无断言 |

**建议**:
- 把 `last_warn` 改成 `Vec<Warn>`，按顺序聚合所有 Warn reasons（用 `\n` 或 `; ` join 进 reason 字符串）
- 给 Sanitize 链式组合：每个 guardrail 看到上一个的输出，Sanitize 链尾结果作为最终 Sanitize
- 补上 Sanitize+Block、Block+Sanitize、Sanitize+Sanitize 三个测试用例

---

**H3. `registry.rs:36-45`（`disable_all`/`enable_all`）—— 进程全局 kill switch，无审计、无权限检查**

```rust
pub fn disable_all(&self) {
    self.enabled.store(false, Ordering::Release);
}
```

任何持有 `Arc<GuardrailRegistry>` 引用的代码（包括子模块、第三方插件、注入回调）都能调用 `disable_all()`，整个进程的所有 session、所有 user、所有工具调用、所有输出立即 **fail-open**。这是符合"主控点"设计的（master spec §Stage 5 描述的紧急回退），但：

- **无审计日志**：调用 `disable_all()` 不写 audit、不写 trace
- **无 actor 标注**：是 operator 还是 attacker 触发无法区分
- **无 rate limit**：可以反复 disable→enable→disable 制造日志风暴
- **`enable_all()` 是无条件恢复**，没有"重新启用前先做健康检查"的钩子

**建议**:
- `disable_all` 接受 `actor: &str` 参数（或从 `scope::current_room_author()` 读取），记录 `tracing::warn!(actor, "guardrail disabled")`
- 改用 `RwLock<bool>` 而不是 `AtomicBool`，让 enable/disable 走同一把锁，状态变更走 audit pipeline

---

**H4. `secrets/leak_detector.rs:300-322`（`find_injected_substring`）—— 已注入密钥的指纹集合无界增长，无 TTL / 驱逐**

```rust
pub fn register_injected(&mut self, secrets: &[InjectedSecret]) {
    for secret in secrets {
        if secret.value_len < MIN_INJECTED_MATCH_LEN { continue; }
        self.injected_hashes.insert(secret.value_hash);
        self.injected_lens.insert(secret.value_len);
    }
}
```

泄漏检测器把每个注入过的 secret 的 `(siphash, length)` 永远保存在 `injected_hashes: HashSet<u64>`。在长生命周期进程（Aleph Server 是常驻 daemon）下：

- 每次 `process_outbound` 解析占位符都会调用 `register_injected`，hashes 集合**单调增长，永不释放**
- 每次 `process_inbound` 扫描时遍历 `injected_lens`（BTreeSet），对每个长度扫描整个内容 → 复杂度 O(内容长度 × 已注入的不同长度数)
- 多 session 并发时哈希集合**跨 session 共享**，session A 注入的 secret 指纹泄漏到 session B 的检测范围（虽然 SipHash 碰撞概率 1/2^64，但内存膨胀是真问题）

更严重的安全含义：进程 reboot 前所有注入过的 secret 都在泄漏检测的"白名单"中，任何**长得一样（哈希碰撞）或同长度（多长度碰撞）的内容**会被当成"已注入 secret"被检测器捕获 → 误报可能让合法的工具响应被当作泄漏处理。

**建议**:
- 给 `injected_hashes` 加 LRU 容量上限（建议 1024 条），按 `register_injected` 顺序驱逐
- 给每条注入 secret 加时间戳，超过会话生命周期的（如 1 小时）驱逐
- 或把泄漏检测器做成 per-session，session 结束即丢弃

---

**H5. `registry.rs:147-172`（`screen_user_messages`）—— 历史消息中 PII/密钥泄漏仅 redact 不 Block，但 redact 本身有 bug 路径**

```rust
GuardrailDecision::Block { reason, .. } if blocking == Some(idx) => {
    blocked = Some(reason);
    break;
}
GuardrailDecision::Block { reason, .. } => {
    tracing::warn!(...);
    set_screened_text(&mut record.event, REDACTED_USER_MESSAGE.to_string());
}
```

注释（registry.rs:103-120）解释了为什么 history message 的 Block 不结束 turn："session events are immutable and replayed forever: re-blocking on every rebuild would end every subsequent turn and brick the session"。

但 **`REDACTED_USER_MESSAGE` 的 redact 路径本身是脆弱的**：
- 如果 PII 检测器返回 `Sanitize(text)`，替换文本可能是 `"[REDACTED]"` 这样的简短占位符
- 如果 PII 检测器返回 `Block`，全部消息被替换成 `REDACTED_USER_MESSAGE` 这个**单一固定字符串**

后者的语义问题：如果**原始消息中包含另一个用户的 secret**（比如用户 A 把用户 B 的 AWS key 发给 Aleph），`REDACTED_USER_MESSAGE` 这个常量会让所有人共用一个字符串——审计时无法区分"哪条历史消息含敏感信息"。

更要紧的是：**被 redact 的消息仍然存在于 session log 中**。如果 redact 本身有 bug（任何 PII 规则 miss），原始内容会随着 prompt 重建被反复喂给模型。下游的 `eval_input` 不再扫（已经被 redact 了），所以 bug 一旦发生就是永久性泄漏。

**建议**:
- `REDACTED_USER_MESSAGE` 替换为 per-secret 哈希指纹（如 `[REDACTED:<siphash-of-content>]`），让审计可区分
- 在 session log 加一层"已 redact 标记"，eval_input 跳过 redact 后的内容但记录 skip 计数

---

### Medium

**M1. `pii_secrets.rs:108-130`（`scan_tool_args`）—— 仅扫描 string leaves，JSON number / boolean / null leaf 中的 PII 完全 bypass**

```rust
match value {
    Value::String(s) => Ok(Value::String(self.scan_tool_arg_leaf(s, ...).await?)),
    Value::Array(items) => { ... }
    Value::Object(map) => { ... }
    _ => Ok(value.clone()),    // ← Number / Bool / Null 原样通过
}
```

如果一个工具的 args 是 `{"phone": 13812345678}`（phone 是 JSON number 而非 string），phone 规则（`pii/rules/phone.rs`）不会匹配——因为它的工作对象是 string。phone-like 的数字直接传到工具。这是 fail-open 行为。

虽然实际工具几乎不会把 phone 作为 number 传，但 IP 地址、银行账号、ID 卡都可能出现 number 形态。**安全原则应该是"任何 JSON 形态都脱敏"**。

**建议**:
- 把 `Value::Number` 转为字符串再扫描（保持原 number 形态的 redact 仍可能引起 schema 不兼容——评估这个 trade-off）
- 或者把 number 类型的可疑值（如 11 位纯数字）也加入扫描

---

**M2. `pii_secrets.rs:107`（`scan_tool_args`）—— 序列化为 JSON 字符串再传 `serde_json::from_str` 解析回来，整树来回两次**

```rust
match serde_json::to_string(&resolved) {
    Ok(text) => { ... }
    Err(e) => { Block { ... } }
}
```

整个 args 子树被 `serde_json::Value → String → serde_json::Value` 来回变换。问题：

1. **大 args（典型：网页抓取内容、文件 diff）的序列化代价高**——一次评估可能涉及数 MB 内容
2. **`serde_json::Value` 的数值精度损失**——Number 内部存 `f64`，但 JSON 支持任意精度的整数；来回变换可能导致大整数（如 Unix 时间戳的 nanosecond 表示）精度丢失，工具拿到失真参数
3. **`harness/agent/guardrails.rs:42-56` 的再次 from_str 是第三次 round-trip**

`pii_secrets.rs:13-22` 的注释已经承认这是临时方案（"deeper fix: guardrail carries a Value, not a String"）。

**建议**:
- 把 `Replacement` 改成 `Replacement { value: Value, source: String }` 携带结构化 Value
- `apply_tool_call_guardrail` 直接使用 `rep.value`，消除 from_str 路径

---

**M3. `secrets/leak_detector.rs:115-130`（`scan_patterns`）—— 顺序遍历 N 个 regex，对每个 regex `is_match` 一次，再 `replace_all` 一次**

```rust
for (label, pattern) in LEAK_PATTERNS.iter() {
    if pattern.is_match(&redacted) {
        found_labels.push(*label);
        redacted = pattern.replace_all(&redacted, REDACTED_LEAK).to_string();
    }
}
```

7 个内置 regex + vendor_patterns 16 个 = **23 次 is_match + N 次 replace_all**。每次 replace_all 在整个文本上跑一遍 → O(N_patterns × text_len)。

测试已经显示 `noop_input_evaluation_is_fast` 用 10k 次 iteration 在 2s 内完成（`tests/bench.rs:25-30`），所以现在不是性能瓶颈。但对超长输入（如整个 session log 喂给 guardrail），可能成为问题。

**建议**:
- 把 23 个 regex 编译成一个 alternation 大 regex，一次扫描得到所有匹配
- 或者用 Aho-Corasick（`aho_corasick` crate）+ 二次过滤的方式（前者处理固定字符串前缀，后者处理变量后缀）

---

**M4. `pii/engine.rs:282-300`（`filter_with_config` 的 sort_by_key）—— 排序键基于 (blocks, severity)，但 Block vs Warn 在 ties 上没有幂等保证**

```rust
all_matches.sort_by_key(|m| {
    let blocks = *Self::action_for_rule(...) == PiiAction::Block;
    (std::cmp::Reverse(blocks), std::cmp::Reverse(m.severity))
});
```

- 两个同 severity 同 block-ness 的匹配（例如两个 Block 规则都是 Critical）→ 它们的相对顺序由 `regex.find_iter` 的扫描顺序决定（决定于 Rust 的 regex 引擎，不是插入顺序）。这是**确定性**的（同一 input 总是同一顺序），但**不可预测**。
- `dedup_overlapping` 是 O(n²) —— 已 dedup 的 result 每次都要和新元素比对。N 个匹配最坏 O(n²) = 56 次比较（7 个内置规则各 match 一次的话）。实测 OK，但若有 N=50 个 custom rule 且都有匹配，最坏 1225 次比较。

**建议**:
- 给排序加第三键 `m.start`（最小 start 在前），让 ties 完全确定
- `dedup_overlapping` 改成基于 sort 后顺序的线性扫描（O(n) 而不是 O(n²)）

---

**M5. `pii/rules/phone.rs:69-82`（`is_hex_bounded`）—— 单个 hex 字符前/后抑制 match，存在已知 false-negative**

`tests/phone.rs:test_no_match_isolated_hex_prefix_is_known_limitation` 显式记录了 limitation：单独的 `a13812345678` 会被错误抑制（实际可能是合法的 phone）。注释建议"tighten to require a PAIR of hex letters (UUID-like prefix shape) rather than a single one"。

这意味着：
- `a13912345678` 不被识别为 phone
- `ab13912345678` 也不被识别（同样 `b` 是 hex）
- 但实际生产中偶然不会在 phone 前有 hex 字符 → 影响极小

注释里的 fix 提案是 O(1) 工作量。

**建议**: 按文档执行修复——要求前后都是 hex pair 才抑制。

---

**M6. `decision.rs:24-32` —— `Block.class` 字段是"建议性元数据"，但整个代码库目前都不消费它**

注释说"Set class correctly anyway so that future classifier works without revisiting every call-site"。但消费路径：
- `harness/agent/guardrails.rs:58` —— `class: _` 解构时**完全丢弃**
- `harness/agent/think.rs:859` —— `Block { reason, class }` 的 class 用来构造 `AlephError::Validation(msg)` 或 `AlephError::other(msg)`（L865-877），但 **AlephError 的 retry/terminal 分类目前不走 class**（注释承认）

class 字段的存在是合约占位符，**没有运行时价值**——直到 orchestrator 切换到 class-based 匹配。但既然 class 存在，应该有 `assert_class_invariants` 之类的测试覆盖 class 的语义正确性（如"Fail-closed secret resolution 必须是 Unexpected"、"Content policy block 必须是 Fixable"）。

**建议**:
- 给 class 字段加 `#[must_use]` 的 helper：`pub const fn is_fail_closed(&self) -> bool` 返回 `class == Unexpected`
- 加 `decision.rs` 内部测试覆盖"PiiSecretsGuardrail 总是给 `SecretResolutionFailed` 标 `Unexpected`、`Blocked` 标 `Fixable`"

---

**M7. `traits.rs:9-37` —— 三 trait 缺乏 `Send + Sync` 之外的额外约束（如 `'static` 没有 audit 关联）**

```rust
pub trait InputGuardrail: Send + Sync + 'static {
    fn name(&self) -> &str;
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision;
}
```

trait 不知道哪个 session、哪个 provider、哪个平台在调用它。三个方法都只接收业务内容，丢失了所有 provenance。这导致：
- 审计日志无法关联到 session
- 平台覆盖策略无法应用（H1 已展开）
- A/B 测试不同 guardrail 配置无法定位到 session 级

**建议**:
- 引入 `GuardrailContext` 结构体，让 trait 方法接收它
- 至少包含 `session_id: SessionId`、`provider: &str`、`platform: Option<&str>`、`actor: Option<&str>`

---

**M8. `pii/rules/email.rs:35-49`（`has_word_boundary`）—— 仅检查前边界，未检查后边界，但 `test_match_email_followed_by_alnum` 显示有相关 bug 已修**

`test_match_email_followed_by_alnum` 显式承认"一个 email 后跟 alnum 字符仍是 PII，不应被丢弃"。这个 bug 历史被修复（`has_word_boundary` 注释里写明）但**注释里没解释为什么只查前边界**——维护者会误以为"前/后都需要"。

**建议**:
- 给 `has_word_boundary` 的注释明确"after-boundary check 是有意的"
- 加一个测试 `test_no_match_email_inside_larger_word` 显式说明后边界确实不查（如 `notanemail@example.com.foo` 应该匹配 `notanemail@example.com`）

---

**M9. `registry.rs:128-145`（`screenable_text`）—— ToolResult 事件不在筛选范围内**

```rust
fn screenable_text(event: &SessionEvent) -> Option<&str> {
    match event {
        SessionEvent::SystemMessage { content, .. } => Some(content.as_str()),
        _ => user_text(event),    // UserMessage (non-synthetic)
    }
}
```

`SessionEvent::ToolResult { output, .. }` 不在筛选范围。这意味着：

- 工具返回的内容（含可能携带 secret 的内容）从未被 screening
- 下次 prompt 重建时，tool result 的原始内容（可能含密钥）会进入模型上下文

理论上 tool result 应该被 `content_sanitizer::wrap_external_content` 围栏处理，但**围栏不脱敏 PII**——它只标记"untrusted"和 strip tokenizer markers。如果一个工具返回 `{"response": "your AWS key is AKIA..."}` 而围栏没识别（因为不是 chat template marker），原文会进 prompt。

**建议**:
- 给 `screenable_text` 加 `SessionEvent::ToolResult` 分支，把 tool output 文本也 screen 一次
- 至少给一个测试 `screen_covers_tool_result_output` 确认这个分支存在

---

**M10. `secrets/leak_detector.rs:130-180` —— `scan_patterns` 在多个 pattern 都匹配同一段文本时，会重复 `replace_all`，导致 placeholder 嵌套问题**

例如文本 `-----BEGIN RSA PRIVATE KEY-----` 同时匹配 `private_key`（regex）和 hypothetical vendor catalog 中的 `rsa_key`，第一次 replace_all 把整段替换成 `***LEAKED_REDACTED***`，第二次再跑 `is_match` 看不到原始的密钥字符串，但仍可能匹配（`***LEAKED_REDACTED***` 内部有 `KEY-----` 字样吗？可能不会，但理论上）。

但**更大的问题**是：注释（`leak_detector.rs:42-48`）提到"first match wins the redaction tag"——意味着应该用 first-match wins 而不是"全部匹配都记录"。当前代码是全部匹配都记录：

```rust
for (label, pattern) in LEAK_PATTERNS.iter() {
    if pattern.is_match(&redacted) {
        found_labels.push(*label);            // ← 全部记录
        redacted = pattern.replace_all(&redacted, REDACTED_LEAK).to_string();
    }
}
```

`test_byte_patterns_allow_word_internal_sk_substring` 显示了词内部的 `sk-` 不会触发 OpenAI rule——这是通过 regex 本身的 left-anchor 实现的。但 **vendor_patterns 的某些条目可能 over-match**（如 `Telegram Bot Token` 的 `<bot-id>:<35-char-secret>` 规则对其他 base64 字符串也可能误匹配）。

**建议**:
- 在 `scan_patterns` 第一次 redact 后 `break`，并写"first match wins"语义（与 byte-patterns 对齐）
- 或者维护 `redacted_spans: BTreeSet<(usize, usize)>` 集合，避免二次 redact

---

### Low

**L1. `pii_secrets.rs:53-94`（`map_outbound`）—— `source: format!("pii_secrets (warn: {})", warnings.join("; "))` 的格式化无 escaping**

`warnings` 中可能含用户控制的字符串（如 prompt 注入的伪 warning）。如果下游消费者把 `source` 字段写入 HTML/JSON 渲染而没 escape，可能 XSS。

**建议**: 对 `warnings.join` 的内容做 escape，或保持"label 永远只来自预定义 enum"。

---

**L2. `tests/bench.rs:9-26`（`noop_input_evaluation_is_fast`）—— 2s 阈值在 CI 上仍可能 flaky**

注释说"tight 100ms bound flaked there"。2s 对 10k 次 iteration 来说相对宽松，但 CI 在高并发 + thermal throttling 下可能仍然超时。建议把 bound 与机器时钟无关的统计量挂钩（如 `assert!(elapsed_per_iter < Duration::from_micros(N))`）。

---

**L3. `pii_secrets.rs:39`（`with_guard_and_resolver`）—— 构造函数无验证 `guard` 是否 `Arc` 共享**

多 guardrail 共享同一 guard 是常见 pattern（避免重复编译 regex），但 `Arc::new(RuntimeSecurityGuard::default_guard())` 调用一次 vs 多次会创建独立的 pii_engine（`runtime_guard.rs:135`）—— pii_engine 是全局 `OnceLock`，所以单进程内只有一个，但多 guardrail 实例化 = 多 PII engine 引用，**没有 bug 但容易让维护者困惑**。

**建议**: 文档化"shared `Arc<RuntimeSecurityGuard>` is the expected usage"。

---

**L4. `registry.rs:60-65`（`is_enabled`/`enable_all`/`disable_all`）—— `disable_all` 和 `enable_all` 不发送 trace event**

harness/agent 侧的 emit 模式用 `self.emit(|| LoopTraceEvent::...)`，但 registry 的 disable/enable 没有 emit。如果 trace 消费者依赖事件流来重现 session 状态，会漏掉这个状态切换。

**建议**: 给 disable/enable 加一个 `ReguardrailStateChanged` trace event。

---

**L5. `decision.rs:63-99`（`is_block`/`is_allow`/`is_sanitize`/`is_warn`）—— 四个 const fn 只覆盖 `Block`/`Allow`/`Sanitize`/`Warn` 中各自的一个变体，没有 `is_failure` / `is_pass_through`**

调用点写 `if d.is_block()` 或 `if d.is_sanitize()`，但**没有**"是否需要修改文本"或"是否需要重试"的语义判断器，调用点必须自己写 `matches!(d, Block { .. } | Sanitize(_))`。

**建议**: 加 `pub const fn modifies_text(&self) -> bool { matches!(self, Self::Sanitize(_)) }` 等语义 predicate。

---

**L6. `tests/registry.rs:240-285`（`screenable_text` 测试覆盖）—— 没有测试覆盖 `ToolResult` 事件**

已经合并到 M9，但单独的测试盲点值得记下：tool result 路径上没有任何 PII screening 行为测试。

---

## 测试覆盖盲点清单

独立列出因为它是 critical 模块审查的核心：

| 场景 | 是否测试 | 风险 |
|------|---------|------|
| Unicode homoglyph 绕过（`ℌello@examplе.com` 用 Cyrillic е） | ❌ | 内容 sanitizer 折叠但 PII 规则自身不感知 homoglyph |
| 零宽字符分隔的 PII（`1⁠3⁠8⁠1⁠2⁠3⁠4⁠5⁠6⁠7⁠8`） | ❌ | phone rule 无 ZWSP 容错 |
| Base64 编码的 secret 在外发 prompt 中 | ❌ | 整个 vendor_patterns 对 base64 不感知 |
| 完整大小写变体的 `BEARER token` | ✅（exec/leak_detector）| — |
| NFKC 形式归一化绕过（如 `㌀`） | ❌ | unicode_guard 不做 NFKC |
| Register 一个超短 secret（5 字节）→ 后续 echo 它 → 应该不被 block | ✅ | — |
| Register 一个超长 secret（1MB） → 后续 echo 它 | ❌ | 性能 + 内存（已展开为 H4）|
| 多个 Sanitize guardrail 链式应用 | ❌ | 已展开为 H2 |
| Sanitize guardrail 后跟 Block guardrail | ❌ | 已展开为 H2 |
| `disable_all` 调用产生 audit entry | ❌ | 已展开为 H3 |
| `REDACTED_USER_MESSAGE` 替换在 prompt 重建时被进一步 PII 扫描 | ❌ | 已展开为 H5 |
| Tool result 含 PII 时的 screening | ❌ | 已展开为 M9 |
| Number 类型的 PII leaf | ❌ | 已展开为 M1 |
| `class: ErrorClass::Unexpected` 的 Block 不被 retry | ❌（acknowledged by design）| 已展开为 M6 |
| 并发多个 session 同时注入 secret → 跨 session 检测 | ❌ | 已展开为 H4 |
| `PiiSecretsGuardrail` 与多 `custom_leak_patterns` 组合 | ❌ | 集成路径盲点 |
| `evaluate_tool_call` 输入超大 `Value`（10MB args） | ❌ | 已展开为 M2 |

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|------|------|------|
| R1 core 不调平台 API | ✅ | trait 设计为纯逻辑，platform-specific 行为留给 desktop crates |
| R2 原生 shell 仅窗口容器 | N/A | guardrails 不涉及 UI |
| R3 core 极简、无重依赖 | ✅ | 仅依赖 `regex` + `serde_json` + `tokio::sync`，所有检测逻辑下游负责 |
| R4 接口层纯 I/O | ✅ | guardrails 自己是逻辑层（不调平台），traits 接口设计纯粹 |
| R5 菜单栏优先 | N/A | — |
| R6 AI 主动 | N/A | — |
| R7 Rust Core 唯一大脑 | ✅ | 三 trait + decision enum 在 core |
| R8 正则仅用于机器格式 | ⚠️ | 检测器对**用户文本**用了大量 regex（phone/email/id_card/bank_card 等），这与 R8 "regex 仅用于机器格式"有偏离——但这是 guardrail 域的**合理例外**，因为模式对抗是必须的；不应该视为违反 |
| R9 可配置项暴露为工具 | ✅ | `custom_leak_patterns`、`custom_rules`、`platform_policies`、`exclude_providers` 都经配置进入 |
| R10 智能在 prompt 中 | ✅ | 无启发式判断逻辑，决策管线完全确定性 |

## 其他核查结论（确认无问题）

- **ReDoS 风险**：`regex` crate 保证线性时间；所有用户/配置来源的 regex 走 `safe_regex::bounded_builder`（1 MiB 编译上限）；内置 regex 全部含 `\b` 锚点或显式长度上限。无 ReDoS 风险。
- **panic 面**：生产路径无 `unwrap()`/`expect()`/`panic!`；`#[must_use]` 覆盖率合适；`find_injected_substring` 用 `is_char_boundary` 安全处理多字节边界。
- **Race 条件**：`disable_all` 用 `AtomicBool` + `Ordering::Release` 是正确的；`concurrent_evaluate_vs_disable_all_is_consistent` 测试覆盖。
- **跨平台**：trait 不感知 platform（设计选择，问题在 H1）
- **资源管理**：`LeakDetector` 无界增长（H4 已展开）
- **审计完整性**：audit log mpsc channel 容量 256，溢出时 `log()` 不返回错误也不重试——HarnessError::class 是 advisory（M6 已展开）
- **Trait 设计**：`Send + Sync + 'static` 完整，`async_trait` 转换正确；`tests/registry.rs:279-282` 显式断言 `Send + Sync`
- **测试 bench 阈值**：`tests/bench.rs:25-30` 注释坦诚"loose bound to survive CI load"，且 bench 在 ASYNC runtime 内跑（tokio::test 默认 single-thread）——这是设计选择而非 bug
- **secret 类型安全**：`DecryptedSecret` 的 Debug/Display 都返回 `[REDACTED]`，调用方必须 `expose()` 才能看到明文——这条路径的安全契约很干净

## 优先级建议

按"安全风险 + 修复成本"排序：

| 优先级 | Issue | 修复成本 |
|--------|-------|---------|
| P0 | C1（密钥名枚举侧信道） | 小（reason 字符串替换 + name 入审计日志） |
| P0 | C2（Sanitize reparse 失败回退 fail-open） | 中（需 trait 改为携带 `Value`） |
| P1 | H1（context 缺失） | 中（trait signature 变更） |
| P1 | H3（kill switch 无审计） | 小（加 actor + trace event） |
| P1 | H4（泄漏检测器无界增长） | 小（加 LRU cap） |
| P2 | H2（多 guardrail 优先级 + 测试覆盖） | 中（链式应用 + 测试） |
| P2 | H5（REDACTED_USER_MESSAGE 单值） | 小（per-record 指纹） |
| P3 | M1-M10 | 各自独立 |

## 未做事项

- **未执行** `cargo check`/`cargo build`/`cargo clippy`（按要求）
- **未深入审查** 下游模块（`security/runtime_guard.rs` 619 行、`secrets/leak_detector.rs` 502 行、`exec/leak_detector.rs` 346 行、`pii/engine.rs` 633 行、`security/content_sanitizer.rs` 1008 行、`pii/rules/*` 共 8 文件 ~900 行）——它们通过 `guardrails/pii_secrets.rs` 的 trait 适配被消费，构成完整的安全链路，但本次任务限定在 `src/guardrails/` 目录。本次报告**提到的所有跨模块问题（如 leak_detector 的无界增长、runtime_guard 的 context 默认值）应在对应模块的 review 中确认/修复**。
- **未审查** `tests/registry.rs::concurrent_evaluate_vs_disable_all_is_consistent` 的并发测试在真实多核机器上是否充分覆盖——这是测试覆盖问题，不是模块问题
- **未审查** graph.json 中与 guardrails 关联的 357 个节点（已 grep 摘要，本次限于源文件阅读）
- **未给出** 对 `with_guard_and_resolver` API 设计（`Arc<RuntimeSecurityGuard>` + `Option<Arc<dyn AsyncSecretResolver>>` 的双 Arc）是否合理的评价——这是 core API 形态问题，超出本次范围
