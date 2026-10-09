# Task 3c static-review follow-up report

## Scope / 范围

- 工作目录：`/home/zou/data/workspace/Aleph-sms-pstn-sdd`
- 基线 HEAD：`aaf3d4dee94386302cf36dced3fb590756e87253`
- 仅修改：`src/gateway/telecom/sms_inbox.rs`
- 本报告为同目录新建文件；未修改 Task3a、Task3b、factory 或 Relay。
- 本次只记录 SMS inbox 的两个 Important 与一个 Minor 修复，不宣称 Task3 完成。

## Changes / 修改

1. `Acceptance::Expired` 现在在同一个 SQLite transaction 中插入 metadata-only tombstone：canonical dedup identity 与必要元数据保留，`ciphertext`、`nonce`、`salt` 为 `NULL`，`content_bytes=0`。插入失败随 transaction 回滚；tombstone 不会出现在 `pending`，相同 identity 在 version/binding/generation 校验后返回 `Duplicate`。
2. admission 使用 `effective_expires_at = min(envelope.expires_at, created_at + 24h)`；`created_at + 24h` 溢出 fail closed。Fresh 但已经到 effective deadline 的事件改走 Expired tombstone；持久化的 payload deadline 使用 effective deadline，不因重试刷新。旧于 24h、但 envelope expiry 尚未到的事件已覆盖测试。
3. durable dedup prune 条件从 `created_at <= cutoff` 改为 `created_at < cutoff`，使 exact-horizon redelivery 仍为 `Duplicate`，超过 horizon 后按 shared protocol 的 `Stale` 处理。
4. 新增测试覆盖：Expired tombstone/redelivery、改变 body/expiry 不绕过 dedup、错误 binding 拒绝、tombstone insertion rollback、24h content TTL、exact horizon boundary 与过 horizon prune。

## Verification / 验证

| Command | Exit | Result |
|---|---:|---|
| `rustfmt --edition 2021 /home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/sms_inbox.rs` | 0 | 单文件格式化；未运行 workspace fmt |
| `cd /home/zou/data/workspace/Aleph-sms-pstn-sdd && cargo test --lib telecom::sms_inbox` | 0 | 11 passed, 0 failed, 0 ignored, 20760 filtered out |
| `rustfmt --edition 2021 --check /home/zou/data/workspace/Aleph-sms-pstn-sdd/src/gateway/telecom/sms_inbox.rs` | 0 | 单文件格式检查通过 |
| `cd /home/zou/data/workspace/Aleph-sms-pstn-sdd && git diff --check` | 0 | 无 whitespace 错误 |

Cargo 输出了既有 warning：`src/gateway/handlers/tools_invoke.rs:39` 的 unused import `AgentDef`，以及 `src/gateway/channel_registry.rs:410` 的 dead-code method；本次未修改这两个产品文件。

## Residual / 残余

- 未运行 workspace 全量测试、clippy 或跨平台构建；未联网。
- 未做真机/Relay 集成验证；本次验证仅覆盖 `telecom::sms_inbox` 定向单元测试。
- Task3a、Task3b、factory、Relay 仍不在本次变更范围内；本报告不表示 Task3 已完成。
