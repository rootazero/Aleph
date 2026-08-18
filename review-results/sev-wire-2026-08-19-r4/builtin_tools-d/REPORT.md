# Code review — `src/builtin_tools/` chunk D (2026-08-19 round r4)

## Scope
- Files reviewed: 58 Rust files / 29,973 physical LoC (including in-file tests):
  - Top level (30): `artifact_publish.rs`, `cron_manage.rs`, `ctx_search.rs`, `flag_user_correction.rs`, `google_meet.rs`, `mcp_login.rs`, `mcp_prompt.rs`, `mcp_resource.rs`, `memory_browse.rs`, `memory_explore.rs`, `memory_reflect.rs`, `memory_search.rs`, `memory_timeline.rs`, `memory_trace.rs`, `node_file.rs`, `node_invoke.rs`, `node_invoke_many.rs`, `node_list.rs`, `node_manage.rs`, `partial_output.rs`, `process_completion.rs`, `process_journal.rs`, `process_registry.rs`, `recall_context.rs`, `recall_events.rs`, `remember.rs`, `scratchpad.rs`, `scratchpad_registry.rs`, `session_complete.rs`, `session_search.rs`.
  - `web_fetch/` (4): `cache.rs`, `extract.rs`, `mod.rs`, `types.rs`.
  - `sessions/` (8): `compact_tool.rs`, `helpers.rs`, `list_tool.rs`, `mod.rs`, `new_tool.rs`, `send_tool.rs`, `set_mode_tool.rs`, `set_topic_tool.rs`.
  - `file_ops/` (16, overlap with chunk A): `apply_patch.rs`, `batch.rs`, `edit.rs`, `edit_match.rs`, `image_read.rs`, `mod.rs`, `ops.rs`, `path_utils.rs`, `read.rs`, `read_cache.rs`, `search.rs`, `stats.rs`, `text.rs`, `tool.rs`, `types.rs`, `write.rs`. Read end-to-end for caller/path-safety context; duplicate chunk-A findings are intentionally omitted.
- LoC total: 29,973 (`wc -l`, all 58 files above).
- Cross-checked callers: `executor/builtin_registry/{builder,registry}` (construction, hidden-argument injection, per-turn identity), `gateway/{visibility,announce_delivery,process_announce}`, A2A `adapter/server/bridge.rs`, memory raw-store SQLite queries / reflector / event traveler, session-store backends and `SimpleExecutionEngine`, scratchpad manager and session-deletion paths, MCP OAuth callback/provider/storage and HTTP transport, fetch-provider implementations and the SSRF contract, `gateway/method_authz.rs`, cluster `NodeRegistry`, and file-layer path/atomic-write helpers.
- Method: pure static, read-first review. Every scoped `.rs` file was read end-to-end; candidate findings were traced through their production construction/dispatch/read-write paths and retained only above 80% confidence. No source edits, builds, tests, scripts, or cargo commands were run.

## Findings

### BT-D-R4-01 — `recall_context` ignores the query and returns the oldest chunks
- **File**: src/builtin_tools/recall_context.rs:89-116
- **Severity**: High
- **Category**: correctness
- **Description**: The tool is advertised as semantic recovery for a specific question, but `args.query` never participates in retrieval. It only performs a path-prefix read and echoes the query in the result. The SQLite implementation orders that read by `created_at ASC`, so a request for a late pre-compression error/code decision returns the oldest `max_results` chunks instead. Every returned fragment is then labelled with `relevance_score: 1.0`, falsely representing storage order as relevance. Once a session has more chunks than the limit, the requested context may be structurally unreachable through this tool.
- **Evidence**:
  ```rust
  let raws = self
      .database
      .get_raw_by_path_prefix(&path_prefix, &self.agent_id, args.max_results)
      .await?;

  let fragments = raws
      .into_iter()
      .map(|r| RecalledFragment {
          content: r.content,
          relevance_score: 1.0,
          source_path: r.path.unwrap_or_default(),
      })
      .collect();

  Ok(RecallContextResult {
      fragments,
      query: args.query,
  })
  ```
- **Suggested fix**: Add a session/agent-scoped retrieval method that ranks raw chunks by the supplied query (FTS5/BM25 is sufficient; vector ranking is optional), and return the top bounded matches with real scores. If no index exists for raw chunks, reuse the session-event FTS or add an indexed `search_raw_by_path_prefix(agent_id, prefix, query, limit)` query rather than teaching the model that chronological chunks are semantic matches.
- **Verification**: Read the production dispatch arm, which correctly supplies the current turn's session and composed memory partition, so identity wiring is not the cause. Read `memory/store/sqlite/raw_memories.rs:get_raw_by_path_prefix`, whose SQL is `ORDER BY created_at ASC LIMIT ?3`. Searched all `RecallContextArgs` uses; `query` is not consumed anywhere else.

### BT-D-R4-02 — `session_complete` writes every retrospective as agent `main` with no session attribution
- **File**: src/builtin_tools/session_complete.rs:49-97, 111-150
- **Severity**: High
- **Category**: correctness
- **Description**: The tool trusts construction-time `self.agent_id` and an optional shared session handle. Production construction hard-codes `agent_id = "main"`, and no production caller invokes `with_session_handle`. Consequently, a non-main agent's completion is inserted into `main`'s memory corpus, while even a main-agent row lacks `session_id`. This both poisons the wrong agent's future memory and severs the retrospective from the conversation that produced it.
- **Evidence**:
  ```rust
  let session_id = self.current_session_id();
  let mut raw = RawMemory::new(
      content,
      RawMemorySource::SessionEnd {
          reason: SessionEndReason::TaskDone,
      },
  )
  .with_agent(self.agent_id.clone());

  if let Some(sid) = &session_id {
      raw = raw.with_session(sid.clone());
  }
  ```
  Production construction:
  ```rust
  let agent_id = "main".to_string();
  let mut tool = SessionCompleteTool::new(db.clone(), agent_id);
  ```
- **Suggested fix**: Construct/execute `SessionCompleteTool` per call, deriving both values from the same `TurnContext`: use the current session key and `caller_memory_partition(base_agent)` (including user/project scope). Remove the mutable global session-handle fallback from this tool, or reserve it only for explicitly privileged non-turn callers.
- **Verification**: Read `build_collab_session_tools` and the `session_complete` dispatch arm. Repository-wide search found `with_session_handle` only at its method definition, not at a production call site; the builder receives `boot_fallback_agent_id` but nevertheless supplies literal `main`.

### BT-D-R4-03 — A capture-filter `Block` is reported as a successfully recorded completion
- **File**: src/builtin_tools/session_complete.rs:132-150
- **Severity**: Medium
- **Category**: error-handling
- **Description**: `insert_with_capture_filter` returns `CaptureDecision::Block` without inserting the raw memory. `session_complete` discards that decision and unconditionally returns `ok: true` with “Task completion recorded. Retrospective extraction queued”. An extension policy can therefore deliberately reject the capture while the model—and potentially the user—receives a false persistence receipt.
- **Evidence**:
  ```rust
  if let Some(ref registry) = self.capture_registry {
      let store: Arc<dyn RawMemoryStore> = self.db.clone();
      let ctx = CaptureCtx { /* ... */ };
      insert_with_capture_filter(&store, registry, &ctx, raw).await?;
  } else {
      self.db.insert_raw_memory(&raw).await?;
  }

  Ok(SessionCompleteResult {
      ok: true,
      message: format!("Task completion recorded. Retrospective extraction queued ..."),
  })
  ```
- **Suggested fix**: Match the returned `CaptureDecision`. Return `ok: false` (or an explicit `blocked` status) with the extension's reason and no “recorded/queued” wording for `Block`; return success only for `Allow` after insertion succeeds.
- **Verification**: Read `memory/extensions/insert_helper.rs`: its `Block` arm logs and performs no `insert_raw_memory`, then returns `Ok(CaptureDecision::Block)`. The production builder wires the capture registry when configured, so this branch is live.

### BT-D-R4-04 — `memory_reflect` is pinned to `main`, allowing cross-agent synthesis and filing
- **File**: src/builtin_tools/memory_reflect.rs:38-91, 111-139
- **Severity**: High
- **Category**: security
- **Description**: Reflection and post-reflection query filing both use the tool's construction-time `self.agent_id`. Production construction hard-codes that value to `main`; the tool is shared through registry dispatch, and its session handle is also never attached. A non-main agent calling `memory_reflect` therefore synthesizes from `main`'s owner namespace and files any resulting query note under `main`, rather than using the caller's scoped memory partition. This is both cross-agent disclosure and write contamination.
- **Evidence**:
  ```rust
  let opts = ReflectOpts {
      agent_id: self.agent_id.clone(),
      namespace: NamespaceScope::Owner,
      max_tokens: None,
      session_id: session_id.clone(),
  };
  let synthesis = reflector.reflect(&args.query, opts).await?;

  if let Some(qf) = self.query_filer.clone() {
      let agent = self.agent_id.clone();
      tokio::spawn(async move {
          if let Err(e) = qf.maybe_file(&agent, &q, &synth, sid.as_deref()).await {
              tracing::warn!("query filer failed: {e}");
          }
      });
  }
  ```
- **Suggested fix**: Make the dispatch arm construct call-specific `ReflectOpts` from `current_agent_id` plus `session_read_ids`/the caller's composed partition and current session key. Do not store a principal on a process-lifetime tool. Pass the same resolved principal into `QueryFiler`.
- **Verification**: Read the reflector/query-filer injection methods and the execution arm. `build_collab_session_tools` constructs `MemoryReflectTool::new("main")`; repository-wide search found no production call to `with_session_handle`.

### BT-D-R4-05 — `memory_timeline` is a global fact-ID oracle with no agent or namespace authorization
- **File**: src/builtin_tools/memory_timeline.rs:31-69
- **Severity**: Medium
- **Category**: security
- **Description**: The tool accepts only `fact_id` and delegates to a process-wide `MemoryTimeTraveler`; it has no caller agent, owner, project, or namespace input. The traveler queries all events by fact ID and reconstructs content, actors, access queries, invalidation reasons, and history. Event payloads already contain `agent` and `namespace`, but neither layer checks them. Any caller that learns or guesses another corpus's note/fact ID can read its lifecycle and current content.
- **Evidence**:
  ```rust
  pub struct MemoryTimelineArgs {
      pub fact_id: String,
  }

  let explanation = self
      .traveler
      .explain_fact(&args.fact_id)
      .await?;
  ```
- **Suggested fix**: Carry the caller's composed memory partition into the tool and query events by `(fact_id, agent_id)` (plus namespace/owner visibility where applicable). As defense in depth, validate the first `NoteCreated` event's `agent`/`namespace` before returning any event details, and return indistinguishable not-found for denied IDs.
- **Verification**: Read `memory/events/traveler.rs`, which calls `StateDatabase::get_memory_events_for_fact(fact_id)` and never filters the `NoteCreated { agent, namespace, ... }` fields. The registry builds one traveler over the shared `StateDatabase` without caller identity.

### BT-D-R4-06 — Actor-less A2A runs can search every on-disk memory partition
- **File**: src/builtin_tools/memory_search.rs:330-402
- **Severity**: High
- **Category**: security
- **Description**: All model-supplied workspace filters are gated through `ambient_actor`, but `None` is explicitly treated as unrestricted. A2A server runs carry only `unattended=true` metadata and no user/scope attribution, so an authenticated remote A2A peer reaches this `None` arm. Calling `memory_search` with `cross_workspace: true` then enumerates every note corpus (`main__u-*`, room/project partitions, and base corpora) and retains all of them. The agent-level A2A policy does not restore user ownership because these partitions commonly belong to different users of the same `main` agent.
- **Evidence**:
  ```rust
  // `None` (cron / A2A / tests) stays unrestricted
  let actor = crate::gateway::visibility::ambient_actor();
  let admits =
      |id: &str| crate::gateway::visibility::partition_visible_to(id, actor.as_deref());

  let workspace_filter = if args.cross_workspace.unwrap_or(false) {
      let all = list_note_corpora(&note_memory_dir());
      let kept: Vec<String> = all.iter().filter(|id| admits(id)).cloned().collect();
      AgentEnvFilter::Multiple(kept)
  // ...
  ```
- **Suggested fix**: Do not encode privilege as missing identity. Stamp A2A runs with an authenticated principal/tenant scope, and make tool-facing partition reads fail closed when no actor is available unless an explicit internal-system capability is present. Disable `cross_workspace` for actor-less external runs or require an operator capability separate from `unattended`.
- **Verification**: Read `gateway/visibility.rs:partition_visible_to`, whose `None` arm returns `true`, and A2A `adapter/server/bridge.rs:build_run_request`, whose metadata contains only `UNATTENDED_KEY`. The memory tool is read-only and therefore not stopped by confirmation gates.

### BT-D-R4-07 — Actor-less A2A runs can quote every user's session transcripts
- **File**: src/builtin_tools/session_search.rs:10-20, 116-140, 243-292
- **Severity**: High
- **Category**: security
- **Description**: The module correctly states that `search_messages` is a global sweep and that `ambient_transcript_visible` is the only user-isolation boundary. That boundary returns `true` when `ambient_actor()` is `None`; A2A runs are created without owner/speaker scope, so remote A2A work is implicitly transcript-superuser work. The A2A agent policy only compares agent IDs and cannot distinguish Alice's and Bob's sessions when both use `main`. The lazy fallback then returns raw 200-character quotes and synthesized summaries from any matching session on the install.
- **Evidence**:
  ```rust
  let visible = crate::gateway::visibility::ambient_transcript_visible(
      self.context.session_store().as_ref(),
      session_key,
  )
  .await;
  // ...
  let raw_hits = self
      .context
      .session_store()
      .search_messages(&args.query, args.max_results.saturating_mul(4))
      .await?;
  ```
- **Suggested fix**: Require a user/tenant principal for transcript search. Propagate an authenticated actor through A2A metadata and fail closed when it is absent; if trusted automation truly needs cross-user search, model it as an explicit admin/system capability and audit it rather than conflating it with `None`.
- **Verification**: Read `gateway/visibility.rs:ambient_transcript_visible`, which immediately returns `true` when no actor exists. Cross-checked the A2A bridge request metadata (no owner/scope), the global session-store search implementations, and the permissive same-agent A2A-policy path.

### BT-D-R4-08 — `session_search` can repeatedly synthesize the same session and has no result/cost ceiling
- **File**: src/builtin_tools/session_search.rs:35-42, 210-292, 316-334
- **Severity**: Medium
- **Category**: perf
- **Description**: `max_results` is model-controlled and never clamped. Primary survivors each trigger a separate global `search_messages` call for evidence. The lazy fallback fetches up to `max_results * 4` message hits and may run one LLM `lazy_for` synthesis per hit. Its `already_covered` set is immutable and contains only primary hits; it is not updated after a lazy hit is added. Multiple matching messages from one unsummarized session therefore produce duplicate output rows and repeated paid synthesis calls for that same session. The file backend makes the amplification worse because each global search scans session metadata and transcript files.
- **Evidence**:
  ```rust
  let already_covered: HashSet<String> =
      hits.iter().map(|h| h.session_key.clone()).collect();
  let raw_hits = self.context.session_store()
      .search_messages(&args.query, args.max_results.saturating_mul(4))
      .await?;

  for raw in raw_hits {
      if already_covered.contains(&raw.session_key) { continue; }
      let summary = if let Some(ref synth) = self.synthesizer {
          synth.lazy_for(&raw.agent_id, &raw.session_key).await
      // ...
      };
      hits.push(SessionSearchHit { session_key: raw.session_key, /* ... */ });
  }
  ```
- **Suggested fix**: Clamp `max_results` (for example 1–20), make the seen-session set mutable and insert the session key before synthesis, and perform one global search that is grouped by session for both fallback selection and evidence quotes. Batch or cache lazy synthesis by `(agent_id, session_key)`.
- **Verification**: Read both session-store backends: search results are message-level, so duplicate session keys are normal; the file backend scans transcripts. Confirmed `fetch_evidence_quotes` invokes the same global search once per primary survivor and no later layer deduplicates `SessionSearchHit`.

### BT-D-R4-09 — `memory_trace` defaults to an unlimited, serial N+1 evidence walk
- **File**: src/builtin_tools/memory_trace.rs:45-59, 132-221
- **Severity**: Medium
- **Category**: perf
- **Description**: The schema explicitly makes evidence unlimited by default. A profile-section trace expands section → sessions → all session raws → citing notes, then performs `sources_of` and `get_raws_by_ids` sequentially for every note. There is no note/evidence page limit before this work, no batching across notes, and no deduplication of repeated raw evidence. A large section or highly connected note graph can therefore issue hundreds/thousands of serialized SQLite calls and return 800 characters per evidence item into one tool result.
- **Evidence**:
  ```rust
  /// Maximum number of items to return. Caps the `evidence` list (default:
  /// unlimited)
  pub max_results: Option<usize>,

  for note in &notes {
      let raw_ids = self.db.sources_of(agent, note).await?;
      let fetched = self.db.get_raws_by_ids(agent, &raw_ids).await?;
      for rid in &raw_ids {
          evidence.push(EvidenceItem { /* up to 800 chars */ });
      }
  }
  if let Some(max) = args.max_results {
      evidence.truncate(max);
  }
  ```
- **Suggested fix**: Apply a bounded default and hard maximum before expansion, expose a stable cursor, deduplicate note/raw IDs, and batch source/raw lookups. Stop traversing as soon as the page is full instead of truncating only after all I/O and allocations have occurred.
- **Verification**: Traced all four `TraceKind` arms and the backend trait methods. Only write-decision rows have a default bound; evidence does not. No generic result budget prevents the database work already performed.

### BT-D-R4-10 — `recall_events` converts database/search failure into a false “no match”
- **File**: src/builtin_tools/recall_events.rs:108-148
- **Severity**: Medium
- **Category**: error-handling
- **Description**: Any `search_events` error is discarded with `unwrap_or_default`. The tool then emits a successful result and says no earlier events matched. A locked/corrupt SQLite store, failed session-ID conversion, or FTS error is therefore indistinguishable from “this never happened”, which is precisely the wrong failure direction for a continuity/recovery tool.
- **Evidence**:
  ```rust
  let hits: Vec<RecallEventsHit> = store
      .search_events(&session_id, &args.query, limit)
      .await
      .unwrap_or_default()
      .into_iter()
      .map(/* ... */)
      .collect();

  let note = if hits.is_empty() {
      Some(format!("No earlier events matched '{}' this session", args.query))
  } else { None };
  ```
- **Suggested fix**: Propagate the typed error as a tool error, or return an explicit `unavailable`/`search_failed` status with `notify_tool_result(..., false)`. Reserve the no-match message for a successful empty query result.
- **Verification**: Read `SessionEventStore::search_events` and its SQLite implementation; it returns real `SessionError` failures. This is the only production caller that discards that `Result`.

### BT-D-R4-11 — A duplicate-only `remember` batch is reported as a new durable write
- **File**: src/builtin_tools/remember.rs:233-250, 324-337, 414-473
- **Severity**: Medium
- **Category**: correctness
- **Description**: The documented `written` contract is “true only when this call actually changed MEMORY.md”. Store-side batch adds deliberately skip exact duplicates and still return `Ok(WriteOutcome)`. `RememberTool` treats every `Ok` as a landed write, sets `written: true`, attaches a destination receipt, records reason `written`, and says “Write saved”. A batch containing only existing entries therefore claims a new persistence event that did not semantically occur and resets the failure/idempotency ledger.
- **Evidence**:
  ```rust
  RememberArgs::Batch { operations } => {
      let ops: Vec<BatchOp> = operations.into_iter().map(Into::into).collect();
      self.store.apply_batch(&ops).await
  }
  // ... every Ok reaches here
  clear_failures(&key);
  outcome.message = format!("{} Write saved — do not repeat this write.", outcome.message);
  Ok(Decided {
      output: self.output(outcome, true),
      reason: MemoryWriteReason::Written,
  })
  ```
- **Suggested fix**: Have `apply_batch` return `changed_count`/`changed: bool`. For an all-no-op batch, return `written: false`, no destination receipt, and an idempotent/duplicate reason; only clear the failure streak and record `Written` when final content differs.
- **Verification**: Read `memory/curated/store.rs:apply_one`, whose duplicate-add arm returns `Ok(())` without pushing, and `apply_batch`, which does not report whether any operation changed state. No tool-side comparison exists.

### BT-D-R4-12 — Scratchpad bindings are changed before the corresponding file operation succeeds
- **File**: src/builtin_tools/scratchpad.rs:571-625, 627-779
- **Severity**: High
- **Category**: correctness
- **Description**: The session→project registry is mutated before dispatching the requested scratchpad operation. A failed `initialize`, `set_plan`, `start_item`, approval read, or other mutation leaves the session rebound to a project it did not successfully touch. More dangerously, `clear` removes the binding before `manager.clear()`; if the file write fails, the unfinished plan remains on disk but the goal verifier can no longer find it, allowing the session to stop as though the plan were retired.
- **Evidence**:
  ```rust
  if !session_key.is_empty() {
      match args.action {
          ScratchpadAction::Read => {}
          ScratchpadAction::Clear => scratchpad_registry::clear(&session_key),
          _ => scratchpad_registry::set_active(&session_key, &project_id),
      }
  }
  let manager = ScratchpadManager::new(&project_id, &session_key);

  match args.action {
      ScratchpadAction::StartItem => manager.start_item(index).await?,
      // ...
      ScratchpadAction::Clear => manager.clear().await?,
  }
  ```
- **Suggested fix**: Commit the file operation first, then update/clear the registry only after success. For actions that create a new binding, retain the previous binding and restore it on any later failure; ideally expose a helper that commits “file state + binding state” as one ordered operation.
- **Verification**: Read every action arm and `ScratchpadGoalVerifier`'s `session_plan` resolution path. The `?` operators return immediately after the registry mutation; there is no rollback or compensating write.

### BT-D-R4-13 — Scratchpad registry persistence and last-owner deletion are not linearizable
- **File**: src/builtin_tools/scratchpad_registry.rs:97-126, 148-174
- **Severity**: High
- **Category**: concurrency
- **Description**: `set_active`/`clear` mutate the in-memory map and clone a snapshot under the lock, but write that snapshot after releasing the lock. Two concurrent calls can therefore persist in reverse order: the stale earlier snapshot can finish last and erase the newer binding on disk, while memory remains correct until restart. The purge path has a second race: it decides that no session shares a project, releases the lock, then awaits file deletion. A concurrent `set_active` can bind a new session to that project in the gap; purge then deletes a plan that once again has a live owner.
- **Evidence**:
  ```rust
  let snapshot = {
      let mut map = active_lock();
      map.insert(session_key.to_string(), project_id.to_string());
      map.clone()
  };
  persist(&snapshot); // no serialization with a later snapshot
  ```
  ```rust
  let (project_id, still_shared, snapshot) = {
      let mut map = active_lock();
      let Some(project_id) = map.remove(session_key) else {
          return;
      };
      let still_shared = map.values().any(|p| *p == project_id);
      let snapshot = map.clone();
      (project_id, still_shared, snapshot)
  };
  persist(&snapshot);
  if still_shared {
      return;
  }
  if let Err(e) = ScratchpadManager::new(&project_id, session_key)
      .purge()
      .await
  {
      tracing::warn!(error = %e, "failed to purge session scratchpad");
  }
  ```
- **Suggested fix**: Serialize mutation plus persistence through one async commit mutex/actor and use a monotonically increasing generation so stale snapshots cannot win. For purge, reserve/tombstone the project under that same owner table, delete only if the generation/reference count is unchanged, then finalize; a new binder must either cancel the tombstone or wait.
- **Verification**: Enumerated all writers (`set_active`, `clear`, epoch pruning, purge). They all persist cloned whole-map snapshots without a shared write-order guard. Session deletion and scratchpad calls can execute concurrently on different tasks.

### BT-D-R4-14 — `append_note` grows every scratchpad forever and rewrites it on each call
- **File**: src/builtin_tools/scratchpad.rs:754-766
- **Severity**: Medium
- **Category**: resource
- **Description**: The public action forwards arbitrary note text to an append implementation with no count or byte limit. The manager explicitly documents the Notes section as “unbounded (one line per call, forever)”; each append reads and atomically rewrites the entire markdown file, and `read` parses/returns that growing document. Long-lived projects therefore accumulate permanent disk, allocation, parsing, and rewrite cost even though downstream output truncation only hides older notes from the model.
- **Evidence**:
  ```rust
  ScratchpadAction::AppendNote => {
      let note = args.value.unwrap_or_default();
      manager.append_note(&note).await?;
      Ok(ScratchpadOutput {
          success: true,
          message: "Note appended".to_string(),
          // ...
      })
  }
  ```
- **Suggested fix**: Enforce both per-note and per-scratchpad byte/count limits. Keep the newest bounded notes in the hot scratchpad and rotate/archive older notes to a separate history file (or reject with an actionable message); report pruning explicitly.
- **Verification**: Read `memory/scratchpad/manager.rs:append_note`, which prepends to the full current document and rewrites it, with an explicit comment that growth is forever. No other lifecycle compacts Notes; `clear` is the only reset.

### BT-D-R4-15 — Polling a completed process is not durably recorded, so restart re-announces it
- **File**: src/builtin_tools/process_registry.rs:386-425
- **Severity**: High
- **Category**: correctness
- **Description**: An authorized terminal `poll`/`wait` sets only the in-memory `reported` flag. The proactive announcer checks that flag and returns early when the model already collected the result, but the early-return path does not invoke its durable `on_delivered` callback. The journal row remains `announced=false`; a restart within the one-hour handback window classifies it as undelivered and announces the result again, spending a new model turn and potentially repeating follow-up work.
- **Evidence**:
  ```rust
  ProcState::Done(out) => {
      let out = out.clone();
      entry.reported = true;
      PollOutcome::Done(out)
  }
  ```
  The delivery gate:
  ```rust
  if already_delivered() {
      debug!("result already collected on demand; skipping proactive delivery");
      return; // on_delivered / record_announced is not called
  }
  ```
- **Suggested fix**: After an authorized terminal read, call `process_journal::record_announced(id)` (outside the registry lock), or make the delivery helper invoke the durable acknowledgement callback when `already_delivered()` is true. Keep “reported/announced/consumed” as one state transition rather than parallel volatile and durable notions.
- **Verification**: Searched all `record_announced` call sites: only the process-announcer success callback and boot reconcile write it. Unlike `BackgroundAgentTracker::mark_consumed`, `ProcessRegistry::poll` has no persistence call.

### BT-D-R4-16 — Boot recovery stamps a completion delivered before it is broadcast
- **File**: src/builtin_tools/process_journal.rs:390-443, 472-493
- **Severity**: High
- **Category**: correctness
- **Description**: `init_and_reconcile` rewrites every fresh undelivered completion with `announced=true` and only then stashes it for asynchronous broadcast. A crash between that state write and `init_and_announce`'s broadcast permanently loses the notice. The same loss occurs when the owner label cannot be parsed or the downstream announcement cannot reach an agent: the loop skips/fails after the row is already stamped. This is an at-most-once outbox ordering bug in code whose stated purpose is to recover notices lost to a prior crash.
- **Evidence**:
  ```rust
  } else if is_undelivered_completion(&record, now) {
      let delivered = JobRecord {
          announced: true,
          ..record
      };
      write_state(&dir, &delivered);
      undelivered.push(delivered.clone());
      index.insert(delivered.id, delivered);
  }
  // later, in init_and_announce:
  for job in take_undelivered_settled() {
      let Some(session) = session_key_from_label(&job.record.owner) else {
          continue;
      };
      process_completion::broadcast(&session, event).await;
  }
  ```
- **Suggested fix**: Persist a `delivery_pending` state, enqueue/broadcast it, and set `announced=true` only through the normal announcer's successful `on_delivered` callback. If a crash occurs first, replay pending rows on the next boot; duplicate-visible delivery is safer than permanent silent loss. Use an outbox/event ID if exactly-once effects matter.
- **Verification**: Read server boot ordering: the subscriber is installed first, but that does not protect against process death or downstream delivery failure after the pre-stamp. `take_undelivered_settled` is destructive and the disk row is already final.

### BT-D-R4-17 — The process registry has no global running-process ceiling
- **File**: src/builtin_tools/process_registry.rs:61-74, 252-304, 665-680
- **Severity**: High
- **Category**: resource
- **Description**: `MAX_ENTRIES=64` is not an enforced cap. Eviction only removes a finished entry; when all entries are running there is no victim, yet registration still inserts. The only admission limit is eight running jobs per session, so creating many sessions permits unbounded detached tasks, live-tail buffers, journal writes, and real OS children. Once the table exceeds 64, `evict_if_needed` removes at most one entry per future registration, so it does not promptly restore the bound even after jobs finish.
- **Evidence**:
  ```rust
  const MAX_ENTRIES: usize = 64;
  const MAX_RUNNING_PER_SESSION: usize = 8;
  // ... count only entries with the same session_label ...
  if running >= MAX_RUNNING_PER_SESSION { return TooManyRunning { /* ... */ }; }
  evict_if_needed(&mut procs);
  procs.insert(id, ProcEntry { state: ProcState::Running, /* ... */ });
  ```
  ```rust
  let victim = procs.iter()
      .filter(|(_, e)| !matches!(e.state, ProcState::Running))
      .min_by_key(/* ... */);
  if let Some(id) = victim { procs.remove(&id); }
  ```
- **Suggested fix**: Add a process-wide `MAX_RUNNING_TOTAL` admission gate under the same lock and reject before spawning/starting execution when it is reached. Enforce retained-entry size with a loop/TTL on terminal entries so the table is restored to `<= MAX_ENTRIES`, not reduced by one opportunistically.
- **Verification**: Read the sole production registration path in `bash_exec`: every accepted registry slot corresponds to a detached task and child command. There is no daemon-wide semaphore or alternate global cap.

### BT-D-R4-18 — Partial output knowingly leaks prefixes of secrets split at the read frontier
- **File**: src/builtin_tools/partial_output.rs:19-34, 62-99
- **Severity**: Medium
- **Category**: security
- **Description**: The gate scans only bytes currently present in the live snapshot. If a credential/private token is emitted in multiple writes, a poll or 15-second journal flush can expose and durably store the first fragment before the rest arrives and makes the detector match. The completed path later rejects the whole output, but the leaked prefix has already reached a model turn or disk. This directly violates the stronger invariant immediately above the documented residual (“nothing durable ... may contain bytes the finished path would refuse”).
- **Evidence**:
  ```rust
  //! The gate sees only the bytes read *so far*, so a secret straddling the
  //! current read frontier can still leak its **prefix** — the pattern cannot
  //! match a value whose second half has not arrived.

  pub(crate) fn gate(snapshot: &LiveSnapshot) -> PartialView {
      let mut probe = SandboxOutput {
          stdout: snapshot.stdout.clone(),
          stderr: snapshot.stderr.clone(),
          ..Default::default()
      };
      if !scrub_and_gate_output(&mut probe).is_empty() { /* withhold */ }
      // otherwise return all currently visible bytes
  }
  ```
- **Suggested fix**: Hold back an overlap suffix from every running snapshot (sized to the maximum detector pattern/secret window, with a conservative fixed ceiling if necessary). Release it only after later bytes prove it safe or at terminal scanning; never persist the unconfirmed suffix. Maintain separate overlap state per stdout/stderr stream.
- **Verification**: Traced all three live consumers: `poll`/`wait`, kill/shutdown capture, and the periodic journal flusher all call this same gate, so the frontier issue affects both rendered and durable paths. Existing tests only cover a complete private-key payload in one snapshot.

### BT-D-R4-19 — `session_new` returns success even when close or creation failed
- **File**: src/builtin_tools/sessions/new_tool.rs:101-132
- **Severity**: High
- **Category**: error-handling
- **Description**: Both state transitions discard errors after logging. The tool always reports a new conversation. If close succeeds and `get_or_create` fails, the user is left with a closed old session and no replacement; if close fails and creation succeeds, the old session remains live despite the response saying it was closed. A parse failure of the legacy key is also silently skipped after the routing key parsed successfully.
- **Evidence**:
  ```rust
  if let Some(ref lk) = legacy_key {
      if let Err(e) = self.session_store.close_session(lk, args.topic.as_deref()).await {
          warn!("session_new: failed to close old session: {}", e);
      }
  }

  if let Err(e) = self.session_store.get_or_create(&new_routing_key).await {
      warn!("session_new: failed to create new session: {}", e);
  }

  Ok(SessionNewOutput { message: "新对话已开始...", /* ... */ })
  ```
- **Suggested fix**: Create/verify the next-epoch session first, then close the old one; propagate either failure and compensate (delete the newly created row if closing must be all-or-nothing). Return success only after both transitions are confirmed, and make the routing/legacy key conversion fail loudly.
- **Verification**: Read both session-store implementations and the `/new`-adjacent paths. Both operations return meaningful `Result`s, and no later caller checks whether the output's `new_session_key` actually exists.

### BT-D-R4-20 — `session_send` silently redirects DM/group sends into a private legacy `peer:` thread
- **File**: src/builtin_tools/sessions/send_tool.rs:657-708
- **Severity**: High
- **Category**: correctness
- **Description**: The conversion used for execution, history lookup, and concurrency claims discards the target DM/group channel and DM scope. It turns current keys such as `agent:main:telegram:dm:user123` into `agent:main:peer:user123`, a different session that inbound routing never writes. The send reports the original target key while the delegated run and reply actually land in a seam-private legacy thread. Group keys collapse identically. The source itself labels this as known, tracked drift; it remains active production behavior.
- **Evidence**:
  ```rust
  fn session_key_to_gateway(key: &routing::SessionKey) -> SessionKey {
      match key {
          SessionKey::DirectMessage { agent_id, peer_id, .. } =>
              GatewaySessionKey::peer(agent_id.clone(), peer_id.clone()),
          SessionKey::Group { agent_id, peer_id, .. } =>
              GatewaySessionKey::peer(agent_id.clone(), peer_id.clone()),
          // ...
      }
  }
  ```
- **Suggested fix**: Use one shared protocol session-key type end to end and preserve channel, peer kind, DM scope, and epoch. The visibility check, concurrency claim, `RunRequest`, reply history lookup, and returned `session_key` must all use exactly the same canonical key.
- **Verification**: Read `routing::SessionKey::to_key_string`, gateway `SessionKey::peer`, and inbound session construction. The inline `KNOWN DRIFT` comment precisely confirms the divergent storage key and resulting private thread; no downstream alias/reconciliation maps them back.

### BT-D-R4-21 — `session_send`'s outer timeout drops execution before engine cleanup
- **File**: src/builtin_tools/sessions/send_tool.rs:472-540, 620-637
- **Severity**: High
- **Category**: resource
- **Description**: Wait mode wraps `ExecutionAdapter::execute` in `tokio::time::timeout` while also putting the same timeout into `RunRequest.timeout_secs`. The outer timer starts earlier, so on a long run it can win first and cancel `execute` by dropping its future. The timeout branch never calls `cancel(run_id)` or waits for cleanup. In `SimpleExecutionEngine`, cleanup (agent→Idle, final run state, session-idle marker, active-run removal) occurs only after the inner execution future returns; dropping it skips all of that. Other adapters may instead leave an internally spawned run executing, so either outcome is unsafe: stranded busy state or work continuing after a reported timeout.
- **Evidence**:
  ```rust
  let request = RunRequest {
      run_id: run_id.clone(),
      timeout_secs: Some(u64::from(args.timeout_seconds)),
      // ...
  };

  let execution_result = tokio::time::timeout(
      timeout_duration,
      execution_adapter.execute(request, target_agent.clone(), emitter),
  )
  .await;

  Err(_) => SessionsSendOutput::timeout(run_id, target_key_str)
  ```
- **Suggested fix**: Prefer the engine-owned `RunRequest.timeout_secs` and await `execute` normally. If a caller-side deadline is still required, make it longer than the engine deadline; on expiry invoke `cancel(run_id)`/`cancel_session`, then await a terminal status or cleanup acknowledgement before returning `Timeout`.
- **Verification**: Read `SimpleExecutionEngine::execute`: agent/session/run cleanup is after its internal `select!`, with no drop guard. Searched `send_tool.rs`; neither the primary timeout nor auto-continuation timeout invokes `ExecutionAdapter::cancel`.

### BT-D-R4-22 — Fetch-provider SSRF validation discards the required DNS pin
- **File**: src/builtin_tools/web_fetch/mod.rs:140-177
- **Severity**: High
- **Category**: security
- **Description**: The provider path correctly recognizes an operator-hosted crawler as a confused deputy, calls `validate_url_async`, and then deliberately discards the returned pinned address before handing the original URL to a different HTTP client/service. The SSRF API contract says the pin **must** be used to close DNS-rebinding TOCTOU. An attacker-controlled public hostname can validate to a public IP, then resolve differently for the LAN-hosted crawler; a public redirect can likewise make that crawler fetch loopback, cloud metadata, or another internal service. Built-in `safe_fetch` protects redirects/pinning, but it is bypassed whenever a provider succeeds.
- **Evidence**:
  ```rust
  validate_url_async(&args.url, &self.ssrf_policy)
      .await
      .map(|(_, _pinned)| ())
      .map_err(/* ... */)?;

  for provider in &self.fetch_providers {
      match provider.fetch(&args.url).await {
          Ok(markdown) => { /* trust provider result */ }
          // ...
      }
  }
  ```
- **Suggested fix**: Do not treat center-side pre-validation as provider-side SSRF protection. Require each provider transport to enforce the same policy on every resolution and redirect, or route the target fetch through `safe_fetch` and give the provider content rather than a URL. For a remote crawler where center-side pinning cannot be applied, disable arbitrary URL delegation unless that service exposes an enforceable SSRF/pinning contract.
- **Verification**: Read `security/ssrf::validate_url_async` documentation (“pinned SocketAddr MUST be used”), `safe_fetch` redirect handling, and both provider clients. Crawl4AI/Firecrawl receive the original URL and resolve/follow it independently; neither accepts the validated pin or Aleph's policy.

### BT-D-R4-23 — Concurrent `mcp_login` flows invalidate each other after already returning authorization URLs
- **File**: src/builtin_tools/mcp_login.rs:126-194
- **Severity**: High
- **Category**: concurrency
- **Description**: Every login uses `CallbackServer::new()` with the same fixed loopback port. The listener is not bound before the tool returns the authorization URL; binding happens later inside the detached task. A second login (even for a different MCP server) therefore returns a URL and then loses the port race in the background, visible only in logs. For the same server, `start_authorization` also overwrites the single stored `oauth_state` and PKCE verifier, so the first callback fails state validation even if it owns the listener. There is no pending-flow registry, rejection, completion event, or status handle.
- **Evidence**:
  ```rust
  let callback = CallbackServer::new();
  let provider = OAuthProvider::new(
      storage.clone(), &server_id, &url, callback.callback_url()
  );
  let authorization_url = provider
      .start_authorization(&metadata, &client_info.client_id, args.scope.as_deref())
      .await?;

  tokio::spawn(async move {
      match callback.wait_for_callback().await {
          Ok(cb) => provider.finish_authorization(/* ... */).await,
          Err(e) => tracing::warn!(/* background-only failure */),
      }
  });
  Ok(McpLoginOutput { authorization_url, /* ... */ })
  ```
- **Suggested fix**: Bind a loopback listener synchronously before generating/returning the URL, preferably on an OS-assigned port, and store pending flows by a unique flow nonce. Reject or replace an existing same-server flow explicitly. Return a flow ID and publish/query terminal success/failure so callback bind, token exchange, and restart errors are observable.
- **Verification**: Read `mcp/auth/callback.rs` (`DEFAULT_CALLBACK_PORT=19877`; bind occurs in `wait_for_callback`) and `provider.rs` (one `oauth_state`/`code_verifier` pair per server entry). The detached task's `JoinHandle` is discarded.

### BT-D-R4-24 — Google Meet control is global and carries no caller/call ownership
- **File**: src/builtin_tools/google_meet.rs:96-172, 198-225, 256-284
- **Severity**: High
- **Category**: security
- **Description**: `leave`, `speak`, and `status` operate on “the active call”, but arguments contain neither an opaque call ID nor session/caller identity. The JSON-RPC request sends only those arguments under one process-wide bridge URL/static bearer token. The tool is always registered and is not in `OPERATOR_TOOLS`, so one user/session can speak into or terminate a call started by another, and the bridge has no attribution with which to reject it. Concurrent joins/creates are also globally ambiguous.
- **Evidence**:
  ```rust
  pub struct GoogleMeetArgs {
      pub action: GoogleMeetAction,
      pub meeting: Option<String>,
      pub transport: Option<GoogleMeetTransport>,
      pub mode: Option<GoogleMeetMode>,
      pub text: Option<String>,
  }

  let request = serde_json::json!({
      "jsonrpc": "2.0",
      "id": 1,
      "method": args.action.rpc_method(),
      "params": args,
  });
  ```
- **Suggested fix**: Have `join`/`create` return an opaque call handle bound server-side to the current session/principal; require that handle for `speak`/`leave`/status and pass authenticated caller/session attribution to the bridge. If the bridge intentionally supports only one global call, operator-gate every mutating action rather than exposing it to chat-tier users.
- **Verification**: Read registry construction/dispatch: one `GoogleMeetTool`/bridge is shared, the tool is advertised as always available, and `gateway/method_authz.rs` does not gate `google_meet`. No downstream request field restores ownership.

### BT-D-R4-25 — `node_file`'s size and no-overwrite guarantees are TOCTOU; pull writes are non-atomic
- **File**: src/builtin_tools/node_file.rs:107-130, 172-220
- **Severity**: High
- **Category**: correctness
- **Description**: On push, the tool checks metadata size and then reads the path without bounding or rechecking the bytes; a file that grows or is swapped between those operations bypasses the 8 MiB cap and can force a large allocation/base64 payload. On pull, path validation and `exists()`/`overwrite` checking happen before parent creation and a direct `tokio::fs::write`. There is no path lock, no `create_new`/no-follow commit, and no atomic temp-file rename. A concurrent creator can be overwritten despite `overwrite=false`; a final-component symlink swap can redirect the write after the deny check; and I/O failure can leave an existing destination truncated or partially written.
- **Evidence**:
  ```rust
  let meta = tokio::fs::metadata(&local).await?;
  if meta.len() > MAX_FILE_BYTES as u64 { return Err(/* ... */); }
  let bytes = tokio::fs::read(&local).await?; // no post-read cap
  ```
  ```rust
  let local = check_and_resolve_path(/* ... */)?;
  if local.exists() && !args.overwrite {
      return Err(AlephError::tool("local target exists (set overwrite)"));
  }
  if let Some(parent) = local.parent() {
      tokio::fs::create_dir_all(parent).await?;
  }
  tokio::fs::write(&local, &bytes).await?;
  ```
- **Suggested fix**: For push, open once and read through a bounded stream (`MAX_FILE_BYTES + 1`) from that handle, rejecting excess before encoding. For pull, take the shared per-path lock, revalidate at commit time, stage to a same-directory temp file, fsync, and atomically rename; implement `overwrite=false` with a no-clobber/create-new primitive and reject symlink final components.
- **Verification**: Compared this path with chunk-D's reviewed `file_ops` writers, which hold per-path locks and use `atomic_write_file`. `node_file` calls neither. The remote payload is hash-verified correctly; the defect is the local filesystem commit window, not transport integrity.

## Cross-cutting concerns
1. **Missing identity is being used as privilege.** `memory_search` and `session_search` both have good per-user predicates for normal turns, but actor-less external A2A work is classified with cron/internal maintenance and becomes unrestricted. A distinct, explicit system capability is safer than `None == superuser`.
2. **Several memory tools retain construction-time principals.** `remember`, `memory_trace`, and `flag_user_correction` have moved to per-call composed identities, while `session_complete`, `memory_reflect`, and `memory_timeline` still carry no trustworthy turn principal. This uneven migration is the source of both wrong-context recall and cross-agent writes.
3. **Volatile state and durable state are not one transition.** Scratchpad file/binding changes and process reported/announced changes each have two writers with a failure gap. These should be serialized state machines/outboxes rather than “update A, then best-effort update B”.
4. **Read-side limits are inconsistent.** Session search, memory trace, and scratchpad Notes can amplify model-controlled input into global scans, repeated LLM synthesis, or permanent growth. Generic result truncation is not a substitute for bounding work before I/O/allocation.
5. **External capability bridges lack end-to-end trust binding.** The web provider loses DNS pinning, MCP OAuth flows share a callback endpoint/state slot, Google Meet has no call owner, and node transfer validates before a non-atomic local commit. Validation must travel with the operation to the final side-effect point.
6. **Fail-soft often becomes false absence/success.** `recall_events`, `session_new`, and capture-filtered `session_complete` turn operational failure or policy refusal into “nothing matched” / “started” / “recorded”. For memory and session lifecycle, unknown/error should be visible and fail closed.

## Summary
- Total: 25 findings (0 Critical, 17 High, 8 Medium, 0 Low)
- Top priority items:
  1. **BT-D-R4-07** — actor-less A2A runs can globally search and quote every user's transcript; add explicit principal propagation and fail-closed transcript authorization.
  2. **BT-D-R4-22** — provider-backed `web_fetch` discards the SSRF DNS pin and delegates redirects/resolution to a LAN crawler; the stated SSRF protection is bypassable.
  3. **BT-D-R4-21** — `session_send` drops the execution future at its outer timeout without cancellation/cleanup, stranding session/run state or allowing work to continue after a timeout response.

## What was NOT covered
- No cargo check, clippy, tests, fuzzing, scripts, network calls, OAuth flows, race reproduction, benchmarks, or process spawning were performed, per the read-only static-review constraint.
- `file_ops/` was read completely but overlap findings were not re-reported; chunk A owns those duplicates.
- Other `src/builtin_tools/` files and unrelated module bodies were not re-audited. Cross-module files were read only far enough to verify the scoped tools' production contracts/callers.
- External Google Meet/Crawl4AI/Firecrawl/MCP server implementations were not available for review; findings concern guarantees the Rust caller demonstrably does not carry to those boundaries.
- Artifact-store/export renderer cryptographic signing, filesystem durability below `ArtifactStore::put`, cron scheduler internals, SQLite locking behavior under real load, and OS-specific symlink/no-clobber primitives were not independently validated.
- The 111 MB semantic graph was not loaded wholesale; source call sites and targeted repository searches were used instead. No runtime reachability claim beyond those verified call paths is made.
