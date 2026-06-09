# Aleph 集群 ③ 节点侧审批路由回中心 — 设计

> **状态**: 设计确认，待写实施计划
> **日期**: 2026-06-09
> **前序**: 0a 反向 RPC / 0b NodeRegistry / 0c-core 节点运行时 / 0c-pairing 配对 / 文件传输②
> **这是三子系统里最重、最新颖的一个**：节点首次**反向发起**请求（此前节点只响应中心的 `tool.call`）。

## 1. 目标

让远程**节点**在命中需人工审批的能力升级时，**反向向中心发起审批请求**。中心经 R5（"AI 主动到达"）呈现给 operator/Panel，operator 批准/拒绝，决策带关联 id + 超时回流到节点，节点据此授予或拒绝该次升级。

今天节点 headless 运行，`ApprovalGate::new(config, None)` → requester 为 None → 任何能力升级**一律自动拒绝**（`gate.rs:117-136`，`workspace.rs:928` 注释证实）。③ 正是填这道缝：把 `None` 换成一个"路由到中心"的 requester。

## 2. 触发点（已决策：复用现有 ApprovalGate 升级钩子）

节点 bash 经 sandbox 执行，命中能力升级时调用既有钩子：

```
src/sandbox/workspace.rs:289-292
    let outcome = self.approval_gate
        .request_approval_for_tool(&cmd.program, &reason)
        .await;
```

- requester=None → `Denied`；换 requester → 路由到中心。
- `granted_elevations`（workspace.rs:299）按 session 缓存 `ApprovedForSession` → 节点侧 AllowSession **天然生效，零额外代码**。
- `denial_ledger`（workspace.rs:301-309）记录 Denied/Timeout，短路同一升级的盲目重试。

节点侧门控逻辑**零改动**，只换注入的 requester。所有需审批场景天然继承现有 sandbox 判定。`ApprovalRequester` 签名 `(tool_name, reason) -> ApprovalOutcome`：节点只供 tool+reason；**节点身份由中心从认证连接盖章**（防伪），不信任 params。

## 3. 核心改动：节点 run_session 并发重构（③ 的心脏）

当前节点是同步循环（`node.rs:286-291`）：读一帧 → `handle_frame(...).await` inline → 同步 `ws.send(reply)`。无独立 outbound 队列。

**致命问题**：若一次 tool.call 内的 bash 阻塞等审批响应，而该响应要从**同一** `ws.next()` 读循环到达 → **死锁**：读循环正停在那个等它的命令里。

**重构**（中心 `handler.rs` 早已是此架构，节点尚未）：
- `ws` split 成读半 + 写半。
- 新增 `outbound: mpsc::Sender<String>` + 一个 writer task 把 outbound 抽干到写半。
- 节点获得自己的 `ReverseRpcChannel` + `PendingInvokes`（**verbatim 复用 `src/cluster/reverse_rpc.rs`**，两端对称）。
- 读循环按帧分流：
  - 响应帧（`id` + `result`/`error`，无 `method`）→ `pending.resolve(id, resp)`（镜像中心 `handler.rs:510-527`）。
  - 请求帧（`method == "tool.call"`）→ **spawn** 并发 dispatch，结果入 outbound（不再 inline 阻塞读循环）。

并发 dispatch 是解死锁的关键：bash 阻塞等审批时，读循环仍在泵帧，可投递审批响应。

## 4. 组件

### 4.1 节点侧

**`CenterApprovalRequester`**（新，impl `ApprovalRequester`）：
```
struct CenterApprovalRequester {
    channel: Arc<RwLock<Option<ReverseRpcChannel>>>,  // 按连接刷新
}
```
- `request_approval(tool, reason)`：读 channel → `Some(ch)` 则 `ch.call("node.approval.request", {tool, reason}, APPROVAL_TIMEOUT_MS)` → 映射响应 payload `{outcome}` → `ApprovalOutcome`；`None` / call 出错 / 超时 → `Denied`（fail-closed）。
- 响应 payload 映射：`approved`→`Approved`、`approved_session`→`ApprovedForSession`、`denied`→`Denied`、`timeout`→`Timeout`。

**`build_command_table`**（`node.rs:171`，改）：
- 创建一个 `CenterApprovalRequester`（channel slot 初始 `None`），`ApprovalGate::new(config, Some(requester))`，**一次绑定**。
- 返回 `(CommandTable, Arc<RwLock<Option<ReverseRpcChannel>>>)`（共享 channel-slot 句柄）。
- run 循环每次连接成功后 `*slot.write() = Some(channel)`；会话结束置 `None`。channel 按连接刷新（reconnect-safe），**table 仍只建一次**（gate 不必每连接重建）。

### 4.2 中心侧

**WS handler 路由**（`handler.rs`，改）：来自 **node-role 连接**、`method == "node.approval.request"` 的入站请求帧 → 路由到 `handle_node_approval_request`（位置：响应拦截 `510-527` 之后、普通 RPC dispatch 之前）。**仅** allowlist 这一个反向 method；其余节点发起的 method 拒绝。`node_id`/`node_name` 从认证连接（conn → NodeRegistry / conn 上下文）派生，**不信任 params**。

**`handle_node_approval_request`**（新）：
- 复用 `ExecApprovalManager.create / register_pending` 铸 `approval_id` + oneshot + timeout。
- publish 新 `GatewayEventFrame::NodeApprovalRequested { approval_id, node_id, node_name, command, reason }`。
- `await_registered` 等决策。
- 决策 → 响应 payload `{outcome}`，作为 JSON-RPC 响应回发节点请求的 `id`。
- 决策落定后 publish `ApprovalResolved` / 超时 publish `ApprovalExpired`（**复用现有 frame，按 `approval_id` 清 Panel 卡**）。

**`r5_router::approval_for`**（`r5_router.rs:75-84`，加一臂）：
```
GatewayEventFrame::NodeApprovalRequested { approval_id, node_name, command, reason, .. }
    => Some(SurfaceApproval {
        approval_id,
        title: format!("Node '{node_name}' requests approval"),
        body:  format!("Run `{command}`: {reason}"),
    })
```
**复用现有 operator Panel 审批卡 + approve/deny + `exec.approval.resolve` RPC，零改动。**

### 4.3 决策 resolve（零新增）

operator 在 Panel 点 approve/deny → 现有 `exec.approval.resolve { approval_id, decision }` → `ExecApprovalManager.resolve` → 唤醒中心 handler 的 oneshot → 响应回发节点。**无新 resolve RPC、无新 Panel 按钮。**

## 5. 数据流（端到端）

```
节点 bash 需升级
  → gate.request_approval_for_tool(prog, reason)        [workspace.rs:289]
  → CenterApprovalRequester.request_approval
  → channel.call("node.approval.request", {tool,reason}, timeout)
  ──[WS 上行]──▶ 中心按 conn-role + method 路由
  → handle_node_approval_request
      → ExecApprovalManager.create + register_pending
      → publish NodeApprovalRequested
      → r5_router → operator Panel 审批卡
  → operator 点 approve
  → exec.approval.resolve（现有）→ ExecApprovalManager.resolve
  → handler oneshot 唤醒 → JSON-RPC 响应 {outcome}
  ──[WS 下行]──▶ 节点读循环响应拦截 → pending.resolve
  → CenterApprovalRequester 映射 → ApprovalOutcome
  → sandbox 授予/拒绝升级（granted_elevations / denial_ledger）
```

关联 id 双命名空间，互不混淆：
- 节点 request id（节点 `PendingInvokes`，`rpc-{n}`）：关联 WS 请求/响应。
- 中心 `approval_id`（`ExecApprovalManager` UUID）：关联 operator 决策。
- 中心 handler 等 manager 决策后，用节点请求的 id 回响应。节点从不见 approval_id。

## 6. 错误 / 超时 / 安全

- **节点**：`channel.call` 超时（`APPROVAL_TIMEOUT_MS`）/ WS 错 / channel=None → `Denied`（fail-closed，镜像现状）+ denial_ledger 短路后续重试。
- **中心**：`ExecApprovalManager` 超时 → publish `ApprovalExpired` → handler 返回 `outcome="timeout"` → 节点 `Timeout`→`Denied`。节点中途断连 → 响应发送被丢弃，manager 记录自然过期。
- **安全**：仅 node-role 连接可发 `node.approval.request`；仅该一 method 反向 allowlist；节点身份从 conn 盖章（防伪）；operator-only resolve（现有 scope）。节点无法自批——中心权威。

## 7. 测试

- **节点单测**：requester 各响应 payload → outcome 映射；call-error / channel=None → `Denied`；读循环响应拦截 resolve pending（mock channel）。
- **中心单测**：`handle_node_approval_request` 建记录 + publish `NodeApprovalRequested`；决策 → 响应 payload；conn-role 门控拒非节点；身份从 conn 非 params 盖章。
- **集成测试**（`tests/cluster_node_runtime.rs` 风格）：mock operator 驱动完整 node→center→node 往返（approve / deny / timeout 三态），断言 outcome + bash 升级被相应授予/拒绝。

## 8. 范围边界（YAGNI）

- 仅一个反向 method（`node.approval.request`）；不开通用 node→center RPC 面。
- 复用 `ExecApprovalManager` + Panel 卡 + `exec.approval.resolve` RPC；**唯一新 frame 是 `NodeApprovalRequested`**（请求呈现）；resolve/expire 复用现有 `approval_id`-keyed frame。
- 节点不做持久 "always allow"（`AllowAlways` → `ApprovedForSession`，同 Phase 2b）；不做 per-command 审批分层；节点侧无审批 UI。
- R10：`src/harness/` 零改动。
- 新 worktree 从 main 切；**不合 main**（cluster 合并用户管）。

## 9. 复用清单（已存在，直接用）

| 机制 | 位置 | 复用方式 |
|------|------|----------|
| `PendingInvokes` / `ReverseRpcChannel` | `src/cluster/reverse_rpc.rs` | 节点侧 verbatim 复用（两端对称） |
| 响应拦截分流 | `handler.rs:510-527` | 节点读循环镜像 |
| `ApprovalGate` 升级钩子 | `workspace.rs:289-292` | 触发点，换 requester |
| `granted_elevations` / `denial_ledger` | `workspace.rs:299-309` | AllowSession / 短路重试天然生效 |
| `ExecApprovalManager` create/register/await/resolve | `src/sandbox/exec_approval/` + `src/approval/` | 中心 pending/timeout/resolve 机制 |
| `OperatorApprovalRequester` 流程 | `src/approval/operator_requester.rs` | 中心 handler 镜像（publish + await） |
| `exec.approval.resolve` RPC | 现有 | operator 决策入口，零改动 |
| Panel 审批卡 + R5 `SurfaceApproval` | `r5_router.rs` + Panel | 复用，仅加 frame→surface 一臂 |
| `ApprovalResolved` / `ApprovalExpired` frame | `frame.rs` | 清卡，复用 |

唯一**新增**：`CenterApprovalRequester`（节点）、`handle_node_approval_request`（中心）、`GatewayEventFrame::NodeApprovalRequested`（呈现）、node run_session 并发重构。
