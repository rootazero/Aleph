# GitHub issue 草稿（待批准后创建）


仓库：`rootazero/Aleph`。创建命令：`gh issue create --title "..." --body "..." --label bug --label security`（按需调整标签）。


---


## 1. E5-03（P0）

**标题**：`WASM 凭据注入的 UrlPath 用整串 replace 替换占位符，多段匹配 / 非路径段替换未被挡`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

A `UrlPath` binding with placeholder `api` targets `https://api.example.com/v1/data`; after the binding resolves, a secret value such as `evil` produces `https://evil.example.com/v1/data` (or a placeholder in another URL component changes the scheme/authority) → the request is sent to a target that was never allowlist-checked, potentially disclosing the secret to the wrong host.

### 位置

`src/extension/runtime/wasm/credential_injector.rs:173`（审查轮基线：`79dacfa33`）

### 当前状态

src/extension/runtime/wasm/credential_injector.rs:192 — "let path = parsed.path().replace(encoded_placeholder, &encoded);" — The egress re-check exists (host_functions.rs:305-307 `kernel.check_http_request(&method, &egress_url)`), but the UrlPath injection still uses a blanket `replace` and its presence guard uses `.split('/').any(...)`, so multi-segment replacement and the absence of an exactly-one-segment proof remain.

### 建议改法

Restrict `UrlPath` replacement to a parsed path segment (reject placeholders not wholly within the path), then re-run the existing method/HTTPS/host/path allowlist check on the final URL immediately before sending. Preserve host-pattern matching, header/query injection, rate/call limits, and redirect prohibition.

（审查轮置信度 0.97；复核结论：PARTIAL — The diff adds `"kernel.check_http_request(&method, &egress_url)"` before sending, but `UrlPath` injection still uses `"let path = parsed.path().replace(encoded_placeholder, &encoded);"`, which can replace multiple matching path segments and does not establish that the placeholder occupies exactly one parsed path segment; therefore the full suggested boundary is not proven. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（E5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/E5.md`、`final/E5.md` 与 `ISSUES.md`。
```


## 2. EE1-01（P0）

**标题**：`无人值守运行的时间线只脱敏了 tool_timeline 的 name/error，ToolInvocation.id 等文本叶未脱敏`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

An unattended run encounters a tool error containing a recognizable credential, such as `AKIAIOSFODNN7EXAMPLE` → the individual `ToolCallCompleted.result.error` is masked, but the same original error survives in `SessionCompleted.tool_timeline[*].error` and is persisted in `task_traces`. The terminal arm masks only `final_text`, not the nested timeline, violating this sink's stated every-text-leaf

### 位置

`src/gateway/execution_engine/unattended_redacting_sink.rs:113`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/execution_engine/unattended_redacting_sink.rs:129 — "changed \|= mask_in_place(masker, &mut invocation.name);" (followed only by masking `invocation.error`), while ToolInvocation still carries an unmasked `pub id: String` (src/orchestrator/dispatch.rs:429). — The timeline arm masks only `name`/`error`, so other string leaves in the timeline entry (notably `id`, which is masked in the ToolCallStarted/ToolCallCompleted arms) survive, leaving the stated every-text-leaf contract unmet.

### 建议改法

Mask the terminal timeline's string fields, including every optional error, before forwarding the event; likewise explicitly handle text-bearing termination-reason fields rather than treating structured payloads as text-free. Preserve the existing trace shape, tool/result correlation and attended-run behavior. Add a terminal fixture with a planted secret in a nonempty timeline and assert that the persisted/protocol trace contains no original credential.

（审查轮置信度 0.99；复核结论：PARTIAL — The diff now masks `tool_timeline` errors via `changed \\|= mask_in_place(masker, err);`, but it still does not mask other string-bearing timeline fields such as `invocation.input` or `invocation.result`, so the stated every-text-leaf contract remains unmet. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（EE1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/EE1.md`、`final/EE1.md` 与 `ISSUES.md`。
```


## 3. H1-01（P0）

**标题**：`hooks_admin 的 handle_hooks_remove 没有拿 HOOKS_MUTEX，与追加路径并发会互相覆盖`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

两个并发 RPC（或 RPC 与工具调用）在不同 runtime worker 上读取同一版 hooks，然后分别追加/删除并写回 → 后写入者覆盖前一个已提交的修改。即使两次 rename 不重叠、两个调用都成功，新增的 hook 仍会丢失；add/remove 交错还会复活已删除项。所有写入者另共用一个临时文件，写入重叠时还会互相截断临时内容或令另一方 rename 失败。这不是 atomic rename 能解决的 read-modify-write 原子性。

### 位置

`src/gateway/handlers/hooks_admin.rs:213`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/hooks_admin.rs:443 — `pub async fn handle_hooks_remove(request: JsonRpcRequest) -> JsonRpcResponse` performs read_hooks_file → remove_event_groups → write_hooks_file with no `HOOKS_MUTEX` guard, while the lock at :209 is taken only at :220 (append_user_hook) and :357 (remove_user_hooks). — The extra `}` regression is gone, but `handle_hooks_remove` is still an unguarded read-modify-write over the same hooks file, so the concurrent clobber can still occur (and all writers still share one fixed temp path).

### 建议改法

保留现有同步文件 helper，在 RPC 面把完整读取或修改事务放进 `tokio::task::spawn_blocking`，显式处理 join 与文件错误。H1-01 的同步 mutation guard 应在 blocking closure 内持有，不跨 await。保留响应形状、原错误分类、原子替换及 reload 顺序；不要把每个 filesystem syscall 分拆成独立任务。

（审查轮置信度 1.0；复核结论：REGRESSION — The diff adds `let _guard = HOOKS_MUTEX.lock().unwrap();` only to `append_user_hook`, while `remove_user_hooks` and `handle_hooks_remove` remain unguarded, so the concurrent read-modify-write clobber described by the finding can still occur; additionally, the extra `}` shown after `append_user_hook` appears to introduce a syntax error. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H1.md`、`final/H1.md` 与 `ISSUES.md`。
```


## 4. H5-01（P0）

**标题**：`会话 checkpoint 的目标存在性检查不是原子的，与并发创建冲突时会覆盖已有会话`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

Create checkpoint C, append further messages, then call branch with `new_session_key` equal to the source key (or another existing visible session). The target check permits it. The file backend rewrites that target's transcript and replaces its metadata with the checkpoint branch, losing post-checkpoint messages while the RPC reports `created: true`. Visibility is not a create-if-absent predicate

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/handlers/session/db_handlers/checkpoint.rs:223`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/session/db_handlers/checkpoint.rs:224 — "Ok(Some(_)) => { return visibility::not_found_response(request.id); }" (a preflight `match manager.get_metadata(&new_key).await`), while the backend src/gateway/session_store/file_backend/mod.rs:1162 takes `let mut guard = self.lock_metadata(&new_key_str).await?;` and then `guard.insert(meta);`. — The target-existence check is still a non-atomic preflight; the backend performs no create-if-absent check under its metadata lock, so a concurrent creation can still overwrite an existing target.

### 建议改法

Reject every existing target, including the source itself; enforce create-if-absent under the backend's target metadata lock so the check also covers simultaneous creation. Preserve source visibility checks, source attribution, and the non-destructive meaning of branch. Cross-contract confirmation: `/home/zou/data/workspace/Aleph/src/gateway/session_store/file_backend/mod.rs:1130-1175`, especially the unconditional transcript rewrite at 1163 and `guard.insert(meta)` at 1172. SQLite checkpoint operations are unsupported, so this finding concerns the file backend.

（审查轮置信度 1.0；复核结论：PARTIAL — The diff rejects all existing targets with `Ok(Some(_)) => { return visibility::not_found_response(request.id); }`, but this is only a preflight check and still does not enforce create-if-absent atomically under the backend’s target metadata lock, so simultaneous creation can still overwrite an existing target. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H5.md`、`final/H5.md` 与 `ISSUES.md`。
```


## 5. H6-02（P0）

**标题**：`删除 generation provider 时先提交配置再删 Vault 凭据，两步不是原子的`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

删除一个非默认 provider，其 Vault 可写但 config.toml 不可写、解析失败或增量保存拒绝外部变更 → handler 先修改共享配置并持久删除 Vault 密钥，再因配置保存失败返回错误。磁盘仍保留 provider，凭据却已永久丢失；失败的删除还使进程中的 Config 与磁盘、未收到变更事件的运行时 registry 不一致。重试不能恢复原密钥。

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/handlers/generation_providers/handlers.rs:387-401`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/generation_providers/handlers.rs:406 — "if let Err(e) = vault.delete_secret(&vault_key(&params.name)) { warn!(provider = %params.name, error = %e, \"Failed to delete generation API key from vault\"); }" after :402 "*cfg = next_cfg;" — The config save now precedes the Vault deletion, but the two remain separate operations, so a Vault failure leaves the provider removed from configuration while its credential remains and the failure is only logged.

### 建议改法

在克隆的下一版配置上完成删除及 fallback 清理，先保存成功，再替换共享 Config 并删除凭据；配置保存失败时不得碰 Vault。若保存后清理 Vault 失败，显式报告清理失败而不是伪装成完整删除。保持默认 provider 禁删规则、fallback 清理与成功后的 live-apply/event 顺序。

（审查轮置信度 0.99；复核结论：PARTIAL — `if let Err(e) = next_cfg.save_incremental(&["generation"])` now precedes `vault.delete_secret(...)`, but `*cfg = next_cfg;` and the Vault deletion are still separate operations, so a Vault failure leaves the provider removed from configuration while its credential remains and the handler only logs the failure. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H6.md`、`final/H6.md` 与 `ISSUES.md`。
```


## 6. IF4-06（P0）

**标题**：`imessage 拉取只有时间戳游标，边界毫秒内的消息会丢`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

离线期间同一 `dateCreated` 毫秒积累超过 100 条记录 → 查询只返回按时间升序的前 100 条，水位直接提交该毫秒；下次查询 created-after 水位时排除其余同时间记录。恢复路径没有分页 offset 或稳定的同时间 tie-breaker，丢失剩余消息。

### 位置

`src/gateway/interfaces/imessage/bluebubbles/api.rs:458`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/imessage/bluebubbles/api.rs:500 — "\"after\": current_after," with ":546 current_after = max_timestamp;" — The cursor is still timestamp-only with no offset or same-millisecond tie-breaker, so messages sharing the boundary timestamp beyond the first 100 are still lost.

### 建议改法

在推进时间水位前分页排空该时间边界；可对边界做重叠查询并按 GUID 去重，但必须能够越过同时间超过一页的数据。保留排序、单调恢复和 webhook/poll 去重契约。

（审查轮置信度 0.93；复核结论：NOT_FIXED — `query_messages_after` still uses `"after": current_after` and advances `current_after` to the maximum timestamp; it never paginates beyond the first 100 rows sharing that timestamp. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF4 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF4.md`、`final/IF4.md` 与 `ISSUES.md`。
```


## 7. IF6-02（P0）

**标题**：`email 的 IMAP 取信错误被 continue 吞掉，调用方拿到不完整的批次`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

`UNSEEN` 找到一批邮件 → 使用会隐式置 `\\Seen` 的 `RFC822` FETCH 读取正文；若 fetch 流中途出错，错误又被 `r.ok()` 丢弃，或者 inbound receiver 已关闭，部分邮件从未成功发布。随后代码仍对整个 search set 执行 `+FLAGS (\\Seen)`。下一轮只搜 `UNSEEN`，这些未处理邮件不再重取。即使仅修复末尾 STORE，前面的 RFC822 读取仍会提前确认。

### 位置

`src/gateway/interfaces/email/message_ops.rs:346-356`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/email/message_ops.rs:365 — "continue;" inside "Err(e) => { tracing::warn!("Email: IMAP fetch error: {e}"); continue; }" — BODY.PEEK[] prevents premature Seen, but fetch errors are still swallowed with `continue`, so the required error propagation and abort of an incomplete batch are still missing.

### 建议改法

用 `BODY.PEEK[]` 读取，保留并传播 fetch 错误；只对成功发布的消息及明确决定丢弃的消息确认 Seen，发布失败则停止并保留未处理项为 UNSEEN。确认集合和 fetch 使用同一套 sequence/UID 语义；不引入盲目重发出站邮件的行为。

（审查轮置信度 1.0；复核结论：PARTIAL — `BODY.PEEK[]` avoids premature `Seen`, but fetch errors are still discarded with `Err(e) => { ... continue; }`, so the required error propagation is missing and an incomplete batch can still proceed to acknowledgement. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```


## 8. IF6-03（P0）

**标题**：`IrcConfig / MattermostConfig 仍派生 Debug，凭据会被打进日志`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

对任一配置执行正常的 `format!("{config:?}")` 或调试日志 → 派生的 Debug 输出完整邮件密码、NickServ 密码或 Mattermost bot token。三个类型均直接派生 Debug，违反本批次明确要求的 secret-bearing Debug 禁令；不需要假设上游错误响应会恰好回显秘密，也不声称已观察到生产日志泄露。

### 位置

`src/gateway/interfaces/email/config.rs:29-49`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/irc/config.rs:16 and src/gateway/interfaces/mattermost/config.rs:12 — "#[derive(Debug, Clone, Serialize, Deserialize)]" — Only EmailConfig lost its derived Debug; IRC and Mattermost configs still derive Debug and can leak credentials.

### 建议改法

保留现有 serde 配置格式，移除敏感类型的自动 Debug，改为与 webhook 配置相同的脱敏 Debug；如没有 Debug 使用契约则直接不实现。不要把 Serialize 移除当成修复，配置保存本身有合法用途。

（审查轮置信度 1.0；复核结论：PARTIAL — Only `EmailConfig` loses derived `Debug`; the diff contains no redacted `Debug` implementation for `IrcConfig` or `MattermostConfig`, so their credentials remain exposed. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```


## 9. IF6-05（P0）

**标题**：`email / signal / mattermost 的工厂没有 create_with_id，配置里的 id 不生效`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`、`security`

```
### 现象

注册 `email-work` 与 `email-personal` 等两个合法同类型实例 → 四个工厂只实现 create，并把 runtime ID 固定成类型名。跨模块确认 `crates/aleph-slot-api/src/channel.rs:1187-1200` 的默认 create_with_id 会忽略传入 id；`src/gateway/channel_registry.rs:475-487` 随后以返回的 channel.id() 为准，`src/gateway/channel_registry.rs:520-545` 停止并替换该 ID 的 incumbent。于是第二个账户覆盖第一个，配置实例 ID 的寻址失效，隔离的账户共享 runtime/channel 去重身份。Webhook 与 SMS 已覆盖 create_with_id，不属于此缺陷。

### 位置

`src/gateway/interfaces/irc/mod.rs:250-262`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/email/mod.rs:217, src/gateway/interfaces/signal/mod.rs:273, src/gateway/interfaces/mattermost/mod.rs:333 — "async fn create(&self, config: serde_json::Value) -> ChannelResult<Box<dyn Channel>>" with no `create_with_id` — Only IRC gained a `create_with_id` factory; the Email, Signal, and Mattermost factories still ignore configured IDs.

### 建议改法

按现有 webhook/SMS 模式给这四个工厂实现 create_with_id，构造时使用传入实例 ID；保留 create 的历史默认 ID 给确实没有实例 ID 的兼容调用。不要通过改注册宏或建立第二张 ID 映射绕过工厂契约。

（审查轮置信度 1.0；复核结论：PARTIAL — Only IRC implements `create_with_id`; no equivalent changes appear for the Email, Signal, or Mattermost factories, so their configured IDs are still ignored. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```


## 10. E5-01（P1）

**标题**：`WASM 工作区读取的生产路径已加固，但证明它的测试打在一个没有生产调用方的 helper 上`

**类别**：B 类：修复已在位、证明不足　**标签**：`bug`

```
### 现象

A plugin granted only `docs/` requests `docs/link/key.pem`, where `docs/link -> ../secrets` points to `secrets/` inside the workspace → the lexical prefix check accepts `docs/link`, the canonical root check accepts the target, and the guest receives a file outside the granted prefix.

### 位置

`src/extension/runtime/wasm/host_functions.rs:50`（审查轮基线：`79dacfa33`）

### 当前状态

src/extension/runtime/wasm/host_functions.rs:458 — "assert!(read_workspace_file(&state, \"docs/link/key.pem\").is_err());" — The production path is guarded (host_functions.rs:79 `if let Err(e) = state.kernel.check_workspace_read(&canonical_relative) {`), but the test still exercises the separate `read_workspace_file` helper (host_functions.rs:384, which only checks `canonical.starts_with(&root)` and has no production caller), so the test still does not prove the production path is fixed.

### 建议改法

After canonicalization, apply the same allowed-prefix boundary check to the canonical path relative to the canonical workspace root (and reject if it is not under an allowed prefix) before reading. Preserve the existing default-deny lexical traversal checks, whole-workspace confinement, and JSON error contract.

（审查轮置信度 0.98；复核结论：PARTIAL — Production code now re-checks the canonical relative path via `"state.kernel.check_workspace_read(&canonical_relative)"`, which closes the granted-prefix symlink escape, but the added test exercises the separate `"fn read_workspace_file"` helper rather than `host_workspace_read`, so it does not prove the production path is fixed. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（E5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/E5.md`、`final/E5.md` 与 `ISSUES.md`。
```


## 11. EE1-03（P1）

**标题**：`goal_wait 先按 session id 清栅栏再认领续跑，被抢占的会话可能永远不被唤醒`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

The wake service reads a goal waiting on task A, then awaits the task-store check; while it waits, the goal is changed to wait on task B → the A wake clears B's barrier because `clear_wait_barrier` accepts only a session id. For a same-goal barrier update, the later identity-bound claim still succeeds and resumes before B settles; for a replacement goal, the later claim rejects the old identity on

### 位置

`src/gateway/execution_engine/goal_wait.rs:379`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/execution_engine/goal_wait.rs:379 — "match crate::goal::global().map(\|s\| s.clear_wait_barrier(&goal.session_id, now)) {" precedes `self.claim_and_spawn(...)` at goal_wait.rs:393. — The barrier is still cleared by session id alone (src/goal/store.rs:1258 `pub fn clear_wait_barrier(&self, session_id: &str, now_ms: u64)`, matching only `g.is_active() && g.has_wait_barrier()`) before the fallible continuation claim.

### 建议改法

Move expected-goal/expected-barrier comparison, barrier consumption and continuation claim into one store operation/transaction. A superseded snapshot must change nothing; a failed/refused claim must leave a recoverable barrier. Preserve the existing once-only claim, creator-origin validation, budget checks and fire-time marker semantics; no new scheduler is needed.

（审查轮置信度 0.99；复核结论：NOT_FIXED — The diff contains no changes to `src/gateway/execution_engine/goal_wait.rs`; the barrier is still cleared by session id before the fallible continuation claim. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（EE1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/EE1.md`、`final/EE1.md` 与 `ISSUES.md`。
```


## 12. G1-04（P1）

**标题**：`出站重放每轮 claim 前不对账残留 inflight 行，失败的对账只告警仍然起 drainer`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

An inline backlog flush stamps a queued row and sends it. The transport succeeds, but the caller is cancelled before settlement—for example while `send_on` awaits its post-send observer—or `mark_delivered` fails with a transient SQLite write error. The drain gate is subsequently released, but the still-due row retains its stamp. Both claim queries select stamped rows, and `begin_replay` overwrites

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/delivery_queue.rs:1408`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/delivery_queue.rs:2447 — "Err(e) => warn!(error = %e, \"delivery queue: in-flight reconciliation failed\")," followed unconditionally by :2449 "tokio::spawn(drain_loop(registry, store));" — Although `begin_replay` now requires `AND inflight_since IS NULL`, a failed startup reconciliation only logs and still starts the drainer, and there is no drain-gate reconciliation of still-due stamped rows.

### 建议改法

While holding `drain_gate`, reconcile residual in-flight rows before each new drain/inline claim pass, not only at startup; no previous send can still own that gate, so these stamps represent unsettled attempts. If reconciliation fails, refuse that replay pass, including startup, rather than proceeding. This also covers failed settlement without adding a second outcome store. Preserve `UnknownOutcome` retention, explicit operator recovery rules, single-drainer/gate ownership, explicit-only policy fences, existing safe transient retry budgets, and per-conversation ordering. The trade-off is an additional reconciliation transaction per claim pass.

（审查轮置信度 0.99；复核结论：PARTIAL — `begin_replay` now requires `AND inflight_since IS NULL`, preventing automatic replay of a stamped row, but the diff does not add the required drain-gate reconciliation or make a failed startup reconciliation prevent `spawn_drain` from running. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（G1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/G1.md`、`final/G1.md` 与 `ISSUES.md`。
```


## 13. GX3-01（P1）

**标题**：`session_manager 的 modify.rs 仍在 Tokio worker 上直接做阻塞 sqlite 调用`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

Any call path that locks `self.conn` (a `std::sync::Mutex<rusqlite::Connection>`) and then executes a SQLite query runs a blocking I/O call inline on a Tokio worker thread. The lock is held for the duration of the query. On a session with many messages, `compact_session` or `get_history` can hold the lock for tens of milliseconds, stalling that worker thread and starving all other futures sharing

### 位置

`src/gateway/session_manager/ops/crud.rs:60`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/session_manager/ops/modify.rs:167 — "let conn = self.conn.lock().map_err(\|e\| SessionManagerError::DatabaseError(format!(\"Lock error: {e}\")))?;" inside `pub async fn close_session` (:158), with the same inline lock in `set_topic` (:270-278) and others. — Only the crud.rs paths were moved to `query_blocking` (ops/mod.rs:53-59 `tokio::task::spawn_blocking`), while modify.rs still executes blocking rusqlite queries inline on the Tokio worker.

### 建议改法

Route the blocking SQLite calls in `get_or_create`, `add_message_full`, `compact_session`, `close_session`, `reset_session`, `delete_session`, and the `modify.rs` methods through `query_blocking` (or `spawn_blocking`) rather than locking `conn` inline. The `query_blocking` pattern already exists and is used correctly for `list_sessions`/`search_messages`; applying it consistently preserves the same observable semantics and error types. Note: `add_message_full` currently calls `self.compact_session(...).await` after releasing the lock — this ordering is preserved.

（审查轮置信度 0.92；复核结论：PARTIAL — The diff moves several named paths through `query_blocking`, including `get_or_create`, `add_message_full`, `reset_session`, `delete_session`, `compact_session`, and `update_ownership` (for example, `self.query_blocking(move \|conn\| { ... }).await`), but other async SQLite operations remain inline and the finding explicitly covers many additional methods, so the blocking failure mode can still occur. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（GX3 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/GX3.md`、`final/GX3.md` 与 `ISSUES.md`。
```


## 14. H5-04（P1）

**标题**：`provider 删除路径先改共享 Config 再 save_config，保存失败会留下内存 / 落盘不一致`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

A provider mutation passes validation, mutates the shared `Config`, then `save_config` fails. Returning the error releases the lock but does not restore the old value. Subsequent list/catalog requests see the uncommitted state while disk and the live registry retain the previous state. A failed create therefore makes its retry fail with “already exists”; a failed default change advertises a defaul

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/handlers/providers/handlers.rs:224`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/providers/handlers.rs:526 — "cfg.providers.remove(&params.name);" mutating the shared config directly, with the save only afterwards at :542 "if let Err(e) = save_config(&cfg, &sections).await {". — Update, create and set-default were converted to the `cfg_candidate` clone-then-publish pattern, but the delete path still mutates the shared `cfg` before saving, so the shared-config divergence on save failure persists for deletes.

### 建议改法

Push the already resolved visible-session constraint into the storage query and derive page/count there, or process bounded storage pages without retaining the entire corpus. Retain visibility-before-pagination semantics; simply restoring `limit+1` before ownership filtering would recreate H5-14's shape. Cross-contract confirmation: `/home/zou/data/workspace/Aleph/src/memory/store/markdown/store_impl.rs:510-587` binds this value as SQL `LIMIT`, decodes all matching rows, and collects them into one vector under its DB lock.

（审查轮置信度 1.0；复核结论：PARTIAL — The update path now uses `let mut cfg_candidate = (*cfg).clone();` and publishes only after `save_config(&cfg_candidate, &["providers"]).await`, but create/delete/default mutation paths still mutate `cfg` directly before saving, so the shared-config failure mode remains for those operations. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H5.md`、`final/H5.md` 与 `ISSUES.md`。
```


## 15. H5-05（P1）

**标题**：`provider 创建 / 设默认路径先落盘再构造，构造失败只记日志并返回成功`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

Submit a provider update with an unknown protocol such as `does-not-exist` and otherwise valid fields. The builder persists that string; `create_provider` then rejects it. The handler logs the failure but broadcasts the configuration change and returns `{ok:true}`. The old runtime provider continues serving the previous endpoint/protocol while the saved configuration describes the replacement. Cre

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/handlers/providers/handlers.rs:247`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/providers/handlers.rs:430 — "Failed to build provider for live registration (config saved)" logged as an error, followed by :461 "JsonRpcResponse::success(request.id, json!({ \"ok\": true }))"; the set-default path logs at :1179 "Failed to create provider for hot-swap (config saved)" and returns ok at :1203. — Only the update path builds the provider before saving and errors out on failure; create and set-default still persist first, log the construction failure, and report success without a usable provider installed.

### 建议改法

When a live registry is mounted, construct the replacement before committing/publishing the change and propagate construction failure to the RPC. Register the already constructed instance only after successful persistence. Preserve the explicit config-only mode when no registry is supplied. Cross-contract confirmation: `/home/zou/data/workspace/Aleph/src/providers/mod.rs:223-230` rejects an unregistered protocol; `/home/zou/data/workspace/Aleph/src/gateway/handlers/providers/helpers.rs:149-151,196` does not validate protocol registry membership.

（审查轮置信度 1.0；复核结论：PARTIAL — The update path constructs the replacement before saving and returns an error on construction failure, but create and set-default paths still save unvalidated configurations and can report success without installing a usable provider. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H5.md`、`final/H5.md` 与 `ISSUES.md`。
```


## 16. H5-13（P1）

**标题**：`团队会话历史有 64,000 行原始窗口硬上限，更早的可见气泡取不到`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

A team has valid conversation bubbles followed by 600 directed system notifications. The handler reads only those newest 600 raw rows; `map_history` discards all of them and the RPC returns `items:[]` although older chat exists. A fixed factor of three does not implement “newest 200 chat bubbles” and has no continuation cursor with which the client could recover the omitted history.

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/handlers/teams/tasks.rs:832`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/teams/tasks.rs:852 — "const MAX_RAW_FETCH_LIMIT: usize = 64_000;" with the cap warning at :879 "team chat history: raw window cap reached — older visible bubbles may be missing". — The handler grows the raw window and stops after 200 visible bubbles, but it still halts at the fixed 64,000-row cap and warns that older bubbles may be missing, so the raw-window failure mode survives beyond that cap.

### 建议改法

Apply the chat-visibility predicate before the storage limit, or page raw rows until enough visible bubbles are collected or the source is exhausted. Preserve oldest-first rendering and include directed conversation messages that the existing predicate intentionally allows. Cross-contract confirmation: `/home/zou/data/workspace/Aleph/src/teams/messages/store.rs:863-905` limits the raw newest-first SQL rows, then reverses them; it does not filter notification traffic.

（审查轮置信度 1.0；复核结论：PARTIAL — The handler grows the raw window and stops after finding 200 visible bubbles, but it still stops at `MAX_RAW_FETCH_LIMIT = 64_000` and warns that older bubbles may be missing, so the fixed raw-window failure mode can still occur beyond that cap. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H5.md`、`final/H5.md` 与 `ISSUES.md`。
```


## 17. H5-14（P1）

**标题**：`会话搜索用 limit*10 过取，外部命中先吃窗口，可见结果会被挤掉`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

An authenticated user has a matching session, but another user's session has the highest FTS rank. Search with `limit:1` fetches only that foreign hit; the subsequent ownership check removes it and returns no results. The user's own matching session cannot appear even though it is eligible. Larger limits fail similarly when foreign matches occupy the initial window.

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/handlers/memory/sessions.rs:522`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/memory/sessions.rs:686 — "let fetch_limit = limit * 10;" followed by :688 ".search_transcripts(&params.query, params.agent_id.as_deref(), fetch_limit)" with ownership filtering applied afterwards. — The finite over-fetch window is still consumed by foreign hits before ownership filtering, so more than ten foreign matches can still exhaust it and hide authorized results.

### 建议改法

Restrict search candidates to owner-visible IDs before applying the result limit, or page ranked candidates until the visible limit is reached. Keep foreign sessions indistinguishable from absence and preserve ranking among authorized results. Cross-contract confirmation: `/home/zou/data/workspace/Aleph/src/memory/store/markdown/store_impl.rs:688-734` applies only optional agent filtering before `ORDER BY rank LIMIT ?`; agents are not authenticated user ownership.

（审查轮置信度 1.0；复核结论：PARTIAL — Search now over-fetches with `let fetch_limit = limit * 10;`, but foreign hits still consume that finite window before ownership filtering, so more than ten foreign matches can still exhaust it and hide authorized results. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H5.md`、`final/H5.md` 与 `ISSUES.md`。
```


## 18. IF1-02（P1）

**标题**：`telegram polling 丢弃 dispatch 失败仍然推进 update_id，失败被当成已处理`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

新账户启动时诊断请求没有更新，tracker 保持 0；此后 dispatcher 正常接收许多消息，但没有任何正常处理路径调用 `advance`。停止后积压的新消息在下次启动仍被当作首次启动，通过 `drop_pending_updates(true)` 永久删除。跨模块确认 teloxide 0.17.0 的 `dispatching/dispatcher.rs:358-367` 使用默认 polling listener，它只维护自己的内存 offset，并不写入此 tracker。诊断请求返回一条更新本身不等于已确认该更新，不能据此声称诊断直接消费了消息。

### 位置

`src/gateway/interfaces/telegram/polling.rs:80-98,128-130,230-232`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/telegram/polling.rs:79 — "let _ = handler.dispatch(dptree::deps![bot.clone(), update]).await;" followed by `tracker.advance(update_id, "polling")` — Dispatch failure is still discarded with `let _ =` and the update is still advanced, so failed handling can be durably acknowledged.

### 建议改法

将真正的 polling/处理进度接入每个 bot 自己的持久化 tracker，并用明确的一次性初始化事实区分首次启动与重启；不要让“offset 仍为 0”授权删除积压。保持更新确认顺序，不能在处理前把探测结果当作 durable completion。

（审查轮置信度 0.99；复核结论：PARTIAL — The diff advances the tracker after `handler.dispatch(...)` and removes diagnostic advancement, but `let _ = handler.dispatch(...)` ignores dispatch failure and still advances the update, so failed handling can be durably acknowledged. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF1.md`、`final/IF1.md` 与 `ISSUES.md`。
```


## 19. IF1-03（P1）

**标题**：`telegram 的 poll / reaction 入站 id 缺 chat / 用户 / 变更维度，仍会碰撞`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

两个群在五分钟内分别收到 Telegram message ID 相同的消息 → 转换后 ID 相同；同一 poll 的第二位投票者、同一消息的另一位 reaction 用户/反应变更也复用 ID。跨模块确认 `src/gateway/inbound_router/mod.rs:612-615` 仅以 `channel_id:message_id` 去重，`src/gateway/inbound_router/dedup.rs:8,31-36` 在 300 秒内拒绝重复，所以真实的新事件被静默当作重放丢弃。普通群的 message ID 只有 chat-local 唯一性。

### 位置

`src/gateway/interfaces/telegram/handlers.rs:161`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/telegram/mod.rs:748 — "id: MessageId::new(format!("poll_{}", q.poll_id))," (reaction ids at reaction_handler.rs:29 are `react_{chat}_{message_id}`, still without a user/reaction discriminator) — Poll ids are still poll-scoped without chat/voter qualification and reaction ids lack a reaction-change discriminator, so the collision the reviewer described remains.

### 建议改法

将 adapter 的事件去重 identity 与 provider message/reply identity 分开；用 account/chat/update identity 保证事件唯一，继续保留原始 Telegram message ID 给回复、reaction、编辑使用。不能直接更改 ID 字符串而不同时更新依赖其为纯数字的消费端。

（审查轮置信度 0.99；复核结论：PARTIAL — The diff changes messages to `format!("{}_{}", msg.chat.id.0, msg.id.0)`, but leaves poll IDs as `format!("poll_{}", q.poll_id)` and reaction IDs unchanged; voter/chat and reaction-change collisions therefore remain. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF1.md`、`final/IF1.md` 与 `ISSUES.md`。
```


## 20. IF1-06（P1）

**标题**：`telegram adapter 把访问权一律收敛到账号默认，Core 侧的 chat / topic 级放行被拒`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

账户 group policy 为 Disabled，但明确给某群覆写 Open → 本地 controller 按群级 resolver 放行并入队；跨模块确认 `src/gateway/inbound_router/types.rs:419-425,444-453` 仅从首账户构造 Core policy，`src/gateway/inbound_router/permission.rs:79-83` 再以 Disabled 拒绝，因此配置允许的群仍不能工作。这里不是声称所有 poll/reaction 绕过 Core 授权：Core 确实会重新检查。R4 的具体问题是两份 policy reduction 已产生不同判决。

### 位置

`src/gateway/interfaces/telegram/access.rs:72-116`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/telegram/access.rs:100 — "(AccessDecision::Denied, _) => AccessDecision::Denied," (comment at access.rs:78-84 still claims Core re-derives policy from the account-level config only) — The adapter still clamps the per-chat/topic decision to the account default, so a permissive group override remains rejected even though Core is now chat/topic aware.

### 建议改法

由 Core 的授权边界按完整 account/chat/topic 解析同一有效配置，adapter 仅携带必要 I/O 上下文；保留 Core 为授权/配对最终权威，不能删除第二道检查来“修复”可用性。

（审查轮置信度 0.97；复核结论：NOT_FIXED — The adapter change explicitly clamps local access to the account default, and the diff contains no Core account/chat/topic policy-resolution change; a permissive group override remains rejected by the original Core policy reduction. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF1.md`、`final/IF1.md` 与 `ISSUES.md`。
```


## 21. IF1-08（P1）

**标题**：`telegram 长流的 finalize（无预览）路径不切块，长文本会超限`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

已存在流式 preview，累计/最终答案超过 Telegram 4096 字符限制 → edit 返回 `EditedMessageIsTooLong` 却被转成 `Ok(())`，lane 认为最终答案已落地；用户永远只看到最后一次成功的短 preview。没有 preview 时一次性长 send 也不拆分并失败。普通 `send_message` 已有分片，但本路径绕开它。

### 位置

`src/gateway/interfaces/telegram/delivery.rs:292-299,1094-1135`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/telegram/streaming/lane_handle.rs:179 — "self.delivery.send_text_message(&text).await?" (delivery.rs:1151-1172 `send_text_message` does not chunk; only delivery.rs:1199 chunks via `split_for_telegram`) — Long edits are now chunked and `EditedMessageIsTooLong` is no longer treated as benign, but the long-send/no-preview finalize path still delivers unchunked, so the send half of the concern stands.

### 建议改法

不把长度错误当作成功；在流式 delivery 层复用有界 HTML-aware 分片，将超出 preview 的内容按顺序发送并在 finalization 确认全部落地。保留真正的 MessageNotModified 幂等语义。

（审查轮置信度 0.99；复核结论：PARTIAL — The diff removes hardcoded streaming configuration and supplies `delivery.config.streaming`, but it does not address long-stream edits/sends being converted to success by `is_benign_edit_error` or implement chunked streaming finalization. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF1.md`、`final/IF1.md` 与 `ISSUES.md`。
```


## 22. IF4-24（P1）

**标题**：`matrix 的 is_direct 查询失败或缺失时按"非 DM"处理，保守方向错了`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

Matrix 的 direct room 发来正常私人消息 → adapter 无论房间关系一律标 is_group=true；Core 因而走群策略/群 mention，而不是 DM 配对策略。默认 require_mention 时普通 DM 被无声拒绝，DM 配对门也不会在这些消息上执行。

### 位置

`src/gateway/interfaces/matrix/events.rs:120`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/matrix/events.rs:41 — "inbound.is_group = !is_direct.unwrap_or(false);" (fed by sync.rs:62-65 "room.is_direct().await.ok()") — A missing or failed direct-lookup still yields `None`, which is still conservatively treated as a group room.

### 建议改法

在拥有 Matrix 房间上下文的 sync 层读取 direct-room 信息并传给转换，反应消息使用相同判据；未知房间类型需明确保守处理。保留 room_id 身份，不用人数临时猜测 direct。范围外确认 `src/gateway/inbound_router/permission.rs:79-149` 完全按 is_group 选择权限分支。

（审查轮置信度 0.99；复核结论：PARTIAL — `run_sync_loop` obtains `room.is_direct().await.ok()` and passes it to the converter, but a missing or failed lookup remains `None` and is conservatively treated as a group. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF4 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF4.md`、`final/IF4.md` 与 `ISSUES.md`。
```


## 23. IF4-29（P1）

**标题**：`xmpp MUC 回声过滤仍保留 bare-JID 比较，会误过滤 nick 等于 bare JID 的成员`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

JID bot@example.org、默认 MUC nick=aleph → join 使用 aleph，但过滤把回声资源名与 own.local=bot 比较。room/aleph 的自身消息被当成用户消息重新路由，可能反复触发回复；真实昵称为 bot 的其他成员反而被过滤。

### 位置

`src/gateway/interfaces/xmpp/message_ops/ops.rs:60`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/xmpp/message_ops/ops.rs:70 — "if nick == own.bare() {" (the `joined_nick` check was added at :75, but the bare-JID comparison remains) — The configured nickname is now used for the MUC echo filter, yet the residual `own.bare()` comparison still filters a member whose nick matches the bot's bare JID.

### 建议改法

把实际 MUC nickname 传给转换并用它过滤 MUC 回声；一对一分支仍用自身 bare JID。保留 room 身份及真正其他参与者消息，不以账号 localpart 代替昵称事实。

（审查轮置信度 1.0；复核结论：REGRESSION — The MUC echo check changed from the configured nickname to `own.bare()` (`if nick == own.bare()`), so the actual MUC nickname is still not used and a different member named like the bot’s bare JID can be filtered. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF4 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF4.md`、`final/IF4.md` 与 `ISSUES.md`。
```


## 24. IF5-05（P1）

**标题**：`nostr 的 relay 等待者既不超时也不在断连时失败，调用方可能一直等到 30s 超时`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

本地 write channel 接受事件后 `send` 已返回成功；随后 WebSocket 写失败，已取出的命令被丢弃，或者 relay 返回 `OK accepted=false`，循环只记录日志。调用者与 Core delivery path 均认为已发送，用户收不到回复，也没有可处理的失败结果。

### 位置

`src/gateway/interfaces/nostr/mod.rs:270–278`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/nostr/message_ops/ops.rs:146 — "waiters.retain(\|waiter\| !waiter.is_closed());" (the 1s tick only prunes closed waiters; :266 `drop(pending);` discards the waiters on disconnect) — Pending waiters are still never explicitly timed out or failed with an error on disconnect, so the caller can still sit until its 30-second timeout for a lost delivery.

### 建议改法

让每个 publish command 携带结果回传，并按 event ID 将 relay OK 与调用关联；对写失败、明确拒绝和有界等待超时返回适当错误。不要把不确定交付自动标成安全可重试，也不在 adapter 内另造 durable queue。

（审查轮置信度 1.0；复核结论：PARTIAL — Relay acceptance is now associated with `pending.remove(&event_id)` and returned through `PublishCommand.result`, but `pending` waiters are never timed out or failed when a connection drops; `drop(pending);` merely closes the oneshot senders, and the caller can still wait until its 30-second timeout for a lost delivery. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF5.md`、`final/IF5.md` 与 `ISSUES.md`。
```


## 25. IF5-12（P1）

**标题**：`slack 的删除路径没有 enable_deletion 开关保护`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

保持默认 `enable_deletion=false`，随后调用 Channel::delete → adapter 无条件执行 Slack chat.delete，真实消息仍被删除。enable_editing/enable_reactions 也未参与对应操作，capabilities 同样没有反映这些设置。

### 位置

`src/gateway/interfaces/slack/mod.rs:279–323`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/slack/mod.rs:336 — "SlackMessageOps::delete_message(" inside `async fn delete(` (:331), with no `enable_deletion` check — The delete path still has no enable_deletion guard, so deletion remains executable while the feature is disabled.

### 建议改法

在操作边界执行已经配置好的 enable 判断，关闭时明确返回不支持/禁用，并让 capabilities 与同一配置一致；开启时继续使用现有 API 和错误分类，不重新实现 Core 业务策略。

（审查轮置信度 1.0；复核结论：PARTIAL — Reactions and editing now check `enable_reactions` and `enable_editing`, but the diff shown does not add an `enable_deletion` guard to `SlackChannel::delete`, so deletion remains executable while disabled. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF5.md`、`final/IF5.md` 与 `ISSUES.md`。
```


## 26. IF6-10（P1）

**标题**：`signal / mattermost 声明支持附件，实际一律发空附件也不报错`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

根据 capability 声明向任一适配器发送含正常非空文本与附件的 OutboundMessage → 三条实际发送路径仅传文本，未读取 message.attachments，远端文本发送成功后仍返回成功，附件没有到达。Signal 还宣称 reactions/replies/typing 支持，但 react 无条件返回 UnsupportedFeature，send 不传 reply_to，send_typing 是成功 no-op。平台本身支持这些功能不等于本适配器已实现。

### 位置

`src/gateway/interfaces/email/mod.rs:88-104`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/signal/mod.rs:103 and src/gateway/interfaces/mattermost/mod.rs:96 — "attachments: true," while their send paths pass "attachments: Vec::new()," (signal/message_ops.rs:379, mattermost/message_ops.rs:399) — Signal and Mattermost still advertise attachment support without sending attachments and without an explicit rejection of nonempty attachments.

### 建议改法

最小修复是把未实现的媒体/其他 capability 置 false（附件大小相应归零），并对直接传入的不支持附件显式返回 UnsupportedFeature，不能继续报成功。Signal 同步修正 reactions/replies/typing 声明；不在本轮凭空补齐三套媒体上传协议。

（审查轮置信度 1.0；复核结论：PARTIAL — Email now advertises `attachments: false` and `max_attachment_size: 0`, but the diff contains no corresponding Signal or Mattermost capability changes or explicit rejection of nonempty attachments. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```


## 27. IF6-11（P1）

**标题**：`cli worker 在 offload 到 spawn_blocking 之前仍在大文本上克隆`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

CLI 输出被重定向到已满且暂不读取的 pipe，或终端输出阻塞 → async send future 在 runtime worker 上执行 std stdout.lock/write/flush，整条 worker 被同步 I/O 占住；单 worker runtime 的其他任务也无法推进。输入已移到专门线程，输出却仍在异步执行器中阻塞。

### 位置

`src/gateway/interfaces/cli.rs:294-316`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/cli.rs:307-308 — "let message_text = message.text.clone(); let prompt = config.prompt.clone();" executed before "tokio::task::spawn_blocking(move \|\| {" at :309 — The large-text clones still happen on the async worker before the blocking offload, so the newly identified copy cost remains.

### 建议改法

把一次回复及 prompt 的完整同步写入放到 spawn_blocking，等待其结果并保持现有写入顺序和 SendFailed 错误传播；不要在 async worker 上持有 StdoutLock 或调用同步 flush。

（审查轮置信度 1.0；复核结论：PARTIAL — Synchronous stdout writing is moved into `spawn_blocking`, but the async worker still performs `message.text.clone()` and `config.prompt.clone()` before offloading, so the newly identified large-text copy remains. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```


## 28. GX1-08（P2）

**标题**：`WebSocket 升级在批准前不预留连接槽位，并发升级可越过全局 / 单 IP 上限`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

With the global or per-IP cap one slot from full, multiple clients submit concurrent upgrades → all read the same below-cap count, all return an upgrade, and only later insert their connection entries. The advertised maximum and pre-auth per-IP flood bound can therefore be exceeded by the in-flight upgrade burst.

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/server/connection/upgrade.rs:143-178,191-223`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/server/connection/upgrade.rs:147 — "if conns.len() >= state.max_connections {" checked under a read lock that is released before "ws.on_upgrade(move \|socket: WebSocket\| async move {". — No connection slot is reserved before approving the upgrade, so the concurrent-upgrade burst can still exceed the global and per-IP caps.

### 建议改法

Reserve capacity atomically before approving the upgrade and release the reservation on failed upgrade/disconnect, or atomically enforce admission at insertion and close rejected sockets. Include pending admissions in both global and per-IP accounting. Preserve loopback exemption, configured zero-cap semantics and resolved-client-IP attribution.

（审查轮置信度 1.0；复核结论：NOT_FIXED — No change reserves a connection slot before approving the WebSocket upgrade; the diff does not address the concurrent-upgrade capacity race. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（GX1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/GX1.md`、`final/GX1.md` 与 `ISSUES.md`。
```


## 29. GX1-09（P2）

**标题**：`服务端后台维护任务是 fire-and-forget，shutdown / 启动失败 / 浏览器运行时替换后仍在跑`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

`run_until_shutdown` returns, a start attempt fails exposure/bind validation, or a browser-live runtime is replaced while the Tokio runtime remains alive → the dropped task handles leave old infinite maintenance loops running. Each new attempt adds rate-limit/idempotency/request-state pruners and a heartbeat publisher; the detached viewer sweeper retains the old lifecycle and keeps sweeping it. Ol

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/server/mod.rs:1178-1255,1295-1297,1353-1355,1405-1414`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/server/mod.rs:1185 — "tokio::spawn(async move { ... loop { interval.tick().await; ... } })" in `fn spawn_background_tasks(&self)` (:1182), with no JoinHandle stored. — Background maintenance loops are still detached with no retained handles or abort, so they can outlive shutdown, a failed startup, or a browser-runtime replacement.

### 建议改法

Retain these maintenance handles under the server/runtime owner and cancel/join them on every exit, including failed startup and replacement. A small owned task collection with drop-abort plus an awaited graceful-close path is sufficient. Preserve maintenance intervals, best-effort sweep retry and the Core lifecycle implementation; no second scheduler or business-state registry is needed.

（审查轮置信度 1.0；复核结论：NOT_FIXED — No background-task handles are retained or cancelled on shutdown, failed startup, or browser-runtime replacement, so detached maintenance loops can still outlive their owner. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（GX1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/GX1.md`、`final/GX1.md` 与 `ISSUES.md`。
```


## 30. GX1-10（P2）

**标题**：`node.approval.request 每帧起一个不受限、不随断连取消的任务`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

A registered node sends approval requests faster than operators resolve them → every frame spawns another independent task and pending approval before the normal IP limiter, lane admission and bounded off-loop machinery. Disconnect does not cancel these tasks. They retain the manager/event bus/reply sender and wait up to 120 seconds, allowing user-controlled task/pending-record growth and an appro

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/server/connection/mod.rs:315-394`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/server/connection/mod.rs:350 — "tokio::spawn(async move {" wrapping :352 "crate::approval::run_node_approval(" for each `node.approval.request` frame. — Approval tasks are still spawned unbounded per frame and never cancelled on disconnect, so they bypass the normal request budgets and teardown cancellation.

### 建议改法

Apply bounded per-connection admission to this special reverse-request path before spawning, reject saturation with a protocol error, and own/cancel the approval tasks at connection teardown so the manager's existing registration cleanup runs. Reuse the existing approval driver and LAN-trust/node identity contract rather than introducing a separate approval policy.

（审查轮置信度 1.0；复核结论：NOT_FIXED — The diff does not bound or cancel detached `node.approval.request` tasks, so approval requests can still bypass the normal request budgets and teardown cancellation. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（GX1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/GX1.md`、`final/GX1.md` 与 `ISSUES.md`。
```


## 31. H1-08（P2）

**标题**：`agent 的终态运行回收只在完成时发生，插入路径不回收且 RunState.input 不清空`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

使用该 manager 的进程持续接收运行请求 → 每次以新 UUID 插入一个含完整 input 的 RunState；任务结束只改 status，私有 map 的全部使用点都没有 remove/retain/clear。即使所有任务都已经完成，内存仍按累计请求数量和 prompt 大小增长，进程不重启就不释放。

### 位置

`src/gateway/handlers/agent.rs:363`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/agent.rs:314 — "runs.insert(run_id.clone(), run_state);" with `retain_terminal_runs_bounded(&mut runs)` (:170) invoked only after completion (:399) and in the test (:2162). — Terminal eviction exists with `MAX_TERMINAL_RUNS` (:165) and `started_at`-based ordering, but reclamation still happens only on completion — the insert path never reclaims and `RunState.input` is never cleared.

### 建议改法

在该 map 的 owner 内对已结束条目施加有限的近期状态保留，结束时及插入时回收最旧的终态项；活跃条目不淘汰。无需保留的 input 不应随状态历史永久保存。保留当前 run 和近期终态的 status wire shape、取消行为与在飞所有权解析；明确过期条目复用既有 Run not found 契约。不要引入第二张完成态索引或另造后台清理服务。

（审查轮置信度 0.99；复核结论：PARTIAL — The diff adds bounded terminal-history eviction and clears completed input, but the shown cleanup runs only after completion and the stated invariant “结束时及插入时回收” is not fully established; the added test references `COMPLETED_RUN_RETENTION` and `finished_at` fields that are not present in the shown diff, so the implementation cannot be proven complete from this diff. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H1 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H1.md`、`final/H1.md` 与 `ISSUES.md`。
```


## 32. H5-18（P2）

**标题**：`受限会话列表仍要物化全部可见 id 并扫完整个存储批次`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

A normal authenticated/scoped caller requests even a one-row page with no agent/date restriction. The handler replaces the SQL limit with `i64::MAX`, reads every matching session across users into a vector, and only then applies visibility and pagination. As persisted sessions accumulate, each bounded page request performs corpus-sized allocations and decoding, including foreign rows, while holdin

### 位置

`/home/zou/data/workspace/Aleph/src/gateway/handlers/memory/sessions.rs:108`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/handlers/memory/sessions.rs:235 — ".list_sessions(SessionFilter { agent_id: ..., owner_visible_to: Some(owner), ..SessionFilter::default() })" materializing all visible session keys, with the scan loop at :113-152 walking every batch (`scan_offset += batch_len;` :152). — The `i64::MAX` fetch is gone and only the requested page rows are retained, but the path still performs corpus-sized storage work and materializes the complete visible-id set to compute the count.

### 建议改法



（审查轮置信度 ?；复核结论：PARTIAL — The scoped path no longer uses `i64::MAX`, but it repeatedly fetches and filters every foreign storage batch and retains all visible rows in `sessions` to compute the count, so it still materializes the complete visible corpus and performs corpus-sized storage work for a bounded page. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（H5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/H5.md`、`final/H5.md` 与 `ISSUES.md`。
```


## 33. IF4-23（P2）

**标题**：`LINE webhook 的连接任务既没有读超时也不归所有者，监听停止时不会回收`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

外部客户端持续建立 TCP 连接但不发送/不完成请求 → 每个连接都创建独立 task/socket/buffer，read 永远等待，任何签名检查都还没执行。单请求 64KiB cap 不限制连接总数；stop 也只 abort listener，未持有的子任务继续存活。

### 位置

`src/gateway/interfaces/line/webhook.rs:56`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/line/webhook.rs:71 — "tokio::spawn(async move {" (fire-and-forget connection task; the read loop in `handle_connection` has no timeout) — The 100-connection semaphore was added, but there is still no request read deadline and the spawned connection tasks are still not owned or aborted when the listener stops.

### 建议改法

为处理中的连接施加总并发限制与整请求读取截止时间，并在 stop 时结束拥有的子任务。保留 TCP 分片累积及签名对原始 body 的验证，不通过单次 read 捷径破坏协议。

（审查轮置信度 1.0；复核结论：PARTIAL — A 100-connection semaphore is added, but there is still no request read deadline and spawned connection tasks are not owned or aborted when the listener stops. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF4 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF4.md`、`final/IF4.md` 与 `ISSUES.md`。
```


## 34. IF4-27（P2）

**标题**：`matrix 去重先写内存缓存再写库，写库失败会留下未持久化的键`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

通道长期不重启，已加入房间中的用户持续产生新 event_id → 每条记录同时加入 HashSet 和 SQLite，运行期间均无淘汰/清理。初始化 SELECT 的 LIMIT 10000 只限制预热，不限制随后的 cache.insert；连不支持或随后被策略拒绝的 timeline event 也先加入，RAM 与磁盘随历史无限增长。

### 位置

`src/gateway/interfaces/matrix/dedupe.rs:9`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/matrix/dedupe.rs:81 — "state.cache.insert(key);" preceding the `state.conn.execute("INSERT OR IGNORE INTO matrix_seen_events ...")` at :82-84 — The cache is still populated before the database insert succeeds, so a failed insert can leave a non-persisted key cached.

### 建议改法

让内存 cache 真正有上限，并在运行期间按既有 7 天窗口清理持久层；缓存 miss 仍查询 DB，不能把淘汰内存误变成允许重复。保留去重键 (room_id,event_id) 与串行插入契约。

（审查轮置信度 1.0；复核结论：PARTIAL — The cache is bounded and pruned hourly, but the diff inserts into the cache before executing the database insert; a failed insert can leave a non-persisted key cached. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF4 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF4.md`、`final/IF4.md` 与 `ISSUES.md`。
```


## 35. IF5-22（P2）

**标题**：`wechat 的同步游标先在内存里推进再落盘，写盘失败会丢同步位置`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

数据目录只读、磁盘满或 rename 失败 → 两处持久化均返回 unit 并丢弃所有文件错误；runtime 仍推进内存游标。重启读取旧游标/旧 context tokens，可能重收历史消息或无法回复，却没有任何“状态未保存”的错误可供用户处理。这里不声称本地游标能提供端到端 exactly-once。

### 位置

`src/gateway/interfaces/wechat/sync_buf.rs:36–46`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/wechat/runtime.rs:101 — "save_sync_buf(&self.config.data_dir, &self.config.account_id, &new_buf)" (result ignored; the in-memory cursor was already advanced at :99) — `save_sync_buf` still returns `()` and the runtime still updates the in-memory cursor before confirming persistence, so a failed write loses the sync position.

### 建议改法

持久化返回 Result，向运行状态/日志明确传播保存失败及上下文，不再把失败与成功等价；只在写成功后 rename，保留已有原子替换方式。错误处理不得靠立即重跑整批已交付消息来“恢复”，避免引入新的重复交付。

（审查轮置信度 0.99；复核结论：PARTIAL — `save_sync_buf` now logs write and rename failures instead of discarding them, but it still returns `()` and the diff does not propagate failure to the runtime or update the in-memory cursor only after successful persistence. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF5 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF5.md`、`final/IF5.md` 与 `ISSUES.md`。
```


## 36. IF6-12（P2）

**标题**：`cli 的 stdin 读取线程每次 start 都新建，旧的从不取消也不 join`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

无输入时 start 后 stop，并让异步 line consumer 退出，再 start → 旧 std 线程仍阻塞在持有 StdinLock 的 lines() 中，新线程等待同一锁。下一行先由旧线程读取；它向已关闭 receiver 做 blocking_send 才发现关闭，随后退出，读掉的那一行没有交给新实例。每次无输入 stop 都可暂留一个无法 join/取消的线程。

### 位置

`src/gateway/interfaces/cli.rs:212-228`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/cli.rs:217-229 — "std::thread::spawn(move \|\| { let stdin = io::stdin(); for line in stdin.lock().lines() { ... } });" (handle never stored; stop() at :278-292 only signals the async task) — A replacement reader now exists, but it is spawned per start and the previous thread is never cancelled or joined, so it still holds the StdinLock and swallows the first line after a restart.

### 建议改法

让一个有明确所有权的 stdin reader/line stream 跨 start/stop 复用，重启只切换异步消费方，不再重复创建争抢 stdin 的阻塞线程；或采用真正可取消的读取。仅保存 JoinHandle 再在 stop 中 join 不够，会因无输入永久等待。

（审查轮置信度 0.99；复核结论：NOT_FIXED — The diff only removes the old stdin thread and does not add any replacement reader or retained, single-owner stdin stream, so restart can no longer deliver input at all. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```


## 37. IF6-13（P2）

**标题**：`sms 授权被取消时操作既不按 Unknown 结算也不释放预留`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

公共 submit/submit_at future 在 authorizer await 期间被 timeout/drop，或在 peer await 期间被取消 → 操作已经插入 operations，outcome 仍为 None，正常错误清理代码没有机会运行。prune 明确永久保留 outcome=None；同 operation ID 后续始终 OperationPending，过 command expiry/retention 也不回收其正文。尤其 authorizing 阶段尚未调用 peer，不可能靠远端 receipt 自动结算；连续取消不同 ID 可无界累积。此处审的是公开的 Core-side seam，不声称仓库已有真实网络 Relay 实现。

### 位置

`src/gateway/interfaces/sms/transport.rs:599-617`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/sms/transport.rs:724 — "let result = self.peer.submit(&lease.binding_id, lease.generation, operation).await;" (plain await with no cancellation select; outcome-None entries are retained indefinitely by prune_operations) — Removal during authorization is detected, but cancellation during the peer await still never settles the operation as Unknown and leaves the pending operation unreclaimed.

### 建议改法

给 pending reservation 明确的 cancellation-safe 生命周期：未越过 peer 提交边界时释放 reservation，越过边界后按 Unknown 语义结算且可按现有策略到期，不把未完成操作永久留在 None。实现时用受拥有者管理的完成/清理路径，不能简单按超时删除 Submitting 项后允许自动重发，以免产生重复收费短信。

（审查轮置信度 0.99；复核结论：PARTIAL — The diff detects removal during authorization via `state.operations.get(&operation_id).is_none()`, but it does not make peer-await cancellation settle as Unknown or otherwise ensure every cancelled pending operation is reclaimed. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```


## 38. IF6-15（P2）

**标题**：`email 的邮件正文在完整性检查之前就被完整物化，超大邮件仍会全量进内存`

**类别**：A 类：缺陷仍可触发　**标签**：`bug`

```
### 现象

一个积累大量未读邮件的账户启动轮询，或外部发送者在轮询间隔内投递大量/大体积邮件 → SEARCH 全量组装 fetch set，随后把所有 RFC822 正文 collect 到 Vec，直到全部取完才开始发布/释放。allowed_senders 在收完之后才判断，因此未授权发件人的大正文也占用这份无上限批次内存；可将 Core 进程推到 OOM。

### 位置

`src/gateway/interfaces/email/message_ops.rs:329-356`（审查轮基线：`79dacfa33`）

### 当前状态

src/gateway/interfaces/email/message_ops.rs:368 — "let body = match fetch.body() { Some(body) if Self::is_complete_imap_body(body, fetch.size) => body, _ => continue, };" — The batch is now consumed one message at a time with a per-message partial-fetch cap, but the body is still materialized before the completeness check, so the oversized body is allocated first.

### 建议改法

用固定有界批次拉取/处理，并约束单封或累计正文大小，避免为了释放 session borrow 收集整个邮箱；结合 PEEK/逐项确认，让本批之外的邮件保持 UNSEEN。只限制发布消息数却仍全量 FETCH/collect 不能解决内存问题。

（审查轮置信度 1.0；复核结论：PARTIAL — UID fetches are limited to 32, but the diff still collects a batch stream and does not establish a cumulative memory bound; additionally `fetch.body()` is obtained before the completeness check, so the oversized body may already be allocated. \|）

### 验收标准

- 一条能复现上述现象的单元测试：修复前失败、修复后通过；
- 该测试真的被运行（本轮 review 新增的测试只被编译、未运行）；
- 不改变该条之外的公共 API、错误变体与顺序 / 幂等语义。

### 出处

2026-11-09 `src/extension` / `src/fetch` / `src/gateway` 静态审查轮（IF6 批）；详见
`docs/superpowers/runs/2026-11-09-occams-extension-fetch-gateway/` 的 `findings/IF6.md`、`final/IF6.md` 与 `ISSUES.md`。
```

