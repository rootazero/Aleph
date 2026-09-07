# Review: src/wizard

## Summary
- 文件数:6 (mod.rs / session.rs / prompter.rs / types.rs / flows/mod.rs / flows/onboarding.rs)
- 估计 P0 / P1 / P2 数量:**0 / 2 / 5**
- 整体评估:**有风险**。模块本身的并发模型、信道生命周期、答案校验都设计良好且有详尽测试,但有一个已知的「数据被丢弃」功能缺口,使 onboarding flow 实质上是 no-op;另外 `RpcPrompter::prompt` 的超时分支在极端边界上存在答案丢失的竞争。

## Findings

### [P1] flows/onboarding.rs:271-287 — OnboardingData 收集后被静默丢弃(wizard 实际上是 no-op)

`run()` 把用户填好的 provider / model / api_key / thinking_level / messaging_apps 累加进局部 `OnboardingData`,然后函数返回 `Ok(())`。`OnboardingFlow` 没有把这个结构传出任何渠道(没有 `WizardFlow::run` 的结果类型,`WizardSessionManager` 也没有消费 flow 结果的字段),所以 `outro("Aleph is ready!")` 的承诺是不成立的——配置没有任何持久化或回写到 gateway / start 引导路径。

代码里有 `KNOWN GAP (deferred)` 注释,说明作者意识到了这一点。但作为静态审查,这就是 P1:用户完成了 onboarding,UI 显示成功,实际配置被丢。

建议修复:
- 把 `WizardFlow::run` 改成 `async fn run(&self, ...) -> Result<FlowOutput, WizardSessionError>`,其中 `FlowOutput` 是 `serde::Serialize` 的通用 `Value` 或 `Box<dyn Any + Send>`,由各 flow 自行定义。
- `WizardSession` 在 spawned task 拿到 `FlowOutput` 后,通过 `mpsc` 把结果回传给 `WizardSessionManager`,再由 manager 通过 `wizard.status` 的扩展返回给调用方。
- `start/mod.rs:865` 的 `WizardFlowFactory` 已经接收 `initial_data: Option<Value>`,顺着这个口子把输出塞回去最干净。

(此改动触及 gateway 与 start 引导,**不在本模块内可单独修复**,需要跨模块协商契约,因此当前不阻塞但应单独排期。)

---

### [P1] prompter.rs:140-166 + session.rs:281-296 — `prompt` 超时与 `answer` 在 answers HashMap 上的锁竞争会让用户答案被静默吞掉

时序(都需要恰好踩在 `ANSWER_TIMEOUT` 的 15 分钟边界上):

1. tokio 计时器触发 `Err(_)`,`RpcPrompter::prompt` 抢到 `answers.write()`,**移除**自己的 entry → `reclaimed = Some`。
2. 客户端几乎同时通过 `WizardSession::answer(step_id, value)` 也尝试抢同一把写锁;等它获得锁时 entry 已没了。
3. `answer()` 进入 `None` 分支,因为 `step.step_type != Note`,返回 `WizardSessionError::InvalidAnswer("Step 'X' has already been answered")`。
4. `prompt` 端因为 `reclaimed.is_some()`,**跳过了 `rx.try_recv()`** 的兜底,直接返回 `AnswerTimeout`。
5. 结果:客户端拿到「already answered」错误,流程任务拿到 `AnswerTimeout`,真实的 value 在 JSON-RPC handler 的局部变量里被丢弃,session 进入 Error 终态。

窗口极窄(锁竞争 + 计时边界),但由于 `ANSWER_TIMEOUT = 15min`,这是一个「客户端正在认真回答案」和「服务端判断超时」几乎同时发生的高价值场景——典型如 API key 输入完毕按下回车的瞬间恰好撞上超时。

建议修复(最小改动方向):
- `prompt` 超时分支把 `if reclaimed.is_none() { rx.try_recv().ok() } else { None }` 改成「无条件尝试 `rx.try_recv()`,然后再判断 reclaimed」——这样即使用户答案在我们抢锁之后送达,也能拿到。
- 或者让 `session::answer` 在 entry 已不存在但 `step_id` 匹配 current_step 时,不立刻返回错误,而是检查这个 step_id 对应的 oneshot 是否已经发送过值(需要 `answers` 同时持有 `Weak<oneshot::Receiver>` 才能观察,代价较大)。
- 最小可行修复:接受 `oneshot::Receiver::try_recv()` 的「迟到也算成功」语义,把 `prompt` 的超时分支简化成:

```rust
Err(_) => {
    self.answers.write().unwrap_or_else(|e| e.into_inner()).remove(&step_id);
    if let Ok(value) = rx.try_recv() {
        return Ok(value);  // 用户的答案赶在超时前送到了
    }
    Err(WizardSessionError::AnswerTimeout { step_id, timeout_secs: ANSWER_TIMEOUT.as_secs() })
}
```

---

### [P2] flows/onboarding.rs:259-263 — 用户「拒绝应用」被映射成 `WizardSessionError::Cancelled`

`review_and_finalize` 在用户对 `Apply this configuration?` 选 `false` 时,直接 `return Err(WizardSessionError::Cancelled)`,session 状态被翻成 `Cancelled`。

这跟 `configure_secondary` 里 `wants_secondary = false; return Ok(())` 是不同的语义——同样是用户的「不」,一个是完成,一个是取消。客户端看到 `status == Cancelled` 可能会误以为是被踢出/超时/系统取消,而不是用户主动放弃。

建议修复:
- 给 `WizardStatus` 加一个 `Declined` 变体(serde lowercase `"declined"`),或
- 在 `WizardNextResult::error` 之外加一个 `declined(reason)` 工厂函数,继续走 `Done` 终态但带 `error: Some("User declined to apply")`。

不算阻塞,但目前的实现把两种语义糅在一起,对 UX 和后续遥测都会造成困扰。

---

### [P2] session.rs (整个文件) — `std::sync::RwLock` 在 async 上下文里使用,违反项目自身 `sync_primitives` 文档的指引

`src/sync_primitives.rs:39-45` 明确说:

> Async `RwLock` for tokio contexts. Daemon and other async modules use this instead of `std::sync::RwLock` to avoid deadlocks when holding a guard across `.await` points.

而 `wizard/session.rs` 与 `wizard/prompter.rs` 全用 `crate::sync_primitives::RwLock`(也就是 `std::sync::RwLock`)。当前所有 critical section 都是「读/写一行然后立刻 drop guard」,没有持锁 await,所以**当下没有死锁**。但这是脆的:任何后续 PR 在 critical section 里加一个 `.await`(比如「写完 status 之后 await 一个 telemetry 钩子」),就会立刻产生持锁跨 await 的隐患,且这种 bug 很难复现。

另外,代码里大量 `read().unwrap_or_else(|e| e.into_inner())` / `write().unwrap_or_else(|e| e.into_inner())` 模式——这是在 panic poisoning 之后**继续**使用锁内数据的兜底。当前保护的 state 都是简单赋值(`status`, `current_step`, `error: Option<String>`, `answers: HashMap<…>`),数据本身不会被 panic 损坏,所以这种处理可以接受;但它意味着「panic 时锁状态不一致」这一不变量是被默默承担的,值得在文件顶部留一段注释说明设计意图。

建议修复:
- `WizardSession` 内部的 `status` / `current_step` / `error` / `cancel_tx` 改用 `sync_primitives::AsyncRwLock`(本质是 `tokio::sync::RwLock`)。`answers` 因为只在 `RpcPrompter` 内部短暂持有,保持 `std::sync::RwLock` 也可,但为了一致性也可以换。
- 或者保留 `std` 锁,在文件顶部明确写出「critical sections MUST NOT contain `.await`」,并把 `unreachable!`-style 的注释贴在每个锁使用点。

---

### [P2] prompter.rs:142-156 — `step_tx.send(step).await` 与 `answers.remove` 之间存在窗口,send 失败后清理路径正确但消息泛化

`prompt` 的清理逻辑是对的(「send 失败就 remove entry」)。但失败时的错误信息是固定的 `"Channel closed"`,跟初始发送失败 (`mpsc::error::SendError(value)`) 的实际原因不区分——channel closed / 接收端 dropped / 队列被取消,在客户端日志里看到的都是同一句话。

进一步说,如果 send 因为 channel 满了 + 客户端连接断开,用户层面看到的是 `Internal("Channel closed")`,而真正的事件流是「客户端先断开 → channel 被 drop → send 失败」,错误消息应该能帮调试时定位。

建议修复:把 `SendError(step)` 的 `step.id` 拼到错误信息里(`"Channel closed while sending step 'step-1'"`),至少保留 step_id 上下文。`step` 本身不应进入错误消息(它可能含 placeholder/sensitive 标记)。

---

### [P2] session.rs:130-145 — `validate_answer` 对无 `options` 的 Select/MultiSelect 完全放行,可能掩盖 flow 端 bug

`offered` 的 `is_none_or` 闭包使得 `options: None` 时任意值都接受。test 里 `bare_select = Step::new("s", StepType::Select); validate_answer(&bare_select, &json!("anything")).is_ok()` 印证了这是 intentional。但效果是:如果某个未来的 flow 漏掉 `step.options = Some(...)`,客户端可以提交任意 JSON,然后 `RpcPrompter::select<T>::from_value` 才在类型层失败,返回 `InvalidAnswer("invalid type: object, expected a string")` 之类的——错误源头是 flow 配置错误,但被表达成「客户端答案非法」。

当前 onboarding flow 的所有 Select 都正确传了 options,所以这个 footgun 不会触发实际故障。但鉴于 `WizardStatus` 都标了 `non_exhaustive`、`StepExecutor` 已经预留扩展位,未来 flow 数量增加时这是个明显的隐患。

建议修复:在 `WizardStep::select` / `WizardStep::new(StepType::MultiSelect)` 构造时,**强制要求** `options.is_some()`(用 `debug_assert!` 或返回 `Result`),把 footgun 推到构造期而不是运行时。

---

### [P2] onboarding.rs:212-227 — secondary provider 与 primary 相同但用户拒绝「同 key」时仍会走二次 text 输入,语义上等价于「用户没填 key」

`configure_secondary` 的嵌套 `if/else`:

```rust
if secondary_provider != "ollama" {
    let api_key = if Some(&secondary_provider) == data.primary_provider.as_ref() {
        let use_same = prompter.confirm("Use the same API key as the primary provider?", true).await?;
        if use_same {
            data.primary_api_key.clone()
        } else {
            Some(prompter.text(...).await?)  // 走二次输入
        }
    } else {
        Some(prompter.text(...).await?)      // 走二次输入
    };
    data.secondary_api_key = api_key;
}
```

「用户对「same API key」选 No」分支下,会去问用户输入新 key,跟 secondary_provider 与 primary 不同时走的是同一条 `prompter.text` 路径——看起来对,但用户可能被绕晕(「我刚说我不要这个 key 了,为什么又要我输?」)。

更大的问题是:`secondary_api_key` 字段是 `Option<String>`,但这里永远只会赋 `Some(...)`(拒绝分支根本没有 `data.secondary_api_key = None;`)——实际上**没有让用户选择「不配 key」**的选项,只要选了 secondary provider 又不是 ollama,就一定要有 key。

建议修复:把 `text` 输入之后增加一个 `confirm("No API key? Continue anyway?", false)`,让用户能明确表态;或者文档化这个意图。

---

## 违反红线的情况

未发现直接违反 `AGENTS.md` 红线 R1-R10 的问题:

- **R1**(Core 不调用平台 API): wizard 模块仅用 std + tokio + serde + tracing + uuid,无平台 API 调用。✓
- **R3**(Core 最小化,无重 dep): uuid/tokio/serde/async_trait/thiserror 都是 workspace 已有 dep,wizard 没引入新重 dep。✓
- **R4**(Interface 层纯 I/O): wizard 是 state machine,被 gateway handler 调用,自身不含 I/O。✓
- **R7**(一个 core 多 shell): 与架构契合,核心 state machine 在 core,UI 通过 RPC handler 接入。✓
- **R8/R10**(LLM 走 prompt、regex 只用于机器格式): JSON 解析走 serde,无手写 regex。✓

唯一沾边的:**R3 的精神** —— onboarding flow 本身是个「用户配置 UI 的状态机」,严格说属于 UX 而不是 core 业务逻辑;但因为它纯粹在 Rust 状态机层、不涉及 Leptos/WASM 或平台 API,与 R2 不冲突。**不算红线违反**,但若严格按 R3,应考虑把 `flows/onboarding.rs` 抽到 `interfaces/cli` 或独立 crate。当前结构是「wizard framework in core + concrete flow in core」,可以接受。

---

## 建议但不阻塞(P3)

1. **`session.rs::next()` 的 terminal race**: `try_recv` 返回 `Ok(step)` 时 status 仍可能已经是终态,代码刻意保留了「缓冲步骤优先于终态」的语义(注释里说明了「notes 是 fire-and-forget」),正确但反直觉。考虑在 `WizardNextResult` 里给步骤带一个 `is_post_terminal: bool` 标记,方便客户端区分「正常步骤」和「在终态后到达的尾巴」。

2. **`onboarding.rs::review_text` 中显示 secondary 信息时使用 `format!("{} / {}", provider.as_deref().unwrap_or(""), model.as_deref().unwrap_or(""))`**,如果用户选了 secondary 但模型选择阶段报错被回退(目前没有这样的回退路径),会显示「 / 」。当前代码没有这种路径,但 format 字符串对空 Option 的容忍不够稳健。

3. **`prompter.rs::prompt` 的 `next_id` 是 `step-{counter}`,单调递增**:同一个 session 内永远唯一,但跨 session 重启后可能复用。对客户端来说 step_id 仅在 session 内有效,影响有限。

5. **`types.rs::StepExecutor` 只有 `Client` 一个变体,但标记了 `#[non_exhaustive]`**:预留扩展位正确,但目前没有任何代码路径使用 `executor` 字段(grep `step.executor` 无结果),字段是 dead state。如果近期没有 `Server` executor 的计划,建议暂时移除。

6. **`session.rs::record_step` 在已经处于终态时仍会写入 `current_step`**:虽然不影响正确性(下一步的 next() 会走 terminal_result),但写入是「在终态下还修改状态」。可以加一个 `if !self.is_done()` 守卫。

7. **`prompter.rs::ANSWER_TIMEOUT` 是 15 分钟**——对于 API key 输入是合理的,但 onboarding 有约 10 步,理论上用户可能被打断,每步都得重新计时。如果想强制重连后整体重新开始,可以加一个 session-level deadline。

8. **`session.rs::cancel()` 取走 `cancel_tx` 后,即使 flow task 已经 settle 也会再调一次 `Self::settle(...)`**:第二次 settle 是 no-op(已非 Running),所以正确,但稍冗余。可以在第一次 `if let Some(tx) = ...` 后直接 `Self::settle(...)`,省一次原子操作。

9. **`onboarding.rs::default()` for `OnboardingFlow` 与 `Self::new()` 行为完全一样**,可以删掉 `Default` impl 避免噪声。

10. **`gateway/handlers/wizard.rs::next()`** 里 `if let Some(_value) = answer` 永远返回错误——这是个明显的 dead branch,不在 wizard 模块内但和 wizard 的 RPC 协议耦合,值得顺手清掉。