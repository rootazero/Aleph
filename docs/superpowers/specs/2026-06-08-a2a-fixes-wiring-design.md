# A2A 模块:聚焦修复 + 连线 + 熵减

**日期**: 2026-06-08
**分支**: `feat/a2a-fixes-wiring`(worktree `/Volumes/TBU4/Workspace/Aleph-wt-a2a`)
**参考项目**: `/Volumes/TBU4/Github/{a2a, a2a-python, ra2a}`

## 背景

Aleph 的 `src/a2a/`(~10k 行,38 文件)已相当成熟:domain/port/service/adapter 分层完整,协议方法
(message/send、tasks/get|cancel|list、tasks/pushNotificationConfig×4、tasks/resubscribe、卡片发现)
均已实现并接入 `aleph-server` 启动、gateway 路由、`a2a_delegate` 工具、config。

与参考实现(ra2a 的 Rust 实现 + a2a 规范 + a2a-python)对比后,真实缺口收敛为 3 项:1 个互操作 BUG、
1 处未连线的死基建、1 处死 trait 表面。本次范围**不**引入分布式特性(Task 版本化 CAS、推送重试、
扩展传播),以遵守 R3(核心轻量)/ R6(奥卡姆)/ R10(薄 harness)。

## 范围

### 项目 1 — 修复 `message/stream` 互操作 BUG(双向)

A2A 规范的流式方法名是 `message/stream`(SendStreamingMessage);Aleph 两端都用了非标准的
`message/send`,导致与 ra2a / a2a-python 客户端、服务端互不流式互通。

- **服务端** `src/a2a/adapter/server/routes.rs:107-115`:stream 端点 `match` 仅匹配 `message/send` 和
  `tasks/resubscribe`,外部客户端发 `message/stream` 落入 `other =>` 被拒("Method does not support
  streaming")。
  **修改**:新增 `"message/stream" => stream_message_send(...)` 分支;**保留** `"message/send"` 作为
  向后兼容别名,二者指向同一 handler。
- **客户端** `src/a2a/adapter/client/http_client.rs:229`:流式请求 `method: "message/send"`。
  **修改**:改为 `"message/stream"`(规范名)。服务端现也接受该名,Aleph 自身闭环不破。

**验证**:新增测试断言 `message/stream` 走 stream 分支成功;`message/send` 别名回归仍可用。

### 项目 2 — 连线 Agent 健康监控(复用现有死基建)

`A2AClientPool::health_check`(`src/a2a/adapter/client/pool.rs:65`)与 `RegisteredAgent.health` 字段均
存在,但**零生产调用方**:卡片刷新只在启动跑一次,`health` 之后永不更新。

- **新增**周期任务 `spawn_health_monitor(registry, pool, interval)`,置于
  `src/a2a/service/card_refresh.rs`(与 `spawn_card_refresh` 职责相邻)。循环每个注册 agent:
  `pool.get_or_create(&agent)`(确保 client 入池,否则 `health_check` 因池中无 client 恒返回
  `Unreachable`)→ `pool.health_check(&agent.card.id)` → `registry.upsert(...)` 更新 `health` +
  `last_seen`。**纯复用** `get_or_create`/`health_check`/`upsert`,零新业务逻辑。
- **配置**:`src/a2a/config.rs` 的 `A2AConfig` 增 `health_check_interval_secs: u64`,**默认 0 = 关闭**
  (非破坏性,显式 opt-in)。
- **接线**:`src/bin/aleph-server/commands/start/mod.rs` 的 `spawn_card_refresh` 调用之后,
  `if interval > 0 { spawn_health_monitor(...) }`。

**验证**:wiremock 起一个/两个 agent,跑一轮 monitor,断言 registry 中对应 agent 的 `health` 被更新
(可达→Healthy,不可达→Unreachable)。

### 项目 3 — 熵减:删死 trait 方法

`AgentResolver` trait(`src/a2a/port/agent_resolver.rs`)有两个全 stub、零生产调用的方法:

- `resolve_by_intent`(:56)—— SmartRouter 走自己的三级路由 + `matcher.match_intent`,从不调它。
- `fetch_card`(:36)—— `card_refresh` 直接构造 `A2AClient`,从不经 resolver。(grep 证实零生产调用方。)

**修改**:从 trait 定义移除这两个方法,并删除其全部实现:`card_registry.rs` 的 stub 实现 + 3 处测试
mock(`tests.rs` / `service/smart_router.rs` / `sub_agent.rs`)。`SmartRouter` / `CardRegistry` 的对外
行为不变。

## 不做(超范围)

- OAuth2 token 校验:安全留白,`tiered.rs` 注释已说明"reject until implemented"。
- `agent/getAuthenticatedExtendedCard`:可选协议方法,本档不实现。
- Task 版本化乐观并发(CAS)、推送通知重试退避、ra2a-ext 扩展传播链:为内存单节点引入分布式
  特性,违反 R3/R6。

## 约束

- 所有改动在 worktree 分支 `feat/a2a-fixes-wiring`,不触碰 main。
- 接口向后兼容(`message/send` 别名保留;新 config 字段默认关闭)。
- 完成后**不**运行 cargo check,直接提交(用户强制约束)。
- 同步清理死代码(项目 3),不留逻辑冗余。
