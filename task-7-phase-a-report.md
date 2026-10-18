# Task 7 — Phase A recovery / 第一阶段恢复报告

**Status / 状态：DONE_WITH_CONCERNS — Phase A only; NOT ACCEPTED.**

English summary: Restored the single-owner pure AI contracts and their original implementations/tests in `aleph-slot-api`, with compatibility reexports in Core. The legacy Core provider trait, HTTP/BoxStream implementations, collectors, and policy application remain unchanged. Scoped rustfmt and static source-preservation checks passed. No Cargo command or compiled test was run; the 18 caller match sites, consumer proof, and full guarded verification remain Phase B work. This report does not accept Task 7 or claim runtime wiring.

## 1. Authority and boundary / 依据与边界

- Root: `/Volumes/TBU4/Workspace/Aleph`; branch: `refactor/modularization-p0`.
- Read and used `.superpowers/sdd/2026-10-14-modularization-p0-implementation/task-7-brief.md`, `task-7-source-review.md`, and `task-7-match-boundary.md`. Rejected `task-7-source-closure.md` was not used as authority. Original source wins over report typos.
- Recovered the four existing untracked `crates/aleph-slot-api/src/{ai,ai_message,ai_response,ai_delta}.rs` files in place; did not delete/reset/recreate the partial work as a baseline implementation. Compared them with the original Core definitions before relying on them.
- Phase A deliberately does not adapt callers or add/edit external tests/examples. Moving the three non-exhaustive enums makes existing external exhaustive matches temporarily invalid; preserving reexport identity alone does not solve that.
- No subagents; no Cargo, install, graph rebuild, commit, stage, unstage, reset, or push. Existing unrelated changes and staging were left alone.

## 2. Exact edited files / 本阶段实际编辑文件

| File | Exact scope of change |
|---|---|
| `crates/aleph-slot-api/src/ai.rs` | Complete source-owned RequestPayload/ToolChoice/SystemPromptPart/ThinkLevel and normalization; new `AiProviderContract`, alias, two ThinkLevel dispatch/const selectors; restore original request/ThinkLevel tests. |
| `crates/aleph-slot-api/src/ai_message.rs` | Complete original messages/content/serde/cache DTOs and all helpers/normalization; borrowed decomposition dispatch helpers; restore all original message tests. |
| `crates/aleph-slot-api/src/ai_response.rs` | Complete original response/call/stop/usage/cost definitions, arithmetic, diagnostics and helpers; restore original response tests. |
| `crates/aleph-slot-api/src/ai_delta.rs` | Original delta enum/visibility predicate, sink and NoopSink, single collect/replay implementation; inline replay/visibility tests. |
| `crates/aleph-slot-api/src/ai_reasoning.rs` | New file: complete original replay/origin/projection implementation; minimum alias/strip-thinking/URL-classification closure; inline original replay tests and pure-leaf coverage. |
| `crates/aleph-slot-api/src/lib.rs` | Add five AI modules and canonical AI exports; document approved pure URL/diagnostics dependency exception. |
| `crates/aleph-slot-api/Cargo.toml` | Add only `url = "2.5"` and `tracing = "0.1"`; update description for AI contracts. Existing dependencies retained. |
| `src/providers/adapter.rs` | Replace moved request/response definitions with API reexports; remove their migrated test module. Original ProtocolAdapter trait body retained. |
| `src/providers/message.rs` | Replace original DTO/helpers/tests with API compatibility reexports. |
| `src/providers/delta.rs` | Reexport API delta/sink/collect/replay; retain original collectors, repair/state/tracker code, stream wrappers and all 25 Core tests. |
| `src/providers/reasoning_replay.rs` | Replace moved replay implementation with API reexports. |
| `src/providers/model_catalog/alias.rs` | Reexport canonicalize_model_id/prefix_matches; remove only their minimum pure implementation closure/vendor-tag table. Vendor inference/prefix catalog and original Core tests remain. |
| `src/providers/model_catalog/capabilities.rs` | Replace only strip-prior-thinking tables/predicate with API reexport; rest of catalog/tests unchanged. |
| `src/providers/protocols/openai_common/provider_policy.rs` | Reexport EndpointClass/detect_endpoint_class; move only classifier/parser/local-host implementation. PayloadPolicy/apply/resolve code and original Core tests remain. |
| `src/agents/thinking.rs` | Reexport ThinkLevel/normalize; retain session key and builtin DialPreset projection. Keep the original wire-ladder assertion in a Core test. |
| `src/thinker/prompt_builder/mod.rs` | Replace SystemPromptPart definition with API reexport; prompt-building behavior unchanged. |
| `task-7-phase-a-report.md` | This report. |

`src/providers/mod.rs` and `src/providers/model_catalog/mod.rs` needed no edits: their existing reexports already route through the compatibility modules. In particular, the old Core `AiProvider` trait is untouched and remains distinct from the new API trait.

## 3. Source preservation / 源码保留核对

### Payload, DTOs and helpers

- `RequestPayload<'a>` remains Debug-only with exactly ten fields: borrowed messages/system_prompt/system_blocks/tools; think_level, temperature, max_tokens, tool_choice, owned model and owned metadata. Original handwritten default and all borrowed constructors retained. **Identity correction / 类型身份更正（2026-10-15）：** the original Phase A claim that tools retained the existing ToolDefinition identity was incorrect. Phase A imported execution-side `aleph_slot_api::tool::ToolDefinition` (`input_schema`), but HEAD `src/providers/adapter.rs` used `crate::tool_metadata::ToolDefinition` (`parameters`), now reexported from `aleph_slot_api::metadata::ToolDefinition`. The bounded identity repair corrects only the import in API `ai.rs`; fields, lifetimes and `with_tools` remain unchanged. 原身份保留断言有误，正确权威是 metadata 类型；证据与本次验证见 `task-7-identity-repair-report.md`，不构成 Task 7 acceptance。
- Original derives, serde attributes/defaults, enum non-exhaustiveness, all fields and helper bodies retained. UnifiedMessage/ContentBlock/ThinkLevel remain non-exhaustive; no fallback policy was invented.
- Full message transform → reasoning projection → tool-pair normalization retained, including orphan removal before interrupted-result synthesis. No duplicated production implementation remains in Core.
- ProviderResponse retains all eight fields, including `truncated_tool_call` and `provider_error`; NativeToolCall thought signature, all StopReason variants/default, TokenCost calculation and all TokenUsage arithmetic retained. `cache_hit_ratio` still delegates to `aleph_protocol`.
- Exact warning texts retained: `Provider response missing usage data` and `Provider response has Unknown stop_reason`. Tool-call validation errors retained, including empty and duplicate IDs.

### Reasoning closure

- Complete ThinkingOrigin/ReasoningReplay defaults and projection bodies moved once. Unchanged messages remain borrowed; signature-shape classification, empty-thinking fill, stripping rules and reasoning-only-turn removal remain source behavior.
- Only VENDOR_TAGS + canonicalize/prefix/dotted-generation/separator helpers, the two prior-thinking strip tables, and EndpointClass + URL/hostname/local-host classifier moved. Pricing, vendor inference/catalog, HTTP/payload/security policy did not move.
- Missing/empty URL still classifies OpenAiPublic; invalid URL still classifies Custom. Existing host/suffix/local matching is preserved, not promoted into a new security boundary.

### Trait and replay bridge

- New `AiProviderContract: Send + Sync`, with `pub use AiProviderContract as AiProvider`, uses `Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>`. Request payload and sink retain borrowed `'a` lifetimes; no BoxStream or new error enum was introduced.
- Required process/name/color and every approved default retained. Explicit reasoning model precedes serving-model hint and then empty string; protocol defaults to borrowed `unknown`; flags false and hints None.
- Default streaming awaits process, replays through the single API collect/replay implementation, and returns the response. Core stream wrappers use that same collection implementation.
- Original replay order remains thinking → thinking signature → text → each tool start/arguments/end → usage → Done. Empty/null arguments are not emitted. Each sink call is awaited sequentially.
- Four owner-side dispatch helpers are present: dispatch_unified_message, dispatch_content_block, dispatch_think_level, and const select_think_level_copy. They use exhaustive owner matches, lazy selected callbacks, shared mutable context where specified, and no unknown→default/drop/panic arm.

### Partial-file corrections and intentional differences

- The initial four files were not accepted on faith: missing original unit-test coverage was restored and all complete DTO/helper bodies were compared to source.
- The partial ThinkLevel `all_covers_every_variant` test had lost the Core wire-ladder assertion. Its owner-local exhaustive assertion lives with the API type; the original wire assertion remains in Core as `all_covers_every_variant_wire_ladder`, because API cannot depend back on Core DialPreset. This is an intentional test split, not a dropped assertion.
- `src/providers/reasoning_replay/tests.rs` remains physically untouched (outside this phase's write boundary). Its eight original tests are now inline under the API owner; the Core shim no longer loads the old external test module.
- Moved-source docs referencing Core-only ProtocolAdapter/catalog types were adjusted to avoid API backedge documentation links; production algorithms were not rewritten.
- Pure helpers needed by Core are doc-hidden public exports/reexports. This visibility engineering is intentional, but still enlarges the public source surface.

## 4. Original test inventory / 原测试去向

Counts below are source inventory, **not executed/passed test counts**.

| Original source | Original #[test] count | Disposition |
|---|---:|---|
| `src/providers/adapter.rs` | 26 | 6 request tests in API ai; 20 response tests in API ai_response. |
| `src/providers/message.rs` | 22 | All moved into API ai_message. |
| `src/providers/reasoning_replay/tests.rs` | 8 | All copied intact into API ai_reasoning; external original file untouched/unloaded. |
| `src/agents/thinking.rs` | 6 | All six represented in API ai; wire-ladder assertion additionally retained in Core via the documented split. |
| `src/providers/delta.rs` | 25 | All retained in Core. |
| `src/providers/model_catalog/alias.rs` | 19 | All retained in Core; seven relevant pure-leaf tests also cover the API owner directly. |
| `src/providers/model_catalog/capabilities.rs` | 32 | All retained in Core; strip-predicate test also covers API owner directly. |
| `src/providers/protocols/openai_common/provider_policy.rs` | 30 | All retained in Core; eight classifier tests also cover API owner directly. |

API inline inventory: ai 14; ai_message 25; ai_response 20; ai_delta 3; ai_reasoning 24; **86 total**. This comprises 62 source-migration tests (one deliberately split), 16 additional owner-local copies of retained pure-leaf tests, and eight new dispatch/replay tests. No claim of behavioral execution or RED/GREEN proof is made.

## 5. Verification evidence / 实际验证

- Direct scoped formatting completed, followed by a fresh `rustfmt --check --edition 2021 --config skip_children=true` over the 16 owned Rust files listed in the verification command below: **exit 0**. Child modules were not traversed/formatted.
- `git diff --check -- <the same scoped Rust files plus API Cargo.toml>`: **exit 0**. This tracked-diff check does not cover untracked file whitespace; direct rustfmt separately covers all new Rust files.
- API manifest parsed with Python tomllib; approved additions are exactly `url: 2.5`, `tracing: 0.1`.
- Static comparison against captured pre-edit source: 21 declaration/body preservation comparisons passed after allowing import qualification, approved public/doc-hidden exposure and formatting; ten new-trait default bodies match the legacy defaults after path qualification. Complete retained collector/repair/tracker region and entire non-strip capabilities catalog also compare unchanged after whitespace/comments normalization.
- Original test-body comparisons found no missing test and no changed assertion body except the documented ThinkLevel owner/Core split. Static inspection found no forbidden Core/HTTP/futures imports in the API AI modules. This is not a dependency-tree or compiler proof.
- Fresh root/branch check matched the requested checkout. Existing five staged paths remain; no index-mutating command was issued. Final staged-diff SHA-256: `eaaaaa3129a40f7b53eea2619f9cca26c02d2ac5c2812e0ba8c01c6e4af94705`. This is a final-state fingerprint, not a claimed before/after hash proof.

Fresh scoped formatting command (followed by scoped `git diff --check`):

```sh
rustfmt --check --edition 2021 --config skip_children=true \
  crates/aleph-slot-api/src/{ai,ai_message,ai_response,ai_delta,ai_reasoning,lib}.rs \
  src/providers/{adapter,message,delta,reasoning_replay,mod}.rs \
  src/providers/model_catalog/{alias,capabilities}.rs \
  src/providers/protocols/openai_common/provider_policy.rs \
  src/agents/thinking.rs src/thinker/prompt_builder/mod.rs
```

**Cargo invocations: 0. Compiled tests executed: 0.** No API/Core build or object-safety/identity assignment/consumer test has been claimed to pass.

## 6. Phase B remains / 后续未完成项

1. Adapt all **18 match sites** frozen in source-review §10 using the approved owner dispatch helpers/accessors. The census groups multiple sites in nine files; no caller file was edited here:
   - `src/providers/protocols/openai_chat/proto_impl.rs`
   - `src/providers/protocols/anthropic/proto_impl.rs`
   - `src/providers/protocols/gemini/proto_impl.rs`
   - `src/providers/responses/shared.rs`
   - `src/providers/ollama.rs`
   - `src/providers/moa/advisory_view.rs`
   - `src/context/compact/compaction_window.rs`
   - `src/context/compact/compactor.rs`
   - `src/context/budget/pressure.rs`
2. Preserve known-arm behavior, early continue/return, merge/index handling, six pressure accumulators, advisory/hash/transcript semantics and existing wildcard behavior. Anthropic const budgets remain None/1024/4096/10000/20000/50000; adaptive XHigh follows original max/xhigh logic, not the report's erroneous high.
3. Add the authorized external `tests/ai_compat.rs` and `examples/modularization/ai/{Cargo.toml,src/lib.rs}` only in Phase B. Prove old↔new DTO identity; distinct legacy/new traits; non-'static payload and sink; dyn port Send+Sync/object safety; default flags/process/replay/error behavior; pending future Drop once and no later sink effects.
4. Run the original brief's focused API/moved/Core/consumer/example checks with nonzero counts, dependency-direction proof and original provider/budget/transcript/advisory/identity tests. Every future Cargo invocation must use the unchanged guarded-cargo.py with fresh resource probe, serial foreground execution, proper timeouts and actual exit logs. No raw Cargo or broad workspace test expansion.
5. Produce the eventual complete Task 7 acceptance evidence/report separately. Previously tracked Task25 failures are not resolved or relabeled by this source migration.

## 7. Concerns and explicit negatives / 风险与未做

- Temporary Core compile failure at external exhaustive matches is expected until Phase B; it has not been compiled/measured in this phase. Other compiler/type/doc issues remain possible because Cargo was expressly forbidden.
- Preserved original behavior: response `provider_error` / `truncated_tool_call` fields do not themselves synthesize a replay Error delta. This existing gap was not silently fixed.
- Generic doc-hidden helpers increase public surface/monomorphization and future API maintenance cost. Preserving borrowed identity/defaults also ties the API to existing third-party/type semantics; no redesign/versioning was attempted.
- No caller adaptation, external consumer/example edits, Cargo/guarded tests, full workspace/clippy/platform/benchmark checks, dependency-tree audit, runtime/registry/security migration, HTTP implementation change, bridge install, effect wiring, remote-effect cancellation guarantee, or Task 7 acceptance.

## 8. Local compiler correction / 局部编译修正（2026-10-15）

- Correction to the earlier source-preservation claim / 更正前文源码保留结论：the migrated `canonicalize_strips_bedrock_dot_tag` test also contained a Core-only `infer_vendor` assertion, causing E0425 at `crates/aleph-slot-api/src/ai_reasoning.rs:843:20` in `repair-logs/20261015-143839-task7-phase-a-api-lib.log`. Removed only that assertion from the API test; its `canonicalize_model_id` assertion remains. 已先核实 `src/providers/model_catalog/alias.rs:412` 完整保留 `assert_eq!(infer_vendor("anthropic.claude-opus-4-8"), Some("anthropic"));`，故 Core 文件未改。This is an owner-boundary test split, not a dropped vendor assertion; no API→Core backedge or duplicated inference policy.
- Direct scoped `rustfmt --edition 2021 --config skip_children=true crates/aleph-slot-api/src/ai_reasoning.rs` and the same command with `--check`: exit 0 / 通过。
- Exactly one fresh guarded run / 本次仅一次新 guarded 验证：`python3 -B .superpowers/sdd/2026-10-14-modularization-p0-implementation/guarded-cargo.py --timeout-s 900 task7-phase-a-api-lib-alias-owner-repair-20261015-144139-92783 -- test -p aleph-slot-api --lib`; foreground tool timeout 960s; fresh probe exit 0, `available_bytes=6396166144 required_bytes=4294967296 ALLOW`; jobs=2; cargo exit 0; 261 passed, 0 failed, 0 ignored; no timeout or surviving descendants. Log: `.superpowers/sdd/2026-10-14-modularization-p0-implementation/repair-logs/20261015-144139-task7-phase-a-api-lib-alias-owner-repair-20261015-144139-92783.log`.
- Scope / 边界：this addendum supersedes the earlier zero-Cargo statements only for this local API-lib repair. Core vendor assertion preservation was inspected, not executed. 未做 Core/consumer/workspace 验证、Phase B 迁移、子代理、commit/stage/push/install/graph；不构成 Task 7 acceptance。Local repair only; overall status remains NOT ACCEPTED.
