# shared-rest 静态审查报告

- **审查单元**: shared-rest — `shared/client/` + `shared/logging/` + `shared/ui_logic/`
- **基线**: `/tmp/aleph-review-batch-5`（git worktree，与 main 一致）
- **方式**: 无 diff 全量静态阅读（rust-doctor 的 `/tmp/rd-shared.json` 为空文件，未提供线索，全部结论均亲自读码确认）
- **日期**: 2026-07-22

## 统计

| 路径 | 文件数 | LOC（含测试） |
|------|-------:|------:|
| shared/client | 6 | 865 |
| shared/logging | 5 | 755 |
| shared/ui_logic | 16 | 1054 |
| **合计** | **27** | **2674** |

无超过 500 行的文件（最大 `ui_logic/src/safety/prompt_injection.rs` 381 行）。

## 发现汇总

| 严重级 | 数量 |
|--------|-----:|
| Critical | 0 |
| High | 0 |
| Medium | 1 |
| Low | 8 |

**无 Critical / High 发现。**

## 发现列表（按严重级排序）

### Medium

**M1. `shared/ui_logic/src/protocol/rpc.rs:74` — `RpcClient::call` 无超时，服务器不应答时永久挂起且 pending 条目泄漏**

`call` 在 `rx.await` 上无限等待（rpc.rs:74），没有任何超时机制。`RpcError::Timeout` 变体（rpc.rs:18-19）已定义但全 crate 从未构造——属死代码兼功能缺失。服务器静默丢包/忘记响应时：调用方 future 永久挂起，`pending` HashMap 中的 oneshot sender 永久驻留（内存泄漏随调用数增长）。对比 `shared/client/src/connection.rs:303` 的 `call_with_timeout` 有完整超时+pending 清理，此处明显缺失。
**修法**: 为 `call` 增加超时（WASM 环境可用 `gloo-timers` 或 `wasm-bindgen-futures` 包装），超时路径中 `self.pending.borrow_mut().remove(&id)`；或删除 `Timeout` 变体并在文档中声明由调用方负责超时。

### Low

**L1. `shared/logging/src/pii_filter.rs:22` — 历史问题现状确认：`PiiScrubbingLayer` 仍是公开 no-op，但已缓解（历史 High → 现 Low）**

2026-07-20 旧审查报告的高危项。当前代码：该类型仍是公开的 passthrough `Layer`，但 (a) 文档明确标注 "PASSTHROUGH ONLY / 仅为向后兼容保留"；(b) 首次观察到事件时 emit 一次性 `warn!`（pii_filter.rs:27-33）提示运维改用 `create_pii_scrubbing_layer()`；(c) 真实 PII 擦除由 `PiiScrubbingFormat` 承担，且 `file_appender.rs:100,109` 的 console/file 两个默认输出层均已安装该 Format——默认路径不存在"误以为在擦除"的窗口。残留风险：该 no-op 仍从 `aleph_logging::PiiScrubbingLayer`（lib.rs:31）公开可达，下游手动 `with(PiiScrubbingLayer)` 依然得不到保护（仅在日志里留一条 warn）。
**修法**: 标注 `#[deprecated(note = "use create_pii_scrubbing_layer()")]` 引导编译期迁移，待下游（含 `tests/steps/logging_steps.rs` 与 `src/logging/` 的镜像副本）清理后删除。

**L2. `shared/ui_logic/src/protocol/rpc.rs:80-91` — 畸形响应（有 id 但无 result/error）被吞为误导性 `ChannelClosed`**

`handle_response` 中：响应含合法 id 但既无 `error` 也无 `result` 时，pending 条目被 remove、sender 直接 drop，调用方收到 `RpcError::ChannelClosed`——错误信息完全掩盖了真实原因（服务器协议违规）。
**修法**: 两个分支都不命中时 `let _ = tx.send(Err(RpcError::ServerError("malformed response: no result/error".into())))`。

**L3. `shared/ui_logic/src/connection/wasm.rs:86` — WASM `connect` 等待 `onopen` 无超时，不可达主机上永久挂起**

`open_rx.await`（wasm.rs:86-88）没有超时包装。浏览器对不可达主机只触发 `onerror`（且 `onerror` 回调仅 console 打印、不唤醒 open oneshot），`connect` 永远不返回。`failure.rs:29` 的 `FailureStage::BeforeOpen` 注释提到 "connect()/open timeout"，说明调用方预期自行包装超时，但该约束未在 `AlephConnector::connect` 的 trait 文档中声明。
**修法**: 在 connector 内或 trait 文档中明确超时责任；推荐 connector 内部用 `wasm_bindgen_futures` + timeout 包裹 `open_rx`。

**L4. `shared/client/src/connection.rs:99-123` — read_loop 异常退出时不 fail 任何 pending 请求**

读循环因 `Err(e)` 或 Close 退出后（connection.rs:113-122），仅翻转 `connected` 原子位；`pending` map 中已注册的请求不会被唤醒，各调用方只能等满自己的 30s 超时才发现断连。
**修法**: read_loop 退出前 drain `pending`，向每个 sender 发送 `Err(JsonRpcError)` 使等待方立即失败。

**L5. `shared/client/src/gateway_client.rs:88-97` — `connect` 握手响应从不校验**

one-shot 客户端发出 `connect`（id=0）后不检查其响应：握手失败（如 AUTH_REQUIRED）的 error 帧被响应循环按 "id != 1" 跳过，随后方法调用的结果只能是 socket 关闭后的 `Disconnected` 或读超时——真实鉴权错误被掩盖，排障困难。
**修法**: 在发送业务请求前先读一帧并校验 id=0 的握手响应，若含 `error` 直接返回 `CliError::Rpc`。

**L6. `shared/logging/src/pii.rs:28-52` — 生产代码使用 `.expect()`（9 处静态正则编译）**

违反 AGENTS.md "生产代码禁止 `unwrap()`/`expect()`"。静态正则字面值不可能编译失败，属可接受惯例，但与明文规定冲突。
**修法**: 二选一：在 AGENTS.md 中为 "静态正则/常量初始化" 立豁免条款；或改用 `Regex::new(...).unwrap_or_else(|e| unreachable!(...))` 之类的显式不可达路径（价值有限，更推荐修文档）。

**L7. `shared/ui_logic` — 4 个空占位模块 + 1 个空 feature**

`api/mod.rs`、`observability/mod.rs`、`protocol/events.rs`、`protocol/streaming.rs` 均为空文件（仅空行），但由 `lib.rs:1-6`、`protocol/mod.rs` 公开导出；`Cargo.toml:42` 还定义了不启用任何代码的 `observability = []` feature。死占位，污染公共 API 面。
**修法**: 删除空模块与空 feature，或填入实际内容前保持私有。

**L8. `shared/ui_logic/Cargo.toml:14` — `uuid` 依赖未被任何源码使用**

全 crate grep `uuid` 仅命中 Cargo.toml 自身。且其带 `js` feature，在非 WASM target 上属无谓负担。死依赖。
**修法**: 从 Cargo.toml 移除。

## 架构红线合规快照

| 红线 | 结论 | 说明 |
|------|------|------|
| R1（core 不调平台 API） | ✅ 合规 | 本单元非 core；`wasm.rs` 的 web_sys 是 UI shell 连接器且 `wasm` feature-gated |
| R3（core 极简/无重依赖） | ✅ 合规 | 三 crate 依赖均轻量；`logging` 引入 `regex` 仅用于 PII 机器格式；`ui_logic` 刻意避免 regex（prompt_injection.rs:37-39 注释说明 WASM 体积考量）。唯一瑕疵是死依赖 uuid（L8） |
| R4（接口层纯 I/O） | ✅ 合规 | `client` 为协议客户端，`ui_logic` 明确只放 "decisions" 纯逻辑、副作用留给各 surface（state/mod.rs 注释） |
| R7（Rust Core 唯一大脑） | ✅ 合规 | prompt_injection 启发式自称 "soft check，final authority 在服务端安全管线"（prompt_injection.rs:7-8），不越权 |
| R8（LLM 负责意图/路由；正则仅机器格式） | ✅ 合规（边界注意） | pii.rs 正则全部针对 email/卡号/key 等机器格式；prompt_injection.rs 用 substring 启发式做安全提示而非意图路由，属合理使用 |
| R9（可配置项暴露为工具） | ⚠️ 残留 | `PiiScrubbingLayer` 公开 no-op 开关仍存在（L1），已加运行时 warn 缓解但未彻底移除 |
| R10（智能在 prompt） | ✅ 合规 | 无中间层智能逻辑 |

## 已验证但**不**报告的点（避免误报）

- `connection.rs:136-137` 日志截断已按 UTF-8 字符边界处理（无多字节 panic）。
- `connection.rs:280-300` 锁顺序与 pending 清理注释与实现一致，`write → pending` 不嵌套。
- `retention.rs` 文件名匹配严格限定 `aleph-*.log[.YYYY-MM-DD]`，不会误删 backup/其他文件（有测试覆盖）。
- `config.rs:112-123` 配置文件 chmod 0600，失败有 warn——无世界可读泄漏。
- `wasm.rs` 的 `Closure::forget()` 泄漏是 wasm-bindgen 事件回调的标准做法，且 `onclose` 已正确唤醒接收流（wasm.rs:65-80 注释说明）。
- `gateway_client.rs` 默认 `ws://127.0.0.1:18790` 为回环地址明文，LAN-trust 模型文档化（connection.rs:326-333），无证书/SSRF 问题。
- `file_appender.rs:48` OnceLock 缓存首次初始化结果（含失败）是文档化行为，每次调用均返回缓存的错误而非静默吞掉。
