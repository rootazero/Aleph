# Memory Wiki Evidence Chain Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist only the sources that reached the synthesis model and make the resulting scoped evidence inspectable through the existing tool and Panel. / 只记录实际进入合成的来源，并让现有工具和 Panel 能在正确分区查看证据。

**Architecture:** Keep `NoteIndexer` and Markdown authoritative; SQLite remains the reverse lookup and degraded-read projection. The existing Core Trace implementation follows the bounded evidence graph; Gateway resolves exactly one authorized partition and Panel consumes shared protocol types. / 沿用 `NoteIndexer`、Markdown 事实源和 SQLite 投影，Core 遍历、Gateway 选定授权分区，Panel 仅呈现共享契约。

**Tech Stack:** Rust, Tokio, SQLite, Serde/Schemars, Leptos/WASM. No added dependencies. / 不增依赖。

**Spec:** [`docs/superpowers/specs/2026-11-26-memory-wiki-evidence-chain-design.md`](../specs/2026-11-26-memory-wiki-evidence-chain-design.md), approved at `8ad21aab4` on `refactor/memory-wiki`.

## Global Constraints / 全局约束

- Work and commit only on the isolated `refactor/memory-wiki` line; no `main`, merge, push, or production mutations. For independent writers, use separate managed worktrees on the same isolated base, verify their diffs, and integrate here. / 仅隔离分支交付；独立写入者各用独立工作树，主会话验证后集成。
- Reuse `NoteIndexer`, Memory/Tool Capability, `memory_trace` and `memory.trace`. No data/schema migration, historical source inference, second catalog or claim store, background service, or Dreaming rollback. / 保留唯一事实源和现有入口，不迁移数据、不补造证据。
- Synthesis: existing five-note stage eligibility and three-source category minimum; at most 15 usable previews, 300 chars per preview, ordered by category and exact path. / 保持阶段门槛，事实按实际可用预览数量判定。
- Trace: 500 evidence rows hard cap, 64 note expansions, 500 source/reverse candidates per call, 100 displayed root facts, existing 800-char raw preview. `max_results` bounds evidence or decisions only. `truncated` means a budget removed something; `incomplete` means referenced evidence could not be read or resolved. / 两种状态不混用。
- Base caller ID remains backward-compatible; exact addressing cannot broaden visibility, recompose a storage ID, or fall back to another partition. Use `max_results: 0` as an actual evidence limit. / 精确地址不扩大权限，`0` 不解释为默认值。
- Run narrowly filtered tests first, then full Panel library tests and WASM build according to `docs/reference/DEVELOPMENT.md`. Build against locked dependencies. Baseline (`builtin_tools::memory_trace::tests`) 9/9 at `0386bdfff`. / 先最小回归，再验证 Panel 出厂形态。

## File Ownership / 文件分工

| Task | Owned changes / 归属 |
|---|---|
| 1. Synthesis | `src/memory/dreaming/stages/note_synthesis.rs`; the independent writer does not change `DreamContext` or global caches. |
| 2. Wire and addressing | `shared/protocol/src/{lib,memory_trace}.rs`, `src/builtin_tools/memory_trace.rs` (wire-only at this step), `src/gateway/handlers/memory/mod.rs`, `src/memory/store/sqlite/memory_write_decisions.rs`. |
| 3. Core evidence | `src/builtin_tools/memory_trace.rs` and `src/builtin_tools/memory_trace/{walk,tests}.rs` if split; `src/memory/notes/store.rs`, `src/memory/store/sqlite/notes/{store_impl,tests}.rs`, `src/memory/store/{raw_memory,markdown/raw_memory,markdown/raw_memory_tests}.rs`. |
| 4. Panel | `interfaces/webchat/src/api/memory.rs`, `interfaces/webchat/src/platform/wide/views/memory/{provenance,mod,curated}.rs`, `interfaces/webchat/locales/{en,zh}.json`. Verify the actual addressing caller in `drawer.rs`; keep curated ledger behavior. |
| 5. End-to-end and docs | Cross-stage fixture in `src/memory/dreaming/stages/note_synthesis.rs`; `docs/reference/{FEATURE_LOCATOR,MEMORY_SYSTEM}.md`; an evidence report under `docs/superpowers/runs/`. |

Task 1 can run concurrently with Task 2. Tasks 3 and 4 consume the frozen Task 2 protocol; only Core owns its module after Task 2. Integrate Task 1 before changing its test calls to the bounded store APIs, and Tasks 3–4 before Task 5. / 分派只在产出可独立验证时使用：综合写入、Panel 验证或干净视角的最终审查；主会话负责共享契约和集成。

## Review Focus / 容易漏掉的路径

1. A bare org row can have the same note path as a personal row; exact bare, exact composed and legacy scoped calls must not silently substitute one another (Task 2).
2. An untyped legacy frontmatter ref can look like a note but have an exact raw match; a *typed* prior-note ref to the same text must not resolve to that raw (Task 3).
3. A hand-edited Markdown file can disagree with SQLite while both are readable; forward citations and fact text must come from the same bytes, not their union (Task 3).
4. An oversized or unreadable file, including a symlink to an authorized sibling's content, must not be reported as complete evidence, and a cyclic or fan-out graph must not defeat the budget (Task 3).
5. An evidence list can be empty while fact annotations or a missing source edge exist; Panel must not hide those sections or show the generic empty message (Task 4).

---

### Task 1: Record only consulted synthesis inputs / 写入合成真正使用的来源

**Interfaces:** `DreamStage::execute(&self, ctx: DreamContext) -> Result<DreamContext, AlephError>`; `DreamContext::load_content(&mut self, path: &str) -> Option<String>`; `NoteIndexer::write_note(&self, agent_id: &str, category: &str, note: &KnowledgeNote) -> Result<PathBuf, AlephError>`; existing `build_synthesis_note(category: &str, synthesis_text: &str, source_links: Vec<String>) -> KnowledgeNote`.

- [ ] **Write the failing production-stage tests.** Use a temp NoteIndexer/SQLite corpus and a test-only recording `AiProvider` (delegate its response to `MockProvider`). `synthesis_persists_only_prompted_sources`: run with 18 reversed category paths, path `00` missing and `01` whitespace, and a >300-codepoint multibyte preview for `02`. Assert the provider saw exactly `02..=16` in that order and a 300-character preview; parse the written note, assert `source_notes == links == prompted_paths`, `sources_of` lists the same paths, and graph links resolve to those indexed inputs. `synthesis_requires_three_readable_previews`: assert `calls == 0`, `synthesis_count == 0`, and the previous summary's bytes are unchanged when only two inputs are usable.
- [ ] **Run the red test.** `cargo test --locked -p alephcore --lib synthesis_persists_only_prompted_sources` should fail on the prompt/member list, not setup.
- [ ] **Fix selection and write.** Order eligible category/path groups (dedup exact paths); collect `(path, preview)` pairs from usable inputs before formatting; derive the prompt count and persisted metadata from those pairs. Keep provider exhaustion, indexer effects and `synthesis_rewrites` intact. Remove manual `create_dir_all` (the existing `write_note` creates parents). Do not replace missing content with empty text or manufacture provenance from model-created links.
- [ ] **Run and commit.** `cargo test --locked -p alephcore --lib memory::dreaming::stages::note_synthesis::tests`; both new assertions and existing tests pass. Commit `fix(memory): cite only notes used in synthesis`.

### Task 2: Unify Trace wire types and fix Gateway addressing / 共享契约与分区寻址

**Produces (public):** In `aleph_protocol::memory_trace`, `TraceKind { Note, Raw, ProfileSection, WriteDecision }` (snake case, existing schema), `TraceRequest { agent_id: Option<String>, target: String, kind: TraceKind, max_results: Option<usize>, exact_partition: bool }`, and the following response types with Serde:

```text
TraceResult { target: String, notes: Vec<String>, evidence: Vec<EvidenceItem>,
              facts: Vec<FactOrigin>, write_decisions: Vec<WriteDecisionRow>,
              note_sources: Vec<NoteSource>, truncated: bool, incomplete: bool }
EvidenceItem { raw_id: String, via_note: Option<String>, via_session: Option<String>,
               content: Option<String>, pruned: bool }
FactOrigin { note_path: String, fact_idx: usize, text: String, origin: String,
             inferred: bool, source_id: Option<String> }
WriteDecisionRow { action: String, reason: String, subject: String, created_at: i64 }
NoteSource { note_path: String, source_path: String, status: NoteSourceStatus }
NoteSourceStatus { Available, Unavailable, NotVisited }   // snake_case
```

`NotVisited` means a budget stopped expansion, not that the source is missing; only `Available` denotes readable, parseable Markdown. `TraceResult::default()` provides the denied/unknown response with the original target filled in. `MemoryTraceTool::call_impl(&self, args: MemoryTraceArgs) -> anyhow::Result<TraceResult>` keeps its signature and builtin `MemoryTraceArgs` never accepts a partition. / 工具参数不接受分区，RPC 请求才有精确寻址。

- [ ] **Pin old wire and exact routing.** Protocol fixture `trace_response_preserves_legacy_shape` reads `{"notes":[],"evidence":[]}` (all new data defaults), preserves the omitted optional arrays/flags on actual empty response, and round-trips each kind plus facts, edge statuses and write decisions. In `gateway::handlers::memory::tests`, extend the active-scope fixture to seed `main` and a personal partition with the same `habits/exercise` and different raw contents; assert exact bare → org, composed without flag → personal, legacy bare → personal. Assert exact with omitted `agent_id` is invalid parameters; denied exact composed → JSON equal to an authorized miss. Keep the existing active-scope curated ledger assertion.
- [ ] **Run the red Gateway test** with the invented address flag supplied as JSON (no new type required to get a real behavioral failure): `cargo test --locked -p alephcore --lib trace_addresses_exact_storage_partition`; current scoped route returns the personal sentinel for the exact bare request.
- [ ] **Implement the contract.** Keep tolerant legacy decoding (`target`, arrays/new flags, optional evidence/ledger fields default if absent); serialize existing keys with previous empty/absent policy. Move the SQLite wire row to shared `WriteDecisionRow` and re-export as `MemoryWriteDecisionRow` where existing Core callers import it. Reuse `TraceKind` from the shared module for schema generation and remove the duplicate Core wire structs; preserve existing callers with re-exports only where used.
- [ ] **Implement addressing in `handle_trace`.** Decode `TraceRequest`. Validate exact requests have an ID, apply `partition_visible(&agent)` before paths/reads; for visible explicit or recognizable composed input take the literal ID, otherwise use `RuntimeMemoryContext::resolve_storage_id` as today. Use `TraceResult` for the existing no-oracle response. No union read and no change to `memory.curated.*`.
- [ ] **Run and commit.** `cargo test --locked -p aleph-protocol --lib memory_trace`; `cargo test --locked -p alephcore --lib gateway::handlers::memory::tests`; `cargo test --locked -p alephcore --lib builtin_tools::memory_trace::tests`. Commit `fix(memory): share trace contract and address exact partitions`.

### Task 3: Resolve bounded evidence from same-partition sources / 有界地追溯同分区的真实来源

**Consumes:** Task 2 shared contract; `KnowledgeNote::from_markdown` and `facts_with_origin() -> Vec<(String, FactProvenance)>`, `note_content_path`, `get_note_index`, `profile::store::parse_user_md`, and `RawMemoryStore::get_raws_by_ids`. **Produces:** `NoteStore::{sources_of,notes_citing}(&self, agent_id: &str, key: &str, limit: usize) -> Result<Vec<String>, AlephError>` and `RawMemoryStore::get_raw_ids_by_session(&self, agent_id: &str, session_id: &str, limit: usize) -> Result<Vec<String>, AlephError>` (ordered, bounded rows/IDs; no profile-side raw body reads just to get IDs). No external store API or new tables.

- [ ] **Pin persisted references and classification.** In trace tests through real `NoteIndexer` and raw store, assert `synthesis/test → reference/l1 → raw-1` returns the note edge and raw with `via_note == "reference/l1"`; fact-only `RawSource` reaches its raw, fact-only `PriorNote` reaches its note, and inferred/system/legacy marker refs do not create edges. In `trace_uses_current_markdown_not_stale_projection`, write/index one citation, then edit on disk: forward trace returns only the new citation/root fact; missing file falls back to projected sources and sets `incomplete`. `trace_legacy_raw_match_beats_path_but_typed_prior_note_does_not` asserts raw evidence and the explicit note edge stay distinct even with the same ID string.
- [ ] **Pin bounded and scoped reads.** Assert a diamond keeps both parent edges while one child's raw is shown once, a cycle terminates, `max_results=0` hides evidence but not facts, exactly max rows from a complete walk is not truncated, >500 results/fan-out is truncated, >64 children stop expansion with `not_visited`, and >100 root facts set `truncated`. A raw trace with >500 reverse hits returns its own row first with `max_results=1`. A partition with the same note path and raw ID uses only its own bytes. Malformed addresses and unknown roots return the empty shape; on Unix tests cover partition symlink alias, nested note/profile symlink escapes and oversized Markdown/profile. Bounded store tests assert `limit = 0`, stable prefix and no duplicate reverse hits when both projections cite one note.
- [ ] **Run the red trace test.** `cargo test --locked -p alephcore --lib trace_follows_note_and_fact_citations` should fail on missing edges/evidence, not fixture setup.
- [ ] **Bound the existing queries.** In `store_impl.rs`, bind `LIMIT` after `ORDER BY source_ref` / reverse ordered SQL UNION and keep zero a real limit; preserve both provenance tables. Reuse scoped raw index access for ordered session IDs (`created_at, rowid`) and replace the old session-body method only used by this path. Update the existing store tests and callers including Task 1’s projection assertion. Use a limit+1 overflow sentinel; cap before loading raw bodies.
- [ ] **Resolve a single safe evidence graph.** In `call_impl` use one per-call bounded walk; split into a focused `walk` module if needed. Validate partition/name components before path joins, use the note path helper and scope checked/bounded file reads; ensure the canonical corpus is its named directory, not a sibling alias. Read at most 4 MiB per note/profile file using a byte-limit sentinel (fits existing Core note write caps); limit displayed fact text to 800 chars and set `truncated` if clipped. Parse each expanded note at most once, and do not retain full Markdown for every visited note. Derive forward fan-out from Markdown; only on read/parse failure use bounded `sources_of`. For an invalid or completely missing root do not invent a note or set a status; indexed fallback/unavailable referenced notes and raws set `incomplete`. No ordinary wikilink traversal. Reuse the profile parser for ProfileSection and the existing decision log for WriteDecision.
- [ ] **Bound and order the walk.** Fixed limits above; BFS with stable sorting, discovered/expanded sets, per-call scoped raw-ID cache or bounded batches, and per-parent evidence/edge dedupe. Count input candidates and reverse/profile work against one budget (including session IDs); apply a sentinel, not `>= max_results`, to distinguish exact-fill versus suppression. `notes` keeps root/reverse matches, not every forward node; exact raw row is first if present. Keep model annotations as annotations in tool description, not testimony. Remove superseded `fact_origins` and production unbounded paths.
- [ ] **Run and commit.** `cargo test --locked -p alephcore --lib builtin_tools::memory_trace::tests`; `cargo test --locked -p alephcore --lib memory::store::sqlite::notes::tests`; `cargo test --locked -p alephcore --lib memory::store::markdown::raw_memory`; rerun synthesis and Gateway tests. Commit `fix(memory): follow bounded Wiki provenance in Core`.

### Task 4: Render the existing evidence drawer / 打通原有证据抽屉

**Consumes:** shared `TraceRequest`/`TraceResult` and statuses from Task 2; no Core editing. **Interface:** `MemoryApi::trace(state: &DashboardState, agent_id: &str, target: &str, kind: TraceKind, max_results: usize, exact_partition: bool) -> Result<TraceResult, String>`. Use the existing drawer target (`NoteAddress`-derived note agent or raw row agent), not the picker; `ProvenanceSection` uses exact requests, curated ledger passes `false`.

- [ ] **Pin the loss and UI decisions.** In Panel, `trace_result_parses_fact_only_response` decodes a server JSON fixture with a root fact and no raw evidence; assert `facts[0].source_id` survives. In provenance presentation test a fact-only result, unavailable note edge, empty result, and combined `truncated`/`incomplete` states; assert nonempty rows are present when `evidence` is empty and that `Note` root is not rendered as a "Cited by" hit. Assert that rows with the same `raw_id` but different `via_note` expand independently; key expansion by the same pair as Core's evidence identity.
- [ ] **Run a red assertion.** `cargo test --locked -p aleph-panel --lib trace_result_parses_fact_only_response` should fail before the local DTO preserves facts (verify via old decoder if a direct field reference does not compile).
- [ ] **Connect Panel.** Replace duplicate DTOs with imports from shared protocol. Serialize typed requests; preserve curated filters and section. In the single `ProvenanceSection` render source edges (including not visited/unavailable), root fact annotations and their source IDs, existing raw excerpts, and separate omission flags. Use existing English/Chinese i18n keys where appropriate; add only user-facing labels. Remove the `evidence.len() >= TRACE_MAX_RESULTS` heuristic and old duplicate local types. Do not make source IDs fetch addresses, add a new surface, or infer facts in Leptos.
- [ ] **Run and commit.** `cargo test --locked -p aleph-panel --lib`; `just wasm` in this worktree, or report the exact missing tool and try the equivalent WASM Cargo check. Commit `fix(panel): show Wiki fact and source provenance`.

### Task 5: Integrate, verify and align references / 集成实证、校对文档并提交

**Consumes:** previous commits, shared protocol and the production `note_synthesis` test harness.

- [ ] **Add one end-to-end replay** in the synthesis regression fixture: seed a real source note declaring `raw-1`, run production `NoteSynthesisStage`, use its persisted index identity as the `memory_trace` target, then assert the exact synthesis→source edge, the raw excerpt and `via_note`. Invoke `handle_trace` through the existing Gateway test harness for the scoped delivery counterpart rather than recreating HTTP or authorization. Send an equivalent response through the shared Panel parse/presentation fixture. / 验证实际持久化和消费，不只断言调用发生。
- [ ] **Run the integrated gates.** Rerun Core filters for synthesis, Trace, Gateway, SQLite notes and raw store; shared protocol tests; full Panel library tests and the shipped WASM build. Attempt a local run of the existing drawer or browser fixture to check evidence is visible; report separately if native view tests/WASM compile are the only verified UI coverage. Run `git diff --check 0386bdfff..HEAD` plus format checks on touched Rust files. Request an independent final diff review with GPT Sol/Kimi Pro for attribution, isolation, budgets and old requests; fix verified findings and rerun affected tests. No runtime/network model needed for the fixtures.
- [ ] **Update `docs/reference/FEATURE_LOCATOR.md`**, §§2.9–2.10 and relevant partition cross-reference: locate the real producer, shared wire, Core trace and UI consumer; distinguish current authoritativeness, historical projection fallback and result limits. **Update `docs/reference/MEMORY_SYSTEM.md`**, Wiki/provenance and scope sections: explain direct vs prior-note source, graph vs evidence edges, exact vs legacy addressing, truncated vs incomplete, and Dreaming health scoring not being rollback. Verify links and code paths.
- [ ] **Record results and commit.** Write `docs/superpowers/runs/2026-11-26-memory-wiki-evidence-chain.md` with reference commits, exact gates, results, remaining gaps, and removed duplicate paths. Commit docs/replay after gates; inspect `git status --short --branch` and full commit list. Keep branch local. / 逐项实报，未跑的验证不写成通过。
