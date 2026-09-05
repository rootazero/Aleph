//! Session modification handlers (reset, delete, patch, compact, `set_topic`).

use crate::sync_primitives::Arc;
use serde_json::{json, Value};

/// Drop every artifact a deleted session produced.
///
/// Best-effort, exactly like [`fire_session_end_hook`]: the transcript is
/// already gone, so failing the RPC here would report a deletion that in fact
/// happened. The handler only says *which* session died — where the bytes live
/// and how they are reclaimed stays inside `crate::artifacts` (R4).
async fn purge_session_artifacts(session_key: &str) {
    let root = match crate::artifacts::ArtifactStore::default_root() {
        Ok(root) => root,
        Err(e) => {
            tracing::warn!(error = %e, "cannot resolve the artifact root; artifacts not purged");
            return;
        }
    };
    if let Err(e) = crate::artifacts::ArtifactStore::new(root)
        .purge_session(session_key)
        .await
    {
        tracing::warn!(error = %e, session_key, "failed to purge session artifacts");
    }
}

/// Drop the resume snapshot a deleted session left on disk.
///
/// `resume.json` holds an LLM-authored summary of the conversation, and the
/// assembler injects the agent's newest one into the NEXT session's prompt as
/// "previous session" context. A user who deletes a conversation *because of
/// what was in it* must not have it read back to them one session later. The
/// session key is stable, so a session deleted today typically already has a
/// snapshot from an earlier close of the same key — that is the one this
/// removes. (The delete path no longer manufactures a *fresh* one: it goes
/// through `emit_session_end_raw_without_resume`, because that write lands long
/// after this handler returns and no purge here could catch it.)
///
/// Best-effort, like [`purge_session_artifacts`]. `remove_for_session` owns the
/// id→directory-name mapping; the call site must not re-derive it.
async fn purge_session_snapshot(session_key: &str) {
    let Some(writer) = crate::memory::session_resume::SnapshotWriter::default_path() else {
        return;
    };
    let key = session_key.to_string();
    // Blocking file work (a `remove_dir_all`) — keep it off the async worker,
    // same as the writer's own call site.
    let removed = tokio::task::spawn_blocking(move || writer.remove_for_session(&key)).await;
    match removed {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            tracing::warn!(error = %e, session_key, "failed to purge session resume snapshot");
        }
        Err(e) => {
            tracing::warn!(error = %e, session_key, "resume snapshot purge task failed");
        }
    }
}

/// Fire the `SessionEnd` extension hook (observers) for a deleted session.
///
/// Best-effort: a missing extension manager, an empty hook set, or a hook
/// failure is ignored — session deletion must never depend on hook
/// availability. Lives here (not in `src/harness/`) so the dumb loop stays
/// free of lifecycle logic (R10).
async fn fire_session_end_hook(session_key: &crate::gateway::router::SessionKey) {
    let Ok(manager) = crate::gateway::handlers::plugins::get_extension_manager() else {
        return;
    };
    let executor = manager.hook_executor_snapshot().await;
    if executor.hook_count() == 0 {
        return;
    }
    let ctx = crate::extension::hooks::HookContext::new(session_key.to_key_string())
        .with_env("AGENT_ID", session_key.agent_id());
    executor
        .execute_observers(crate::extension::HookEvent::SessionEnd, &ctx)
        .await;
}

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::gateway::router::SessionKey;
use crate::gateway::session_store::SessionStore;
use crate::gateway::visibility;

/// Handle sessions.reset RPC request with database backend
pub async fn handle_reset_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let session_key_str = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key_str {
        Some(key_str) => {
            let session_key = match SessionKey::from_key_string(key_str) {
                Some(k) => k,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Invalid session_key format",
                    );
                }
            };

            let meta = match manager.get_metadata(&session_key).await {
                Ok(Some(m)) => m,
                Ok(None) => return visibility::not_found_response(request.id),
                Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
            };
            if !visibility::session_visible(&meta) {
                return visibility::not_found_response(request.id); // same error as missing (GC 4)
            }

            // Retire the SSOT event log before the `messages` projection —
            // same ordering rationale as `chat.clear`: resetting only the
            // projection leaves the model replaying the cleared conversation.
            let retired = match crate::session::store::retire_live_events(&session_key, 1).await {
                Ok(n) => n,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to reset session event log: {e}"),
                    );
                }
            };

            // Same reasoning as `chat.clear`: the side session holds a
            // copied prefix of this transcript, so a reset that spares it
            // leaves the cleared content readable through the next `/btw`.
            // Side session only — the key is unchanged, so the loop/goal
            // chains keyed to it are still reachable and must survive.
            crate::gateway::continuation_lifecycle::retire_side_session(
                &session_key,
                "sessions.reset",
                Some(manager.clone()),
            );

            match manager.reset_session(&session_key).await {
                Ok(reset) => JsonRpcResponse::success(
                    request.id,
                    json!({
                        "session_key": key_str,
                        "reset": reset || retired > 0,
                        "events_retired": retired,
                    }),
                ),
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to reset session: {e}"),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Handle sessions.delete RPC request with database backend.
///
/// Before dropping the transcript we capture its tail into `raw_memories`
/// as a `SessionEnd` raw so the `CompressionService` / `ProfileSynthesizer`
/// can mine durable knowledge from the dying session. Without this hook
/// `USER.md` never updates and per-session digests are silently lost.
pub async fn handle_delete_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    handle_delete_db_inner(request, manager, None).await
}

/// Variant accepting an explicit raw-memory writer for the `SessionEnd` capture.
/// The default `handle_delete_db` keeps the writer optional for backwards
/// compatibility with the macro-generated registration shape.
pub async fn handle_delete_db_with_capture(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
    writer: Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>,
) -> JsonRpcResponse {
    handle_delete_db_inner(request, manager, Some(writer)).await
}

async fn handle_delete_db_inner(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
    writer: Option<Arc<dyn crate::memory::store::raw_memory::RawMemoryStore>>,
) -> JsonRpcResponse {
    let session_key_str = match &request.params {
        Some(Value::Object(map)) => map.get("session_key").and_then(|v| v.as_str()),
        _ => None,
    };

    match session_key_str {
        Some(key_str) => {
            let session_key = match SessionKey::from_key_string(key_str) {
                Some(k) => k,
                None => {
                    return JsonRpcResponse::error(
                        request.id,
                        INVALID_PARAMS,
                        "Invalid session_key format",
                    );
                }
            };

            let meta = match manager.get_metadata(&session_key).await {
                Ok(Some(m)) => m,
                Ok(None) => return visibility::not_found_response(request.id),
                Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
            };
            if !visibility::session_visible(&meta) {
                // Same error as missing (GC 4) — and returning here, before any
                // of the mutations below, is what keeps the foreign session
                // intact: deny must have no side effect.
                return visibility::not_found_response(request.id);
            }

            // Terminate the autonomous continuations FIRST — deleting the
            // transcript does not stop the loop/goal chains keyed to it. They
            // are process/DB state keyed by the session string, so without this
            // the deleted conversation keeps ticking: each tick re-enters the
            // post-run hook under a key whose session no longer exists, posting
            // to the origin channel until its cap runs out. (An operator can
            // still reach it with `loop(action='stop', session=…)`, but nothing
            // should require that after the user deleted the conversation.)
            //
            // The same call retires the `/btw` side session derived from this
            // key: the conversation it was a sidebar to is being deleted, and
            // nothing else in the tree would ever reach that row again.
            // Canonical spelling lives inside the seam now, like
            // `purge_session_artifacts` below: the registry and the goal store
            // are keyed by `to_key_string()`.
            crate::gateway::continuation_lifecycle::terminate_session_continuations(
                &session_key,
                "sessions.delete",
                Some(manager.clone()),
            );

            // Capture session tail BEFORE deletion so SessionEnd raw fires.
            if let Some(ref w) = writer {
                if let Ok(history) = manager.get_history(&session_key, Some(64)).await {
                    let tail = history
                        .iter()
                        .map(|m| format!("[{}] {}", m.role, m.content))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !tail.is_empty() {
                        // Review fix: fetch the row's P1 scope columns
                        // BEFORE the delete below removes it — same reason
                        // as `SessionManager::close_session`: the session-end
                        // reflector needs them to write OPEN_LOOPS.md under
                        // the same composed id the curated-envelope reader
                        // resolves, and the ambient scope task-local is not
                        // reliably live on this delete-RPC path.
                        let (owner_user_id, scope_id) = manager
                            .get_metadata(&session_key)
                            .await
                            .ok()
                            .flatten()
                            .map_or((None, None), |m| (m.owner_user_id, m.scope_id));
                        // Canonical spelling, not the caller's `key_str` — same
                        // rule as the two siblings in this function: the
                        // downstream snapshot store encodes the key straight
                        // into a directory name, so a legacy spelling would
                        // file this session's memory under a name nothing else
                        // ever looks up.
                        //
                        // The `_without_resume` entry point is the one that
                        // does NOT also write a `resume.json` of the
                        // conversation being deleted; the tail capture into
                        // `raw_memories` (this function's documented purpose)
                        // is unchanged.
                        crate::gateway::session_manager::ops::emit_session_end_raw_without_resume(
                            Arc::clone(w),
                            session_key.agent_id().to_string(),
                            session_key.to_key_string(),
                            tail,
                            crate::memory::store::raw_memory::SessionEndReason::Disconnect,
                            owner_user_id,
                            scope_id,
                        );
                    }
                }
            }

            // Retire the SSOT event log before the projection — same ordering
            // as `chat.clear`. Deleting the `messages` rows alone would leave
            // the conversation alive in `session_events`: still replayed by the
            // model, still searchable, and — when its run reads as interrupted
            // (`session::reduction::reduce_disposition`) — re-materialised into
            // a brand-new transcript by `ProjectionReconciler` at the next boot.
            //
            // Retire rather than physically purge, even though this is the
            // strongest deletion a user can perform: retirement already removes
            // the events from EVERY read path (replay, BM25 search — the FTS
            // mirror is physically dropped, run markers, reconciler), so no
            // content the user believes deleted can surface again. A hard purge
            // would be a second deletion mechanism for the same job with no
            // additional user-visible effect, and it would free seqs that a
            // re-created session under the same (stable) key could collide with.
            let retired = match crate::session::store::retire_live_events(&session_key, 1).await {
                Ok(n) => n,
                Err(e) => {
                    return JsonRpcResponse::error(
                        request.id,
                        INTERNAL_ERROR,
                        format!("Failed to delete session event log: {e}"),
                    );
                }
            };

            match manager.delete_session(&session_key).await {
                Ok(result) => {
                    // The transcript is gone; the bytes it produced must go
                    // with it. Without this the store keeps up to
                    // `MAX_ARTIFACTS_PER_SESSION` blobs per deleted session on
                    // disk forever, and a re-created session under the same
                    // (stable) key would inherit the dead session's artifacts.
                    //
                    // Canonical spelling, not the caller's `key_str`: the
                    // harvest points stamp `to_key_string()`, and the store
                    // encodes the key straight into a directory name — a
                    // legacy spelling would purge a directory that never
                    // existed and silently leave the real one behind.
                    purge_session_artifacts(&session_key.to_key_string()).await;
                    // Same for the resume snapshot: it is a summary OF this
                    // conversation, and the assembler feeds the newest one into
                    // the next session's prompt.
                    purge_session_snapshot(&session_key.to_key_string()).await;
                    // …and the scratchpad plan file, for the same reason. Done
                    // HERE rather than only inside `SessionManager` so it holds
                    // for every backend: the file backend never goes through
                    // the manager. The purge is idempotent, so the manager's own
                    // call (which covers deletes that never reach this handler)
                    // costs nothing when both run.
                    crate::builtin_tools::scratchpad_registry::purge_session_scratchpad(
                        &session_key.to_key_string(),
                    )
                    .await;
                    // SessionEnd — the session has been removed; extension
                    // observers witness the teardown.
                    fire_session_end_hook(&session_key).await;
                    JsonRpcResponse::success(
                        request.id,
                        json!({
                            "session_key": key_str,
                            "deleted": result.deleted || retired > 0,
                            "events_retired": retired,
                        }),
                    )
                }
                Err(e) => JsonRpcResponse::error(
                    request.id,
                    INTERNAL_ERROR,
                    format!("Failed to delete session: {e}"),
                ),
            }
        }
        None => JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    }
}

/// Every per-session knob `sessions.patch` may write, with the parser that
/// decides whether a value is a legal id for it.
///
/// The list is the contract: a knob missing from here can be written with any
/// junk at all, which persists, renders as an override to every client, and is
/// then dropped with a warn on each turn — the session runs at the global value
/// while every surface says otherwise.
///
/// `model_pin` / `model_pin_provider` are deliberately absent: their legal
/// values are the live provider catalog, not a closed set, and `select_model`
/// (the writer that has the catalog in hand) already refuses ids it knows are
/// retired. Validating them here against a snapshot of the catalog would refuse
/// models released after the binary was built — the failure mode
/// `refuse_unusable_model` was written to avoid.
type KnobValidator = fn(&str) -> bool;
fn knob_validators() -> [(&'static str, KnobValidator); 4] {
    [
        (crate::config::types::policies::EXEC_TIER_SESSION_KEY, |v| {
            crate::config::types::policies::ExecTier::from_id(v).is_some()
        }),
        (crate::config::types::policies::MODE_SESSION_KEY, |v| {
            crate::config::types::policies::SessionMode::from_id(v).is_some()
        }),
        (crate::agents::thinking::THINK_LEVEL_SESSION_KEY, |v| {
            crate::agents::thinking::normalize_think_level(v).is_some()
        }),
        (
            crate::memory::session_memory_mode::MEMORY_MODE_SESSION_KEY,
            |v| crate::memory::session_memory_mode::MemoryMode::from_id(v).is_some(),
        ),
    ]
}

/// Session-metadata keys this endpoint refuses rather than validates.
///
/// The model pin has **two** stores that must agree: the session row (durable,
/// restored at run start) and `providers::session_model_handle`'s process map
/// (what the run builder actually reads). `select_model` writes both, in that
/// order. A `sessions.patch` writing only the row would be silently ineffective
/// in a live process and take effect after the next restart — the shape of
/// defect where a setting "sometimes works", which is worse to diagnose than
/// one that never does.
///
/// Refusing is honest and cheap: no shipped client writes these, and the one
/// that would want to (a `/model` command) should call the tool.
const NOT_PATCHABLE: &[&str] = &[
    crate::providers::session_model_handle::MODEL_PIN_SESSION_KEY,
    crate::providers::session_model_handle::MODEL_PIN_PROVIDER_SESSION_KEY,
];

/// The first key in `bag` that this endpoint refuses to write at all.
fn first_foreign_knob(bag: &serde_json::Map<String, Value>) -> Option<&'static str> {
    NOT_PATCHABLE
        .iter()
        .copied()
        .find(|key| bag.contains_key(*key))
}

/// The first knob in `bag` carrying a value its own parser rejects.
///
/// `null` passes for every knob — that is how a client clears an override back
/// to "follow global". A non-string value fails: the bag is flat strings by
/// construction, and a number silently stored under `exec_tier` reads back as
/// no override at all.
fn first_invalid_knob(
    bag: &serde_json::Map<String, Value>,
) -> Option<(&'static str, &serde_json::Value)> {
    knob_validators().into_iter().find_map(|(key, valid)| {
        let v = bag.get(key)?;
        if v.is_null() {
            return None;
        }
        let ok = v.as_str().is_some_and(valid);
        (!ok).then_some((key, v))
    })
}

/// Handle sessions.patch RPC request with database backend
pub async fn handle_patch_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let meta = match manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4), and checked before any field
        // validation below — a foreign caller must get an identical response
        // regardless of what they put in `metadata`.
        return visibility::not_found_response(request.id);
    }

    // Both stores merge `metadata` opaquely into `identity_meta.custom`, so this
    // is the only place a knob written through `sessions.patch` can be checked.
    // An unknown id would persist, render as an override in the Panel, and then
    // be dropped with a warn at run time — every turn silently running at the
    // GLOBAL value, possibly weaker than the one the caller believes it armed.
    // `chat.send` already refuses unknown ids (handlers/agent.rs); the two write
    // paths must agree. `null` stays legal on every knob: it is how "follow
    // global" clears an override.
    //
    // One table rather than one `if let` per knob. The per-knob form is how the
    // family drifted in the first place: `exec_tier` and `session_mode` each got
    // their own block and `think_level` — persisted by `turn_thinking` since it
    // was written — got none, so junk could be stored on it and silently
    // dropped every turn. `every_session_knob_is_validated_on_patch` pins the
    // table against the constants rather than against a remembered list.
    if let Some(bag) = params.get("metadata").and_then(Value::as_object) {
        // Keys this endpoint must refuse outright, before validation: writing
        // them here would be accepted, persisted, and then ignored for the rest
        // of the process's life.
        if let Some(key) = first_foreign_knob(bag) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!(
                    "`{key}` is not writable through sessions.patch — the model pin's writer is \
                     `select_model`, which also updates the in-process map the run builder reads. \
                     A patch here would be honored only after a restart."
                ),
            );
        }
        if let Some((key, value)) = first_invalid_knob(bag) {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Unknown {key}: {value}"),
            );
        }
    }

    let patch = crate::gateway::session_manager::SessionPatch {
        label: params
            .get("label")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        status: params
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        model: params
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        model_provider: params
            .get("model_provider")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        metadata: params.get("metadata").cloned(),
    };

    match manager.patch_session(&session_key, &patch).await {
        Ok(updated) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "updated": updated,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to patch session: {e}"),
        ),
    }
}

/// Handle session.compact RPC request.
///
/// Drives the SAME `context::compact::manual::compact_session` the
/// `session_compact` tool does (via the shared `run_manual_compaction`), so
/// TUI `/compress`, CLI `aleph session compact`, Panel `/compact` and a model's
/// own tool call are one behaviour (R6).
///
/// The compaction operation itself still takes no `SessionStore`: it operates
/// on the session **event log** (the single source of truth the prompt is
/// rebuilt from), not on the `messages` read projection — that part of the
/// original doc still holds. `manager` here is ONLY the P1 visibility gate:
/// this RPC returns a summary of the addressed session's real conversation in
/// its response AND irreversibly rewrites that session's event log, so a
/// caller-supplied `session_key` belonging to someone else must be refused
/// before either happens.
pub async fn handle_compact_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let session_key = match request
        .params
        .as_ref()
        .and_then(|p| p.get("session_key"))
        .and_then(|v| v.as_str())
    {
        Some(k) => k.to_string(),
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };

    let session_key_typed = match SessionKey::from_key_string(&session_key) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let meta = match manager.get_metadata(&session_key_typed).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — before the summary is ever computed
        // or the event log rewritten.
        return visibility::not_found_response(request.id);
    }

    // `/compact <instructions>` (codex / pi / kimi-cli parity). The TUI passes
    // whatever followed the command; absent/blank keeps the default summary.
    let instructions = request
        .params
        .as_ref()
        .and_then(|p| p.get("instructions"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    // No per-call keep-budget override: the verbatim tail budget is an operator
    // setting (`[context_budget] manual_compact_keep_tokens`), and a knob with
    // no caller is a knob that drifts. Deliberately absent, not forgotten.
    match crate::builtin_tools::sessions::run_manual_compaction(
        &session_key,
        crate::context::compact::manual::ManualCompactOptions {
            instructions,
            keep_tokens: None,
        },
    )
    .await
    {
        Ok(outcome) => {
            let rendered = crate::builtin_tools::sessions::render_manual_compaction(&outcome);
            JsonRpcResponse::success(
                request.id,
                json!({
                    "message": rendered.message,
                    "compacted": outcome.events_compacted,
                    "kept": outcome.events_kept,
                    "tokens_before": outcome.tokens_before,
                    "tokens_after": outcome.tokens_after,
                    "tokens_saved": outcome.tokens_saved(),
                    "summary": outcome.summary,
                }),
            )
        }
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Compact failed: {e}"))
        }
    }
}

/// Handle session.truncate RPC request with database backend.
///
/// Removes messages from the tail of a session, keeping only the first
/// `keep_count` messages by chronological order. Used by the TUI `/undo`
/// command to drop the most recent user+assistant turn pair.
pub async fn handle_truncate_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
    run_manager: Option<Arc<crate::gateway::handlers::agent::AgentRunManager>>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key"),
    };

    let keep_count = match params.get("keep_count").and_then(|v| v.as_u64()) {
        Some(n) => n as usize,
        None => return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing keep_count"),
    };

    let key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let meta = match manager.get_metadata(&key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — before the irreversible tail
        // deletion below.
        return visibility::not_found_response(request.id);
    }

    // Retire the SSOT event log before the projection, exactly as
    // `sessions.reset`, `sessions.delete`, `chat.clear` and `chat.rewind` do.
    // `truncate_messages` touches `messages` / `messages_fts` /
    // `transcript.jsonl` and nothing else, and the model's prompt is rebuilt
    // from `session_events` — so on its own this verb removed the turn from
    // every screen while the model went on replaying it. `/retry` was worse:
    // it undoes, then re-sends, so the log grew [U, A, U] — the duplicated
    // turn its own comment claims the undo prevents.
    //
    // The boundary is derived from the row that is about to be dropped, not
    // from `keep_count`: `messages` is not a 1:1 image of the live event log
    // (boot-time orphan notices and other writers append rows with no source
    // event), so a count is an ordinal in the projection's index space, while
    // `retire_live_events` wants a seq in the log's. Taking the MINIMUM source
    // seq over every dropped row keeps the two halves describing the same cut
    // even when the dropped range straddles rows that carry no seq.
    let cut_seq = match manager.get_history(&key, None).await {
        Ok(rows) => rows
            .get(keep_count..)
            .unwrap_or_default()
            .iter()
            .filter_map(|m| {
                crate::session::projection::parse_source_seq(&m.id, &key.to_key_string())
            })
            .min(),
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Truncate failed: could not read history to place the cut: {e}"),
            );
        }
    };
    if let Some(seq) = cut_seq {
        // Fail the RPC rather than truncating half of it: a projection cut with
        // a surviving event log is precisely the state this handler shipped in,
        // and it reads to the user as a successful undo.
        if let Err(e) = crate::session::store::retire_live_events(&key, seq).await {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to retire session event log: {e}"),
            );
        }
        // The twin of `chat.rewind`'s balance: a cut that removed a
        // `RunFinished` and left its `RunStarted` makes the log say a run is
        // still open, and the boot scan then resumes a turn the user undid on
        // every later boot. Same helper, so the rule cannot exist on one verb
        // and not the other.
        crate::gateway::handlers::balance_run_markers_after_retire(&key, run_manager.as_ref())
            .await;
    }

    match manager.truncate_messages(&key, keep_count).await {
        Ok(result) => JsonRpcResponse::success(
            request.id,
            json!({
                "messages_removed": result.messages_removed,
                "tokens_removed_estimate": result.tokens_removed_estimate,
            }),
        ),
        Err(e) => {
            JsonRpcResponse::error(request.id, INTERNAL_ERROR, format!("Truncate failed: {e}"))
        }
    }
}

/// Handle `sessions.set_topic` RPC request with database backend
///
/// Params:
///   - `session_key` (required): session key string
///   - topic (required): new topic string (max 100 chars)
///
/// P1 visibility: KeyChecked, the same block [`handle_truncate_db`] runs, and
/// for the same reason — this is a cross-user **write**. The topic is the
/// title the Panel sidebar renders, so an unchecked caller could rename any
/// other user's conversation (defacement, and a channel into a surface the
/// victim reads). It was deferred once as "a title-rename side effect"; the
/// side effect lands in the victim's own UI.
pub async fn handle_set_topic_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    let topic = match params.get("topic").and_then(|v| v.as_str()) {
        Some(t) => t,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing topic");
        }
    };

    // Validate topic length (P7: boundary validation)
    let topic = if topic.len() > 100 {
        &topic[..topic
            .char_indices()
            .nth(100)
            .map_or(topic.len(), |(i, _)| i)]
    } else {
        topic
    };

    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let meta = match manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — before the rename below.
        return visibility::not_found_response(request.id);
    }

    match manager.set_topic(&session_key, topic).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "updated": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to set topic: {e}"),
        ),
    }
}

/// Handle `sessions.set_project_root` RPC request with database backend.
///
/// Params:
///   - `session_key` (required): session key string
///   - `project_root` (optional): absolute project folder; `null` / absent
///     clears the override (revert to the default agent workspace).
///
/// Persists the user-chosen working directory onto the session's identity
/// metadata so the Panel can restore it after a reload. Validation of the path
/// (absolute / exists / directory) happens at run time in the `agent.run`
/// handler — this RPC only records the preference, so a not-yet-existing folder
/// can be remembered without failing the call.
pub async fn handle_set_project_root_db(
    request: JsonRpcRequest,
    manager: Arc<dyn SessionStore>,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(Value::Object(map)) => map,
        _ => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params object");
        }
    };

    let session_key_str = match params.get("session_key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing session_key");
        }
    };

    // `project_root` is optional: a missing key or JSON null clears the override.
    let project_root = params
        .get("project_root")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty());

    let session_key = match SessionKey::from_key_string(session_key_str) {
        Some(k) => k,
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Invalid session_key format",
            );
        }
    };

    let meta = match manager.get_metadata(&session_key).await {
        Ok(Some(m)) => m,
        Ok(None) => return visibility::not_found_response(request.id),
        Err(_) => return visibility::not_found_response(request.id), // fail closed (GC 3)
    };
    if !visibility::session_visible(&meta) {
        // Same error as missing (GC 4) — before redirecting the foreign
        // session's next run to a caller-chosen path.
        return visibility::not_found_response(request.id);
    }

    match manager.set_project_root(&session_key, project_root).await {
        Ok(()) => JsonRpcResponse::success(
            request.id,
            json!({
                "session_key": session_key_str,
                "project_root": project_root,
                "updated": true,
            }),
        ),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to set project_root: {e}"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::session_manager::{SessionManager, SessionManagerConfig};
    use crate::session::events::{MessageContent, SessionEvent};
    use crate::session::store::SessionEventStore;
    use tempfile::tempdir;

    /// Census: every knob the attach snapshot reads back must either be
    /// validated on write, or be named here with a reason.
    ///
    /// Derived from `session_snapshot.rs`'s **source** rather than from a list
    /// remembered in this file, because a remembered list is the thing that
    /// went wrong: `exec_tier` and `session_mode` were each given a validation
    /// block, `think_level` was not, and the gap was invisible precisely
    /// because nothing enumerated the family in one place. Adding a knob to the
    /// snapshot now turns this red until it is either validated or excused.
    #[test]
    fn every_session_knob_is_validated_on_patch() {
        let snapshot_src = include_str!("../../../session_snapshot.rs");
        let production = crate::utils::source_scan::production_prefix(snapshot_src);

        // Constants the snapshot decodes, in the order it decodes them.
        let read_by_snapshot: Vec<&str> = [
            ("EXEC_TIER_SESSION_KEY", "exec_tier"),
            ("MODE_SESSION_KEY", "session_mode"),
            ("THINK_LEVEL_SESSION_KEY", "think_level"),
            ("MEMORY_MODE_SESSION_KEY", "memory_mode"),
            ("MODEL_PIN_SESSION_KEY", "model_pin"),
            ("MODEL_PIN_PROVIDER_SESSION_KEY", "model_pin_provider"),
            ("PROJECT_ROOT_SESSION_KEY", "project_root"),
        ]
        .into_iter()
        .filter(|(constant, _)| production.contains(constant))
        .map(|(_, wire)| wire)
        .collect();
        assert!(
            read_by_snapshot.len() >= 4,
            "the census found almost nothing — did `session_snapshot.rs` move? \
             a guard that scans the wrong text is green for the wrong reason"
        );

        // Knobs whose legal values are not a closed set, so a table here would
        // refuse values that work. Each needs its refusal to live with whoever
        // owns the value space.
        const OPEN_VALUE_SPACE: &[&str] = &[
            // An absolute path, checked by `sessions.set_project_root` (which
            // must hit the filesystem — a string test cannot).
            "project_root",
        ];

        let validated: Vec<&str> = knob_validators().iter().map(|(k, _)| *k).collect();
        for knob in read_by_snapshot {
            assert!(
                validated.contains(&knob)
                    || NOT_PATCHABLE.contains(&knob)
                    || OPEN_VALUE_SPACE.contains(&knob),
                "`{knob}` is persisted and read back but this endpoint neither validates nor \
                 refuses it: junk would render as an override on every client and be dropped \
                 with a warn on every turn"
            );
        }
    }

    #[test]
    fn a_junk_knob_value_is_refused_and_null_is_not() {
        let bag = |v: serde_json::Value| {
            let mut m = serde_json::Map::new();
            m.insert("think_level".to_string(), v);
            m
        };
        assert!(first_invalid_knob(&bag(serde_json::json!("high"))).is_none());
        // `null` is how a client clears an override back to "follow global".
        assert!(first_invalid_knob(&bag(serde_json::Value::Null)).is_none());
        assert_eq!(
            first_invalid_knob(&bag(serde_json::json!("nonsense"))).map(|(k, _)| k),
            Some("think_level"),
            "the twin that had no validation block until now"
        );
        // A non-string stores as an unreadable value, which reads back as "no
        // override" — a write that reports success and changes nothing.
        assert_eq!(
            first_invalid_knob(&bag(serde_json::json!(3))).map(|(k, _)| k),
            Some("think_level")
        );
    }

    #[test]
    fn unrelated_metadata_keys_pass_through() {
        // The bag is open by design (topic, status, custom client keys); only
        // the knobs this server enforces are policed.
        let mut m = serde_json::Map::new();
        m.insert("topic".to_string(), serde_json::json!("anything at all"));
        m.insert("some_client_key".to_string(), serde_json::json!(42));
        assert!(first_invalid_knob(&m).is_none());
    }

    /// The pin has two stores that must agree, and only `select_model` writes
    /// both. A row-only write would take effect after the next restart and not
    /// before it — "sometimes works" is the worst shape to debug.
    #[test]
    fn the_model_pin_is_refused_rather_than_written_row_only() {
        let mut m = serde_json::Map::new();
        m.insert("model_pin".to_string(), serde_json::json!("gpt-5"));
        assert_eq!(first_foreign_knob(&m), Some("model_pin"));

        let mut m2 = serde_json::Map::new();
        m2.insert(
            "model_pin_provider".to_string(),
            serde_json::json!("openai"),
        );
        assert_eq!(first_foreign_knob(&m2), Some("model_pin_provider"));

        // Even `null` — "clear the pin" — goes through the tool, which also
        // evicts the process map. Clearing the row alone leaves the live pin.
        let mut m3 = serde_json::Map::new();
        m3.insert("model_pin".to_string(), serde_json::Value::Null);
        assert_eq!(first_foreign_knob(&m3), Some("model_pin"));

        let mut ok = serde_json::Map::new();
        ok.insert("exec_tier".to_string(), serde_json::json!("auto"));
        assert_eq!(first_foreign_knob(&ok), None);
    }

    #[test]
    fn memory_mode_is_policed_too() {
        let mut m = serde_json::Map::new();
        m.insert("memory_mode".to_string(), serde_json::json!("maybe"));
        assert_eq!(first_invalid_knob(&m).map(|(k, _)| k), Some("memory_mode"));
        m.insert("memory_mode".to_string(), serde_json::json!("off"));
        assert!(first_invalid_knob(&m).is_none());
    }

    fn user_event(text: &str) -> SessionEvent {
        SessionEvent::UserMessage {
            turn_id: uuid::Uuid::new_v4(),
            content: MessageContent {
                text: text.into(),
                blocks: vec![],
                thinking: None,
                thinking_signature: None,
            },
            at: 0,
            synthetic: false,
            author_user_id: None,
        }
    }

    /// `/undo` must actually undo. `truncate_messages` touches only the
    /// projection (`messages` / `messages_fts` / `transcript.jsonl`); the
    /// model's prompt is rebuilt from `session_events`, so this handler used to
    /// remove the turn from every screen while the model went on replaying it,
    /// and `/retry` (undo, then re-send) grew the log to [U, A, U] — the
    /// duplicated turn its own comment claims the undo prevents.
    ///
    /// Four sibling verbs already retire the log first (`sessions.reset`,
    /// `sessions.delete`, `chat.clear`, `chat.rewind`); this was the only one
    /// that did not, and the criterion is stated verbatim 400 lines above it.
    #[tokio::test]
    async fn truncate_retires_the_event_log_not_just_the_projection() {
        let events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("truncate_ssot.db"),
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::from_key_string("agent:trunctest:main").unwrap();
        manager.get_or_create(&key).await.unwrap();
        let key_str = key.to_key_string();

        // Two turns, projected the way the projector does it: the row id
        // carries the source seq, which is what places the cut in the log's
        // index space.
        for seq in 1..=4u64 {
            events
                .append(&key, seq, &user_event(&format!("line {seq}")), 0)
                .await
                .unwrap();
            manager
                .append_message(
                    &key,
                    crate::gateway::session_store::types::MessageRecord {
                        id: crate::session::projection::row_id(&key_str, seq),
                        role: if seq % 2 == 1 { "user" } else { "assistant" }.into(),
                        content: format!("line {seq}"),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                        metadata: None,
                        input_tokens: 0,
                        output_tokens: 0,
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
                .unwrap();
        }
        assert_eq!(events.load_all_events(&key).await.unwrap().len(), 4);

        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "session.truncate".into(),
            params: Some(json!({ "session_key": key_str, "keep_count": 2 })),
            id: Some(json!(1)),
        };
        // `None` run manager = "I cannot tell whether a run is live", which the
        // marker balance must read as "leave it alone" (see
        // `handlers::balance_run_markers_after_retire`). Nothing here opens a
        // run marker, so the balance is a no-op either way — the assertion
        // below still counts the surviving events, which would catch a
        // fail-open balance appending a closer.
        let response = handle_truncate_db(request, store.clone(), None).await;
        assert!(response.error.is_none(), "{:?}", response.error);

        let surviving = events.load_all_events(&key).await.unwrap();
        assert_eq!(
            surviving.len(),
            2,
            "the reverted turn is still in the event log, so the model replays \
             a turn the user was told was undone (surviving seqs: {:?})",
            surviving.iter().map(|e| e.seq).collect::<Vec<_>>()
        );
        // And the two halves must describe the SAME cut.
        assert_eq!(store.get_history(&key, None).await.unwrap().len(), 2);
    }

    /// Deleting a conversation must also stop the autonomous chains keyed to
    /// it. They are keyed by the session STRING, not by the transcript's
    /// existence, so without this the deleted conversation keeps ticking —
    /// posting to the origin channel until its cap runs out, against a session
    /// the user removed. `terminate_session_continuations` is the single seam
    /// for that; `/new` and `sessions.new` called it, `sessions.delete` did not.
    #[tokio::test]
    async fn delete_stops_the_sessions_timer_loop() {
        use crate::looping::{Cadence, LoopState, LoopStatus};

        let reg = crate::looping::global().unwrap_or_else(|| {
            crate::looping::init_global(crate::sync_primitives::Arc::new(
                crate::looping::LoopRegistry::default(),
            ));
            crate::looping::global().expect("registry installed")
        });

        let _events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("delete_loop.db"),
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::from_key_string("agent:looptest:main").unwrap();
        manager.get_or_create(&key).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let session = key.to_key_string();
        reg.put(LoopState::new(
            &session,
            "watch the deploy",
            Cadence::Fixed {
                interval_ms: 300_000,
            },
            0,
        ));
        assert_eq!(reg.get(&session).unwrap().status, LoopStatus::Active);

        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.delete".into(),
            params: Some(json!({ "session_key": session })),
            id: Some(json!(1)),
        };
        let response = handle_delete_db(request, store).await;
        assert!(response.error.is_none(), "{:?}", response.error);

        let after = reg.get(&session).expect("row still readable");
        assert_eq!(
            after.status,
            LoopStatus::Stopped,
            "the deleted session's loop must not keep ticking"
        );
        assert!(
            after.stop_reason.unwrap().contains("sessions.delete"),
            "the stop reason must name the surface that retired it"
        );
    }

    /// Deleting a conversation must delete the conversation — not just the
    /// transcript the Panel happens to read. The event log is the SSOT: leaving
    /// it live keeps the content replayable by the model, searchable via BM25,
    /// and — when its run reads as interrupted — re-materialisable by
    /// `ProjectionReconciler` at the next boot.
    #[tokio::test]
    async fn delete_retires_the_event_log_so_nothing_replays_or_searches() {
        let events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("delete_ssot.db"),
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::from_key_string("agent:deltest:main").unwrap();
        manager.get_or_create(&key).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        events
            .append(&key, 1, &user_event("the passphrase is hunter2"), 0)
            .await
            .unwrap();

        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.delete".into(),
            params: Some(json!({ "session_key": key.to_key_string() })),
            id: Some(json!(1)),
        };
        let response = handle_delete_db(request, store).await;
        assert!(
            response.error.is_none(),
            "delete failed: {:?}",
            response.error
        );
        let result = response.result.expect("result");
        assert_eq!(result["deleted"], true);
        assert_eq!(result["events_retired"], 1);

        assert!(
            events.load_all_events(&key).await.unwrap().is_empty(),
            "a deleted conversation must yield nothing to a replay"
        );
        assert!(
            events
                .search_events(&key, "passphrase", 5)
                .await
                .unwrap()
                .is_empty(),
            "a deleted conversation must yield nothing to FTS"
        );
    }

    /// Deleting a conversation must delete the summary OF that conversation.
    /// `resume.json` is an LLM-authored recap that the assembler injects into
    /// the next session's prompt as "previous session" context — a user who
    /// deletes a conversation because of what was in it must not meet it again
    /// one session later. The purge must also use the CANONICAL key spelling:
    /// the store encodes the key into a directory name, so purging under the
    /// caller's raw spelling would remove a directory that never existed and
    /// leave the real one on disk.
    #[tokio::test]
    async fn delete_purges_the_resume_snapshot_under_the_canonical_key() {
        // Isolate `ALEPH_HOME`: the snapshot store resolves off it, and this
        // test writes into it.
        let _home = crate::utils::paths::IsolatedAlephHome::new();
        let _events = crate::session::store::install_test_event_store();
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("delete_snapshot.db"),
            ..Default::default()
        })
        .unwrap();
        // A legacy spelling that parses to a DIFFERENT (canonical) key string,
        // which is where the snapshot actually lives.
        let raw_spelling = "AGENT:DelSnap:main";
        let key = SessionKey::from_key_string(raw_spelling).unwrap();
        let canonical = key.to_key_string();
        assert_ne!(canonical, raw_spelling, "the two spellings must differ");
        manager.get_or_create(&key).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);

        let writer = crate::memory::session_resume::SnapshotWriter::default_path().unwrap();
        let snapshot = writer
            .write_from_summary(&canonical, "The user shared their diary.", "delsnap")
            .unwrap();
        assert!(snapshot.exists(), "precondition: a snapshot on disk");

        let request = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.delete".into(),
            params: Some(json!({ "session_key": raw_spelling })),
            id: Some(json!(1)),
        };
        let response = handle_delete_db(request, store).await;
        assert!(response.error.is_none(), "{:?}", response.error);

        assert!(
            !snapshot.exists(),
            "the deleted conversation's resume snapshot must not survive"
        );
        assert!(
            crate::memory::session_resume::SnapshotReader::default_path()
                .unwrap()
                .load_latest("delsnap", "none")
                .is_none(),
            "and it must not come back as the next session's 'previous session'"
        );
    }

    /// `sessions.patch` is the tier picker's own write path. An id the run loop
    /// cannot resolve must be refused here — otherwise it persists, the Panel
    /// renders it as an armed override, and every turn silently runs at the
    /// global tier. `chat.send` already refuses one; the two write paths must
    /// agree. `null` must keep working: it is how "follow global" clears the
    /// override.
    #[tokio::test]
    async fn patch_refuses_an_unknown_exec_tier_but_accepts_null_and_a_real_tier() {
        let temp = tempdir().unwrap();
        let manager = SessionManager::new(SessionManagerConfig {
            db_path: temp.path().join("patch_tier.db"),
            ..Default::default()
        })
        .unwrap();
        let key = SessionKey::from_key_string("agent:tierpatch:main").unwrap();
        manager.get_or_create(&key).await.unwrap();
        let store: Arc<dyn SessionStore> = Arc::new(manager);
        let key_str = key.to_key_string();

        let patch = |tier: Value| JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "sessions.patch".into(),
            params: Some(json!({
                "session_key": key_str,
                "metadata": { "exec_tier": tier },
            })),
            id: Some(json!(1)),
        };

        let rejected = handle_patch_db(patch(json!("strict")), Arc::clone(&store)).await;
        assert!(
            rejected.error.is_some(),
            "an unknown tier id must not persist"
        );

        for accepted in [json!("ask"), Value::Null] {
            let response = handle_patch_db(patch(accepted.clone()), Arc::clone(&store)).await;
            assert!(
                response.error.is_none(),
                "`{accepted}` is a legal exec_tier write: {:?}",
                response.error
            );
        }
    }

    /// P1 visibility chokepoint — pinned per task-6-brief.md Step 1.
    mod visibility_guards {
        use super::*;
        use crate::gateway::caller_identity::CALLER_USER;
        use crate::gateway::protocol::RESOURCE_NOT_FOUND;
        use crate::scope::{with_scope, ScopeAttribution};

        fn store(temp: &tempfile::TempDir) -> Arc<dyn SessionStore> {
            Arc::new(
                SessionManager::new(SessionManagerConfig {
                    db_path: temp.path().join("visibility_modify.db"),
                    ..Default::default()
                })
                .unwrap(),
            )
        }

        fn keyed_request(method: &str, session_key: &str) -> JsonRpcRequest {
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: method.into(),
                params: Some(json!({ "session_key": session_key })),
                id: Some(json!(1)),
            }
        }

        async fn alice_session(store: &Arc<dyn SessionStore>) -> SessionKey {
            let key = SessionKey::from_key_string("agent:alicemodvis:main").unwrap();
            with_scope(
                Some(ScopeAttribution::personal("u-alice")),
                store.get_or_create(&key),
            )
            .await
            .unwrap();
            key
        }

        /// `sessions.delete` deny must leave the foreign session INTACT — a
        /// denial is not a side effect. This is the property distinguishing
        /// the deny path from a real delete: both return quickly, only one
        /// actually removes the row.
        #[tokio::test]
        async fn sessions_delete_denies_cross_user_and_leaves_session_intact() {
            let temp = tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_delete_db(
                        keyed_request("sessions.delete", &alice_key_str),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );

            let still_there = store.get_metadata(&alice_key).await.unwrap();
            assert!(
                still_there.is_some(),
                "a denied delete must not remove the foreign session"
            );

            // alice herself can still delete it — a stamped session belongs
            // exclusively to its stamped owner, not to the org-era owner id
            // (that adoption-by-absence rule only applies to legacy rows).
            let as_alice = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_delete_db(
                        keyed_request("sessions.delete", &alice_key_str),
                        store.clone(),
                    ),
                )
                .await;
            assert!(as_alice.error.is_none());
        }

        #[tokio::test]
        async fn sessions_reset_denies_cross_user_as_not_found() {
            let temp = tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_reset_db(
                        keyed_request("sessions.reset", &alice_key_str),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );

            // Byte-identical to a genuinely nonexistent key (no existence oracle).
            let as_bob_missing = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_reset_db(
                        keyed_request("sessions.reset", "agent:nosuchresetkey:main"),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                serde_json::to_string(&as_bob.error).unwrap(),
                serde_json::to_string(&as_bob_missing.error).unwrap(),
            );
        }

        /// Guardrail-downgrade shape: bob attempting to weaken alice's
        /// exec_tier via a foreign session_key must be denied, and the
        /// session's metadata (its label, standing in for "anything at
        /// all") must be untouched.
        #[tokio::test]
        async fn sessions_patch_denies_cross_user_and_leaves_session_intact() {
            let temp = tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            let req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.patch".into(),
                params: Some(json!({
                    "session_key": alice_key_str,
                    "label": "hacked-by-bob",
                    "metadata": { "exec_tier": "full" },
                })),
                id: Some(json!(1)),
            };
            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_patch_db(req, store.clone()),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );

            let after = store.get_metadata(&alice_key).await.unwrap().unwrap();
            assert_ne!(
                after.label.as_deref(),
                Some("hacked-by-bob"),
                "a denied patch must not write the foreign session's label"
            );
        }

        #[tokio::test]
        async fn sessions_set_project_root_denies_cross_user_as_not_found() {
            let temp = tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;

            let req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.set_project_root".into(),
                params: Some(json!({
                    "session_key": alice_key.to_key_string(),
                    "project_root": "/attacker/chosen/path",
                })),
                id: Some(json!(1)),
            };
            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_set_project_root_db(req, store.clone()),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
        }

        #[tokio::test]
        async fn session_compact_denies_cross_user_as_not_found() {
            let temp = tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;

            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_compact_db(
                        keyed_request("session.compact", &alice_key.to_key_string()),
                        store.clone(),
                    ),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
        }

        /// `session.truncate` irreversibly drops the tail of the transcript
        /// — a denial must leave every message alone.
        #[tokio::test]
        async fn session_truncate_denies_cross_user_and_leaves_messages_intact() {
            let temp = tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            store
                .append_message(
                    &alice_key,
                    crate::gateway::session_store::types::MessageRecord {
                        id: "m1".into(),
                        role: "user".into(),
                        content: "alice's message".into(),
                        timestamp: 0,
                        metadata: None,
                        input_tokens: 0,
                        output_tokens: 0,
                        tool_call_id: None,
                        tool_name: None,
                    },
                )
                .await
                .unwrap();

            let req = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "session.truncate".into(),
                params: Some(json!({
                    "session_key": alice_key.to_key_string(),
                    "keep_count": 0,
                })),
                id: Some(json!(1)),
            };
            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_truncate_db(req, store.clone(), None),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );

            let history = store.get_history(&alice_key, None).await.unwrap();
            assert_eq!(
                history.len(),
                1,
                "a denied truncate must not remove the foreign session's messages"
            );
        }

        /// `sessions.set_topic` is a cross-user WRITE: the topic is the title
        /// the owner's own sidebar renders, so an ungated caller renames
        /// somebody else's conversation. The assertion that matters is the
        /// stored topic AFTER the denial — a check that merely returned the
        /// right error code while the `UPDATE` still ran would pass a
        /// response-shape-only test.
        #[tokio::test]
        async fn sessions_set_topic_denies_cross_user_and_leaves_the_title_intact() {
            let temp = tempdir().unwrap();
            let store = store(&temp);
            let alice_key = alice_session(&store).await;
            let alice_key_str = alice_key.to_key_string();

            // Alice names her own conversation first, so the deny case has a
            // real value to preserve rather than an absent one.
            let mine = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.set_topic".into(),
                params: Some(json!({
                    "session_key": alice_key_str,
                    "topic": "alice's quarterly plan",
                })),
                id: Some(json!(1)),
            };
            let as_alice = CALLER_USER
                .scope(
                    Some("u-alice".to_string()),
                    handle_set_topic_db(mine, store.clone()),
                )
                .await;
            assert!(as_alice.error.is_none(), "the owner may rename her own");
            assert_eq!(
                store
                    .get_metadata(&alice_key)
                    .await
                    .unwrap()
                    .unwrap()
                    .topic
                    .as_deref(),
                Some("alice's quarterly plan")
            );

            let theirs = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.set_topic".into(),
                params: Some(json!({
                    "session_key": alice_key_str,
                    "topic": "renamed-by-bob",
                })),
                id: Some(json!(1)),
            };
            let as_bob = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_set_topic_db(theirs, store.clone()),
                )
                .await;
            assert_eq!(
                as_bob.error.as_ref().map(|e| e.code),
                Some(RESOURCE_NOT_FOUND)
            );
            assert_eq!(
                store
                    .get_metadata(&alice_key)
                    .await
                    .unwrap()
                    .unwrap()
                    .topic
                    .as_deref(),
                Some("alice's quarterly plan"),
                "a denied set_topic must not rewrite the foreign session's title"
            );

            // Byte-identical to a genuinely nonexistent key (no existence
            // oracle): the rename target's existence must not be learnable.
            let missing = JsonRpcRequest {
                jsonrpc: "2.0".into(),
                method: "sessions.set_topic".into(),
                params: Some(json!({
                    "session_key": "agent:nosuchtopickey:main",
                    "topic": "renamed-by-bob",
                })),
                id: Some(json!(1)),
            };
            let as_bob_missing = CALLER_USER
                .scope(
                    Some("u-bob".to_string()),
                    handle_set_topic_db(missing, store.clone()),
                )
                .await;
            assert_eq!(
                serde_json::to_string(&as_bob).unwrap(),
                serde_json::to_string(&as_bob_missing).unwrap(),
            );
        }
    }
}
