# rust-doctor static review — `src/config`, `src/context`, `src/diagnostics` — finding list

**Date**: 2026-10-30 · **Base commit**: `569128f74` (main) · **Scope**: whole-file review, no diff

Each finding carries the severity proposed by the reader, the calibrated Jev triage score, the
triage disposition, and the outcome recorded by the worker that implemented it.

| id | reviewer | Jev | triage | outcome | location |
|---|---|---:|---|---|---|
| B01-1 | P1 | 0.85 | fix-now | fixed | `src/config/patcher.rs:658` — Hot-application is not atomic across live targets |
| B01-2 | P1 | 0.89 | fix-now | partial | `src/config/patcher.rs:430` — mtime conflict checking still permits an external edit to be overwritten |
| B01-3 | P2 | 0.86 | fix-now | fixed | `src/config/patcher.rs:417` — Candidate normalization silently overrides user input and can leave memory and disk inconsistent |
| B01-4 | P2 | 0.87 | fix-now | fixed | `src/config/validate.rs:177` — Rule validation does not enforce the constraints documented by `Config::validate` |
| B02-1 | P1 | 0.89 | fix-now | fixed | `src/config/save.rs:120` — 固定临时文件名使并发保存可能破坏已提交配置 |
| B02-2 | P1 | 0.93 | fix-now | fixed | `src/config/save.rs:123` — 敏感配置先以默认权限落盘，提交后才限制为 0600 |
| B02-3 | P1 | 0.87 | fix-now | fixed | `src/config/save.rs:357` — 全量保存删除同一文件中由其他解析器拥有的配置 |
| B02-4 | P1 | 0.89 | fix-now | fixed | `src/config/save.rs:274` — 保存 security 节会删除 loader 实际使用的嵌套 SSRF 配置 |
| B02-5 | P2 | 0.90 | fix-now | fixed | `src/config/load.rs:431` — SSRF 桥接解析失败被静默忽略，错误配置仍被报告有效 |
| B02-6 | P2 | 0.90 | fix-now | fixed | `src/config/save.rs:272` — 增量合并遗漏最终父节点的类型检查，未写入也返回成功 |
| B03-1 | P1 | 0.92 | fix-now | fixed | `src/config/agent_manager/crud.rs:215` — Agent creation silently drops access restrictions and other supplied fields |
| B03-2 | P1 | 0.91 | fix-now | fixed | `src/config/agent_manager/crud.rs:498` — Updating an inline identity table erases unpatched identity fields |
| B03-3 | P1 | 0.90 | fix-now | fixed | `src/config/agent_manager/crud.rs:262` — Concurrent agent mutations can overwrite already successful configuration changes |
| B03-4 | P1 | 0.90 | fix-now | fixed | `src/config/agent_manager/crud.rs:216` — Failed agent creation leaves a persisted definition that blocks retry |
| B03-5 | P1 | 0.90 | fix-now | fixed | `src/config/agent_manager/crud.rs:326` — Failed deletion removes the definition and can resurrect an unrestricted agent on restart |
| B03-6 | P1 | 0.80 | verify-then-fix | fixed | `src/config/reload_impact.rs:82` — The whole execution section is advertised as live despite startup-captured settings |
| B03-7 | P2 | 0.72 | verify-then-fix | fixed | `src/config/backup.rs:93` — Millisecond backup names can overwrite an earlier restore point |
| B04-1 | P2 | 0.92 | fix-now | fixed | `src/config/agent_manager/toml_ops.rs:55` — The whole config (provider API keys included) is staged through a world-readable fixed-name temp file that is never cleaned up |
| B04-2 | P2 | 0.80 | verify-then-fix | fixed | `src/config/agent_resolver/mod.rs:337` — An agent whose `profile` names a non-existent profile silently loses the profile tier instead of reporting it |
| B04-3 | P2 | 0.81 | verify-then-fix | fixed | `src/config/agent_manager/toml_ops.rs:159` — `agent_definition_to_table` persists a strict subset of `AgentDefinition`, so the create path is the lossy half of the row |
| B04-4 | P2 | 0.67 | defer | fixed | `src/config/tests/skill_doc_drift.rs:478` — The doc-drift guard's field arm switches off silently for a section whose field set stops resolving, and the production test never notices |
| B04-5 | P2 | 0.74 | verify-then-fix | fixed | `src/config/tests/skill_doc_drift.rs:930` — Guard B's "derives its path from ALEPH_HOME" escape hatch is evaluated per fence, so any mention of the token satisfies it |
| B04-6 | P3 | 0.59 | defer | fixed | `src/config/agent_manager/agent_files.rs:120` — `require_contained` documents symlink coverage it does not have |
| B04-7 | P3 | 0.76 | verify-then-fix | fixed | `src/config/agent_resolver/templates.rs:126` — The agent name is interpolated into the seeded prompt files unescaped; a newline smuggles identity fields that win the parser's first-occurrence rule |
| B05-1 | P2 | 0.53 | defer | deferred (Jev < 0.70) | `src/config/types/provider.rs:210` — Model compatibility fields are exposed as operator knobs without runtime consumers |
| B05-2 | P2 | 0.79 | verify-then-fix | rejected | `src/config/types/provider.rs:327` — Provider deserialization silently accepts misspelled configuration keys |
| B05-3 | P2 | 0.81 | verify-then-fix | fixed | `src/config/tests/save_incremental.rs:8` — Incremental-save preservation test does not exercise incremental-save production code |
| B05-4 | P2 | 0.79 | verify-then-fix | fixed | `src/config/tests/schema_integration.rs:35` — Schema structure test conditionally skips its required type assertion |
| B05-5 | P3 | 0.72 | verify-then-fix | fixed | `src/config/tests/serialization.rs:126` — “Multi-provider roundtrip” test never serializes or deserializes |
| B05-6 | P3 | 0.68 | defer | fixed | `src/config/tests/serialization.rs:218` — Real-config parse test depends on the developer’s HOME and conditionally skips all work |
| B06-1 | P1 | 0.93 | fix-now | fixed | `src/config/types/acp.rs:485` — ACP 预设回填将显式禁用的信任级别提升为 Full |
| B06-2 | P1 | 0.81 | verify-then-fix | fixed | `src/config/types/tools.rs:498` — 原生工具的启用开关和访问白名单没有接入运行时 |
| B06-3 | P2 | 0.87 | fix-now | fixed | `src/config/types/acp.rs:54` — 自定义名称的 ACP 条目不会继承其指定预设 |
| B06-4 | P2 | 0.63 | defer | fixed | `src/config/types/tools.rs:32` — ToolService 的全局及按工具超时配置没有消费者 |
| B06-5 | P2 | 0.63 | defer | fixed | `src/config/types/agents_def.rs:113` — Agent bootstrap 的两个长度限制只解析保存，没有应用 |
| B06-6 | P2 | 0.65 | defer | fixed | `src/config/types/agents_def.rs:109` — AgentDefaults.dm_scope 是仍被接受的无效配置 |
| B07-1 | P1 | 0.90 | fix-now | fixed | `src/config/types/voice_local.rs:318` — Changing `[voice.local]` leaves the generated `local` provider stale |
| B07-2 | P1 | 0.86 | fix-now | fixed | `src/config/types/routing.rs:115` — Routing rule `provider` is accepted but never selects the provider |
| B07-3 | P2 | 0.59 | defer | deferred (Jev < 0.70) | `src/config/types/routing.rs:128` — Four exposed routing controls are silently dropped |
| B07-4 | P1 | 0.90 | fix-now | partial | `src/config/types/voice_local.rs:11` — Voice bearer tokens are exposed by derived `Debug` |
| B08-1 | P1 | 0.78 | verify-then-fix | fixed | `src/config/types/generation/defaults.rs:172` — Configured generation defaults are validated but never applied to any request |
| B08-2 | P1 | 0.83 | verify-then-fix | fixed | `src/config/types/generation/presets/mod.rs:129` — `get_preset_by_type` hands an arbitrary vendor's endpoint and model to any custom provider of a protocol-level type |
| B08-3 | P2 | 0.83 | verify-then-fix | fixed | `src/config/types/generation/presets/registry.rs:485` — Three STT presets in the catalogue name provider types the generation factory cannot build |
| B08-4 | P2 | 0.88 | fix-now | fixed | `src/config/types/generation/config.rs:270` — `validate_provider_reference` accepts a default provider that does not support the modality it is the default for |
| B08-5 | P2 | 0.76 | verify-then-fix | partial | `src/config/types/generation/config.rs:83` — The legacy `providers` map and the five typed maps are two sources of truth, and a Panel update of a legacy provider lands in the wrong map |
| B08-6 | P2 | 0.81 | verify-then-fix | fixed (coordinator) | `src/config/types/generation/presets/mod.rs:157` — `GenerationPresetsOverride` has no speech/transcription map and no `deny_unknown_fields`, so a TTS/STT override is silently dropped |
| B08-7 | P2 | 0.86 | fix-now | fixed | `src/config/types/stop_hooks.rs:26` — `StopHookConfig.timeout_secs = 0` is accepted and permanently disables the stop-hook gate |
| B08-8 | P2 | 0.86 | fix-now | needs-other-file | `src/config/types/generation/presets/mod.rs:149` — The preset catalogue bypasses the merge function, so a `presets.toml` override is defeated by the value the client sends back |
| B09-1 | P1 | 0.75 | verify-then-fix | needs-other-file | `src/config/types/memory/embed.rs:63` — Configured embedding dimensions are silently bypassed by response truncation |
| B09-2 | P2 | 0.76 | verify-then-fix | partial | `src/config/types/memory/mod.rs:98` — Memory retrieval defaults contradict the documented legacy path |
| B11-1 | P1 | 0.84 | verify-then-fix | fixed | `src/config/types/policies/tool_permissions.rs:197` — Merging overlapping glob allowances can deny a tool allowed by both layers |
| B11-2 | P2 | 0.89 | fix-now | partial | `src/config/types/policies/spend.rs:37` — Non-finite spend ceilings parse successfully and silently disable enforcement |
| B12-1 | P1 | 0.87 | fix-now | fixed | `src/context/fabric.rs:664` — Snapshot admission has no redaction boundary |
| B12-2 | P1 | 0.91 | fix-now | fixed | `src/context/fabric.rs:662` — Claimed snapshot sizes can bypass the input budget |
| B12-3 | P1 | 0.91 | fix-now | fixed | `src/context/fabric.rs:1132` — Claimed result sizes can bypass the output budget |
| B13-1 | P1 | 0.91 | fix-now | fixed | `src/context/budget/mod.rs:353` — Invalid token-estimate ratios can disable budget enforcement |
| B13-2 | P2 | 0.71 | verify-then-fix | partial | `src/context/budget/preflight.rs:283` — External preflight admission uses a different token estimator from the budget |
| B13-3 | P2 | 0.82 | verify-then-fix | fixed | `src/context/file_ops.rs:80` — File-operation keys miss valid relative path aliases |
| B14-1 | P1 | 0.90 | fix-now | fixed | `src/context/budget/cheap_passes/file_op_supersede.rs:143` — A partial edit incorrectly supersedes the entire earlier file read |
| B14-2 | P1 | 0.91 | fix-now | fixed | `src/context/budget/cheap_passes/file_op_supersede.rs:104` — Whitespace-normalized path keys allow a write to one file to erase another file's read |
| B14-3 | P2 | 0.79 | verify-then-fix | fixed | `src/context/budget/cheap_passes/image_stripping.rs:86` — Image stripping reports gross image cost instead of net tokens saved |
| B15-1 | P2 | 0.58 | defer | deferred | `src/context/compact/compactor.rs:636 (LLM arm), :673 (truncation arm), :997 (merge arm); consumer: src/context/compact/directive.rs:288` — `tokens_after` measures only the summary body, so the "compaction applied" telemetry under-reports the real prompt by up to ~20k tokens |
| B15-2 | P2 | 0.85 | fix-now | fixed | `src/context/compact/compactor.rs:492-503 (the guard), :587-588 (the file's own statement of the real layout), :1176-1198 (`splice_preserved`), :561-570 (the `idx + 1 < window_end` branch that picks the full re-summarization prompt)` — The "already compacted" guard probes `messages[0]`, a shape this file never produces → a second in-place compaction rewrites the running summary at 25% of its size with nothing new folded in |
| B15-3 | P3 | 0.81 | verify-then-fix | fixed | `src/context/compact/compactor.rs:906 (the first `splice_preserved`), :960-965 (the cancel branch), :82-90 (`cancelled_result`), :390-420 (`compact`'s watchdog gate)` — The extend-merge arm commits its splice before the cancel check, contradicting the contract stated in its own comment |
| B15-4 | P3 | 0.35 | defer | deferred | `src/context/compact/compactor.rs:384 ("5. On success: replace the window with a single summary message.")` — `compact()`'s flow doc still describes the pre-preservation single-summary drain shape |
| B17-1 | P1 | 0.88 | fix-now | partial | `src/context/compact/session_split.rs:184` — Child-seed failure closes the parent and loses the live session tail |
| B17-2 | P2 | 0.86 | fix-now | fixed | `src/context/compact/session_split.rs:373` — Session-split summaries silently omit tool failures |
| B18-1 | P1 | 0.88 | fix-now | fixed | `src/context/compact/plan_carry.rs:106` — Clearing the scratchpad resurrects an older unfinished plan during compaction |
| B18-2 | P1 | 0.87 | fix-now | fixed | `src/context/compact/plan_carry.rs:71` — Recompaction drops an unfinished plan when all visible bounded items are completed |
| B18-3 | P1 | 0.89 | fix-now | fixed | `src/context/compact/plan_carry.rs:115` — Marker text in an ordinary message can impersonate an unbounded prior plan carry |
| B18-4 | P2 | 0.83 | verify-then-fix | fixed | `src/context/compact/event_snap.rs:67` — Reused run IDs pull the compaction cut back across already closed runs |
| B19-1 | P2 | 0.75 | verify-then-fix | fixed | `src/context/fabric/runtime.rs:1045` — A preflight that is skipped for low pressure is reported as a plugin contract violation |
| B19-2 | P2 | 0.62 | defer | deferred | `src/context/fabric/runtime.rs:822` — A Compose-mode plugin overwrites the compaction strategy and hides the deterministic-truncation degradation |
| B19-3 | P2 | 0.65 | defer | deferred | `src/context/fabric/runtime.rs:715` — `compact()` bounds each plugin invocation separately, so a batch of slow guests multiplies the turn's stall |
| B19-4 | P3 | 0.79 | verify-then-fix | fixed | `src/context/fabric/runtime.rs:628` — The plugin admission path persists a JSON-encoded original into `reduced_from` |
| B19-5 | P3 | 0.33 | defer | deferred | `src/context/compact/mod.rs:21` — `pub(crate) mod compaction_window;` contradicts its own "private to the compaction module" contract |
| B20-1 | P1 | 0.89 | fix-now | partial | `src/context/fabric/tool_admission.rs:32` — Invalid host session identity silently bypasses scoped tool admission |
| B20-2 | P1 | 0.88 | fix-now | fixed | `src/context/retrieval/content_index.rs:300` — Public multi-session retrieval accepts an unvalidated trust-domain set |
| B20-3 | P1 | 0.73 | verify-then-fix | fixed | `src/context/retrieval/content_index.rs:185` — Indexing has no input or storage bound and duplicates every blob into two FTS tables |
| B21-1 | P1 | 0.93 | fix-now | fixed | `src/diagnostics/redact.rs:111` — URL userinfo credentials pass through diagnostic redaction unchanged |
| B21-2 | P1 | 0.86 | fix-now | fixed | `src/diagnostics/redact.rs:67` — Authorization redaction only recognizes Bearer and Basic schemes |
| B21-3 | P1 | 0.88 | fix-now | fixed | `src/diagnostics/redact.rs:81` — JSON secret fields and quoted assignment values evade full masking |
| B21-4 | P1 | 0.90 | fix-now | fixed | `src/diagnostics/redact.rs:77` — Multiline PEM private keys retain their key material after redaction |
| B21-5 | P1 | 0.88 | fix-now | fixed | `src/diagnostics/redact.rs:101` — Query token masking skips values beginning with an asterisk or equals sign |
| B21-6 | P1 | 0.87 | fix-now | fixed (coordinator) | `src/diagnostics/check.rs:64` — A panic in a HealthCheck future aborts the complete diagnostic report |
| B22-1 | P1 | 0.92 | fix-now | fixed | `src/diagnostics/checks/discord_channel_health.rs:258` — Discord health check cannot read the production configuration |
| B22-2 | P1 | 0.88 | fix-now | fixed | `src/diagnostics/checks/session_log.rs:214` — Projection enumeration failure can produce a false clean session-log result |
| B22-3 | P1 | 0.87 | fix-now | fixed | `src/diagnostics/checks/session_log.rs:251` — A single huge session log is loaded unboundedly into memory |
| B22-4 | P1 | 0.88 | fix-now | fixed | `src/diagnostics/checks/fold_economics.rs:316` — Malformed provider token counts can panic or wrap fold economics |
| B22-5 | P1 | 0.81 | verify-then-fix | fixed | `src/diagnostics/checks/fold_economics.rs:280` — Undecodable fold events are silently treated as no folds |
| B22-6 | P2 | 0.84 | verify-then-fix | fixed | `src/diagnostics/checks/engine_missing.rs:359` — A non-executable pinned obscura file is reported as an available engine |
| B23-1 | P1 | 0.87 | fix-now | fixed (coordinator) | `src/diagnostics/checks/hooks_consent.rs:38` — An unreadable or corrupt hook-consent registry is reported as "[ok] No shell hooks" |
| B23-2 | P1 | 0.92 | fix-now | fixed | `src/diagnostics/checks/providers_connectivity.rs:174` — The vault value overwrites the provider's configured api_key, so a working provider is probed unauthenticated |
| B23-3 | P2 | 0.88 | fix-now | fixed | `src/diagnostics/checks/providers_connectivity.rs:249` — Every probe failure is titled "unreachable", so a rejected credential is misclassified and trips the outage gate |
| B23-4 | P2 | 0.88 | fix-now | fixed | `src/diagnostics/checks/media_codecs.rs:173` — The `gst-inspect-1.0` child is not killed when the check's deadline fires |
| B23-5 | P2 | 0.86 | fix-now | partial | `src/diagnostics/checks/stale_lock.rs:100` — "[ok] Server running" trusts the recorded PID alone, and a start-time-less record falls back to bare liveness |
| B23-6 | P2 | 0.77 | verify-then-fix | fixed | `src/diagnostics/checks/cache_hit_rate.rs:87 (the fixture at :210; the same copy in src/diagnostics/checks/cache_health.rs:238)` — The cache checks hand-copy the producer's JSON field names, so a rename silently blinds them |
| B24-1 | P1 | 0.90 | fix-now | fixed | `src/diagnostics/checks/config_parse.rs:113` — 配置解析错误会把未被模式识别的 API 密钥带入诊断输出 |
| B24-2 | P1 | 0.91 | fix-now | fixed | `src/diagnostics/checks/config_parse.rs:85` — 只读配置诊断调用会持久化自动迁移 |
| B24-3 | P1 | 0.94 | fix-now | fixed | `src/diagnostics/checks/data_dir.rs:31` — 固定名称的写探针会截断现有文件并跟随符号链接破坏外部文件 |
| B24-4 | P1 | 0.90 | fix-now | partial | `src/diagnostics/checks/duplicate_instance.rs:25` — 冷启动 doctor 将唯一正常 daemon 误报为重复实例并建议杀掉它 |
| B24-5 | P1 | 0.90 | fix-now | fixed | `src/diagnostics/checks/disk_space.rs:93` — 使用包含保留块的空闲容量会在普通用户已无法写入时报告磁盘健康 |

## Outcome totals

- **fixed**: 79
- **partial**: 10
- **deferred**: 5
- **fixed (coordinator)**: 3
- **deferred (Jev < 0.70)**: 2
- **needs-other-file**: 2
- **rejected**: 1

## Detail

### Batch B01

#### B01-1 [P1] — Jev 0.85 (fix-now) — fixed

- **Location**: `src/config/patcher.rs:658`
- **Defect**: `rollback` installs the restored configuration and then calls `apply_live_sections` for every live target, but the live applier executes targets sequentially and only records the ones whose individual operation returns true. There is no preflight, rollback, or failure result that makes the runtime transition atomic.
- **Failure mode**: A rollback that changes `execution`, `policies.spend`, and `search` can update `execution` and `policies.spend`, then encounter a missing search handle or failed search rebuild. The file and in-memory `Config` are fully restored while the running process continues with a mixture of old and restored live settings; `RollbackResult.success` still returns true.
- **Change**: rollback now compares the live targets requested by `reload_impact::live_targets()` with the targets actually returned by `apply_live_sections`. An incomplete hot-apply adds a warning and returns `RollbackResult.success = false`.
- **Notes**: this uses the brief's allowed failed/incomplete-result strategy rather than adding a cross-module compensating transaction.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B01-2 [P1] — Jev 0.89 (fix-now) — partial

- **Location**: `src/config/patcher.rs:430`
- **Defect**: `commit_patch` rechecks the recorded mtime and then later writes the incremental config at `src/config/patcher.rs:446`; the check and write are separate filesystem operations. The in-memory write lock serializes this process's callers but cannot exclude another process editing the file between those two operations.
- **Failure mode**: An external editor changes `config.toml` immediately after `check_conflict()` returns and before `save_incremental_to_file()` runs. The patcher sees the old mtime, writes its section, and overwrites the external edit without returning the documented “modified externally” error.
- **Change**: config saves now take the shared sidecar advisory lock; incremental saves hold it across read, merge, mtime recheck, and atomic replacement. The mtime is checked again immediately before serialization/commit.
- **Notes**: an unrelated external process that ignores the repository's advisory lock can still race in the final filesystem interval; a fully portable compare-and-write primitive is outside the five owned files.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B01-3 [P2] — Jev 0.86 (fix-now) — fixed

- **Location**: `src/config/patcher.rs:417`
- **Defect**: Every patch candidate is passed through `normalize_default_provider`, which replaces an explicitly supplied missing `general.default_provider` with the alphabetically first configured provider. The commit then persists only `top_section` at `src/config/patcher.rs:446`, even though normalization may have changed `general.default_provider` outside that section.
- **Failure mode**: A user patches `general.default_provider` to `"missing"` while providers exist; the request succeeds but the stored value becomes a different provider without the requested value being rejected. More seriously, deleting the current default through a `providers` patch can make the in-memory config fall back to (for example) `beta` while the incremental save leaves `general.default_provider = "alpha"` on disk; the running process and the next load observe different documents.
- **Change**: patch candidate validation no longer calls `normalize_default_provider`; it only applies the existing voice-local normalization and then validates.
- **Notes**: load-time normalization remains unchanged for boot compatibility.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B01-4 [P2] — Jev 0.87 (fix-now) — fixed

- **Location**: `src/config/validate.rs:177`
- **Defect**: `Config::validate` documents provider-reference and regex validation, but `validate_rules` only warns about retired keyword rules and the empty-provider/default case. It never checks that a rule's provider exists or that its regex compiles.
- **Failure mode**: A patch can write a rule with a misspelled provider or malformed regex, `Config::validate()` still returns `Ok(())`, and the invalid rule is accepted into the live configuration until the routing path attempts to use it, causing the rule to be unusable or making request handling fail at runtime.
- **Change**: every rule regex is compiled during validation; every explicit provider reference must exist; command rules must provide a provider.
- **Notes**: retired keyword rules retain their existing warning behavior.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

### Batch B02

#### B02-1 [P1] — Jev 0.89 (fix-now) — fixed

- **Location**: `src/config/save.rs:120`
- **Defect**: `write_atomically` 的所有调用都使用同一个 `path.with_extension("tmp")`，且通过 `fs::write` 截断该文件。临时文件没有独占创建，也没有跨调用的写入互斥，因此单次调用的 rename 不能保证并发保存安全。
- **Failure mode**: 两个进程同时保存同一配置，B 截断 A 已写好的临时文件，A 随后将其 rename 为正式配置；B 的未完成写入继续作用于已经成为正式配置的同一 inode。此时正式配置可能短暂或永久只包含部分内容，B 后续还可能因临时路径已不存在而返回失败。
- **Change**: replaced the fixed `.tmp` implementation with the repository's unique temporary-file `write_atomic` implementation and serialized saves under `with_file_lock`.
- **Notes**: the existing atomic helper also fsyncs the temporary file and parent directory.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B02-2 [P1] — Jev 0.93 (fix-now) — fixed

- **Location**: `src/config/save.rs:123`
- **Defect**: 临时文件由 `fs::write` 使用默认创建权限写入，`enforce_0600_strict` 和 `enforce_0600_warn` 分别直到第 373、477 行才作用于 rename 后的目标。配置中的 API key 会先落入可能允许其他用户读取的临时文件。
- **Failure mode**: 在 umask 为 022、配置目录可被其他用户遍历的 Unix 环境中，保存含 API key 的配置会生成 0644 的 `config.tmp`；其他用户可在保存期间读取它。若进程在 rename 或 chmod 前退出，宽松权限的敏感文件会持续保留。
- **Change**: the shared atomic writer creates temporary files with Unix mode `0600` before writing any configuration bytes.
- **Notes**: the post-rename strict/best-effort permission checks remain as defense in depth.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B02-3 [P1] — Jev 0.87 (fix-now) — fixed

- **Location**: `src/config/save.rs:357`
- **Defect**: `save_to_file` 只序列化 `Config`，随后整体替换目标文件；`Config` 不包含 `[gateway]` 等仍然有效的外部所有权字段。加载时允许并认可这些字段，保存时却将其永久丢弃。
- **Failure mode**: 含自定义 `[gateway]` 的配置经加载后调用 `save_to_file`，该节消失。现有生产调用 `src/config/patcher.rs:650` 的配置回滚也走此路径，因此回滚一个包含 gateway 设置的快照后，重启仍可能回到 gateway 默认配置。
- **Change**: full saves start from the serialized `Config` value and preserve existing top-level TOML sections not owned by `Config`; the existing `[security.ssrf]` bridge table is also preserved when absent from the typed value.
- **Notes**: malformed existing TOML is still treated as an ordinary full-save failure boundary by the existing caller behavior.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B02-4 [P1] — Jev 0.89 (fix-now) — fixed

- **Location**: `src/config/save.rs:274`
- **Defect**: `merge_sections` 整体替换指定节，未保留该节内由其他路径读取的字段。`Config.security` 不包含 `ssrf`，但 `src/config/load.rs:417` 的桥接逻辑实际读取 `[security.ssrf]`，因此该有效子节会在保存 `"security"` 时被删除。
- **Failure mode**: 文件仅通过 `[security.ssrf] blocked_hosts` 配置阻止某个站点，随后通过配置 patcher 修改 `security.enable_custom_patterns`。`src/config/patcher.rs:446` 保存整个 security 节，删除 `security.ssrf`，又不会同步保存承接桥接结果的根级 `[ssrf]`；下次加载时，站点阻止列表丢失。
- **Change**: incremental replacement of the `security` section carries forward an existing `security.ssrf` table when the typed `Config.security` value does not contain it. Full saves use the same preservation rule.
- **Notes**: no dependency from config to gateway was introduced.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B02-5 [P2] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/config/load.rs:431`
- **Defect**: `SecuritySsrfOverrides` 反序列化失败时直接返回，既不报错也不告警，而且会丢弃该表中的全部合法覆盖。`src/config/dead_keys.rs:87` 又整体容忍 `security.ssrf`，因此 dead-key 检查也无法暴露该问题。
- **Failure mode**: 配置 `[security.ssrf]` 中同时包含合法的 `blocked_hosts = ["blocked.example"]` 和误写为字符串的 `max_redirects = "2"`。整个覆盖对象解析失败，阻止列表也不生效，但配置加载和 dead-key 检查仍可成功。
- **Change**: `SecuritySsrfOverrides` now denies unknown fields; bridge deserialization returns `Result`; malformed values and out-of-range `max_redirects` fail config loading with an explicit error.
- **Notes**: missing `[security.ssrf]` remains valid and leaves defaults unchanged.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

#### B02-6 [P2] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/config/save.rs:272`
- **Defect**: `navigated` 只检测遍历中间节点时遇到的非表值；遍历结束后，如果 `target` 本身不是表，`if let` 会直接跳过写入，且不会向 `blocked` 添加错误。随后增量保存仍写回文档并报告成功。
- **Failure mode**: 已有文件包含合法 TOML `general = "invalid"`，调用方持有有效的内存配置并请求保存 `"general.browser"`。遍历取得字符串类型的 `general` 后结束，browser 没有写入，函数却返回 `Ok(())`；用户要求的修改没有持久化。
- **Change**: `merge_sections` now records an error when the final parent value is not a TOML table, in addition to intermediate blocked parents.
- **Notes**: no partial write is committed when merge validation returns errors.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

### Batch B03

#### B03-1 [P1] — Jev 0.92 (fix-now) — fixed

- **Location**: `src/config/agent_manager/crud.rs:215`
- **Defect**: `AgentManager::create` delegates persistence to `append_agent_to_document`, whose `agent_definition_to_table` in `src/config/agent_manager/toml_ops.rs` writes only a subset of `AgentDefinition`. In particular, `allowed_users`, `allowed_links`, `skills_blacklist`, and `tool_permissions` are never written, although they have runtime consumers and are accepted by the public creation API.
- **Failure mode**: Creating an agent with `allowed_users = Some(["u-alice"])` returns `Ok(())`, but `get` immediately returns `allowed_users = None`. A subsequent boot resolves that persisted definition as accessible to every authenticated user; supplied link restrictions and blocked tools are likewise lost rather than preserved.
- **Change**: Agent creation now serializes the complete `AgentDefinition` through its serde contract instead of a hand-maintained subset. The generated table retains active restriction and access fields, while the deprecated `workspace` field is explicitly removed.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B03-2 [P1] — Jev 0.91 (fix-now) — fixed

- **Location**: `src/config/agent_manager/crud.rs:498`
- **Defect**: `apply_patch_to_table` recognizes only `Item::Table` as an existing identity. A valid TOML inline table is an `Item::Value(InlineTable)`, so the `!ident_item.is_table()` branch replaces it with an empty table before applying the partial patch. The preservation test seeds the identity through `update`, which creates a regular table and never exercises this representation.
- **Failure mode**: With `identity = { emoji = "robot", description = "Original", avatar = "avatar.png" }` in an agent definition, patching only `identity.description` succeeds but removes both `emoji` and `avatar` from the saved configuration.
- **Change**: `apply_patch_to_table` now uses `as_table_like_mut`, preserving both regular TOML tables and inline tables when applying a partial identity patch. An unexpected scalar identity value returns a configuration error instead of being replaced with an empty table.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B03-3 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/config/agent_manager/crud.rs:262`
- **Defect**: The CRUD methods perform unlocked load/modify/save transactions on `&self`; `AgentManager` has no transaction lock. Production RPC handlers and agent tools share it as `Arc<AgentManager>`, and `save_document` additionally uses the same `config.toml.tmp` pathname for every writer. Atomic rename protects a single replacement, not the whole read/modify/write operation.
- **Failure mode**: Two updates to different agents read the same original document. If the first saves before the second writes its temporary file, both return success, but the second document restores the first agent's old values. If their temporary-file operations overlap, one operation can instead rename another operation's bytes or fail because that other operation already renamed the shared temporary file. Concurrent creates can similarly lose an agent or admit duplicate IDs.
- **Change**: Agent create, update, delete, default changes, and orphan cleanup now run inside a shared sidecar lock. The lock covers the complete load/validate/modify/save operation, and saving uses the repository atomic writer rather than a shared fixed temporary pathname.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B03-4 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/config/agent_manager/crud.rs:216`
- **Defect**: `create` commits the TOML definition before initializing the agent directory, identity files, and workspace. Any failure in those subsequent operations returns `Err` without removing the committed row or reporting that creation partly succeeded.
- **Failure mode**: If the configured agents root or workspace root is unwritable, creating `researcher` returns an I/O error after adding `researcher` to config. Fixing permissions and repeating the same request then fails with `Agent 'researcher' already exists`; on the RPC path, the first error also skips runtime registration even though the agent is now persisted.
- **Change**: `create` provisions the agent directory, identity files, and workspace before publishing the TOML definition. Failed provisioning removes only directories created by that attempt and reports cleanup failures.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B03-5 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/config/agent_manager/crud.rs:326`
- **Defect**: `delete` saves removal of the TOML row before either archival operation, and does not restore the definition when an archive fails. `AgentManager::new` later reconciles a remaining agent-state directory by appending a minimal `AgentDefinition`, dropping the original model, permissions, and admission restrictions.
- **Failure mode**: With an agent root on a different filesystem from the trash root, archiving the agent-state directory fails with a cross-device rename error after the config row has already been removed (and the workspace may already have moved). The caller gets an error, retry reports the agent missing, and restart imports the remaining directory as a fresh unrestricted definition. A read-only or otherwise unwritable trash directory produces the same missing-row/reconciliation problem even when the first move fails.
- **Change**: `delete` archives workspace and agent-state directories before saving the removed definition. If archiving or the final save fails, moved directories are restored where possible and the original restriction-bearing definition remains in memory/document state.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B03-6 [P1] — Jev 0.80 (verify-then-fix) — fixed

- **Location**: `src/config/reload_impact.rs:82`
- **Defect**: `LIVE_SECTIONS` includes all of `execution`, so every descendant is classified `Live`. Its apply arm in `src/config/live_apply.rs` updates only run-concurrency caps and the subagent cap; other settings are captured at startup, including `max_iterations` and `prompt_mode` in `src/bin/aleph-server/commands/start/orchestrator_init.rs:427` and `src/bin/aleph-server/commands/start/orchestrator_init.rs:431`, and timeout/steering/progress settings in `src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:985`. Successfully updating the concurrency handle makes `classify_verified` endorse unrelated execution fields too.
- **Failure mode**: On a running server with the concurrency handle installed, changing `execution.max_iterations` from 1000 to 10 returns the guidance that the change applies on the next prompt without restart. The existing harness bridge retains its startup-captured `default_max_iterations`, so subsequent runs still use 1000; changing `execution.prompt_mode` similarly keeps the previous prompt tier.
- **Change**: `execution` is now live only for `max_runs_global`, `max_runs_per_agent`, and `max_concurrent_subagents`. Whole-section changes and startup-captured execution fields classify as restart, and tests cover both sides of the boundary.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B03-7 [P2] — Jev 0.72 (verify-then-fix) — fixed

- **Location**: `src/config/backup.rs:93`
- **Defect**: `create_snapshot` uses only a wall-clock timestamp with millisecond precision to identify a backup, and `fs::copy` overwrites an existing destination. There is no exclusive creation or collision retry, so the timestamp reduces collisions without preventing them.
- **Failure mode**: Two snapshots of successive config states taken in the same millisecond resolve to the same `config.toml.<timestamp>` path and both return success. The second replaces the first backup, leaving only one restore point and losing the state needed to undo both changes. Concurrent patch requests can reach snapshot creation independently, before the patcher's configuration write lock.
- **Change**: backup destinations are reserved with `create_new`, created with Unix mode `0600`, and retried with sortable `-NNNN` suffixes on collision. Copy failures remove the reserved destination.
- **Notes**: no cargo, rustc, clippy, or test command was run. `rustfmt --edition 2021` and `git diff --check` passed for the five owned files.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W1.md`

### Batch B04

#### B04-1 [P2] — Jev 0.92 (fix-now) — fixed

- **Location**: `src/config/agent_manager/toml_ops.rs:55`
- **Defect**: `save_document` serialises the ENTIRE document (`doc.to_string()`, which includes `[providers.*].api_key`) and writes it to `self.config_path.with_extension("toml.tmp")` with `fs::write` (L55-57) — i.e. mode `0666 & !umask` → `0644` under the common umask 022. Only the *destination* is chmod'ed back to `0600` after the rename (L80-88); the staging file itself is never chmod'ed, never given a unique name, and never removed when the rename fails. The repo already owns two canonical atomic writers that get this right: `crate::utils::atomic_io::write_atomic` (random `.aleph_atomic_<rand>` sibling, `options.mode(0o600)` at creation, fsync, best-effort cleanup of the leaked temp on rename failure) and `crate::utils::atomic_write::atomic_write_bytes`.
- **Failure mode**: `AgentManager` is constructed with `Config::effective_path()` (src/bin/aleph-server/commands/start/mod.rs:1284-1285) — the same `~/.aleph/config.toml` that `src/config/save.rs:92` keeps at `0600` and that holds provider secrets (`ProviderConfig.api_key`, src/config/types/provider.rs:336). Every agent create/update/delete/default-set rewrites that file through the staging path. On a host where `~` and `~/.aleph` are traversable (the data dir is created with a plain `create_dir_all`, src/utils/paths.rs:386 → 0755), any local user can read `~/.aleph/config.toml.tmp` for the duration of the write window, and — because the error path (L77-78) returns without removing the temp — permanently if `fs::rename` fails or the process dies between the write and the rename. The fixed temp name also collides between two writers on the same config.
- **Change**: `save_document` now delegates to `crate::utils::atomic_io::write_atomic`, which uses private unique staging, synchronization, rename, and cleanup behavior already owned by the repository.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B04-2 [P2] — Jev 0.80 (verify-then-fix) — fixed

- **Location**: `src/config/agent_resolver/mod.rs:337`
- **Defect**: `resolve_one` resolves the profile with `agent.profile.as_ref().and_then(|name| profiles.get(name)).cloned().unwrap_or_default()` — a missing `[profiles.<name>]` block yields `ProfileConfig::default()` (model: `None`) with no log line. The sibling selection path `resolve_model_ref` (L47-64) explicitly warns when a configured selection cannot be honoured ("selected agent model unavailable … falling back to system default"), so the two halves of the same resolution disagree about whether an unavailable selection is worth telling the operator about. Nothing else in `src/config` ties `agent.profile` to the profiles map.
- **Failure mode**: `[[agents.list]]` has `profile = "coding"` but the config no longer defines `[profiles.coding]` (typo, or the profile block was deleted) → `profile.model` is `None` → the model chain (L344-350: agent.model → `resolve_model_ref` → defaults.model → profile.model → `DEFAULT_MODEL`) skips the profile tier → the agent runs on the system default model. No warning, no error, and the agent list still shows the stale profile name — the operator believes the agent is pinned to the profile's model.
- **Change**: Profile resolution explicitly distinguishes no profile, an existing profile, and an unknown named profile. Unknown profiles emit a warning and then use the existing default fallback.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B04-3 [P2] — Jev 0.81 (verify-then-fix) — fixed

- **Location**: `src/config/agent_manager/toml_ops.rs:159`
- **Defect**: `agent_definition_to_table` writes only `id`, `default`, `name`, `profile`, `model`, `skills`, `archetype`, `identity{emoji,description,avatar}` and `subagents{allow}`. It silently drops `skills_blacklist`, `tool_permissions`, `allowed_links` and `allowed_users` — all four of which exist on `AgentDefinition` (src/config/types/agents_def.rs) and all four of which the sibling writer `apply_patch_to_table` DOES persist (`skills_blacklist` L521, `allowed_links` L537, `allowed_users` L555; `tool_permissions` is written by neither). Two writers of the same `[[agents.list]]` row disagree about the row's shape.
- **Failure mode**: `AgentManager::create(def)` persists the row from `def`, while the runtime agent is hot-registered from the *same* `def` (src/gateway/handlers/agents.rs:227 → `AgentDefinitionResolver::resolve_one`) — so during the process lifetime the live agent carries the fields, but after a restart the resolver reads the TOML row, where they are gone: an `allowed_users` admission list silently becomes "everybody admitted" (`agent_admits_user` treats `None` as allow-all) and a `skills_blacklist` silently becomes "no blacklist". This is latent today (the two production `create` callers — src/gateway/handlers/agents.rs:202 and src/builtin_tools/agent_manage/create.rs:499 — set none of the four), but it is one call site away from a silent, security-relevant loss, and it is exactly the class of defect the brief calls "TOML round-trip loses fields".
- **Change**: The hand-written partial `AgentDefinition` projection was replaced with serde serialization into a TOML document table, followed by removal of only the deprecated `workspace` key.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B04-4 [P2] — Jev 0.67 (defer) — fixed

- **Location**: `src/config/tests/skill_doc_drift.rs:478`
- **Defect**: in `audit`, a row whose section is live but whose field set did not resolve is pushed into `report.unchecked` and skipped (L478-481): no section finding, no field finding, not a failure. The production test `the_bundled_self_skill_names_no_phantom_config_keys` (L569) asserts `tables_found == TABLES.len()`, a global `rows_scanned > 10` floor, a global `truth.fields.len() >= 20` count and `findings.is_empty()` — it never asserts anything about `report.unchecked`, and a global count of 20 resolved sections stays satisfied while one specific section loses its field set.
- **Failure mode**: a schema-shape change (schemars renaming `$defs`, a section becoming a free-form map, or a section nested deeper than `collect_field_names`' hard depth cap of 8, L251-286) makes one section's field truth unresolvable → every row of that section now only gets the "is this section live?" check, and a phantom field token inside it (e.g. `[security].ssrf_typo`) is no longer reported → the guard stays green while the field arm is dark for that section. The fixture test pins `unchecked == {"gateway"}` for the fixture (L768) but the real document's `unchecked` set is never asserted, so a new unchecked section is invisible to the guard that is supposed to police it.
- **Change**: The production doc-drift test now asserts the exact known `report.unchecked` baseline, `{"gateway"}`, in addition to the existing global field-resolution floor. An unexpected unchecked section fails the guard instead of being silently skipped.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B04-5 [P2] — Jev 0.74 (verify-then-fix) — fixed

- **Location**: `src/config/tests/skill_doc_drift.rs:930`
- **Defect**: `hardcoded_config_paths` skips a fence when `fence.body.contains(ALEPH_HOME_VAR)` — a whole-fence substring test, not a per-snippet or per-line one, so the escape hatch is satisfied by the mere presence of the token anywhere in the fence (a comment, a different snippet, a prose line). The guard's own fixture relies on exactly that shape: its last fence is tolerated only because the word appears in a trailing comment, and that fence contains no ALEPH_HOME-derived path at all.
- **Failure mode**: a bundled-skill fence that hardcodes `~/.aleph/config.toml` in one snippet while mentioning `ALEPH_HOME` in a comment (or in an unrelated second snippet) is never reported → the hardcoded path ships to the model → on a host with ALEPH_HOME set the snippet edits a file the daemon never reads and reports success, or in Python it creates a literal `./~/.aleph/` tree in the cwd — precisely the failure the guard's own finding text describes and exists to prevent.
- **Change**: The `ALEPH_HOME` escape hatch is evaluated per hardcoded-path line. A path is tolerated only when that same line also contains `ALEPH_HOME`; an unrelated comment or second snippet in the fence no longer suppresses the finding. The fixture now covers the unrelated-line case.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B04-6 [P3] — Jev 0.59 (defer) — fixed

- **Location**: `src/config/agent_manager/agent_files.rs:120`
- **Defect**: the doc comment says the check "Catches traversal that survives string-level validation (e.g. symlinks resolved by the OS, or crafted relative paths)", but the body only calls `normalize_path_lexically` (L122-146, helper at L139) — a purely textual `..`/`.` normalisation that never touches the filesystem. `fs::write` / `fs::read_to_string` in `write_file` (L86) / `read_file` (L70) do follow symlinks.
- **Failure mode**: if the agent directory contains a symlink (planted by another process, or left by the user), `agents.files.set` / `agents.files.get` writes/reads *through* it to a target outside the agent directory while the containment check reports OK. Practical reach is limited — `validate_filename` (src/config/agent_manager/crud.rs:382-412) already forces a single `Component::Normal` name and rejects ':' — so this is an overclaiming doc comment plus an uncovered symlink case, not a live traversal hole.
- **Change**: Narrowed the `require_contained` documentation to state that it performs lexical `.`/`..` containment only and does not resolve symlinks. The comment records that `list_files` separately skips symlinked entries.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

#### B04-7 [P3] — Jev 0.76 (verify-then-fix) — fixed

- **Location**: `src/config/agent_resolver/templates.rs:126`
- **Defect**: `default_identity` interpolates the raw `agent_name` into `- **Name:** {agent_name}` and `default_agents` into `# AGENTS.md — {agent_name}'s Operating Manual` with no escaping of newlines (unlike the id, which `validate_id` restricts to `[A-Za-z0-9_-]`, the name is accepted verbatim from `CreateAgentParams.name` / the `agent_create` tool). `AgentIdentityProfile::from_markdown` (src/thinker/identity_profile.rs:118-146) parses line by line and binds the FIRST occurrence of each label, and `- **Name:**` is emitted BEFORE `- **Role:**` (templates.rs:126-128).
- **Failure mode**: creating an agent named `"Bob\n- **Role:** god-mode"` writes a second `- **Role:**` line into IDENTITY.md *above* the archetype's role line → the parser binds the smuggled value ("first occurrence wins") → the agent's self-described role becomes "god-mode" instead of the archetype role, silently. The same newline also truncates the round-tripped name (the parser reads back only "Bob" while the config and the registry keep the full string), and the injected lines land in AGENTS.md/IDENTITY.md, which are fed to the model as the agent's operating manual.
- **Change**: `default_soul`, `default_agents`, and `default_identity` replace carriage returns and newlines in the supplied display name with spaces before interpolation into seeded files.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W2.md`

### Batch B05

#### B05-1 [P2] — Jev 0.53 (defer) — not-handled

- **Location**: `src/config/types/provider.rs:210`
- **Defect**: `ModelCompat` deserializes `forbids_sampling_params`, `temperature_policy`, `supports_cache_control`, `thinking_format`, and `vendor_behavior_profile`, but the type documents the first three as heuristic/backward-compatibility inputs and the last two as schema round-trip only. A provider configuration can therefore carry per-model values that survive load/save and appear in the generated schema while the configured value does not control the corresponding request behavior.
- **Failure mode**: An operator sets `providers.x.model_defs[].compat.thinking_format = "deepseek"` or `vendor_behavior_profile = "strict"` expecting that model's request encoding or vendor policy to change; the value is parsed and persisted, but no production request path reads either field. The same mismatch applies when an operator sets the per-model sampling/cache flags instead of the currently hard-coded protocol heuristics.

#### B05-2 [P2] — Jev 0.79 (verify-then-fix) — rejected

- **Location**: `src/config/types/provider.rs:327`
- **Defect**: `ProviderConfig` derives `Deserialize` without `#[serde(deny_unknown_fields)]`, and its nested configuration structs follow the same permissive default. TOML/JSON keys that are not part of the schema are discarded without an error or warning.
- **Failure mode**: A typo such as `stream_idle_timeout_sec` or `cache_retentionn` is accepted as a valid provider configuration, the actual field remains at its default (`None`/the adapter default), and the operator receives no indication that the requested timeout or retention policy is inactive. This is especially easy to miss because the config can still pass validation and save back without the unknown key.
- **Change**: 核实后保留现有宽容反序列化；未添加 `deny_unknown_fields`。Added `provider_typos_are_reported_by_the_config_loader`, using a temporary file and IsolatedAlephHome, asserting the exact diagnostic paths `providers.example.cache_retentionn` and `providers.example.stream_idle_timeout_sec` and unchanged actual defaults.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3b.md`

#### B05-3 [P2] — Jev 0.81 (verify-then-fix) — fixed

- **Location**: `src/config/tests/save_incremental.rs:8`
- **Defect**: `test_save_incremental_preserves_other_sections` says it tests `save_incremental`, but it reads TOML, inserts the `behavior` table, serializes, and writes the file directly in the test. `Config::save_incremental_to_file` is never called, so the test duplicates the intended algorithm rather than checking the implementation's guards, section selection, atomic write, or error handling.
- **Failure mode**: A regression in `Config::save_incremental_to_file` that erases `[providers]`, mishandles a missing section, or writes non-atomically can leave this test green because the test's locally copied merge logic continues to preserve the fixture. The test therefore reports coverage for the most important preservation behavior while proving only TOML library behavior.
- **Change**: Removed the test-local TOML merge/write algorithm. Parse the initial fixture as Config, set behavior output_mode to `instant`, call production `save_incremental_to_file(&config_path, &["behavior"])`, then read the result. Added structural equality assertions on general/providers/rules/memory plus an exact updated behavior assertion.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3b.md`

#### B05-4 [P2] — Jev 0.79 (verify-then-fix) — fixed

- **Location**: `src/config/tests/schema_integration.rs:35`
- **Defect**: `test_schema_json_structure` only checks that the top-level `type` is correct when the key happens to exist. If schema generation regresses to omit `type`, the test passes; its comment says the field is required but the assertion is optional.
- **Failure mode**: A schema generator change removes or relocates the top-level object type while leaving `properties` or `$ref` present. The test still succeeds, allowing consumers that rely on the object shape to receive a weaker schema without a test failure.
- **Change**: Replaced optional `if let Some(type)` with an unconditional `assert_eq!(json.get("type").and_then(serde_json::Value::as_str), Some("object"))`.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3b.md`

#### B05-5 [P3] — Jev 0.72 (verify-then-fix) — fixed

- **Location**: `src/config/tests/serialization.rs:126`
- **Defect**: `test_multi_provider_config_roundtrip` only inserts two providers into an in-memory `Config` and checks the map length and keys. It performs no TOML serialization, deserialization, or field comparison despite its name and location in the serialization suite.
- **Failure mode**: A serde rename, provider enum change, or round-trip omission affecting one of the two provider entries can break persisted configuration while this test remains green, because the values are never passed through serde.
- **Change**: Multi-provider test now serializes the populated Config to TOML and deserializes it. It asserts the loaded provider count and each provider's protocol and complete models vector against the source Config.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3b.md`

#### B05-6 [P3] — Jev 0.68 (defer) — fixed

- **Location**: `src/config/tests/serialization.rs:218`
- **Defect**: `test_real_config_file_parse` reads `$HOME/.aleph/config.toml`; when the file is absent it only prints a skip message, and when present its behavior depends on arbitrary user configuration and local validation state. This makes the test non-reproducible and gives different coverage on different machines or CI workers.
- **Failure mode**: A broken migration or parser can go completely untested on a clean CI environment because no fixture exists. Conversely, an unrelated user-local config can make the test fail or take a different validation branch, so a green result does not establish a stable repository contract.
- **Change**: Replaced the developer-HOME conditional test with `test_config_fixture_file_parse`, an inline deterministic TOML fixture in TempDir. IsolatedAlephHome prevents the full loader from reading real Aleph defaults/presets. Direct parse, MCP migration plus parse, and full file load all run unconditionally and assert provider/model/protocol/timeout/default-provider/agent identity.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3b.md`

### Batch B06

#### B06-1 [P1] — Jev 0.93 (fix-now) — fixed

- **Location**: `src/config/types/acp.rs:485`
- **Defect**: `hydrate_from_preset` 通过与 `Self::default()` 比较判断字段是否缺省，无法区分“未填写”和显式填写 `trust_level = "disabled"`。内置预设的信任级别为 `Full`，因此会覆盖用户明确设置的 `Disabled`。同样的缺省判断还会覆盖显式 `args = []`、`default_mode = "oneshot"` 等与裸默认值相等的配置。
- **Failure mode**: 用户在 `[acp.adapters.gemini]` 中设置 `trust_level = "disabled"`，加载后却得到 `Full`；`src/acp/adapters/generic.rs:121` 的权限映射随后选择 `ApproveAll`，而不是用户要求的 `DenyAll`。保存并重新加载配置也会触发这一变化。
- **Change**: ACP adapter deserialization now uses an input structure with per-field presence tracking. Preset hydration fills only fields that were absent, so explicit `trust_level = "disabled"`, `args = []`, `default_mode`, output format, and timeout values remain explicit overrides.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3a.md`

#### B06-2 [P1] — Jev 0.81 (verify-then-fix) — fixed

- **Location**: `src/config/types/tools.rs:498`
- **Defect**: `FsToolConfig`、`GitToolConfig` 和 `ShellToolConfig` 声明了启用开关、路径白名单及命令白名单，但全 crate 检索显示这些限制只进入声明、默认值、旧格式转换或测试，没有生产执行端读取。`is_fs_enabled` 等方法也没有生产调用者；实际文件工具和 Bash 工具在 `src/executor/builtin_registry/builder/constructor/mod.rs:90`、`:130` 构造，在 `src/executor/builtin_registry/builder/core_tools.rs:19` 注册，没有应用这些配置。
- **Failure mode**: 用户设置 `[unified_tools.native.shell] enabled = false`，或设置 `allowed_commands = ["ls"]`，运行时仍注册 Bash 工具，且不会依据该配置拒绝其他命令。文件系统的 `allowed_roots` 同样不会限制实际文件工具；其他独立权限层允许的操作仍可执行。
- **Change**: Native filesystem, git, and shell enable switches and allow-lists now reject configured values that the production native registry does not consume. The deserializer returns clear errors pointing to the unsupported configuration capability; defaults remain available through normal defaulting.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3a.md`

#### B06-3 [P2] — Jev 0.87 (fix-now) — fixed

- **Location**: `src/config/types/acp.rs:54`
- **Defect**: `deserialize_adapters_with_presets` 只按内置预设的 map key 查找需要回填的条目，没有根据其他条目的 `preset` 字段解析预设。`src/acp/manager/harness_admin.rs:55` 随后直接使用 `GenericAcpAdapter::from_entry`，该构造函数也不补齐预设配置。
- **Failure mode**: `[acp.adapters.review] preset = "claude-code"` 可以成功解析，但保留 `executable = None` 和空参数。`src/acp/adapters/generic.rs:45` 将 executable 回退为 `"claude-code"`，而正确预设应调用 `"claude"` 并使用 `--print --output-format json -p`；结果通常是可执行文件找不到，或者调用方式错误。
- **Change**: User entries are now resolved using their own `preset` field before built-in preset-key merging. Unknown preset identifiers return a deserialization error, while the map key remains the entry identity.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3a.md`

#### B06-4 [P2] — Jev 0.63 (defer) — fixed

- **Location**: `src/config/types/tools.rs:32`
- **Defect**: `default_timeout_seconds` 和 `per_tool_seconds` 被描述为驱动工具调用超时，但全 crate 检索没有找到这两个字段的运行时读取点。`src/gateway/execution_engine/tool_service_builder.rs:155` 的生产服务构造链也没有应用这些超时。
- **Failure mode**: 用户设置 `default_timeout_seconds = 1` 或某个工具的秒数覆盖，配置可以加载和保存，但该调用不会因此在指定时间结束。具有自身超时的工具继续使用自身配置；缺少自身超时的工具也得不到这里承诺的统一截止时间。
- **Change**: Non-default `tool_service.default_timeout_seconds` and non-empty `tool_service.per_tool_seconds` now fail at deserialization with messages explaining that ToolService does not consume them.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3a.md`

#### B06-5 [P2] — Jev 0.63 (defer) — fixed

- **Location**: `src/config/types/agents_def.rs:113`
- **Defect**: `bootstrap_max_chars` 和 `bootstrap_total_max_chars` 声称限制单个及全部 bootstrap 文件，但全 crate 检索结果仅有本文件内的声明、示例和解析测试，没有运行时消费者。生产身份文件加载在 `src/orchestrator/harness_bridge/prompt_build.rs:452` 使用默认或按上下文窗口推导的 `IdentityFilesConfig`，没有应用这两个设置。
- **Failure mode**: 用户设置单文件上限为 1,000 字符、总上限为 4,000 字符，实际加载仍使用默认或窗口推导的更大上限；超过用户上限的内容仍进入提示词，带来意料之外的上下文占用和费用。
- **Change**: Configured `bootstrap_max_chars` and `bootstrap_total_max_chars` now fail at the config boundary with an explicit message identifying the identity-file loader as the required source of truth. These fields are omitted from generated schema output.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3a.md`

#### B06-6 [P2] — Jev 0.65 (defer) — fixed

- **Location**: `src/config/types/agents_def.rs:109`
- **Defect**: `AgentDefaults.dm_scope` 被文档描述为全局 Agent 默认值，但没有生产消费者。crate 内其他 `dm_scope` 引用属于独立的会话及路由配置；`src/bin/aleph-server/commands/start/builder/subsystems.rs:681` 明确从 `[session]` 配置建立实际路由。
- **Failure mode**: 用户设置 `[agents.defaults] dm_scope = "main"`，希望私信合并到主会话，加载成功后路由仍使用 `[session]` 的默认隔离策略。修改该字段不会改变会话归属，也没有对应的失效配置提示。
- **Change**: Configured `agents.defaults.dm_scope` now fails with a migration-oriented error directing users to `session.dm_scope`, and the field is omitted from generated schema output.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3a.md`

### Batch B07

#### B07-1 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/config/types/voice_local.rs:318`
- **Defect**: `normalize_voice_local` builds the synthetic speech/transcription provider from the current `[voice.local]` values, but installs it with `or_insert_with`. Once a `provider_type == "local"` entry exists, later changes to `endpoint`, `api_key`, `stt_model`, `tts_model`, `tts_voice`, or `tts_format` are never propagated to the provider maps.
- **Failure mode**: Start with local voice enabled and endpoint A, then edit `voice.local.endpoint` to endpoint B through a reload or config patch. `normalize_voice_local` runs again, but the existing `generation.speech_providers["local"]` and `transcription_providers["local"]` remain pointed at A, so subsequent voice requests continue sending audio to the old endpoint and can keep using the old credential/model without an error.
- **Change**: 受管 local provider 每次 normalization 同步 endpoint、api_key、models、speech voice/format，包括清空配置值。Reconcile only these owned fields; retain enabled, timeout, capabilities and other unrelated provider settings. A different provider_type under the `local` map key remains untouched; explicit default providers remain untouched.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3b.md`

#### B07-2 [P1] — Jev 0.86 (fix-now) — fixed

- **Location**: `src/config/types/routing.rs:115`
- **Defect**: `RoutingRuleConfig::provider` is presented as the provider to use, but `register_custom_commands` copies only the regex and system prompt into the custom tool; `CommandContext::Custom` has no provider field and no later routing pass consumes it. The validation path also does not enforce the provider-reference requirement described by this module's documentation.
- **Failure mode**: Configure `[[rules]]` with `regex = "^/draw"`, `provider = "gemini"`, and a different `general.default_provider`. Invoking `/draw` registers and executes the custom command, but its provider selection falls through to the global/default route, so the explicit per-command provider choice is silently ignored.
- **Change**: The provider field remains deserializable for compatibility and remains available to existing in-memory validation callers, but is skipped during serialization and schema generation because custom command dispatch has no provider-aware context.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3a.md`

#### B07-3 [P2] — Jev 0.59 (defer) — not-handled

- **Location**: `src/config/types/routing.rs:128`
- **Defect**: `strip_prefix`, `intent_type`, `preferred_model`, and `icon` are deserializable public config fields, but crate-wide search found no production consumer for those fields. `register_custom_commands` uses only `regex`, `system_prompt`, and `is_builtin`; the `get_intent_type` and `get_preferred_model` methods are exercised only by in-module tests.
- **Failure mode**: An operator sets `strip_prefix = false`, `preferred_model = "..."`, `intent_type = "research"`, or an `icon` for a slash command. The config loads and round-trips, while command parsing, model selection, routing metadata, and display remain unchanged; the configuration therefore reports success for settings that have no effect.

#### B07-4 [P1] — Jev 0.90 (fix-now) — partial

- **Location**: `src/config/types/voice_local.rs:11`
- **Defect**: `VoiceLocalConfig` and `StreamingConfig` both derive `Debug` while containing runtime bearer tokens in `api_key`. `serde(skip_serializing)` prevents TOML/JSON serialization, but it does not affect the derived debug formatter; `Config` also derives `Debug`, so formatting a loaded config recursively includes these token strings.
- **Failure mode**: A debug log, diagnostic, panic context, or operator-side `format!("{cfg:?}")` includes a loaded `[voice.local].api_key` or `[voice.streaming].api_key`, exposing the bearer token to logs or error-reporting storage despite the documented never-persisted guarantee.
- **Change**: `VoiceLocalConfig` and `StreamingConfig` now use manual Debug implementations with constant `[REDACTED]` api_key fields, retaining useful nonsecret fields. Added ordinary/pretty Config Debug assertions containing both voice token kinds before normalization.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W3b.md`

### Batch B08

#### B08-1 [P1] — Jev 0.78 (verify-then-fix) — fixed

- **Location**: `src/config/types/generation/defaults.rs:172`
- **Defect**: `GenerationDefaults::to_params()` is the only function that would translate the configured defaults into request parameters, and it has no production caller — the sole caller in the tree is the unit test `test_defaults_to_params` at `src/config/types/generation/mod.rs:250`. The request path builds `GenerationParams` from tool arguments only (`src/builtin_tools/generation/image_generate.rs:138`, `src/builtin_tools/generation/speech_generate.rs:205`), so `width`, `height`, `quality`, `style`, `n`, `fps`, `duration_seconds`, `guidance_scale` and `steps` never leave the config file. `GenerationDefaults::validate()` nevertheless range-checks exactly those fields (thresholds 8192/10/120/150), so the config accepts, validates and stores values that are then dropped on the floor.
- **Failure mode**: A user writes `[generation.image_providers.mine.defaults] width = 2048, height = 2048, quality = "hd"`. `GenerationConfig::validate()` passes (the values sit inside the thresholds), the value is persisted, and every later image request is issued as `GenerationParams { width: args.width, height: args.height, quality: args.quality, style: args.style, ..GenerationParams::default() }` — i.e. the provider's own 1024x1024 standard-quality default. No error, no warning: the user's explicit setting is silently discarded.
- **Change**: ** `GenerationDefaults::validate` now rejects configured request parameters that have no production request consumer (`width`, `height`, `aspect_ratio`, `quality`, `style`, `n`, `format`, `duration_seconds`, `fps`, `speed`, `language`, `guidance_scale`, and `steps`). The `voice` default remains accepted because the factory applies it.
- **Notes**: ** The former range-validation helper was removed because those fields are now rejected before range validation is relevant.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B08-2 [P1] — Jev 0.83 (verify-then-fix) — fixed

- **Location**: `src/config/types/generation/presets/mod.rs:129`
- **Defect**: `get_preset_by_type` collects every preset whose `provider_type` matches and returns the lexicographically first one, so a provider *type* that is a protocol rather than a vendor inherits one specific vendor's `base_url` and `default_model`. `openai_compat` has six such presets across five vendors (ideogram, recraft, bytedance-seedream, siliconflow-image, xai-image, bytedance-seedance — `src/config/types/generation/presets/registry.rs:~130`), and the lexicographic minimum is "bytedance-seedance": a ByteDance *video* preset pointing at `https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks`. `get_merged_generation_preset` (line 149) falls back to it for any provider name that is not itself a preset id, and `build_generation_provider_for_persistence` then persists `preset.base_url` and `preset.default_model` whenever the client supplied neither.
- **Failure mode**: In the Panel, "Add Custom Provider" for the Transcription category pre-fills `provider_type = "openai_compat"` and leaves `base_url` and `model` empty. The provider is saved with `base_url = "https://ark.cn-beijing.volces.com/api/v3/contents/generations/tasks"` and `models = ["doubao-seedance-2-0-260128"]`. At boot the user's own API key (stored in the vault under that provider name) is hydrated into the provider, and `src/generation/providers/openai_compat/generate.rs` sends it as `Authorization: Bearer <key>` to `ark.cn-beijing.volces.com` — the user's key is disclosed to a vendor they never chose — while every transcription request fails at that endpoint.
- **Change**: ** `get_preset_by_type` now returns a type-level fallback only when every matching preset shares the same `base_url`. It keeps deterministic name ordering. `get_merged_generation_preset` supplies the built-in `default_model` only for an exact preset-id match; a type-level fallback supplies no vendor-specific model.
- **Notes**: ** Exact preset IDs retain their existing built-in model and endpoint behavior.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B08-3 [P2] — Jev 0.83 (verify-then-fix) — fixed

- **Location**: `src/config/types/generation/presets/registry.rs:485`
- **Defect**: `PROFILES` contains "whisperlivekit" (`provider_type = "deepgram_stream"`, line 486), "whisperlive" (`provider_type = "whisperlive"`, line 496) and "deepgram-stream" (`provider_type = "deepgram_stream"`, line 506), but `create_provider` in `src/generation/providers/factory.rs` has no arm for `whisperlive` or `deepgram_stream` — both fall through to the catch-all `Err(invalid_parameters("Unknown provider type: …"))`. Those names belong to the separate voice-streaming subsystem (`src/gateway/voice/streaming/mod.rs:113-114`), which matches `"whisperlive"` and `"deepgram" | "whisperlivekit"` against a different provider enum.
- **Failure mode**: `generation_providers.list_presets` returns every entry of `PRESETS` (`src/gateway/handlers/generation_providers/handlers.rs:625`), so the Panel renders "WhisperLive" and "Deepgram Stream" as selectable preset cards. The user picks one, the provider is persisted, and at boot `init_generation_registry` calls `create_provider`, receives "Unknown provider type", logs `Skip generation provider` and never registers it. The Panel shows a configured provider that can never serve a request; the only trace is a log line.
- **Change**: ** Removed the `whisperlivekit`, `whisperlive`, and `deepgram-stream` entries from the generation preset catalogue.
- **Notes**: ** The separate voice-streaming subsystem is outside this batch and was not changed.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B08-4 [P2] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/config/types/generation/config.rs:270`
- **Defect**: `validate_provider_reference` resolves the name against the five typed maps and the legacy map and only checks that the entry exists and is `enabled`. It never checks that the resolved provider supports the modality the field selects, even though the field name already carries it (`default_image_provider` -> `GenerationType::Image`).
- **Failure mode**: `[generation] default_image_provider = "elevenlabs"` — a SPEECH-only provider — passes `GenerationConfig::validate()` and the config is accepted at boot. Every later image request is routed to the ElevenLabs TTS provider and fails at request time with "provider does not support image generation", instead of the misconfiguration being reported when the config is loaded.
- **Change**: ** Default-provider validation now carries the modality selected by each default field. Typed maps are matched by their map modality and enabled state; legacy entries must be enabled and explicitly list that modality in `capabilities`. The configuration example now uses the typed `generation.image_providers` map.
- **Notes**: ** The typed-map modality is authoritative because `merged_providers` already derives typed entries' capabilities from the map they came from.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B08-5 [P2] — Jev 0.76 (verify-then-fix) — partial

- **Location**: `src/config/types/generation/config.rs:83`
- **Defect**: `GenerationConfig` still carries the legacy `providers: HashMap<String, GenerationProviderConfig>` next to the five typed maps (`image_providers` … `transcription_providers`, lines 87-105). `merged_providers` (line 204) emits typed entries first and silently skips a legacy entry whose name is already typed, and the legacy loop (lines 222-230) emits one entry per capability under the same name. `find_provider_type` (`src/gateway/handlers/generation_providers/helpers.rs:36-61`) checks the typed maps first and, for a legacy entry, derives the map name from `format!("{:?}", capabilities.first()).to_lowercase()` — e.g. "image" — which `get_typed_provider_map_mut` (same file, line 21) maps to `config.image_providers`, never back to the legacy map.
- **Failure mode**: A user who follows the doc example in this very file (`src/config/types/generation/config.rs:22-38`: `[generation.providers.dalle]` plus `default_image_provider = "dalle"`) edits "dalle" in the Panel. The update is written into `[generation.image_providers.dalle]` while `[generation.providers.dalle]` is left untouched, so config.toml now holds the same provider twice; `merged_providers` prefers the typed copy, so the legacy copy is silently shadowed and any later hand-edit of the legacy block is ignored.
- **Change**: ** Updated the public TOML example to use the typed `generation.image_providers` map.
- **Notes**: ** The two-source-of-truth runtime problem remains. A complete fix requires changes in non-owned `src/gateway/handlers/generation_providers/helpers.rs` (`find_provider_type` / `get_typed_provider_map_mut`) or a load-time migration that removes the legacy map. This batch did not edit those files.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B08-6 [P2] — Jev 0.81 (verify-then-fix) — needs-other-file

- **Location**: `src/config/types/generation/presets/mod.rs:157`
- **Defect**: `get_merged_generation_preset` looks the override up in `generation_overrides.image`, then `.video`, then `.audio` (lines 157-159) — `GenerationPresetsOverride` (`src/config/presets_override.rs`) has no `speech` or `transcription` map, although 14 of the 44 presets are SPEECH (8) or TRANSCRIPTION (6). Neither `GenerationPresetsOverride` nor `PresetsOverride` sets `deny_unknown_fields`, so an unknown table is parsed away by serde without error, and `load_presets_override` only warns when parsing fails.
- **Failure mode**: The operator writes `[presets_override.generation.speech.elevenlabs] base_url = "https://eu.api.elevenlabs.io"` — the obvious table name given the five modalities — and the file loads with no warning at all. `get_merged_generation_preset` never sees the table, so the provider keeps the built-in `https://api.elevenlabs.io` endpoint and the override has no effect whatsoever, silently.
- **Change**: ** No local schema-only change was made.
- **Notes**: ** The owned lookup currently searches only image, video, and audio; changing it without the missing schema fields would not provide a usable configuration path.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B08-7 [P2] — Jev 0.86 (fix-now) — fixed

- **Location**: `src/config/types/stop_hooks.rs:26`
- **Defect**: `timeout_secs` is an unvalidated `Option<u64>` and `StopHookConfig` has no `validate()`; `build_from_config` (`src/verification/stop_hooks.rs:122-124`) applies `Duration::from_secs(secs)` verbatim, while the omitted case correctly falls back to the 30 s default installed by `ShellStopHook::new` (line 69). Compare `GenerationProviderConfig::validate` (`src/config/types/generation/provider.rs`), which explicitly rejects `timeout_seconds == Some(0)`.
- **Failure mode**: `[[stop_hooks]] name = "gate" command = "./gate.sh" timeout_secs = 0` loads without complaint. In `execute_shell_hook` the `sleep(hook.timeout)` arm wins immediately, the child is killed before it can exit, and the verdict is `Error { message: "timed out" }` on every run — never `Allow`, never `Block`. Because the hook can never reach exit code 2, the objective gate's veto path is unreachable: the gate is silently disabled instead of the bad value being rejected at load.
- **Change**: ** Added a serde deserializer for `StopHookConfig.timeout_secs` that rejects `Some(0)` with `timeout_secs must be greater than 0`.
- **Notes**: ** The omitted value remains `None`, preserving the existing 30-second runtime default.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B08-8 [P2] — Jev 0.86 (fix-now) — needs-other-file

- **Location**: `src/config/types/generation/presets/mod.rs:149`
- **Defect**: `get_merged_generation_preset` is the only override-aware accessor for a preset, but `generation_providers.list_presets` (`src/gateway/handlers/generation_providers/handlers.rs:625-648`) reads `PRESETS` raw and returns the built-in `base_url` / `default_model` for every id. The Panel pre-fills its "set up this preset" form from that catalogue (`interfaces/webchat/src/platform/wide/views/settings/generation_providers/preset_setup.rs:21-22`) and submits the resulting non-empty `base_url` on create; `build_generation_provider_for_persistence` prefers the incoming value over the merged preset, so the override is dropped.
- **Failure mode**: The operator overrides the ElevenLabs preset in `~/.aleph/presets.toml` (e.g. to a regional endpoint) and then sets the preset up from the Panel. The form is pre-filled with the built-in `https://api.elevenlabs.io`, that value is submitted as an explicit `base_url`, and the provider is persisted with the built-in endpoint. The override therefore works only for providers created without an explicit `base_url`, and the UI never shows the effective value.
- **Change**: ** No raw-catalogue change was made in the owned files.
- **Notes**: ** The owned merge accessor is already override-aware; the bypass is in the handler/UI persistence path.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

### Batch B09

#### B09-1 [P1] — Jev 0.75 (verify-then-fix) — needs-other-file

- **Location**: `src/config/types/memory/embed.rs:63`
- **Defect**: `EmbeddingProviderConfig::dimensions` is the declared storage and comparison dimension, but the provider consumer truncates any API vector longer than that dimension before returning it. Because `test_connection()` checks the already-truncated result, a provider/model that returns a different dimension can pass the connection check instead of reporting the configuration mismatch.
- **Failure mode**: A custom provider is configured with `dimensions = 1024` while its model returns 1536-component vectors. Every embedding is reduced to the first 1024 components and renormalized, then stored and compared as if it were a valid 1024-dimensional embedding; retrieval silently loses 512 components and can return incorrect rankings. A shorter response fails later through the dimension contract, but the oversized case is silently accepted.
- **Change**: Confirmed runtime still calls `.map(|(_, emb)| truncate_and_normalize(emb, self.dimension))` in `src/memory/embedding_provider.rs:315`. The helper at `src/memory/embedding_provider.rs:119` truncates oversized vectors and renormalizes them. Declared dimensions are consumed as the target, so changing the config declaration cannot validate the original response. | ** No config-only change was made.
- **Notes**:  | ** No speculative projection behavior was introduced.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

#### B09-2 [P2] — Jev 0.76 (verify-then-fix) — partial

- **Location**: `src/config/types/memory/mod.rs:98`
- **Defect**: The `MemoryConfig::retrieval_scoring` comment says the default has recency and reinforcement "both off" and preserves byte-for-byte legacy ranking; `AssemblerConfig` repeats the same inactive-default claim at `src/config/types/memory/assembler.rs:29`. However, `RetrievalScoringConfig::default()` sets both `recency_enabled` and `reinforcement_enabled` to `true`, and the config tests explicitly assert that those refinements are active by default.
- **Failure mode**: An operator leaves retrieval scoring unconfigured while relying on the published config contract to retain legacy ordering. The proactive assembler and on-demand retrieval apply time decay and recall-count reinforcement anyway, changing result ordering and potentially surfacing a different memory than the legacy path; setting fields explicitly to `false` is required even though the documentation says the unconfigured default already does that.
- **Change**: Current MemoryConfig comment already says recency/reinforcement default on and MMR default off. Verified `RetrievalScoringConfig::default()` agrees. However `src/config/types/memory/assembler.rs:31` still states `Default-inactive → byte-for-byte legacy ranking when unconfigured`. | ** Updated the `MemoryConfig::retrieval_scoring` documentation to state that recency and reinforcement are enabled by default, while MMR remains disabled, and that explicit `false` restores the legacy ranking path.
- **Notes**:  | ** The same stale legacy-default statement remains in non-owned `src/config/types/memory/assembler.rs`; it must be updated separately for the documentation contract to be fully consistent.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W4.md`

### Batch B11

#### B11-1 [P1] — Jev 0.84 (verify-then-fix) — fixed

- **Location**: `src/config/types/policies/tool_permissions.rs:197`
- **Defect**: `ToolPermissionsConfig::merge` resolves each override key as though the key itself were a concrete tool name. That does not preserve the intersection of two policies when different glob patterns overlap: a default-derived denial attached to one pattern subsequently competes with a valid allowance from the other pattern. `resolve_turn_permissions` uses this merge for global, agent, and channel policies on production turns (`src/gateway/execution_engine/turn_permissions.rs:431`).
- **Failure mode**: Configure global `default = "deny"` with `"github__*" = "allow"`, and agent `default = "deny"` with `"github__read*" = "allow"`. Both original policies resolve `github__read_file` to Allow. Merge nevertheless evaluates the literal key `github__*` against the agent policy as Deny, producing `github__* = deny` and `github__read* = allow`. Both patterns match the real tool, so the merged resolver chooses Deny and the tool is unavailable despite being explicitly permitted by both policies.
- **Change**: `ToolPermissionsConfig::merge` now preserves glob entries as glob entries instead of resolving glob strings as literal tool names. Each glob is constrained by the other layer's default, and exact override names are emitted with the pointwise restrictive result from both complete policies.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5a.md`

#### B11-2 [P2] — Jev 0.89 (fix-now) — partial

- **Location**: `src/config/types/policies/spend.rs:37`
- **Defect**: All three USD ceiling fields accept unrestricted `f64` values, including TOML `nan` and `inf`; `Config::validate_policies` validates only the execution tier, not these limits (`src/config/validate.rs:511`). The enforcement comparison is `spent_usd >= limit_usd` (`src/spend/mod.rs:917`), so a NaN ceiling can never be reached, while positive infinity also acts as an unlimited ceiling despite the policy being enabled.
- **Failure mode**: Set `[policies.spend] total_usd = nan` with the per-principal ceilings omitted. The policy reports enabled and the ledger is consulted, but every finite recorded total compares false against NaN, so all subsequent calls are allowed regardless of accumulated USD. The same input on `per_user_usd` or `telecom_public_usd` disables that axis; negative ceilings instead reject even zero-spend callers without a configuration error.
- **Change**: Added a serde boundary validator for `per_user_usd`, `total_usd`, and `telecom_public_usd`. Present ceilings must now be finite and non-negative; zero remains valid.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5a.md`

### Batch B12

#### B12-1 [P1] — Jev 0.87 (fix-now) — fixed

- **Location**: `src/context/fabric.rs:664`
- **Defect**: `ContextSnapshot::payload` is an unrestricted `serde_json::Value`, and `validate_admitted` verifies only fingerprints, identity/scope metadata, and claimed budgets. No redaction or privacy policy is applied before the snapshot is admitted to a capability path, so the contract permits credentials, hidden reasoning, unauthorized project data, or another session's private content to be carried onward as model context whenever the upstream builder or caller supplies it.
- **Failure mode**: A snapshot containing an API token or hidden reasoning is serialized with the request and its matching payload fingerprint; admission succeeds because the payload is schema-valid JSON and the fingerprint matches, and a transform or downstream context assembler can return that content in the model-visible result.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5b.md`

#### B12-2 [P1] — Jev 0.91 (fix-now) — fixed

- **Location**: `src/context/fabric.rs:662`
- **Defect**: `payload_fingerprint` hashes only `payload`, while `validate_admitted` enforces `token_count` and `byte_count` from mutable wire metadata. Those size claims are not derived from or authenticated with the payload, so admission trusts attacker- or producer-supplied counters instead of the actual context size.
- **Failure mode**: A request carries a payload larger than the declared input ceiling but sets `token_count` and `byte_count` to small values and computes the matching payload fingerprint. The snapshot passes `validate_admitted`, allowing an oversized context through the budget gate and potentially causing memory or model-window exhaustion.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5b.md`

#### B12-3 [P1] — Jev 0.91 (fix-now) — fixed

- **Location**: `src/context/fabric.rs:1132`
- **Defect**: `ContextTransformResult::validate_for` checks only the caller-provided `output_bytes` and `output_tokens`; it never measures or fingerprints `output` in a way that includes those sizes. A result can therefore carry arbitrarily large output while reporting counters within the admitted output budget.
- **Failure mode**: A capability returns a result whose `output` contains a payload larger than the granted byte/token ceiling but sets `output_bytes` and `output_tokens` below the limits. `validate_for` returns `Ok(())`, so downstream code can accept and expose the oversized result as a valid bounded transformation.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5b.md`

### Batch B13

#### B13-1 [P1] — Jev 0.91 (fix-now) — fixed

- **Location**: `src/context/budget/mod.rs:353`
- **Defect**: `ContextBudget::new` validates only that `token_budget` is nonzero, then accepts any `token_estimate_ratio`. A zero, negative, or non-finite ratio reaches the pressure estimator; the estimator treats a non-positive ratio as zero token cost, so the budget can remain blind to message content.
- **Failure mode**: A caller constructs `ContextBudgetConfig { token_budget: 100_000, token_estimate_ratio: 0.0, ... }` and sends a large file-read result. `estimate_tokens_aware` returns zero for the message, `before_turn` sees no pressure and returns `Continue`, and the prompt can exceed the provider context without the budget circuit firing.
- **Change**: `ContextBudget::new` now asserts that `token_estimate_ratio` is finite and strictly positive, alongside the existing positive token-budget invariant.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5a.md`

#### B13-2 [P2] — Jev 0.71 (verify-then-fix) — partial

- **Location**: `src/context/budget/preflight.rs:283`
- **Defect**: `accept_external_candidate` measures the before/after candidate with `estimate_tokens_smart`, whose prose anchor is the fixed default, while the supplied `ContextPressure` was computed with the configured budget estimator. The minimum-savings decision therefore does not use the same accounting basis as the budget it protects.
- **Failure mode**: With a configured ratio of `1.0`, a candidate that removes 250 characters from a 10,000-token budget frees 250 budget-estimated tokens and should clear the 2% (200-token) gate, but the default 3.5-ratio estimate reports only about 72 freed tokens and rejects the candidate. With a larger configured ratio, the inverse can admit a rewrite that does not actually clear the configured savings floor, causing avoidable prompt-cache invalidation and inconsistent preflight admission.
- **Change**: `accept_external_candidate` now takes the supplied budget's `token_estimate_ratio` and measures both candidate snapshots with `estimate_tokens_aware` using that ratio.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5a.md`

#### B13-3 [P2] — Jev 0.82 (verify-then-fix) — fixed

- **Location**: `src/context/file_ops.rs:80`
- **Defect**: `canonicalize_path_string` trims whitespace and removes only one leading `./`; it does not normalize `.`/`..` components or equivalent separators. The file-operation supersession and file-carry consumers consequently compare textual spellings rather than the logical relative path.
- **Failure mode**: The conversation first reads `src/../src/config.rs` and later successfully writes `src/config.rs`. The two calls produce different keys, so the earlier read result is retained as if it were still current and the cheap pass misses the stale content, consuming budget and potentially leaving the model with conflicting file views. The same occurs for repeated `./` prefixes or equivalent path spellings.
- **Change**: `canonicalize_path_string` now removes duplicate separators and `.` components and lexically resolves `..` components without filesystem I/O. Leading parent components are retained when there is no prior component to remove.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W5a.md`

### Batch B14

#### B14-1 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/context/budget/cheap_passes/file_op_supersede.rs:143`
- **Defect**: `obsolete_call_ids` selects the last successful mutating operation, including `Edit`, and marks every earlier operation on the path obsolete without checking what content the mutation actually replaces. A successful local edit is not a replacement snapshot: the production `FileEditOutput` carries only a snippet around the first replacement (`src/builtin_tools/file_ops/edit.rs:205`), and its call arguments contain only the edited strings.
- **Failure mode**: The model reads a sufficiently large source file, edits one function near the top, and later must update another function near the bottom based on that read. At pressure >= the stage gate, once the read is outside the protected tail, its entire body is replaced by the superseded stub even though the unedited bottom section is still current and appears nowhere in the later edit result or arguments. The model loses its only supplied view of the code it still needs; successive edits also erase earlier result snippets for disjoint sections. This can occur while the cheaper pipeline is avoiding an LLM compaction, so no summary preserves those details.
- **Change**: `obsolete_call_ids` now selects only the last successful full `Write` operation as a replacement snapshot. Successful `Edit` operations remain in the ledger and no longer invalidate earlier reads or results merely because they share a path.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B14-2 [P1] — Jev 0.91 (fix-now) — fixed

- **Location**: `src/context/budget/cheap_passes/file_op_supersede.rs:104`
- **Defect**: `ops_by_path` treats the shared `index_file_ops` path key as file identity, but that key is produced by `canonicalize_path_string`, which calls `raw.trim()` (`src/context/file_ops.rs:86`). Actual file read/write operations pass the untrimmed argument to `Path::new` (`src/builtin_tools/file_ops/read.rs:351`, `src/builtin_tools/file_ops/write.rs:170`), so the transformation merges distinct legal filesystem paths.
- **Failure mode**: On Linux, `/tmp/report` and `/tmp/report ` are different files. A large successful read of `/tmp/report ` followed by a successful write to `/tmp/report` is grouped under the same key; when the read falls outside the fresh tail and pressure gate opens, its body is stubbed as superseded although the file read was never changed. Its unchanged content is lost from context and the stub falsely asserts that a different file's write replaced it.
- **Change**: Supersede grouping preserves significant whitespace from the original `path`/`file_path` tool argument instead of using the whitespace-trimmed shared key. Syntactic path normalization remains available for ordinary paths.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B14-3 [P2] — Jev 0.79 (verify-then-fix) — fixed

- **Location**: `src/context/budget/cheap_passes/image_stripping.rs:86`
- **Defect**: `strip_history` reports `IMAGE_TOKENS_ESTIMATE` (1500) freed per image, but `strip_images_in_place` inserts the nonempty text `[image stripped from history]`. The pressure estimator correctly charges that replacement text, so the stage's savings are overstated rather than matching the before/after estimate as documented. The pipeline consumes the reported savings to decide whether to commit a prefix rewrite, not just for logging.
- **Failure mode**: With a 75,000-token budget and the production 2% minimum-savings gate, stripping one old standalone image reports exactly the 1500-token commit floor. Its replacement placeholder costs 9 estimated tokens at the default prose baseline, so the real saving is only 1491; nevertheless `PreflightPipeline::run` commits the history rewrite (`src/context/budget/preflight.rs:359`). It therefore crosses the cache-rewrite gate despite failing its minimum net-savings requirement. More stripped images accumulate the discrepancy.
- **Change**: Image stripping now estimates each changed message before and after replacing image content and adds the saturating net difference to `total_freed`. The placeholder's token cost is therefore included in the result.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

### Batch B15

#### B15-1 [P2] — Jev 0.58 (defer) — deferred

- **Location**: `src/context/compact/compactor.rs:636 (LLM arm), :673 (truncation arm), :997 (merge arm); consumer: src/context/compact/directive.rs:288`
- **Defect**: every arm computes `tokens_after` from the summary/truncated string alone (`estimate_tokens(&summary)`, `estimate_tokens(&truncated)`, `estimate_tokens(&summary_text)`), but the same arms splice the drained window into `[preserved user turns…, summary, carried artifacts…]` (`splice_preserved`, compactor.rs:1176-1198). The preserved turns are budgeted up to `PRESERVED_USER_TOKEN_BUDGET = 20_000` tokens (`src/context/compact/preserve.rs:23`) and the carriers add more, so the reported "after" size is not the size of the list the provider actually receives.
- **Failure mode**: a 60k-token window in which the user had ~12k tokens of turns produces `tokens_before=60000 tokens_after=480` in the `context_budget` log line ("compaction applied"), while the provider really receives ~12.5k tokens (preserved turns + summary + carriers). An operator reading the budget telemetry concludes the context is now tiny and stops investigating a session that is still at 12.5k tokens. The budget gauge itself is **not** affected — `directive.rs:292` recomputes the true size through `ContextBudget::note_compaction_effect(messages, …)` (`src/context/budget/mod.rs:598`), which measures the spliced list — so only the reported before→after pair is wrong.
- **Change**: No code change. `tokens_after` still measures the summary body in the affected compaction arms.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B15-2 [P2] — Jev 0.85 (fix-now) — fixed

- **Location**: `src/context/compact/compactor.rs:492-503 (the guard), :587-588 (the file's own statement of the real layout), :1176-1198 (`splice_preserved`), :561-570 (the `idx + 1 < window_end` branch that picks the full re-summarization prompt)`
- **Defect**: Step 2 skips a compaction when the history "already" starts with a summary (`first_message_text(&messages[0])` + `is_summary_text` + `cut_end <= 2`). But every drain site in this file puts the user's verbatim turns **above** the summary — the file says so itself at compactor.rs:587-588 ("preservation re-attaches the user's verbatim turns ABOVE it, so it no longer necessarily opens the window") and `splice_preserved` implements exactly that. After any real compaction `messages[0]` is therefore a preserved user turn, not a summary, so the guard can only fire on a summary-SEEDED history (a split child session) and never on a compacted one. Its test (`idempotent_on_already_compacted`, compactor.rs:2223-2246) builds `[summary, assistant, 6 × user]` — a shape production does not produce — so the gap is untested.
- **Failure mode**: the rescue path compacts the same in-flight vector a **second** time in the same turn after the provider overflowed (`src/context/compact/rescue.rs:310` → `compact_for_session(fabric, messages, 0, cx.transient_tail, …)`; `fresh_tail = 0` ⇒ `effective_tail = 6 + transient_tail`). Take a compacted list `[user(preserved), summary, tail×6]` (8 messages): `cut_end = 8 - 6 = 2`, so the guard's `cut_end <= 2` threshold is met — but the probe reads `messages[0]`, the preserved user turn, so the guard misses; the window becomes `[user, summary]`; `prior_summary` is found at index 1, and because `idx + 1 == window_end` (nothing sits after it) the code takes the **full re-summarization** branch (`build_window_summary_prompt` over a transcript that is just the old summary) with `token_budget = 25 %` of that transcript. The running summary that carried the whole pre-compaction history is rewritten at a quarter of its size although there is nothing new to fold — silent loss of the accumulated summary — plus one wasted side-channel summarizer call and a fingerprint-cache/carryover purge (the cached hash was taken over log-rebuilt coordinates, so the next turn re-summarizes again).
- **Change**: The idempotency guard now checks the end of the selected compaction window for a recognized running summary, matching the production layout where preserved user turns precede the summary. The prompt builder also skips re-summarization when the prior summary is the final item in the window and reuses it verbatim.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B15-3 [P3] — Jev 0.81 (verify-then-fix) — fixed

- **Location**: `src/context/compact/compactor.rs:906 (the first `splice_preserved`), :960-965 (the cancel branch), :82-90 (`cancelled_result`), :390-420 (`compact`'s watchdog gate)`
- **Defect**: `reapply_cached` splices the drained window and the cached summary into `messages` at compactor.rs:906 and only then calls the summarizer at :960; the comment directly above the cancel branch states "Same contract as the window path: the extend-merge arm also splices and caches, so a cancelled turn must commit neither." The window path checks the cancel **before** splicing (compactor.rs:620), the merge path checks it **after**. On `SummarizerOutcome::Cancelled` the merge path returns `cancelled_result(tokens_before)` = `Skipped { reason: "cancelled" }` with `tokens_after == tokens_before` — it reports "nothing moved" for a list it has already rewritten — and because the strategy is `Skipped`, `compact()` skips `notify_compaction`, so the provider prompt cache that the splice just invalidated is never accounted for and the monitor's consecutive-miss streak is not reset.
- **Failure mode**: a cancelled extend-merge compaction leaves the history compacted while reporting "cancelled / unchanged" ⇒ the next real provider cache miss is attributed to the run rather than to the compaction ⇒ a false "provider prompt cache is not working" warning for that conversation. **Reachability caveat (why this is P3 and not P1/P2):** no production path reaches the inner cancel branch today — `ContextFabric::run_native` (`src/context/fabric/runtime.rs:1195-1212`) wraps the compaction future in a `biased` select whose cancel arm always wins and drops the future, and `src/context/compact/manual.rs:255` wires no cancel token — so today the future is dropped instead of returning `Cancelled`. The branch is exercised only by unit tests that call `ContextCompactor::compact` directly, i.e. the violated contract is not covered by any production-shaped test.
- **Change**: The extend-merge path saves the exact region replaced by its initial splice. If the summarizer returns `Cancelled`, it restores that region before returning the unchanged/cancelled result.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B15-4 [P3] — Jev 0.35 (defer) — deferred

- **Location**: `src/context/compact/compactor.rs:384 ("5. On success: replace the window with a single summary message.")`
- **Defect**: the documented drain shape contradicts the implementation, which replaces the window with `[preserved user turns…, summary, carried artifacts…]` (`splice_preserved`, compactor.rs:1176-1198). The same drift is what makes the Step-2 guard of B15-2 look correct at a glance: a reader of the doc concludes that a compacted history *opens* with the summary.
- **Failure mode**: no runtime failure on its own; a maintainer reasoning about the window from this doc writes a second guard on the same wrong premise (the first one is B15-2).
- **Change**: No code change to the stale flow documentation.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

### Batch B17

#### B17-1 [P1] — Jev 0.88 (fix-now) — partial

- **Location**: `src/context/compact/session_split.rs:184`
- **Defect**: The split commits `RunFinished { outcome: Completed }` to the parent before emitting the child seed. If the second `emit_batch` fails, the function returns `SplitError::Failed` after the parent has already been durably closed, while the child summary and verbatim tail were never written. The comments describe this as a fallback to compact-to-fit, but the parent no longer has an open run for that fallback.
- **Failure mode**: The parent closer commits, then storage rejects `emit_batch(&child, ...)` (for example, a transient database failure). Routing still points at the parent, the child does not exist, and the parent reduces as clean/completed with no open `RunStarted`; the current tail and continuation are absent from both logs, so the caller cannot resume the run or safely retry the split.
- **Change**: The existing edit improves split failure handling around child creation/seed failure, but the parent close and child seed are still separate event-store operations rather than one atomic boundary or an explicit durable pending-split recovery protocol.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B17-2 [P2] — Jev 0.86 (fix-now) — fixed

- **Location**: `src/context/compact/session_split.rs:373`
- **Defect**: `event_to_message` maps `ToolResult` into the pre-tail summary input but has no `SessionEvent::ToolError` arm, so every tool failure in the summarized prefix is dropped by the catch-all branch. A split therefore preserves successful tool output but loses the fact and contents of failed tool calls from the child’s reconstructed context.
- **Failure mode**: A tool call fails before `tail_start`, producing `SessionEvent::ToolError { call_id, error, .. }`, and the split summarizes that prefix. `summarize_pretail` never passes the error to the summarizer, and the child receives only the generic summary plus the fresh tail; the resumed model can believe the failed operation was never attempted and choose an incorrect next action.
- **Change**: Session split's event-to-message mapping now includes `SessionEvent::ToolError` alongside successful tool results, preserving the call id and error text for the pre-tail summarizer input.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

### Batch B18

#### B18-1 [P1] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/context/compact/plan_carry.rs:106`
- **Defect**: `latest_plan` uses `find_map`, so a newer scratchpad result without a snapshot does not invalidate an older snapshot or prior carry. The successful `clear` action actually returns `snapshot: None` and `message: "Scratchpad cleared"` (`src/builtin_tools/scratchpad.rs:784`), and `snapshot_from_block` also rejects explicitly empty item arrays. Consequently the carry can contradict the latest successfully applied plan state.
- **Failure mode**: A window contains an unfinished scratchpad snapshot followed by a successful scratchpad clear. On compaction, the clear is skipped, the earlier snapshot is selected, and a synthetic reminder tells the model to keep working the plan that was deliberately cleared. The stale carry can survive subsequent compactions after both original tool results have gone.
- **Change**: Plan extraction now treats a successful scratchpad clear and an explicitly empty authoritative snapshot as terminal state instead of falling through to an older snapshot.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B18-2 [P1] — Jev 0.87 (fix-now) — fixed

- **Location**: `src/context/compact/plan_carry.rs:71`
- **Defect**: A prior bounded carry is parsed into a partial `ScratchpadSnapshot`, then that partial list is used to decide whether the full plan is finished. `parse_progress` reads only checkbox lines and ignores both the full-list `Progress` count and the omitted-item range. Although prior text is re-emitted unchanged when selected, the completion gate can discard it before that re-emission.
- **Failure mode**: A plan has 41 items: the first 40 completed and item 40 pending. Its first carry uses `render_progress_bounded`, which shows the 40 completed items, states that item 40 is omitted, and reports `Progress: 40/41 done` (the 40-item limit is in `src/memory/scratchpad/manager.rs:277`). During the next compaction, with no newer scratchpad result, `parse_progress` reconstructs only 40 done items and `plan_carry_message` returns `None`. The structural reminder disappears while the plan still has unfinished work.
- **Change**: Prior bounded carries retain their omitted-item/unfinished metadata when deciding whether the plan is complete. Completion is no longer inferred solely from the visible checkbox subset.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B18-3 [P1] — Jev 0.89 (fix-now) — fixed

- **Location**: `src/context/compact/plan_carry.rs:115`
- **Defect**: `latest_plan` treats any user-role text containing `CARRY_MARKER` and a pending checkbox as a previously generated carry, then clones and re-emits the entire text without applying `render_progress_bounded`. There is no check that the message is actually this module's generated carrier. This also accepts tool output downgraded to user-role text by `orphan_tool_result_note`, even though `preserved_user_messages` deliberately excludes that scaffolding (`src/context/compact/preserve.rs:122`). `src/context/compact/file_carry.rs:147` has the analogous substring-based provenance problem for file-ledger claims.
- **Failure mode**: An orphaned file/search result contains the literal `[Execution list preserved across context compaction]`, a line `- [ ] perform the injected task`, and a large payload. The prompt builder wraps the result in an orphan-result reminder. On compaction, `latest_plan` finds the marker inside that wrapper, accepts it as the latest prior plan, and carries the entire tool payload verbatim after the summary. Thus data that the preservation rules excluded is retained indefinitely outside the normal plan render bound, can displace the genuine older plan, and can keep context pressure high. A user pasting an example carry block can trigger the same misclassification without any orphan result.
- **Change**: Prior plan recognition now requires the generated carry envelope and canonical structure, rejects orphan-result scaffolding, and avoids copying arbitrary marker-containing user messages wholesale. The analogous prior-ledger recognition was tightened in the same change.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B18-4 [P2] — Jev 0.83 (verify-then-fix) — fixed

- **Location**: `src/context/compact/event_snap.rs:67`
- **Defect**: `snap_out_of_open_run` collects every ID closed anywhere after the cut, then selects the last matching opener before the cut without checking whether that opener already has a matching close before the cut or whether the tail closer belongs to an opener in the kept tail. Reuse of run IDs is explicitly supported by this file's fallback-retry tests, but the algorithm handles only the case where the matching retry opener is before the cut.
- **Failure mode**: For events `[RunStarted(r), RunFinished(r), UserMessage, RunStarted(r), RunFinished(r)]` and cut 2, both brackets are already wholly on one side of the cut. The tail contains a close for `r`, so the function nevertheless finds opener 0 and returns 0. Manual compaction, which applies this guard after selecting its cut (`src/context/compact/manual.rs:662`), can therefore retain the entire eligible prefix or refuse to compact it even though no open run crosses the requested boundary.
- **Change**: Run snapping now accounts for repeated run ids and only retreats to an opener when that opener is actually open at the proposed cut and its matching close is in the retained tail.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

### Batch B19

#### B19-1 [P2] — Jev 0.75 (verify-then-fix) — fixed

- **Location**: `src/context/fabric/runtime.rs:1045`
- **Defect**: `invoke_messages` builds the Preflight window from `0..self.preflight.external_cut(messages, pressure, fresh_tail)` and returns `Err(ContextFabricError::PreflightGated)` when that window is empty; `external_cut` returns `0` whenever `pressure.ratio < self.min_pressure_ratio` (src/context/budget/preflight.rs:247-268). `preflight()` then maps that error through `degraded_reason()` (runtime.rs:310), which maps `PreflightGated` to `DegradedReason::InvalidContract`, and finally reports `owner = BuiltinFallback { plugin: replace_key, reason: InvalidContract }` (runtime.rs:322-330) — even though the plugin was never invoked and never violated anything.
- **Failure mode**: production builds the pipeline with `.with_min_pressure_ratio(cfg.preventive_floor())` (src/context/budget/preflight.rs:431) and `preventive_floor()` defaults to `0.60` (src/context/budget/mod.rs:255), so every turn below 60% pressure yields `external_cut == 0`. With a Replace-mode Preflight plugin registered, `ContextFabric::preflight` returns `PreflightOutcome { owner: BuiltinFallback { plugin: "<key>", reason: InvalidContract }, degraded: Some(InvalidContract) }` on every such turn. `DegradedReason` is documented at src/context/fabric.rs:1109-1110 as "Always surfaced to the agent and to diagnostics — a fallback that hides itself is a lie about what happened", so a plugin host that surfaces it blames a healthy plugin for a contract violation, and the recorded fallback counts a plugin run that never happened.
- **Change**: `ContextFabric::preflight` now checks the pipeline's `external_cut` before attempting external owners. When the gate is closed, it runs the native preflight directly and returns builtin ownership with no degradation, while preserving the normal lifecycle notification.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B19-2 [P2] — Jev 0.62 (defer) — deferred

- **Location**: `src/context/fabric/runtime.rs:822`
- **Defect**: the compose/interceptor loop in `compact()` sets `owner = ContextOwner::Plugin(key.clone())` for any plugin that returns a non-empty result (runtime.rs:787-830), and the final derivation then rewrites the strategy: `if let ContextOwner::Plugin(capability) = &owner { CompactStrategy::Plugin { capability: capability.clone() } }` (runtime.rs:822-829), discarding the `strategy = Some(native_result.strategy_used)` recorded by the native compactor (runtime.rs:757-766) or the Replace plugin's win (runtime.rs:740-746). A compose plugin post-processes the compacted vector; it is not the thing that condensed the window.
- **Failure mode**: the aux summarizer model starts failing, so the native compactor falls back to `CompactStrategy::DeterministicTruncation` (src/context/compact/compactor.rs:701). With any Compose-mode compaction plugin installed and returning successfully, `CompactResult.strategy_used` becomes `Plugin { capability: "<compose-key>" }` instead of `DeterministicTruncation`. `src/context/compact/directive.rs:257-269` fires its operator warning only on `matches!(result.strategy_used, CompactStrategy::DeterministicTruncation)` ("compaction degraded to deterministic truncation — the side-channel summarizer is not producing summaries; check [context_budget] / the aux model on this provider"), so the warning never fires; the `context_budget` "compaction applied" log at directive.rs:281-287 also stops naming the strategy that actually condensed the window. Every compaction then silently degrades to first-line truncation with no operator signal.
- **Change**: No code change to compose-mode owner/strategy derivation.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B19-3 [P2] — Jev 0.65 (defer) — deferred

- **Location**: `src/context/fabric/runtime.rs:715`
- **Defect**: the replace/bundle loop (runtime.rs:715) and the compose/interceptor loop (runtime.rs:787) invoke plugins sequentially, and every invocation gets its own `tokio::time::timeout(self.timeout, future)` inside `run_with_cancel` (runtime.rs:1206-1226, default `timeout` 15s from `ContextFabricBuilder::new`). There is no deadline over the loop as a whole, unlike `notify_lifecycle_selected`, which bounds the entire observer batch with one `tokio::time::timeout(self.timeout, deliveries)` and the comment "One deadline bounds the whole observer batch, including a slow guest" (runtime.rs:253).
- **Failure mode**: a guest compaction plugin whose handler never returns (its provider is down, or it awaits a bridge that is wedged) costs 15s per invocation. With one hanging Replace plugin and one hanging Compose plugin, a single compaction stalls the turn for 30s — the compaction sits on the critical path of `compact_for_session` (src/context/compact/directive.rs:120-140) — and the stall repeats on every turn that compacts, because the per-invocation timeout is restarted for the next plugin rather than cutting the batch short.
- **Change**: No code change to plugin loop deadlines.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B19-4 [P3] — Jev 0.79 (verify-then-fix) — fixed

- **Location**: `src/context/fabric/runtime.rs:628`
- **Defect**: the plugin branch of `admit_tool_output_with_context` computes `let original_text = serde_json::to_string(&original)?;` and then `let reduced_from = if output_changed { Some(original_text.as_str()) } else { outcome.reduced_from.as_deref() };`. The native branch's `reduced_from` comes from `clean_for_ingress_of` → `flatten(value)` (src/tool_output/ingress.rs:102,168-173), which yields the plain string for `Value::String` and compact JSON otherwise; the plugin branch always uses JSON encoding.
- **Failure mode**: a tool result whose value is a JSON string (e.g. `"line1\nline2"`) is rewritten by a context plugin. The plugin branch persists the offload blob as `"\"line1\\nline2\""` — quoted and newline-escaped — while the native path would have persisted `line1\nline2`, and the caller in src/tools/scoped/dispatch.rs:2068-2077 independently reports `reduced_from: Some(original)` with the plain text. The persisted "full original" and the reported original are then two different strings for the same fact; when the model later asks for the recovered output it sees the escaped form.
- **Change**: The plugin admission path now derives `reduced_from` with `value_as_model_text`, matching native ingress flattening semantics, instead of serializing the original JSON value with `serde_json::to_string`.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

#### B19-5 [P3] — Jev 0.33 (defer) — deferred

- **Location**: `src/context/compact/mod.rs:21`
- **Defect**: `compaction_window` is declared `pub(crate)` while the module's own doc comment says it is "Private to the compaction module because each helper is an internal step of one of the drain sites the compactor orchestrates", and every function inside it is `pub(super)`. The sibling helper modules (`cheap_poison`, `compaction_cache`, `event_snap`, `file_carry`, `image_carry`, `plan_carry`, `preserve`) are plain private `mod`.
- **Failure mode**: the declared visibility says "the whole crate may depend on this module", the doc and the `pub(super)` items say "only the compaction module may". Any crate-level caller can now reach a module whose helpers were never meant to be a crate-wide API, and the drift hides the fact that the only real consumer is `src/context/compact/compactor.rs:13`.
- **Change**: No code change to `src/context/compact/mod.rs` visibility.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W6.md`

### Batch B20

#### B20-1 [P1] — Jev 0.89 (fix-now) — partial

- **Location**: `src/context/fabric/tool_admission.rs:32`
- **Defect**: `with_tool_admission` converts `AuthenticatedCaller::from_host_boundary` errors into `None` with `.ok()`. The later admission chokepoint treats a missing task-local frame, or a missing admission capability at lines 91–95, as a normal “no scoped admission” result rather than a denial.
- **Failure mode**: A host boundary supplies an empty or untrimmed session id, or the context frame is otherwise absent, while a tool result has a budget and should pass through the configured admission/allow-list. `admit_scoped_tool_output` returns `Ok(None)`, and `src/tools/scoped/dispatch.rs` falls back to the ordinary native ingress path, so the tool output is admitted without the scoped policy check.
- **Change**: The task-local admission state now retains an invalid host-boundary identity as an error instead of converting it to `None`. Scoped admission reports that invalid state as an error, and a configured fabric with no admission capability is also denied explicitly rather than returning the normal unconfigured result.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

#### B20-2 [P1] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/context/retrieval/content_index.rs:300`
- **Defect**: `ContentIndex::search_sessions` and `search_source` are public and use every caller-supplied `session_id` directly in the SQL isolation predicate. The documentation says callers own the trust boundary, but the type system and implementation do not constrain the set to epochs of one authenticated session.
- **Failure mode**: Any caller that has access to a `ContentIndex` and combines an attacker-controlled or unrelated session id with its own id can query a secret term and receive the other session’s complete chunk body through `SearchHit::body`; the SQL predicate then correctly returns the cross-session row because the API authorized the wrong set.
- **Change**: The raw multi-session `search_sessions`, `search_source`, and `len_sessions` methods are now `pub(crate)`, preventing external callers from composing arbitrary session-id sets. The exact single-session public methods remain available, and SQL session predicates remain in place as the second isolation layer.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

#### B20-3 [P1] — Jev 0.73 (verify-then-fix) — fixed

- **Location**: `src/context/retrieval/content_index.rs:185`
- **Defect**: `index_text` materializes all input lines into an owned `Vec`, creates a chunk for every line range, and inserts every chunk into both `chunks` and `chunks_tri` without a byte, section, per-session, or total-index quota. Replacing one source only limits growth for that exact `(session_id, source)` pair.
- **Failure mode**: A tool or repeated stream emits large outputs under unique call/source labels. One invocation allocates the complete input plus all chunk strings, then persists roughly two indexed copies; repeated unique sources accumulate indefinitely until memory, SQLite disk space, or FTS write time is exhausted, taking the tool-result path down with the index.
- **Change**: Indexing rejects input over 1 MiB, more than 4,096 logical chunks, or a session whose resulting two-table indexed body bytes would exceed 16 MiB. The quota calculation subtracts the replaced source before adding new bytes and counts both FTS tables; each rejection returns a typed degraded `IndexError`.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

### Batch B21

#### B21-1 [P1] — Jev 0.93 (fix-now) — fixed

- **Location**: `src/diagnostics/redact.rs:111`
- **Defect**: Neither redaction pass recognizes URL userinfo. Central application through `Finding::redacted` therefore does not protect credentials carried by a provider or proxy URL.
- **Failure mode**: A finding containing `connection failed: https://alice:hunter2@api.example.com/v1` retains the complete username and password in CLI output, JSON support bundles and LLM tool output; none of the current patterns matches this string.
- **Change**: Vendor-pass redaction now recognizes HTTP and FTP URL userinfo and preserves the scheme while replacing the complete userinfo with `***@`. Percent-encoded userinfo is covered by the non-host character class.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

#### B21-2 [P1] — Jev 0.86 (fix-now) — fixed

- **Location**: `src/diagnostics/redact.rs:67`
- **Defect**: Header credential masking matches the words `bearer` and `basic`, rather than the value of an Authorization header. Other authentication schemes with opaque credentials remain visible unless their value independently matches a vendor pattern.
- **Failure mode**: `Authorization: Token abc123opaque` is unchanged. Likewise, `Authorization: Digest username="alice", nonce="servernonce", response="clientresponse"` remains visible. A provider error echoing such a header becomes a credential-bearing support report.
- **Change**: Authorization and Proxy-Authorization headers now match their header value independently of the authentication scheme, including opaque one-token schemes and structured Digest values. The header name and separator are retained while the value is replaced.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

#### B21-3 [P1] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/diagnostics/redact.rs:81`
- **Defect**: Generic secret fields require an unquoted name followed by `=`, and their value match stops at whitespace or an apostrophe without understanding quoted strings. Common JSON error/config snippets are not masked, and quoted assignment values can be only partly masked.
- **Failure mode**: `{"password":"hunter2","client_secret":"opaqueCredential","api_key":"opaqueKey"}` is unchanged. `password="correct horse battery staple"` becomes `*** horse battery staple"`, leaking the remainder; `password='hunter2'` is entirely unchanged because the first value character is excluded. These are ordinary config/error representations, not unknown vendor token formats.
- **Change**: Generic masking now accepts quoted JSON field names, colon separators, `api_key`, and complete single- or double-quoted values with escaped characters. Unquoted assignment values remain supported.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

#### B21-4 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/diagnostics/redact.rs:77`
- **Defect**: The special PEM assignment branch consumes the BEGIN header but then uses a non-whitespace value class. A real PEM block contains newlines, so its base64 body and END marker survive. Bare PEM blocks also have no masking branch.
- **Failure mode**: A finding with `private_key=-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKc...\n-----END PRIVATE KEY-----` (actual newlines) becomes `***\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKc...\n-----END PRIVATE KEY-----`. The private-key body remains in the report; a PEM header is public and can be restored by the reader.
- **Change**: PEM private-key matching now spans newlines from supported BEGIN markers through matching END markers, both bare and after a secret-field assignment. The assignment form precedes the generic field branch so the generic matcher cannot strand the PEM body.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

#### B21-5 [P1] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/diagnostics/redact.rs:101`
- **Defect**: The query value class excludes `*` and `=` and requires at least one accepted character. Thus an opaque token starting with either character is not masked at all; an internal occurrence leaves the remaining suffix exposed. Excluding every asterisk to preserve an already-redacted `***` value treats legitimate secret content as a redaction marker.
- **Failure mode**: `https://api.example.com/?access_token=*opaqueCredential&refresh_token==anotherCredential` is unchanged: the unanchored `token=` branch reaches both parameter names but cannot consume their first value character. `?refresh_token=abc*remainingSecret` becomes `?refresh_****remainingSecret`, exposing a suffix. Importantly, ordinary `?access_token=abc&refresh_token=def` IS masked by the existing substring match; the supplied suspicion that these parameter names always leak does not hold for this version.
- **Change**: Query values are now matched through actual `&`, `#`, whitespace, or `?` delimiters, allowing leading or internal `*` and `=` characters. A complete existing `***` value is preserved by the replacement closure rather than by excluding those characters from the value grammar.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

#### B21-6 [P1] — Jev 0.87 (fix-now) — needs-other-file

- **Location**: `src/diagnostics/check.rs:64`
- **Defect**: The `HealthCheck::run` execution boundary has deadline handling but no panic isolation. Supporting call-path inspection confirms `src/diagnostics/mod.rs:391` awaits `timeout(deadline, check.run(posture))` directly and `src/diagnostics/mod.rs:326` combines these futures with `join_all`; neither catches unwinding. `settle_probe` only handles panics of separately spawned blocking tasks, not a panic in the check future itself.
- **Failure mode**: If one registered check panics while constructing or polling its `run` future, unwinding escapes `run_with_filter`; the other checks' collected observations never become a report. In a spawned caller this becomes a failed task, and in a direct caller it unwinds that caller. A timeout does not convert this failure into the expected unknown/unhealthy finding.
- **Change**: The requested execution boundary is not in the owned file: `HealthCheck` is declared in `check.rs`, but the actual `timeout(..., check.run(posture))` and `join_all` boundary is in `src/diagnostics/mod.rs`.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W7.md`

### Batch B22

#### B22-1 [P1] — Jev 0.92 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/discord_channel_health.rs:258`
- **Defect**: The probe deserializes the entire `config.toml` with `serde_json::from_slice`, even though the production configuration loader parses TOML. Even if the input were JSON, the probe then requires `channels` to be an array of `{channel_type, config}` entries, while the config model stores channel instances under a keyed TOML table/map; the real Discord section therefore is never inspected.
- **Failure mode**: A valid production TOML containing a configured Discord channel returns `DiscordVerdict::Unknown` at the JSON parse, so the doctor cannot report a real misconfiguration; a JSON-shaped configuration with a keyed/object `channels` table falls through lines 262-269 and returns `Ok`, falsely declaring the configured channel healthy.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8b1.md`

#### B22-2 [P1] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/session_log.rs:214`
- **Defect**: Errors from `projection_store().list_sessions(...).await` are discarded by `if let Ok(metas)`, with no error marker added to `list_unreadable`. This defeats the documented union of event-store markers and projection sessions, including sessions with activity but no run markers.
- **Failure mode**: If `load_run_markers` returns an empty successful list while the projection store is locked, unavailable, or otherwise returns an error, `keys` remains empty and `list_unreadable` is `None`; the check returns `Session logs consistent` / `no session has an event log yet` without reading any session logs, hiding contradictions and unmarked activity.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8b2.md`

#### B22-3 [P1] — Jev 0.87 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/session_log.rs:251`
- **Defect**: For every candidate session, `load_rows` is called and its complete `Vec<DecodedRow>` is retained before `walk_rows` processes it. The check has no row count, payload-size, or total-byte bound, and it repeats this for every session in one run.
- **Failure mode**: A large or attacker-influenced event log causes the diagnostic task to allocate the entire session history, potentially exhausting memory or taking the doctor run down before it can emit a finding. The later `Vec<SessionEventRecord>` in `walk_rows` can add another full-size allocation for decoded rows.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8b2.md`

#### B22-4 [P1] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/fold_economics.rs:316`
- **Defect**: SQLite `INTEGER` token fields are cast directly from `i64` to `u64`, so negative values become enormous, and `input + output` / `input + read + creation` use unchecked `u64` addition. The subsequent aggregate sums in `rollup` are also unchecked.
- **Failure mode**: A malformed or externally edited `provider_usage` row with a negative count makes the check report absurd economics; sufficiently large values overflow the additions (panic in checked/debug execution or wrapped values in release), causing the diagnostic task to fail or emit a bogus payback verdict instead of identifying invalid telemetry.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8b2.md`

#### B22-5 [P1] — Jev 0.81 (verify-then-fix) — fixed

- **Location**: `src/diagnostics/checks/fold_economics.rs:280`
- **Defect**: Any `SessionEvent` payload that fails deserialization is discarded with `if let Ok(event)`, and the rollup continues as if the row did not exist. This makes a partial probe indistinguishable from an empty ledger.
- **Failure mode**: If all recent `fold_recorded` or `compaction_performed` rows were written by a newer schema, are malformed, or contain an unsupported variant, `by_session` remains empty and `report` returns `No folds to judge in the last 24h` with an informational success. The doctor then silently omits the folds it was supposed to evaluate.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8b2.md`

#### B22-6 [P2] — Jev 0.84 (verify-then-fix) — fixed

- **Location**: `src/diagnostics/checks/engine_missing.rs:359`
- **Defect**: The pinned obscura path is classified as available solely through `Path::is_file()`. That accepts any regular file, without checking that it is executable or that the runtime can answer its version probe.
- **Failure mode**: A user pins a text file, a file with execute permission removed, or another unreadable regular file in `binary_path`; this check emits `Obscura engine available`, while the browser launch later fails when trying to execute it.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8b1.md`

### Batch B23

#### B23-1 [P1] — Jev 0.87 (fix-now) — needs-other-file

- **Location**: `src/diagnostics/checks/hooks_consent.rs:38`
- **Defect**: The check's only input is `ShellHookConsent::entries()`, which returns an empty `Vec` when the registry file is missing, unreadable or corrupt — `ShellHookConsent::read_file` swallows the `fs::read` error with no log at all (`Err(_) => return (BTreeMap::new(), stamp)`) and turns a serde parse error into an empty map after a `warn!`. The check therefore cannot distinguish "there are no shell hooks" from "the hook-consent registry could not be read".
- **Failure mode**: `~/.aleph/shell-hooks-allowlist.json` holds pending hook approvals, but the file cannot be read (EACCES after a permission change, or the doctor runs as a different user than the daemon) or is truncated by an interrupted write → `entries()` is empty → the check returns `Finding::ok(ID, "No shell hooks", "No shell-command hooks recorded; nothing to approve.")` and `report.ok()` stays true → the operator is told there is nothing to approve while unapproved (and therefore non-running) hooks are on disk. The corrupt-file case behaves the same way: consent.rs logs a warning, but the finding still says OK.
- **Change**: No code change was made in `src/diagnostics/checks/hooks_consent.rs`. The check only receives `ShellHookConsent::entries()`, which collapses a missing, unreadable, or corrupt registry into the same empty vector; the consent object's path and load state are private.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W9.md`

#### B23-2 [P1] — Jev 0.92 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/providers_connectivity.rs:174`
- **Defect**: When the vault read succeeds the check assigns `runtime.api_key = k.clone()` unconditionally — including `KeyLookup::Ready(None)` (no secret in the vault), which erases an api_key the provider config already carried. Production resolves credentials the other way round: `hydrate` in `src/bin/aleph-server/commands/start/builder/agent_init/provider_registry.rs:39-48` keeps the config's key and only falls back to the vault, and `src/gateway/health_prober.rs:178-182` does the same (`if runtime.api_key ... is_empty { runtime.api_key = vault }`).
- **Failure mode**: `config.toml` carries `[providers.openai] api_key = "sk-..."` (`ProviderConfig::api_key` is deserializable — `#[serde(default, skip_serializing)]` in `src/config/types/provider.rs:333`, and the repo's own `src/config/tests/basic.rs:59-66` parses exactly that) and the vault has no `ai:openai` secret → the daemon registers and uses the provider with the config key, while the doctor overwrites it with `None` → the probe is sent unauthenticated → HTTP 401 → "[warn] openai: unreachable", and for a single configured provider also "[error] No providers reachable — Every enabled LLM provider failed its connectivity probe — the agent has no working model access." The doctor reports a total outage for a provider that is working, and its fix hint points at the vault where nothing is wrong.
- **Change**: Vault credentials now fill `ProviderConfig::api_key` only when the configured key is absent or empty. A configured key is preserved when the vault returns no secret. Providers with no key in either source receive an explicit `no credentials configured` finding and are not probed.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W9.md`

#### B23-3 [P2] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/providers_connectivity.rs:249`
- **Defect**: All non-success outcomes become `Finding::problem(ID, Severity::Warning, format!("{name}: unreachable"), outcome.error...)` and carry `TAG_UNREACHABLE`, whatever the provider actually answered. An HTTP 401/403 (the provider error text is `OpenAI Chat API error (401 Unauthorized): <body>`, `src/providers/protocols/openai_chat/adapter.rs:336-338`) is a credential fact, not a reachability fact, and the aggregate gate `total_outage` (`:317`) turns "no provider produced a reachable tag" into an Error-level "the agent has no working model access".
- **Failure mode**: the vault holds an expired or revoked key for the only configured provider → the provider answers 401 → the doctor prints "[warn] openai: unreachable" plus "[error] No providers reachable — Every enabled LLM provider failed its connectivity probe" and the fix hint routes the operator to the vault store / `ai:{name}` / self-config as if the endpoint were down → the operator debugs DNS, proxy and firewall instead of the key, and the Error-level aggregate claims an outage where only the credential is wrong.
- **Change**: Probe errors containing HTTP 401/403 or common authentication rejection text are reported as `{name}: credentials rejected` with a credential-specific tag and hint. The total-outage gate ignores credential-rejection findings, while transport failures retain the existing `unreachable` title and taxonomy.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W9.md`

#### B23-4 [P2] — Jev 0.88 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/media_codecs.rs:173`
- **Defect**: `element_exists` spawns `gst-inspect-1.0 --exists <name>` with `tokio::process::Command` but never sets `.kill_on_drop(true)`; tokio's default is to leave the child running when the future is dropped. The comment above the function claims "only a genuinely cancellable child honours" the engine's timeout — the timeout stops the *wait*, not the *process*.
- **Failure mode**: `gst-inspect-1.0` hangs (a cold registry rebuild on a slow disk, a plugin on an unresponsive mount, a held GStreamer registry lock) → the engine's 20 s check deadline (`DEFAULT_CHECK_TIMEOUT`, `src/diagnostics/check.rs:27,71`) fires → the check reports the codecs as `Unknown` (Info, so `report.ok()` stays true) and the gst-inspect process is orphaned and keeps running; each later doctor run leaks another child while the earlier ones hold the registry lock, so subsequent gst-inspect calls block on that lock — the media capability degrades and every later probe keeps timing out, all reported as "unknown".
- **Change**: `gst-inspect-1.0` is spawned with `kill_on_drop(true)`.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W9.md`

#### B23-5 [P2] — Jev 0.86 (fix-now) — partial

- **Location**: `src/diagnostics/checks/stale_lock.rs:100`
- **Defect**: The healthy verdict comes from the holder record's PID alone — the lock itself is never probed on that path — and `process_matches` (`src/utils/process_alive.rs`) returns `true` for bare liveness when the record carries no start-time line (`expected_start == None`). `write_holder` still emits that legacy single-line form (`format!("{pid}\n")`) when `process_start_time(pid)` is unavailable, and `parse_holder` treats the second line as optional, so old records parse into that unsafe shape.
- **Failure mode**: the daemon crashes leaving `aleph.lock.pid` containing only `12345\n`; Linux later reuses PID 12345 for an unrelated process → `process_alive` is true → the doctor prints "[ok] Server running — aleph.lock held by live PID 12345" although the singleton lock is free and no server is running; the stale record is never repaired, because the repair path is only reached when `process_alive` is false. The doctor's own dead-PID branch (:107-114) already knows that "the record's PID" and "the lock's state" are two different facts.
- **Change**: The live-PID path now probes `aleph.lock` before reporting `Server running`. A held lock permits the existing OK finding; an unreadable lock probe becomes unknown; a live PID beside a free lock becomes a stale-record warning.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W9.md`

#### B23-6 [P2] — Jev 0.77 (verify-then-fix) — fixed

- **Location**: `src/diagnostics/checks/cache_hit_rate.rs:87 (the fixture at :210; the same copy in src/diagnostics/checks/cache_health.rs:238)`
- **Defect**: The SQL filter and sums read `$.cache_read_tokens`, `$.cache_creation_tokens` and `$.input_tokens` out of `event_json`, and the unit test writes its own hand-rolled JSON with the same key names. The producer writes `aleph_protocol::AgentTraceEvent` (`shared/protocol/src/events.rs:447`, `#[serde(tag = "kind", rename_all = "snake_case")]`, variants at :534 and :608), so the field names are a second, hand-maintained copy of a wire shape owned by another crate — and the test re-spells that copy instead of using the type, so nothing in the build ties the two together.
- **Failure mode**: `cache_read_tokens` is renamed in the protocol enum → `json_extract` returns NULL for every row → the WHERE clause (`cache_read_tokens > 0 OR cache_creation_tokens > 0`) matches nothing → the check reports "[ok] Not enough cache-active calls — 0 cache-reporting call(s) in the last 24h (< 10)" forever, i.e. the cache monitoring silently dies while the unit test stays green (its fixture carries its own copy of the old names). The same blind spot applies to `cache_health.rs`, whose fixture hand-writes `scope`/`streak`/`reads`/`writes` while the check reads them with `unwrap_or` defaults (`:91-94`) — after a rename the alarms are still counted but every row degrades to scope "unknown", streak 0.
- **Change**: The provider-usage test fixture now serializes `aleph_protocol::AgentTraceEvent::ProviderUsage` instead of hand-written JSON.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W9.md`

### Batch B24

#### B24-1 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/config_parse.rs:113`
- **Defect**: `ConfigParseCheck::run` 把加载错误的完整 Display 文本写入 finding；底层迁移解析器将 TOML 错误连同原始出错行格式化进该文本。虽然引擎最终调用 `Finding::redacted`，其正则仅识别部分凭据前缀及紧贴等号的 `key=`/`apikey=`/`token=`，不能覆盖普通 TOML 的 `api_key = "..."` 中没有已知前缀的密钥。
- **Failure mode**: 用户在 provider 配置中写入 `api_key = "opaqueCredential123456789" trailing`，尾部意外文字导致解析失败；TOML 错误包含整行，因此这个原本真实的 opaque API 密钥随 doctor 的终端、JSON 或工具结果输出。空格使 `key=` 分支不匹配，凭据本身也不匹配 vendor 前缀，最终脱敏仍保留密钥。
- **Change**: Replaced the migration-capable config loader in the diagnostic with a read-only file read followed by dead-key deserialization and validation. Read and invalid-config failures now use fixed safe messages rather than formatting the underlying TOML or error text.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8a.md`

#### B24-2 [P1] — Jev 0.91 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/config_parse.rs:85`
- **Defect**: `ConfigParseCheck::run` 忽略 posture，无条件调用 `Config::load_from_file_reporting_dead_keys`，但该函数并不是纯解析器。`src/config/load.rs:231` 执行迁移，并在 `src/config/load.rs:293` 起的 `if migrated` 分支调用 `save_incremental_to_file` 写回配置，因此 Inspect/Lint 也可以修改用户配置。
- **Failure mode**: 一份其余字段有效、仍有 `[memory] vector_db = "lancedb"` 的旧配置，在只运行 config-parse 的 Inspect 或 JSON Lint 时就会被改写为新后端配置；用户没有请求修复，finding 仍只报告“Config parses”，也没有 repair outcome 说明已经改变文件。
- **Change**: The check now reads and deserializes the config directly instead of calling `Config::load_from_file_reporting_dead_keys`, whose implementation performs migrations and may persist them.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8a.md`

#### B24-3 [P1] — Jev 0.94 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/data_dir.rs:31`
- **Defect**: `DataDirCheck::write_probe` 使用固定路径 `.aleph-doctor-write-probe`，`tokio::fs::write` 以创建或截断模式打开它，没有 `create_new` 或排除符号链接。随后又删除该路径，因此这不是仅针对本次创建对象的无损探针，而且在 Inspect/Lint 下也执行。
- **Failure mode**: 数据目录中已存在同名普通文件时，运行 doctor 会先将其内容覆盖为 `ok` 再删除；若同名项是指向目录外可写配置或其他有价值文件的符号链接，写入会跟随链接，将目标文件截断成 `ok`，随后只删除链接，并最终报告“Data directory OK”。
- **Change**: The write probe now derives a process/time/counter name, opens it with `create_new`, writes through the opened handle, and removes only the path successfully created by that probe. Name collisions are retried and existing objects are never opened.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8a.md`

#### B24-4 [P1] — Jev 0.90 (fix-now) — partial

- **Location**: `src/diagnostics/checks/duplicate_instance.rs:25`
- **Defect**: `classify_other_instances` 把任意一个其他 `aleph-server` 进程都认定为重复，隐含假设当前进程本身是持有 vault 的 daemon。但该检查也注册在冷启动 `aleph-server doctor` 的默认 registry 中；doctor 自己只是诊断进程，不是第二个 daemon。发现问题后的 fix hint 又直接建议 `pkill` 正常 server。
- **Failure mode**: 系统仅运行一个健康的 `target/release/aleph-server` daemon，用户另起 `aleph-server doctor`。扫描排除 doctor 的 PID 后得到 `others == 1`，返回“Multiple aleph-server processes running”、非零诊断退出码，并建议执行能杀掉这唯一健康 daemon 的命令。
- **Change**: A cold process whose arguments include the `doctor` subcommand now contributes zero daemon instances to this check. The broad `pkill` repair hint was replaced with guidance to confirm the shared data-directory lock owner before stopping a process.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8a.md`

#### B24-5 [P1] — Jev 0.90 (fix-now) — fixed

- **Location**: `src/diagnostics/checks/disk_space.rs:93`
- **Defect**: `DiskSpaceCheck::run` 用 `fs2::free_space` 作为 SQLite/vault 写入可用容量；Unix 实现取 `f_bfree * f_frsize`，包括普通用户不能使用的保留块。应与当前非特权 daemon 可用容量对应的是 `fs2::available_space`（`f_bavail`），而不是磁盘总空闲块。
- **Failure mode**: 一个带 5% root 保留空间的 100 GiB ext4 卷，普通用户已用尽可用块但仍有约 5 GiB 保留空间。非 root Aleph 的数据库及 vault 写入报 ENOSPC，doctor 却拿这 5 GiB 与 1 GiB 阈值比较，返回“Disk space OK”。
- **Change**: Replaced `fs2::free_space` with `fs2::available_space` and updated the finding text and comments to describe available capacity.
- **Worker report**: `.review/rd-2026-10-30-modules/fix-W8a.md`

