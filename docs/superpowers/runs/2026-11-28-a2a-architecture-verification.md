# Aleph A2A 验收记录 / Verification record

分支 `a2a-architecture`，基线 `1b959fdf4`，验收代码 `34263fc32`。实现依据为[设计](../specs/2026-11-28-a2a-architecture-design.md)和[实施计划](../plans/2026-11-28-a2a-architecture.md)。

The `a2a-architecture` branch starts at `1b959fdf4`. Verification ran against `34263fc32`. The [design](../specs/2026-11-28-a2a-architecture-design.md) and [implementation plan](../plans/2026-11-28-a2a-architecture.md) define the scope.

## 参考版本 / Reference revisions

| 参考 / Reference | 版本与提交 / Version and revision |
|---|---|
| A2A specification | `v0.3.0` `8d57eba286de756176892518a8fc39b0ac2ccefb`, `v1.0.1` `3303592588e388e62e0f69f701af531d2f4e3991` |
| a2a-python | `v0.3.26` `ead75f95b852810f1837248b9e9ffc5092c2d463`, `v1.0.3` `8a82061571142b12745576c972bf07077930a4ff` |
| ra2a | `1a76d6e5af906737974fecfb4b5960fe8431dd9d` |

Graphify 用于检索 Aleph、A2A 协议仓和 ra2a 的现有图。对两版固定 Python SDK 的客户端工厂、JSON-RPC、请求处理、事件队列、存储和结果聚合各做一次 AST 提取，分别得到 179 节点和 342 条边、231 节点和 448 条边。请求、响应、发现和流契约另按固定上游标签核实。设计中的差异表记录了落到 Aleph 的修改。

Graphify queries covered the existing Aleph, A2A specification and ra2a graphs. Focused AST extraction covered the client factory, JSON-RPC transport, request handler, event queue, storage and result aggregator in each pinned Python SDK. It found 179 nodes and 342 edges for 0.3.26, and 231 nodes and 448 edges for 1.0.3. Pinned upstream source established the request, response, discovery and streaming contracts. The design's comparison table records the resulting Aleph changes.

## 挂载 Gateway / Mounted Gateway

SDK 夹具以独立目录启动实际 Gateway 和受控模型提供方。两个环境分别安装 `a2a-sdk==0.3.26` 和 `a2a-sdk==1.0.3`。

The SDK harness starts the actual Gateway and a controlled model provider in an isolated home. It installs `a2a-sdk==0.3.26` and `a2a-sdk==1.0.3` in separate environments.

```bash
ALEPH_SERVER_BIN="$PWD/target/debug/aleph-server" ./qa/a2a/run.sh gateway 0.3.26
ALEPH_SERVER_BIN="$PWD/target/debug/aleph-server" ./qa/a2a/run.sh gateway 1.0.3
```

### SDK 结果 / SDK results

两版官方 SDK 均通过默认发现、真实 SSE、断开观察后的任务查询与活动 Core 取消。夹具确认模型提供方开始处理后才取消。两版的版本化 JSON-RPC 还通过了 `ask_user` 暂停、问题回传、拒绝其他凭据续跑、原凭据保留任务和上下文 ID 续跑、以及取消已暂停任务。0.3.26 通过 23 项断言，1.0.3 通过 24 项。

Both official SDKs passed default discovery, live SSE, task lookup after detaching the observer, and active Core cancellation. The harness confirms the provider received the work before cancelling. Versioned JSON-RPC also passed `ask_user` pause, delivery of the question, cross-credential resume rejection, same-credential resume with the original task and context IDs, and cancellation of a paused task. The 0.3.26 and 1.0.3 runs passed 23 and 24 assertions, respectively.

### 出站对端 / Outbound peers

在真实 Gateway WebSocket 上调用 `a2a_agents` 和 `a2a_delegate`，为 0.3 和 1.0 分别注册一个受控 HTTP 对端。对端发布非默认 JSON-RPC 地址，逐次核对方法名、版本和 POST 次数。两版发送都收到产物，每次准入收到一次 POST，移除之后收到零次。两版流式对端在建立 SSE 前返回 JSON-RPC 错误，Gateway 保留对应的协议错误并确认收到响应，每次只发出一个 POST。注册表的单元测试另覆盖准备完成后、最终交接前的撤回顺序。

The real Gateway WebSocket invokes `a2a_agents` and `a2a_delegate` against controlled 0.3 and 1.0 HTTP peers. Each advertises a non-default JSON-RPC address and checks the method, version and POST count. Both calls returned an artifact. Each admitted call produced one POST and a call after withdrawal produced zero. Streaming peers for both versions returned JSON-RPC errors before establishing SSE. The Gateway reported the protocol error and confirmed receipt after one POST per call. Registry tests also cover withdrawal after request preparation and before dispatch handoff.

### ra2a

固定版本的客户端在默认发现时将 1.0 `SecurityRequirement.schemes` 中的值按数组解码，读取规范的 `StringList` 对象时失败。显式指定 `/a2a/rpc` 和 1.0 时，该客户端发送 0.3 的 `message/send`，Gateway 返回 `-32601`。切换到 0.3 后请求达到操作处理器，消息校验返回 `-32602 Invalid params: Expected a user message with a messageId`。 [固定协议定义](https://github.com/a2aproject/A2A/blob/3303592588e388e62e0f69f701af531d2f4e3991/specification/a2a.proto#L497)

During default discovery, the pinned client expects arrays as values in 1.0 `SecurityRequirement.schemes` and fails to decode the specified `StringList` object. With `/a2a/rpc` and 1.0 selected explicitly, the client sends the 0.3 `message/send` operation and the Gateway returns `-32601`. Selecting 0.3 reaches the operation handler, where message validation returns `-32602 Invalid params: Expected a user message with a messageId`. [Pinned protocol definition](https://github.com/a2aproject/A2A/blob/3303592588e388e62e0f69f701af531d2f4e3991/specification/a2a.proto#L497)

## 旧路径清理 / Retired paths

任务存取统一经过 `TaskStore` 的凭据范围和内部实例，移除了按公开 ID 操作的 `A2ATaskManager`。同步、SSE 和立即返回都复用 `AgentLoopBridge` 的一个 Core 执行入口。Webhook 配置随任务记录保留和回收。出站先完成协议选择和注册代次检查，每次委派交接一次请求。旧版客户端仍可使用兼容路由。

Task operations use `TaskStore` and its credential scope and internal instance. The public-ID-based `A2ATaskManager` was removed. Blocking, SSE, and immediate replies all use one Core execution entry point in `AgentLoopBridge`. Webhook configuration follows the lifetime of its task record. Outbound delegation selects a protocol and checks the registration generation before handing off one request per delegation. Legacy clients can use the compatibility routes.

`ProjectionHost` 从 Zahir 取得同一个 special registry，用于发布、订阅、解析和撤回。各构造入口沿用同一条接线，避免宿主发布到独立实例。

`ProjectionHost` obtains the special registry from Zahir. Publication, subscription, resolution, and withdrawal use that registry across the host constructors.

## Rust 验证命令 / Rust verification commands

```bash
cargo build --locked --offline -p alephcore --bin aleph-server
cargo test --locked --offline -p alephcore --lib -- a2a::
cargo test --locked --offline -p alephcore --lib -- gateway::execution_adapter::
cargo test --locked --offline -p alephcore --lib -- capability::
cargo test --locked --offline -p alephcore --lib --no-run
cargo test --locked --offline -p alephcore --bins
cargo check --locked --offline -p alephcore --bin aleph-server
```

执行适配器、Capability 与单独的 `ask_user` 保密问题用例复用了同一份 Core 测试二进制。

The execution-adapter, Capability, and standalone `ask_user` secret-question checks reused the linked Core test binary.

| 门禁 / Gate | 结果 / Result |
|---|---|
| Gateway build | 通过 / Passed |
| `a2a::` | 255 通过 / passed |
| `gateway::execution_adapter::` | 7 通过 / passed |
| `capability::` | 349 通过 / passed，1 项既有忽略 / pre-existing ignored test |
| `remote_clarification_refuses_secret_without_stopping_the_run` | 1 通过 / passed |
| `--lib --no-run` | 通过 / Passed |
| `--bins` | 152 通过 / passed |
| `cargo check --bin aleph-server` | 通过 / Passed |

共享注册表回归验证了同一 Agent 的发布、解析、实际委派、健康更新和撤回。暂停任务的回归验证了旧 Core run 无法结算或释放已续跑任务。

The shared-registry regression covered publication, resolution, actual delegation, health updates, and withdrawal of one Agent. The paused-task regression confirmed that an earlier Core run cannot settle or release a resumed task.

## 文档与格式 / Documentation and formatting

`git diff --check`、A2A 相对链接、Shell 与 Python 夹具语法检查通过。48 个变动的 Rust 文件中，46 个通过独立格式检查，另两个文件的格式差异与基线相同。`python3 scripts/doc_wiring_audit.py` 在隔离基线和当前分支都报告 12 项失败、10 项提示，诊断内容相同。

`git diff --check`, A2A relative links, and the shell and Python harness syntax checks passed. Of the 48 changed Rust files, 46 passed isolated formatting checks and two had the same formatting differences as the baseline. `python3 scripts/doc_wiring_audit.py` reported the same 12 failures and 10 advisories on the isolated baseline and this branch.
