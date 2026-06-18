//! System-prompt assembly for `AgentHarnessRunner`: memory headroom,
//! extra-file loading, prompt building, plus iteration-cap and query helpers.

use crate::orchestrator::flow_spec::FlowInput;
use crate::providers::AiProvider;
use crate::sandbox::Sandbox;
use crate::session::service::SessionId;

use super::*;

impl AgentHarnessRunner {
    /// Compute how many tokens the context window can spare for memory
    /// injection this turn, or `None` when no `[context_budget]` is configured
    /// (memory then uses its full configured budget — legacy behaviour).
    ///
    /// Memory is injected once, before the Think→Act loop, into the system
    /// prompt — which the pressure sensor counts as `overhead` and which message
    /// compaction can NOT reclaim. So an oversized recall silently forces the
    /// in-loop compactor to over-trim recent history to make room. We cap memory
    /// so existing history + memory stays under the compaction *warning* line,
    /// leaving the rest of the window for the base system prompt, tool schemas,
    /// and the model's reply. Reuses the exact estimator the in-loop budget uses
    /// (`estimate_message_tokens_aware`) so the two views agree.
    ///
    /// No reference agent (hermes / openclaw / Pi / opensquilla) coordinates the
    /// memory and history budgets — they inject memory at a fixed size
    /// regardless of conversation pressure.
    pub(crate) async fn memory_injection_headroom(&self, session_id: &SessionId) -> Option<u32> {
        let cfg = self.context_budget_config.as_ref()?;
        // Best-effort: a read failure must never block a turn — fall back to the
        // full configured budget (None) just like a missing context budget.
        let events = self
            .session_service
            .get_events(session_id, None, None)
            .await
            .ok()?;
        let messages = crate::harness::agent::prompt::build_prompt(&events, events.len());
        let history_tokens: usize = messages
            .iter()
            .map(|m| {
                crate::context::budget::pressure::estimate_message_tokens_aware(
                    m,
                    cfg.token_estimate_ratio,
                )
            })
            .sum();
        let ceiling = (cfg.token_budget as f64 * cfg.warning_threshold).max(0.0) as usize;
        let available = ceiling.saturating_sub(history_tokens);
        Some(available.min(u32::MAX as usize) as u32)
    }

    /// Load `[prompt.extra_files]` content off disk, size-capped.
    ///
    /// Relative paths resolve against `workspace` (the per-run workspace
    /// override) when present, else the daemon's working directory. Missing,
    /// unreadable, or blank files are skipped with a debug log so a stale
    /// config entry never blocks prompt assembly (P7 graceful degradation).
    /// Caps mirror `IdentityFilesConfig` (20k chars/file, 100k total) so a
    /// runaway file cannot blow the context budget. Returns `None` when the
    /// section is absent, disabled, or yields no content.
    pub(crate) fn load_prompt_extra_files(
        &self,
        workspace: Option<&std::path::Path>,
    ) -> Option<Vec<crate::thinker::prompt_layer::ExtraPromptFile>> {
        use crate::thinker::prompt_layer::ExtraPromptFile;

        const PER_FILE_MAX_CHARS: usize = 20_000;
        const TOTAL_MAX_CHARS: usize = 100_000;

        let cfg = self.prompt_extra_files.as_ref()?;
        if !cfg.enabled || cfg.paths.is_empty() {
            return None;
        }

        let mut out = Vec::new();
        let mut total = 0usize;
        for raw in &cfg.paths {
            if total >= TOTAL_MAX_CHARS {
                tracing::warn!(
                    path = %raw,
                    "[prompt.extra_files] total budget exhausted; skipping remaining files"
                );
                break;
            }
            let path = std::path::Path::new(raw);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                match workspace {
                    Some(ws) => ws.join(path),
                    None => path.to_path_buf(),
                }
            };
            let content = match std::fs::read_to_string(&resolved) {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!(
                        path = %resolved.display(),
                        error = %e,
                        "[prompt.extra_files] unreadable; skipping"
                    );
                    continue;
                }
            };
            if content.trim().is_empty() {
                continue;
            }
            let budget = PER_FILE_MAX_CHARS.min(TOTAL_MAX_CHARS - total);
            let capped = truncate_chars(&content, budget);
            total += capped.chars().count();
            out.push(ExtraPromptFile {
                name: raw.clone(),
                content: capped,
            });
        }
        (!out.is_empty()).then_some(out)
    }

    /// Assemble the per-turn system prompt with curated memory + hybrid
    /// retrieval. Returns `None` when no `MemoryContextProvider` is wired
    /// (test envs without a memory backend) or when both memory builders
    /// returned empty envelopes.
    ///
    /// Errors from individual builders are downgraded to a warn log: the
    /// remaining sections (curated/memory/agent role) still render so a
    /// transient memory failure never blocks a turn. This matches the
    /// `Ok(None)` semantics already exposed by `MemoryContextProvider`'s
    /// builders and keeps the harness path resilient.
    pub(crate) async fn build_system_prompt(
        &self,
        agent_id: &str,
        session_id: &SessionId,
        user_query: &str,
        provider: &dyn AiProvider,
        iteration_cap: usize,
        channel_manifest: Option<&crate::thinker::InteractionManifest>,
        sandbox: &dyn Sandbox,
        workspace: Option<&std::path::Path>,
    ) -> Option<(
        String,
        Vec<crate::thinker::prompt_builder::SystemPromptPart>,
    )> {
        use crate::providers::message::UnifiedMessage;
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        // Phase 1 — fetch the eligible-skill snapshot once; reused below.
        let skill_snapshot = match self.skill_system.as_ref() {
            Some(sys) => Some(sys.current_snapshot().await),
            None => None,
        };

        let session_key_str = session_id.to_key_string();

        let (curated_text, memory_text) = if let Some(mcp) = self.memory_context_provider.as_ref() {
            let curated_text: Option<String> =
                match mcp.build_curated_message(agent_id, &session_key_str).await {
                    Ok(opt) => opt.as_ref().map(UnifiedMessage::text_content),
                    Err(e) => {
                        tracing::warn!(
                            agent_id,
                            session = %session_key_str,
                            error = %e,
                            "build_curated_message failed; degrading curated envelope to None"
                        );
                        None
                    }
                };

            let memory_text: Option<String> = if user_query.is_empty() {
                None
            } else {
                // Coordinate the one-shot memory injection with the per-turn
                // context budget so a large recall never forces the in-loop
                // compactor to over-trim recent history (memory lands in the
                // system prompt = un-compactable overhead). `None` when no
                // `[context_budget]` is configured → full configured budget.
                let headroom = self.memory_injection_headroom(session_id).await;
                match mcp
                    .build_memory_user_message(agent_id, user_query, headroom)
                    .await
                {
                    Ok(opt) => opt.as_ref().map(UnifiedMessage::text_content),
                    Err(e) => {
                        tracing::warn!(
                            agent_id,
                            error = %e,
                            "build_memory_user_message failed; degrading memory envelope to None"
                        );
                        None
                    }
                }
            };
            (curated_text, memory_text)
        } else {
            (None, None)
        };

        let agent_def = self.agent_registry.get(agent_id);

        // Load user-editable identity files from `~/.aleph/agents/{agent_id}/`
        // (SOUL.md / IDENTITY.md / AGENTS.md / TOOLS.md / HEARTBEAT.md). The
        // loader was previously only exercised from its own tests — wiring it
        // here is what gets `IdentityFilesLayer` (and the soul / profile layers
        // that read the same source) usable content on the harness path.
        // Tolerant of missing home / dir / IO failure: returns IdentityFiles
        // with all-None content, which the layer treats as "skip".
        let identity_files = crate::discovery::aleph_agents_dir().ok().map(|agents_dir| {
            crate::thinker::identity_files::IdentityFiles::load(
                &agents_dir.join(agent_id),
                &crate::thinker::identity_files::IdentityFilesConfig::default(),
            )
        });
        let has_identity = identity_files
            .as_ref()
            .is_some_and(|f| f.files.iter().any(|file| file.content.is_some()));

        let has_skills = skill_snapshot
            .as_ref()
            .is_some_and(|s| !s.eligible_manifests.is_empty());

        // Load `[prompt.extra_files]` content (size-capped). `None` when the
        // section is absent / disabled / yields no readable content, so the
        // default config keeps the assembled prompt byte-identical.
        //
        // Then append project-scoped instruction files (`CLAUDE.md` / `AGENTS.md`
        // discovered from the active project folder, walking up to the git
        // root). These only exist when the run targets a user-chosen
        // `workspace_override`; the default agent workspace has none, so the
        // assembled prompt stays byte-identical for the no-project case. Both
        // sets render through `ExtraFilesLayer` (same sanitization boundary).
        let extra_files = {
            let mut files = self.load_prompt_extra_files(workspace).unwrap_or_default();
            if let Some(ws) = workspace {
                files.extend(crate::thinker::project_instructions::load_project_instructions(ws));
            }
            (!files.is_empty()).then_some(files)
        };

        // Aggregate connected MCP servers' advertised `instructions`. One actor
        // round-trip per prompt build (negligible next to the file/skill IO
        // above) keeps the data always-fresh without a shared mutable snapshot.
        // `None` when no manager is wired, the call fails, or no server supplied
        // instructions — `McpInstructionsLayer` then renders nothing.
        let mcp_instructions = match &self.mcp_handle {
            Some(handle) => {
                let items = handle.aggregate_instructions().await.unwrap_or_default();
                (!items.is_empty()).then_some(items)
            }
            None => None,
        };

        // Skip prompt assembly entirely when there is nothing to inject:
        // no memory, no AgentDef, no eligible skills, no identity files, no
        // extra files, and no MCP server instructions.
        if curated_text.is_none()
            && memory_text.is_none()
            && agent_def.is_none()
            && !has_skills
            && !has_identity
            && extra_files.is_none()
            && mcp_instructions.is_none()
        {
            return None;
        }

        // Capture the configured prompt budget before the snapshot is consumed
        // (Copy type, so a by-ref read suffices). `SkillInstructionsLayer` uses
        // it to bound the injected `<available_skills>` index.
        let skill_prompt_budget = skill_snapshot.as_ref().map(|s| s.prompt_budget);
        let eligible_skills = skill_snapshot
            .map(|s| s.eligible_manifests)
            .filter(|m| !m.is_empty());
        // The harness path delivers tool schemas via native tool_use
        // (`with_tools(tools_ref)` in agent.rs). When `native_tools_enabled` is
        // false (the default), `ToolsLayer` injects the literal string
        // "No tools available" and `ResponseFormatLayer` mandates the legacy
        // `{reasoning, action}` JSON envelope — both of which contradict the
        // native-tool-use API the harness actually drives. Force the flag on
        // here so the assembled prompt matches the runtime contract.
        let mut builder = PromptBuilder::new(PromptConfig {
            native_tools_enabled: true,
            eligible_skills,
            skill_prompt_budget,
            mcp_instructions,
            ..PromptConfig::default()
        });
        let role_present = agent_def.is_some();
        if let Some(def) = agent_def {
            builder = builder.with_agent(def);
        }
        let curated_chars = curated_text.as_ref().map_or(0, String::len);
        builder = builder.with_curated_envelope(curated_text);
        let memory_chars = memory_text.as_ref().map_or(0, String::len);
        if let Some(text) = memory_text {
            builder = builder.with_memory_user_message(text);
        }
        let identity_chars = identity_files.as_ref().map_or(0, |f| {
            f.files
                .iter()
                .filter_map(|file| file.content.as_ref().map(String::len))
                .sum::<usize>()
        });
        if let Some(files) = identity_files {
            if has_identity {
                builder = builder.with_identity_files(files);
            }
        }
        if let Some(files) = extra_files {
            builder = builder.with_extra_files(files);
        }
        // Phase 4 (F4): channel-aware `ResolvedContext`. When the
        // caller (Gateway, subagent dispatcher, etc.) supplies a
        // channel-specific `InteractionManifest`, use it so per-channel
        // paradigm, capabilities, and constraints flow into the prompt
        // (`SecurityLayer`, `OperationalGuidelinesLayer`,
        // `ProtocolTokensLayer`). Fall back to `Background` paradigm —
        // aleph-server's always-on-daemon default — when no manifest
        // is provided (subagent dispatch / internal tooling / tests).
        //
        // Phase 5 (F2): SecurityContext is also paradigm-derived via
        // `SecurityContext::for_paradigm`. CLI / WebRich / Background /
        // Embedded stay permissive (trusted-self-host); Messaging surfaces
        // a Standard sandbox + approval-required posture for elevated
        // operations, signalling the LLM to be cautious on public-channel
        // bots. Actual tool enforcement still happens in the tool
        // execution layer — this is a prompt-text signal, not a hard gate.
        //
        // Tools list is empty because the harness wires actual tool
        // schemas via native tool_use rather than the prompt;
        // `disabled_tools` therefore stays empty too.
        let default_manifest;
        let manifest_ref = match channel_manifest {
            Some(m) => m,
            None => {
                default_manifest = crate::thinker::InteractionManifest::new(
                    crate::thinker::InteractionParadigm::Background,
                );
                &default_manifest
            }
        };
        let security_ctx =
            crate::thinker::security_context::SecurityContext::for_paradigm(manifest_ref.paradigm);
        let mut resolved_context =
            crate::thinker::context::ContextAggregator::resolve(manifest_ref, &security_ctx, &[]);
        // Phase 4 (F1): populate `runtime_context` so `RuntimeContextLayer`
        // surfaces shell / arch / hostname / timezone / model. EnvironmentLayer
        // emits OS/cwd in a Markdown list (Stable, priority 300);
        // `RuntimeContext::to_prompt_section()` emits a pipe-separated
        // single-line summary (Dynamic, priority 1720) — formats deliberately
        // differ. We accept the minor OS/cwd overlap; the unique fields
        // (arch, shell, repo_root, model, hostname, timezone, current_time)
        // carry the value. Phase 5 (F3) populates `repo_root` via a
        // `OnceLock`-cached `.git` walk-up — process-lifetime amortized,
        // no `git` subprocess.
        resolved_context.runtime_context = Some(
            crate::thinker::runtime_context::RuntimeContext::collect(provider.name()),
        );
        // Populate runtime-state fragments from the tool catalog's
        // `ToolHealthCache`. Each currently-cached `Unhealthy` entry becomes
        // a `RuntimeStateFragment::unavailable(name, reason)` that
        // `ToolRuntimeStateLayer` @502 renders into `<tool_runtime_state>`.
        // `None` tool_catalog (test / early boot) → empty vec → the
        // layer emits nothing.
        resolved_context.runtime_state_blocks =
            compute_runtime_state_blocks(self.tool_catalog.as_ref());
        // Codex-inspired: surface active sandbox posture (backend tag,
        // policy tier, writable roots, network state) to the LLM so it
        // can plan within its envelope instead of probing limits at runtime.
        // `Sandbox::summary()` defaults to `None`, so mock/noop sandboxes
        // in tests leave this absent and the SecurityLayer skips the
        // sandbox bullet block.
        resolved_context.sandbox_summary = sandbox.summary();
        // Re-surface the session's active scratchpad execution list so the
        // live plan stays in context across long tool-only stretches where
        // the model never re-calls the `scratchpad` tool. Reuses the same
        // `scratchpad_registry` binding the tool / steering / stop-verifier
        // already key off — a mechanical lookup, no reasoning. `None` (no
        // active plan with pending work) leaves the prompt byte-identical;
        // `ExecutionPlanLayer` @1755 renders it as `<execution_plan>`.
        //
        // The execution-plan and standing-goal lookups are independent
        // session-keyed reads (a scratchpad file read + a goal-store read with
        // a wall-clock stamp). Run them concurrently with `tokio::join!` so
        // prompt assembly — on the hot per-turn path — pays the max of the two
        // latencies, not their sum. `join!` polls both on the current task, so
        // there is no spawn cost and no extra `Send` bound; both futures take a
        // shared `&session_key_str` borrow, which co-exist fine.
        let (exec_plan, standing) = tokio::join!(
            active_execution_plan(&session_key_str),
            active_standing_goal(&session_key_str),
        );
        resolved_context.execution_plan = exec_plan;
        resolved_context.standing_goal = standing;
        // Voice mode: read the session-keyed flag the gateway inbound router set
        // for this turn so `VoiceModeLayer` (priority 1710) injects the
        // spoken-reply guidelines. Mirrors `execution_plan` / `standing_goal` —
        // a mechanical session-keyed lookup, no judgment. `false` (no voice)
        // leaves the prompt byte-identical.
        resolved_context.voice_mode_active =
            crate::gateway::voice::session_mode::is_active(&session_key_str);
        builder = builder.with_resolved_context(resolved_context);
        // Phase 3: thread the provider's wire-protocol family so
        // `ProviderGuidanceLayer` can pick the right per-family
        // operational directives. `model_behavior_override()` wins over
        // the raw protocol so providers like OpenRouter that proxy a
        // different model family can advertise the correct target
        // (e.g., `protocol = "openai"`, override = `"anthropic"`).
        let provider_protocol = provider
            .model_behavior_override()
            .map_or_else(|| provider.protocol().to_string(), |s| s.to_string());
        builder = builder.with_provider_protocol(provider_protocol);
        // Phase 4 (F2): surface the resolved iteration cap to
        // `SessionBudgetLayer`. Saturating to `u32::MAX` (instead of
        // truncating) preserves "no practical cap" semantics for callers
        // that pass a huge value; the layer's own zero-guard keeps the
        // unset case silent.
        let cap_for_prompt = u32::try_from(iteration_cap).unwrap_or(u32::MAX);
        builder = builder.with_iteration_cap(cap_for_prompt);
        // Cache-first wiring: build the stable/dynamic split AND the
        // legacy flat string. The split lights up `RequestPayload::
        // system_blocks` (consumed by the Anthropic adapter to place the
        // prompt-cache breakpoint at the stable/dynamic boundary). The
        // flat string remains the source of truth for adapters that do
        // not consume `system_blocks` (everything except Anthropic today)
        // and for callsites that read `HarnessDeps::system_prompt`.
        let parts = builder.build_system_prompt_cached_with_mode(&[], self.default_prompt_mode);
        let prompt: String = parts.iter().map(|p| p.content.as_str()).collect();
        // Phase 6 observability — confirm BUG-2/BUG-3 wiring at runtime.
        // Logs character counts (not contents) so prompts are observable
        // without leaking memory content to disk-side telemetry.
        let stable_chars: usize = parts
            .iter()
            .filter(|p| p.cache)
            .map(|p| p.content.len())
            .sum();
        let dynamic_chars: usize = parts
            .iter()
            .filter(|p| !p.cache)
            .map(|p| p.content.len())
            .sum();
        tracing::info!(
            target: "alephcore::orchestrator::prompt",
            agent_id,
            session = %session_key_str,
            curated_chars,
            memory_chars,
            identity_chars,
            role_present,
            prompt_chars = prompt.len(),
            cache_stable_chars = stable_chars,
            cache_dynamic_chars = dynamic_chars,
            prompt_mode = self.default_prompt_mode.label(),
            "system prompt assembled"
        );
        Some((prompt, parts))
    }
}

/// Resolve the hard per-run iteration cap for the harness loop.
///
/// D2 precedence (highest → lowest, first positive value wins):
/// 1. `runtime_override` — `FlowRequest.max_iterations_override` (cron jobs
///    set this so a single misbehaving job can't burn the global cap).
/// 2. `flow_override` — `FlowOverrides.max_iterations` (per-flow preset).
/// 3. `default` — boot-time `[execution] max_iterations` (1000 default).
///
/// A zero on any input is treated as "unset" so a misconfigured `0` can
/// never leave the loop uncapped — it falls through to the next layer,
/// and ultimately to [`FALLBACK_MAX_ITERATIONS`].
pub(crate) fn resolve_max_iterations(
    runtime_override: Option<u32>,
    flow_override: Option<u32>,
    default: usize,
) -> usize {
    let positive_or_none = |n: Option<u32>| n.map(|n| n as usize).filter(|&n| n > 0);
    positive_or_none(runtime_override)
        .or_else(|| positive_or_none(flow_override))
        .or(Some(default).filter(|&n| n > 0))
        .unwrap_or(FALLBACK_MAX_ITERATIONS)
}

/// Truncate `s` to at most `max_chars` characters, appending a marker when
/// content was dropped. Cuts on a `char_indices` boundary so multi-byte
/// UTF-8 content never panics the slice.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => {
            let mut t = s[..idx].to_string();
            t.push_str("\n…[truncated]");
            t
        }
        None => s.to_string(),
    }
}

/// Extract the user's most recent prompt text from a `FlowInput` for use as
/// the retrieval query against `MemoryContextProvider::build_memory_user_message`.
/// Returns an empty string when no user-side text is available; callers treat
/// the empty case as "skip retrieval".
pub(crate) fn last_user_query(input: &FlowInput) -> String {
    const fn text_of(content: &crate::session::events::MessageContent) -> &str {
        content.text.as_str()
    }
    match input {
        FlowInput::Prompt(s) => s.clone(),
        FlowInput::Messages(msgs) | FlowInput::Multimodal(msgs) => msgs
            .iter()
            .rev()
            .map(text_of)
            .find(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_default(),
        FlowInput::History { prompt, .. } => prompt.clone(),
        FlowInput::Resume => String::new(),
    }
}

/// True when `~/.aleph/agents/<agent_id>/` exists on disk. Directory-form agents
/// (config `[[agents.list]]` / team-created) live only in the gateway registry,
/// not the orchestrator's `AgentRegistry`; their identity still loads by agent_id
/// from this directory, so an on-disk dir is sufficient proof the agent is real —
/// the orchestrator trusts the gateway's prior `AgentInstance` resolution.
pub(crate) fn agent_identity_dir_exists(agent_id: &str) -> bool {
    crate::discovery::aleph_agents_dir()
        .map(|dir| dir.join(agent_id).is_dir())
        .unwrap_or(false)
}

