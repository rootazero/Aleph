# ACP 会话生命周期收敛 / ACP session lifecycle convergence

状态 / Status · 已确认 / Approved

## 目的 / Intent

让 Aleph 在模型工具、Teams 和 Gateway 的正常入口上使用同一套 ACP 会话生命周期。修复同键重复启动、旧实例清理与新实例发布交错、取消和关闭结果失真、以及 Graceful shutdown 删除恢复记录的问题。保留现有 ACP 客户端、协议 transport、工具注册、授权策略与原子文件写入。验证对象是实际发生的子进程、协议请求和持久化状态。

Give model tools, Teams, and Gateway one ACP session lifecycle. Fix duplicate child creation, races between retiring and publishing owners, misleading control outcomes, and loss of restore records during graceful shutdown. Reuse the client, transport, tool registration, authorization, and atomic file writes. Verify effects at the process, protocol, and storage boundaries.

## 对比与取舍 / Comparison and trade-offs

| 维度 / Dimension | acpx `0701de4` | Aleph `bf64fec27` |
|---|---|---|
| 会话身份 / Identity | `src/types.ts` separates local record and remote session IDs. `src/runtime/engine/manager.ts` coordinates acquisition and operations. | `src/acp/manager/session_key.rs` canonicalizes the local key. `src/acp/manager/lifecycle.rs` checks the pool after spawning. |
| 取消和回收 / Cancellation and retirement | `src/session/queue/owner-turn-controller.ts` and `src/session/queue/control-admission.ts` address the current owner and separate turn and control admission. | `src/acp/session.rs` exposes a separate `CancelHandle`. `src/acp/manager/lifecycle.rs` clones the handle before removing a pool entry in a separate step. |
| 恢复 / Recovery | `src/acp/client.ts` reads peer capabilities. `src/runtime/engine/manager.ts` selects resume or load, with a separate path for new sessions. | `src/acp/manager/harness_admin.rs` attempts load, then creates a fresh context if load fails. `src/acp/manager/lifecycle.rs` removes records during server shutdown. |
| 观察与授权 / Observation and authority | `src/session/journal.ts` supports journal consumers. The local client answers agent requests under its own policy. | `src/acp/manager/persistence.rs` owns the single writer. Gateway exposes `acp.sessions.changed`. The existing Tool Capability registry publishes ACP tools, and the client answers subprocess requests through `src/acp/incoming.rs`. |

- **会话优先 / Lifecycle first**。用户选择此方案。单进程管理器借鉴按键独占和回收顺序，继续使用 Aleph 的 `SessionKey` 和 Tokio 同步原语。需要明确迁移现有记录、状态投影和关闭语义。 The user selected this approach. It applies single-owner admission and ordered retirement within the existing Tokio process. It requires careful handling of stored records, projections, and close behavior.
- **同批增强出站协议 / Include outbound protocol enhancements**。加上握手能力、`session/resume` 和类型化流事件。协议变体、进程状态、客户端兼容同时进入同一验证矩阵。 This would extend the compatibility matrix to negotiation, recovery methods, and stream consumers in the same change.
- **同批增加入站服务 / Include an inbound server**。需要外部客户端身份、每个调用的授权及会话事件出口。 The inbound server requires a separate caller identity, authorization, and session event contract.

## 实施顺序 / Implementation order

1. **连线 / Connect existing entry points.** Keep the existing RPC names, tool arguments, Teams routing, `SessionKey` normalization, and `NativeAcp` versus `Oneshot` selection. Apply the existing `trust_level=disabled` delegation prohibition at the shared manager admission point before any process starts. Model and Teams entry points must reach that decision, and startup restore must honor it for saved sessions. Keep the global shutdown owner in `StartupResources` so normal exit and startup failures run the same teardown. Reuse the current gateway invalidation, tool admission, and persistence writer. Verify the registered ACP tools through their actual dispatch route, using the existing `ToolHandlerRegistry` and Capability exposure. / 保持现有入口与路由。将已有的 `trust_level=disabled` 禁止委派规则接入共享管理器，覆盖模型、Teams 与启动恢复。将 ACP 关停挂到共同资源收尾，走现有 Gateway 通知、工具调用和持久化路径，并从真实入口验证 Capability 连线。
2. **所有权与控制 / Ownership and control.** The manager admits at most one native session start or retirement for a canonical key at a time. Prompts and ordinary same-key control operations serialize. Unrelated keys can spawn and retire concurrently. Publish live owners only against a current harness registration, and close admission before reconfiguration removes old entries or server shutdown starts. Take the retirement target atomically, keep its identity through cancellation and termination, and treat waiting callers against an invalidated owner as stale. A process that ignores cancellation must still reach bounded teardown. Existing cancel outcomes continue to distinguish no dispatch from a sent notification. / 同键启动和回收由唯一所有者管理，异键可并行。配置变更和停止先收入口，回收目标在确认时固定。子进程忽略取消时仍有时间上界。取消结果继续区分无派发与已发送通知。
3. **恢复与事件 / Restore and events.** Reuse the existing `acp_sessions.json` record shape and single writer. Keep the local key and remote ID separate. Record an established remote ID when the manager first owns it, and order persistence mutations with the owner changes that produced them. Use the same admission for startup restore and later calls that retry a known record through `session/load`. Keep the observed failure and remote ID when load fails. A later reuse may retry loading. Only an explicit replacement can start with a blank context for this key. Explicit close removes the record even when its process is absent. A server stop retains recoverable IDs and joins the writer after queued changes settle. The panel notification is a refetch signal from the committed in-memory transition, and the file remains the source for load after restart. / 沿用旧记录格式。启动恢复与后续重试走同一个准入路径和既有 `session/load`。恢复失败保留远端 ID 和原因，复用请求可重试恢复。只有显式替换才为已知记录开始空白上下文，显式关闭可删除没有活动进程的记录。服务关停保存可恢复 ID，并等待写入完成。会话变更通知只驱动重读当前快照。
4. **清理旧路径 / Retire superseded paths.** Remove duplicate spawn and restore insertion paths in `src/acp/manager/{lifecycle,harness_admin}.rs`, the split cancel-and-remove shutdown path, repeated persistence writes on unchanged IDs, and the separate normal-exit cleanup in `src/bin/aleph-server/commands/start/mod.rs` as their replacements take over. Keep test utilities that exercise observable subprocess behavior. / 替代路径接通后，清理重复启动、恢复直接插入、分步关闭、无变化的记录写入以及单独的正常退出收尾。

## 操作语义 / Operation semantics

- **取消与关闭 / Cancel and close**。`acp_session_control` 和 `acp.sessions.cancel` 通过独立的取消句柄向当前活跃 turn 发送通知，无需等待 prompt 锁。`acp.sessions.shutdown` 先撤销指定会话的准入，再尝试协作取消，必要时有界终止对应进程。模式、模型和配置更改与 prompt 串行。 / Cancel bypasses the prompt lock and addresses the active turn via the existing cancellation handle. Explicit shutdown withdraws admission, tries cooperative cancellation, and enforces a bounded exit for the captured owner. Mode, model, and configuration changes serialize with prompts.
- **替换与重新配置 / Replace and reconfigure**。原生 ACP 的 `acp_delegate` 参数 `reuse_session=false` 是显式替换。它在同键内撤销旧关联、回收旧实例并准入一次新会话启动，其他请求等待或收到明确的关闭结果。成功的 harness 更新、禁用和注销沿用现有失效行为，清理该 harness 的旧恢复关联，包括仅落盘的记录。配置拒绝或校验失败保留旧关联。 / `acp_delegate` with `reuse_session=false` on the native route replaces the association within the same key. Retirement and the reserved new start form one operation. Successful harness update, disable, and removal invalidate that harness's saved associations, including records without a process. Rejected configuration changes keep the existing associations.
- **恢复与失败 / Restore and failure**。保留启动恢复的现有时机。一次加载失败只影响对应会话，服务可继续启动。后续复用报告同一路径的加载结果，显式关闭可移除无进程的记录。 / Keep the existing startup restore timing. A per-session load failure leaves startup available and the association intact. Later reuse reports the load result from the same path. Explicit close can remove a record without a running process.

## 契约边界 / Contract boundaries

ACP delegation and session controls use the existing Tool Capability for discovery and invocation. Capability exposure leaves admission checks in the existing tool and Core paths. Protocol capabilities describe peer support. Gateway administrator controls continue to use existing RPC authorization. Teams continue to route through their established target and execution checks. The shared manager enforces the established delegation prohibition for every launch route. This slice covers the outbound client lifecycle, its local persistence, and its current consumers. Protocol wire corrections, filesystem isolation, and cross-caller session ownership require separate contracts.

ACP 委派和会话控制继续走现有 Tool Capability 发现和调用链。工具授权、Gateway 管理员准入和 Teams 目标检查仍由各自既有边界负责。所有进程启动入口共用已有的禁止委派规则。交付面覆盖出站客户端生命周期、本地记录和现有消费者。协议报文修正、文件系统隔离和跨调用方会话归属需要独立契约。

实现者决定持有者状态的最小表示、Tokio 原语、快照更新位置和清理时间预算。跨进程租约、共享队列 daemon、自动重投 prompt、完整事件日志、`session/resume` 协商和入站服务保留为后续设计项。对端会话历史由对端管理，Aleph 保存本地关联与重连结果。

The implementer will choose the smallest owner representation, Tokio primitives, snapshot update point, and teardown budget. Cross-process leases, a shared queue daemon, automatic prompt replay, a full journal, `session/resume` negotiation, and an inbound server are follow-up designs. The peer owns its conversation history. Aleph owns the local association and reconnect outcome.

## 验收 / Verification

- 同键并发启动只握手一次，另一个会话键可在首个启动或关闭期间取得进展。 / Concurrent same-key starts perform a single handshake. A different key makes progress during start or teardown.
- 覆盖 cancel、shutdown、replace 和 harness 变更交错。确认取消落在原目标或报告未派发，等待中的调用不会使用已撤销实例。 / Exercise cancellation, shutdown, replacement, and harness changes under interleaving. Observe dispatch to the original target or a no-dispatch outcome, and verify stale waiters cannot operate on the retired owner.
- 用旧格式夹具验证读入、恢复失败、显式关闭后磁盘状态和正常停止后的恢复。包含对端忽略取消的 fixture，观察受限时间内进程仍可退出。 / Use legacy fixtures to check load, restore failure, explicit close, and restart after a graceful stop. Observe a bounded exit with a peer that ignores cancellation.
- 通过现有工具、Gateway 和 Teams 入口验证调用或投影效果。覆盖 disabled 条目下模型、Teams 和启动恢复均不会派生子进程。运行受影响的 ACP、工具和调度测试，检查服务器编译、格式与 diff。 / Exercise tool, Gateway, and Teams entry points. Observe zero child starts when a disabled entry reaches model delegation, Teams delegation, or startup restore. Run affected ACP, tool, and dispatch tests, plus server compilation, format, and diff checks.
- 随实现更新 `docs/reference/FEATURE_LOCATOR.md` 及相关架构、进程管理和 Gateway 文档，说明现有入口、恢复语义和验证边界。 / Update the locator and the relevant architecture, process management, and Gateway references with actual entry points, recovery semantics, and evidence.
