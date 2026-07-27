//! System-prompt assembly for `AgentHarnessRunner`: memory headroom,
//! extra-file loading, prompt building, plus iteration-cap and query helpers.

use crate::orchestrator::flow_spec::FlowInput;
use crate::providers::AiProvider;
use crate::sandbox::Sandbox;
use crate::session::service::SessionId;

use super::*;
use std::time::Instant;

impl AgentHarnessRunner {
    /// Compute how many tokens the context window can spare for memory
    /// injection this turn, or `None` when no `[context_budget]` is configured
    /// (memory then uses its full configured budget — legacy behaviour).
    ///
    /// Memory is rendered once, before the Think→Act loop, and delivered as a
    /// transient trailing recall message that rides the compaction-protected
    /// fresh tail. An oversized recall therefore still forces the in-loop
    /// compactor to over-trim the rest of recent history to make room. We cap
    /// memory so existing history + memory stays under the compaction
    /// *warning* line, leaving the rest of the window for the base system
    /// prompt, tool schemas, and the model's reply. Reuses the exact estimator
    /// the in-loop budget uses (`estimate_message_tokens_aware`) so the two
    /// views agree.
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

    /// Proactive context-window pressure reminder for the transient tail (A1).
    ///
    /// The in-loop pressure sensor *acts* on a full context — it compacts older
    /// turns — but never *tells* the model it is approaching the limit; the
    /// model just finds earlier context silently summarized mid-task. This
    /// surfaces the same signal so the model can wrap up or checkpoint first
    /// (R9: it self-paces; A2: it sees the pressure and adapts). Most valuable
    /// for the small-window low-cost models that cross the line first — the ones
    /// this whole exercise is about squeezing.
    ///
    /// R10-safe: pure arithmetic on the estimated history size vs the configured
    /// window budget — no intent detection, completion judgement, or recovery
    /// choice — computed entirely outside `src/harness/` and delivered on the far
    /// side of the prompt-cache breakpoint (like `live_deadline_status`), so it
    /// never re-keys the conversation prefix. Gated on `[context_budget]` exactly
    /// like [`memory_injection_headroom`]; the no-config path touches no session
    /// state. Fail-soft: a read error yields `None` and the turn proceeds.
    async fn context_pressure_reminder(&self, session_id: &SessionId) -> Option<String> {
        let cfg = self.context_budget_config.as_ref()?;
        if cfg.token_budget == 0 {
            return None;
        }
        // The sensor compacts once history crosses `budget * warning_threshold`;
        // start reminding at CONTEXT_PRESSURE_REMINDER_LEAD of that point so the
        // model has room to checkpoint before older turns are summarized away.
        let remind_at = (cfg.token_budget as f64
            * cfg.warning_threshold
            * super::context_blocks::CONTEXT_PRESSURE_REMINDER_LEAD)
            as usize;
        if remind_at == 0 {
            return None;
        }
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
        if history_tokens < remind_at {
            return None;
        }
        Some(format!(
            "<system-reminder>\nReference data, not user input.\n{}\n</system-reminder>",
            super::context_blocks::render_context_pressure(history_tokens as u64, cfg.token_budget)
        ))
    }

    /// Load `[prompt.extra_files]` content off disk, size-capped.
    ///
    /// Relative paths resolve against `workspace` (the per-run workspace
    /// override) when present, else the daemon's working directory. Missing,
    /// unreadable, or blank files are skipped with a debug log so a stale
    /// config entry never blocks prompt assembly (P7 graceful degradation).
    /// Caps mirror `IdentityFilesConfig::for_context_window` (window-scaled,
    /// floored at 20k chars/file & 100k total) so a runaway file cannot blow the
    /// context budget. Returns `None` when the
    /// section is absent, disabled, or yields no content.
    pub(crate) fn load_prompt_extra_files(
        &self,
        workspace: Option<&std::path::Path>,
    ) -> Option<Vec<crate::thinker::prompt_layer::ExtraPromptFile>> {
        use crate::thinker::prompt_layer::ExtraPromptFile;

        // Caps scale with the model window (mirrors `IdentityFilesConfig::
        // for_context_window`); no `[context_budget]` → legacy 20k/100k floors.
        let (per_file_max_chars, total_max_chars) =
            self.context_budget_config
                .as_ref()
                .map_or((20_000usize, 100_000usize), |cb| {
                    use crate::thinker::prompt_budget::window_char_budget;
                    (
                        window_char_budget(cb.token_budget, 0.025, 20_000, 120_000),
                        window_char_budget(cb.token_budget, 0.10, 100_000, 480_000),
                    )
                });

        let cfg = self.prompt_extra_files.as_ref()?;
        if !cfg.enabled || cfg.paths.is_empty() {
            return None;
        }

        let mut out = Vec::new();
        let mut total = 0usize;
        for raw in &cfg.paths {
            if total >= total_max_chars {
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
            let budget = per_file_max_chars.min(total_max_chars - total);
            let capped = truncate_chars(&content, budget);
            total += capped.chars().count();
            out.push(ExtraPromptFile {
                // rust-doctor-disable-next-line excessive-clone
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
    /// remaining sections (curated/agent role) still render so a transient
    /// memory failure never blocks a turn. This matches the `Ok(None)`
    /// semantics already exposed by `MemoryContextProvider`'s builders and
    /// keeps the harness path resilient.
    ///
    /// The third tuple element is the per-run recall context (hybrid memory
    /// retrieval + routing experience). It is deliberately NOT welded into
    /// the system prompt: recall varies with the user query, and varying
    /// bytes in the system prompt sit ahead of every message-level
    /// prompt-cache breakpoint — re-keying the entire conversation prefix
    /// each run for every provider. The caller threads it into
    /// `HarnessDeps::recall_context`, where `think` appends it as a
    /// transient trailing user message (tail changes cost only themselves;
    /// codex initial-context / hermes frozen-system-prompt parity). The
    /// curated envelope stays in the Stable prefix — it is session-scoped
    /// and rarely changes.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
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
        routing_text: Option<String>,
        has_session_summaries: bool,
        envelope: &crate::thinker::TurnEnvelope,
    ) -> Option<(
        String,
        Vec<crate::thinker::prompt_builder::SystemPromptPart>,
        Option<String>,
    )> {
        use crate::providers::message::UnifiedMessage;
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        let prompt_build_start = Instant::now();

        // Phase 1 — fetch the eligible-skill snapshot once; reused below.
        let skill_snapshot = match self.skill_system.as_ref() {
            Some(sys) => Some(sys.current_snapshot().await),
            None => None,
        };

        let session_key_str = session_id.to_key_string();

        let memory_phase_start = Instant::now();
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

            // Codex-style session-start memory-index injection: the wiki
            // orientation envelope (schema + note index + recent-log tail)
            // rides the same pre-rendered stable envelope as curated memory
            // (`CuratedMemoryLayer` injects the merged string verbatim, and
            // its own tests already exercise a multi-XML-block envelope).
            // The builder gates itself on `MemoryInjectionMode::Tools` and on
            // a missing wiki handle, so deployments without notes stay
            // byte-identical. Same warn-and-degrade posture as curated above.
            // Read through the frozen per-(agent, session) path: orientation
            // lands in the Stable curated zone, so a per-build disk re-read
            // would churn the provider prompt-cache prefix whenever the wiki
            // mutated mid-session. Invalidation shares the curated snapshot's
            // eviction points (session end / post-compression).
            let orientation_text: Option<String> = match mcp
                .build_orientation_message_cached(agent_id, &session_key_str, mcp.injection_mode())
                .await
            {
                Ok(opt) => opt.as_ref().map(UnifiedMessage::text_content),
                Err(e) => {
                    tracing::warn!(
                        agent_id,
                        error = %e,
                        "build_orientation_message_cached failed; degrading orientation envelope to None"
                    );
                    None
                }
            };
            let curated_text = merge_stable_memory_envelopes(curated_text, orientation_text);

            let memory_text: Option<String> = if user_query.is_empty() {
                None
            } else {
                // Coordinate the one-shot memory injection with the per-turn
                // context budget so a large recall never forces the in-loop
                // compactor to over-trim recent history (memory lands in the
                // system prompt = un-compactable overhead). `None` when no
                // `[context_budget]` is configured → full configured budget.
                // The session key excludes this session's own end-of-session
                // resume snapshot from the "previous session" recall source.
                let headroom = self.memory_injection_headroom(session_id).await;
                match mcp
                    .build_memory_user_message(
                        agent_id,
                        user_query,
                        Some(&session_key_str),
                        headroom,
                    )
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
        let memory_phase_ms = memory_phase_start.elapsed().as_millis() as u64;

        let identity_phase_start = Instant::now();
        let agent_def = self.agent_registry.get(agent_id);

        // Load user-editable identity files from `~/.aleph/agents/{agent_id}/`
        // (SOUL.md / IDENTITY.md / AGENTS.md / TOOLS.md / HEARTBEAT.md). The
        // loader was previously only exercised from its own tests — wiring it
        // here is what gets `IdentityFilesLayer` (and the soul / profile layers
        // that read the same source) usable content on the harness path.
        // Tolerant of missing home / dir / IO failure: returns IdentityFiles
        // with all-None content, which the layer treats as "skip".
        // Identity-file caps scale with the model window (feature 1.3), same
        // window source as the prompt budget below; no `[context_budget]`
        // configured → ::default() (legacy 20k/100k, byte-identical).
        let identity_cfg = self.context_budget_config.as_ref().map_or_else(
            crate::thinker::identity_files::IdentityFilesConfig::default,
            |cfg| {
                crate::thinker::identity_files::IdentityFilesConfig::for_context_window(
                    cfg.token_budget,
                )
            },
        );
        let identity_files = crate::discovery::aleph_agents_dir().ok().map(|agents_dir| {
            crate::thinker::identity_files::IdentityFiles::load(
                &agents_dir.join(agent_id),
                &identity_cfg,
            )
        });
        let has_identity = identity_files
            .as_ref()
            .is_some_and(|f| f.files.iter().any(|file| file.content.is_some()));

        let has_skills = skill_snapshot
            .as_ref()
            .is_some_and(|s| !s.eligible_manifests.is_empty());
        let identity_phase_ms = identity_phase_start.elapsed().as_millis() as u64;

        let extra_files_phase_start = Instant::now();
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
        let extra_files_phase_ms = extra_files_phase_start.elapsed().as_millis() as u64;

        let mcp_phase_start = Instant::now();
        // Aggregate the free-text `instructions` connected MCP servers advertise
        // (→ `McpInstructionsLayer`). A cheap actor round-trip; `None` when no
        // manager is wired, the call fails, or nothing is advertised — the layer
        // then renders nothing. MCP resources/prompts are deliberately NOT indexed
        // into the prompt: the model discovers them on demand via
        // `mcp_list_resources` / `mcp_list_prompts`, whose doubled server-qualified
        // ids round-trip through the two-strip read path (a single-prefix eager
        // index id did not) — R7/R10 static partition, see
        // MODEL_PERCEIVABLE_ECOSYSTEM.md.
        let mcp_instructions = match &self.mcp_handle {
            Some(handle) => {
                let instructions = match handle.aggregate_instructions().await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(error = %e, "MCP aggregate_instructions failed; proceeding without server instructions");
                        Vec::new()
                    }
                };
                (!instructions.is_empty()).then_some(instructions)
            }
            None => None,
        };
        let mcp_phase_ms = mcp_phase_start.elapsed().as_millis() as u64;

        let runtime_caps_phase_start = Instant::now();
        // Surface the persisted runtime capability ledger so
        // `RuntimeCapabilitiesLayer` (priority 400) can tell the model which
        // managed runtimes are installed and their absolute executable paths —
        // letting it invoke `uv run` / a managed interpreter instead of bare
        // `python3`/`node`, which a GUI-launched daemon's minimal PATH may lack.
        // Best-effort: any IO error (or an empty ledger) leaves this `None` and
        // the layer emits nothing. Mirrors `ledger::build_enhanced_path`'s
        // on-disk load — pure data wiring, no cognition (R10-safe).
        let runtime_capabilities = crate::runtimes::get_runtimes_dir()
            .ok()
            .map(|dir| {
                let ledger =
                    crate::runtimes::CapabilityLedger::load_or_create(dir.join("ledger.json"));
                crate::runtimes::format_entries_for_prompt(&ledger.list_ready())
            })
            .filter(|s| !s.is_empty());
        let runtime_caps_phase_ms = runtime_caps_phase_start.elapsed().as_millis() as u64;

        // Sub-agent catalog: surface the delegatable agents so `AgentCatalogLayer`
        // (priority 1704) renders `<available_agents>`. This field had NO
        // production population site — the layer was inert and the model
        // discovered agents only reactively (by guessing an id and reading the
        // error). The delegatable set is the builtin sub-agents (explore / coder /
        // researcher / plan / verify) ∪ the registry's registered sub-agents
        // (user / project defs, folded in by `register_from_dirs`) ∪ the
        // plugin-shipped sub-agents published by `ExtensionManager::load_all`
        // (bridged via `crate::agents::plugin_subagents`). Precedence: registry
        // entries win id collisions over builtins (a user override), and plugin
        // agents fold in insert-if-absent so they never shadow a
        // builtin/user/project id — mirroring `AgentRegistry::resolve`'s three
        // passes so the catalog matches exactly what `delegate` can spawn.
        // Session-stable, so it sits in the cached stable prefix without re-keying.
        let available_agents = {
            use crate::agents::AgentMode;
            let mut by_id: std::collections::BTreeMap<String, crate::agents::AgentDef> =
                crate::agents::builtin_agents()
                    .into_iter()
                    .filter(|a| a.mode == AgentMode::SubAgent)
                    .map(|a| (a.id.clone(), a))
                    .collect();
            for a in self.agent_registry.list_subagents() {
                by_id.insert(a.id.clone(), a);
            }
            // Plugin sub-agents last, insert-if-absent (lowest precedence).
            for a in crate::agents::plugin_subagents().iter() {
                by_id.entry(a.id.clone()).or_insert_with(|| a.clone());
            }
            (!by_id.is_empty()).then(|| {
                by_id
                    .into_values()
                    .map(|a| crate::thinker::prompt_layer::AgentCatalogEntry {
                        id: a.id,
                        description: a.description,
                        when_to_use: a.when_to_use,
                    })
                    .collect::<Vec<_>>()
            })
        };

        // Skip prompt assembly entirely when there is nothing to inject:
        // no memory, no AgentDef, no eligible skills, no identity files, no
        // extra files, no MCP server instructions, no runtime capabilities, and
        // no delegatable agents to advertise.
        if curated_text.is_none()
            && memory_text.is_none()
            && agent_def.is_none()
            && !has_skills
            && !has_identity
            && extra_files.is_none()
            && mcp_instructions.is_none()
            && runtime_capabilities.is_none()
            && available_agents.is_none()
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
        // Tool schemas reach the model via native tool_use
        // (`with_tools(tools_ref)` in agent.rs), never as prompt text — which is
        // why the prompt is assembled with an empty tools slice. (`ToolsLayer`
        // and its `native_tools_enabled` opt-out were removed 2026-07-26: both
        // writers forced the flag on, and the `{reasoning, action}` text
        // envelope the layer's listings fed was deleted 2026-05-10.)
        // Model-aware system-prompt budget (feature 1.2): when a context budget
        // is configured, size the prompt char cap off the same chain-minimum
        // window the history side uses (feature 2.2), so large-window models
        // stop being capped at the fixed 80k default. No `[context_budget]`
        // configured → legacy fixed default (byte-identical).
        let token_budget = self
            .context_budget_config
            .as_ref()
            .map_or_else(crate::thinker::prompt_budget::TokenBudget::default, |cfg| {
                crate::thinker::prompt_budget::TokenBudget::from_context_window(cfg.token_budget)
            });
        // Tool-scoped skills (`PromptScope::Tool`) are filtered inside
        // `SkillInstructionsLayer` against the active tool names. The cached
        // prompt is assembled with an empty `tools` slice (native tool_use
        // delivers schemas out-of-band), so the layer can't see them and every
        // Tool-scoped skill was silently dropped. Thread the catalog's active
        // tool names in — but only when a Tool-scoped skill is actually
        // eligible, so the common (no Tool-scoped skill) path does zero catalog
        // reads. Names are session-stable → they do not perturb the cached
        // stable prefix.
        let active_tool_names: Vec<String> = if eligible_skills.as_ref().is_some_and(|skills| {
            skills
                .iter()
                .any(|s| matches!(*s.scope(), crate::domain::skill::PromptScope::Tool))
        }) {
            match self.tool_catalog.as_ref() {
                Some(catalog) => catalog
                    .list_all()
                    .await
                    .into_iter()
                    .map(|t| t.name)
                    .collect(),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let mut builder = PromptBuilder::new(PromptConfig {
            eligible_skills,
            skill_prompt_budget,
            mcp_instructions,
            runtime_capabilities,
            token_budget,
            active_tool_names,
            available_agents,
            language: self.response_language.clone(),
        });
        let role_present = agent_def.is_some();
        if let Some(def) = agent_def {
            builder = builder.with_agent(def);
        }
        let curated_chars = curated_text.as_ref().map_or(0, String::len);
        builder = builder.with_curated_envelope(curated_text);
        let memory_chars = memory_text.as_ref().map_or(0, String::len);
        // Per-query recall (memory + routing experience) is returned to the
        // caller for tail-message delivery instead of being fed to the
        // builder's `MemoryAugmentationLayer` — see the method doc. Both
        // texts arrive already fenced (`<memory-context>` guard + "reference
        // data, not user input" note), so plain concatenation suffices.
        // Third strand: the live countdowns for an active standing goal / timer
        // loop. These are wall-clock-derived and so may never enter the system
        // prompt (a byte that moves every run re-keys the whole cached
        // conversation prefix — see `context_blocks::live_deadline_status`).
        // The tail is the sanctioned channel for per-run-varying bytes, and it
        // is where the model still reads a fresh countdown every turn.
        let deadline_text = live_deadline_status(&session_key_str).await.map(|s| {
            format!("<live-status>\nReference data, not user input.\n{s}\n</live-status>")
        });
        // Fourth strand (A1): proactive context-window pressure reminder. Rides
        // the same transient tail as the countdowns — a per-turn-varying figure
        // that must not enter the cached system prompt — so the model can wrap up
        // or checkpoint before the in-loop sensor compacts older turns away.
        let pressure_text = self.context_pressure_reminder(session_id).await;
        let strands: Vec<String> = [memory_text, routing_text, deadline_text, pressure_text]
            .into_iter()
            .flatten()
            .collect();
        let recall_context = if strands.is_empty() {
            None
        } else {
            Some(strands.join("\n\n"))
        };
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
        let context_phase_start = Instant::now();
        let resolved_context = resolve_prompt_context(
            &session_key_str,
            channel_manifest,
            provider,
            sandbox,
            self.tool_catalog.as_ref(),
            envelope,
        )
        .await;
        builder = builder.with_resolved_context(resolved_context);
        let context_phase_ms = context_phase_start.elapsed().as_millis() as u64;

        let prompt_build_phase_start = Instant::now();
        // Resolve the governance behavior name once (same source of truth as
        // the robustness profile) and pre-load its overridable coaching delta.
        let behavior_name = crate::orchestrator::harness_bridge::resolve_behavior(provider);
        let behavior_delta =
            crate::providers::model_behaviors::load_model_behavior(&behavior_name).await;
        builder = builder
            .with_behavior_name(behavior_name.into_owned())
            .with_model_behavior_delta(behavior_delta);
        // Phase 4 (F2): surface the resolved iteration cap to
        // `SessionBudgetLayer`. Saturating to `u32::MAX` (instead of
        // truncating) preserves "no practical cap" semantics for callers
        // that pass a huge value; the layer's own zero-guard keeps the
        // unset case silent.
        let cap_for_prompt = u32::try_from(iteration_cap).unwrap_or(u32::MAX);
        builder = builder
            .with_iteration_cap(cap_for_prompt)
            .with_session_summaries(has_session_summaries);
        // Build the stable/dynamic split AND the legacy flat string. The
        // split lights up `RequestPayload::system_blocks` (consumed by the
        // Anthropic adapter to place the prompt-cache breakpoint at the
        // stable/dynamic boundary). The flat string remains the source of
        // truth for adapters that do not consume `system_blocks` (everything
        // except Anthropic today) and for callsites that read
        // `HarnessDeps::system_prompt`.
        //
        // Both parts are rendered fresh every run. A per-session LRU used to
        // pin the stable prefix, but every stable-layer input (identity files,
        // skills snapshot, strategy, curated memory, runtime capabilities) is
        // already loaded fresh above — the cache only skipped the pure string
        // render while silently serving stale content whenever a tool mutated
        // a stable input mid-session (a strategy welded after turn 1 never
        // reached the prompt; the token-estimate path polluted the entry with
        // its NoopSandbox posture). Layer renders are deterministic functions
        // of their inputs, so re-rendering stays byte-stable across runs when
        // nothing changed — the provider-side prefix cache is unaffected — and
        // a byte change now happens exactly when the content genuinely changed.
        let parts = builder.build_system_prompt_cached_with_mode(&[], self.default_prompt_mode);
        let prompt_build_phase_ms = prompt_build_phase_start.elapsed().as_millis() as u64;
        let prompt: String = parts.iter().map(|p| p.content.as_str()).collect();
        // Phase 6 observability — confirm BUG-2/BUG-3 wiring at runtime.
        // Logs character counts (not contents) so prompts are observable
        // without leaking memory content to disk-side telemetry.
        let stable_chars: usize = parts
            .iter()
            .filter(|p| p.cache)
            .map(|p| p.content.chars().count())
            .sum();
        let dynamic_chars: usize = parts
            .iter()
            .filter(|p| !p.cache)
            .map(|p| p.content.chars().count())
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
            memory_phase_ms,
            identity_phase_ms,
            extra_files_phase_ms,
            mcp_phase_ms,
            runtime_caps_phase_ms,
            context_phase_ms,
            prompt_build_phase_ms,
            total_ms = prompt_build_start.elapsed().as_millis() as u64,
            "system prompt assembled"
        );
        Some((prompt, parts, recall_context))
    }
}

/// Resolve the channel/runtime context block that feeds
/// `with_resolved_context`. Extracted from `build_system_prompt` to keep the
/// latter's cyclomatic complexity under control.
async fn resolve_prompt_context(
    session_key_str: &str,
    channel_manifest: Option<&crate::thinker::InteractionManifest>,
    provider: &dyn AiProvider,
    sandbox: &dyn Sandbox,
    tool_catalog: Option<&Arc<crate::tool_metadata::ToolCatalog>>,
    envelope: &crate::thinker::TurnEnvelope,
) -> crate::thinker::context::ResolvedContext {
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
    // Populate `runtime_context` — the single source of the environment
    // envelope's facts, split across the two prompt zones that own them:
    // `EnvironmentLayer` (Stable @300) renders the process-invariant half
    // (OS/arch, shell, host) and `RuntimeContextLayer` (Dynamic @1720) the
    // per-run half (cwd, repo, git branch, model, local time). The two used to
    // overlap on OS *and* cwd; the cwd copy in the Stable prefix was also wrong,
    // reading the daemon's `current_dir()`.
    //
    // `envelope.cwd` is the run's EFFECTIVE workspace — the same path the gateway
    // gives the tool adapters as `default_working_dir`, so the advertised `cwd=`
    // is the directory a shell call actually lands in, and `repo=` / `git=`
    // describe the project the model is working on rather than the daemon's own
    // checkout. `repo_root` resolution is cached per directory (no `git`
    // subprocess); the branch is re-read from `.git/HEAD` on every render so a
    // mid-session `checkout` shows up next turn.
    resolved_context.runtime_context =
        Some(crate::thinker::runtime_context::RuntimeContext::collect_in(
            provider.name(),
            envelope.cwd.as_deref(),
        ));
    // Populate runtime-state fragments from the tool catalog's
    // `ToolHealthCache`. Each currently-cached `Unhealthy` entry becomes
    // a `RuntimeStateFragment::unavailable(name, reason)` that
    // `ToolRuntimeStateLayer` @1703 (Dynamic) renders into `<tool_runtime_state>`.
    // `None` tool_catalog (test / early boot) → empty vec → the
    // layer emits nothing.
    resolved_context.runtime_state_blocks = compute_runtime_state_blocks(tool_catalog);
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
    // `ExecutionPlanLayer` @1756 renders it as `<execution_plan>`.
    //
    // The execution-plan, standing-goal, and strategy lookups are
    // independent session-keyed reads (a scratchpad file read, a goal-store
    // read with a wall-clock stamp, and a strategy-store read). Run them
    // concurrently with `tokio::join!` so prompt assembly — on the hot
    // per-turn path — pays the max of the three latencies, not their sum.
    // `join!` polls all on the current task, so there is no spawn cost and
    // no extra `Send` bound; all futures take a shared `&session_key_str`
    // borrow, which co-exist fine.
    let (exec_plan, standing, timer_loop, strategy) = tokio::join!(
        active_execution_plan(session_key_str),
        active_standing_goal(session_key_str),
        active_timer_loop(session_key_str),
        active_strategy(session_key_str),
    );
    resolved_context.execution_plan = exec_plan;
    resolved_context.standing_goal = standing;
    // Governance-topology context: a mechanical session-keyed lookup against
    // the loop-graph store (mirrors the standing-goal read; two indexed
    // SELECTs when the graph exists, one when it is empty, zero when the
    // subsystem never booted). Deterministic bytes — unchanged graph leaves
    // the prompt byte-identical (cache-safe).
    resolved_context.graph_topology =
        crate::loop_graph::service::render_session_topology(session_key_str);
    resolved_context.timer_loop = timer_loop;
    // Render the welded Strategy into its two prompt surfaces: the full
    // `<strategy>` body for the Stable `StrategyLayer` (cacheable head) and
    // the guardrail-only echo for the Dynamic `StrategyPointerLayer` (per-
    // turn tail near the read head). Both renders are pure/deterministic
    // (no timestamps). `None` Strategy leaves both fields `None`, so both
    // layers emit nothing and the prompt is byte-identical.
    if let Some(s) = strategy {
        resolved_context.strategy = Some(crate::strategy::render_strategy_summary(&s));
        resolved_context.strategy_guardrails = Some(crate::strategy::render_guardrails_only(&s));
    }
    // Voice mode: read the session-keyed flag the gateway inbound router set
    // for this turn so `VoiceModeLayer` (priority 1710) injects the
    // spoken-reply guidelines. Mirrors `execution_plan` / `standing_goal` —
    // a mechanical session-keyed lookup, no judgment. `Off` (no voice)
    // leaves the prompt byte-identical; the `transcribed` bit distinguishes
    // a spoken-only turn from one whose input was ASR-transcribed.
    resolved_context.voice = match crate::gateway::voice::voice_mode::get(session_key_str) {
        None => crate::thinker::context::VoiceContext::Off,
        Some(false) => crate::thinker::context::VoiceContext::Spoken,
        Some(true) => crate::thinker::context::VoiceContext::SpokenTranscribed,
    };
    // Approval regime (codex `<approval_policy>` parity): the turn's resolved
    // exec tier, already computed by the gateway with request/session/global
    // precedence and the channel clamp applied. Threaded through here rather
    // than re-derived so the prompt shows the exact tier the tool gate enforces.
    // `None` on internal / subagent dispatch leaves the approval line absent.
    resolved_context.approval_tier = envelope.exec_tier;
    // Usage-mode register (chat / work / code): the turn's resolved session
    // mode, threaded through the same way as the tier so the prompt names the
    // exact partition the tool surface was built with. `None` on internal /
    // subagent dispatch leaves the mode line absent.
    resolved_context.session_mode = envelope.session_mode;
    resolved_context
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

/// Merge the curated-memory and wiki-orientation envelopes into the single
/// pre-rendered stable string `CuratedMemoryLayer` injects verbatim. Both are
/// already self-contained XML blocks, so a newline join suffices; either side
/// missing passes the other through unchanged, and both missing stays `None`
/// so the layer emits nothing.
fn merge_stable_memory_envelopes(
    curated: Option<String>,
    orientation: Option<String>,
) -> Option<String> {
    match (curated, orientation) {
        (Some(c), Some(o)) => Some(format!("{c}\n{o}")),
        (c, o) => c.or(o),
    }
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
        // rust-doctor-disable-next-line excessive-clone
        FlowInput::Prompt(s) => s.clone(),
        FlowInput::Messages(msgs) | FlowInput::Multimodal(msgs) => msgs
            .iter()
            .rev()
            .map(text_of)
            .find(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_default(),
        // rust-doctor-disable-next-line excessive-clone
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

#[cfg(test)]
mod orientation_wiring_tests {
    use super::merge_stable_memory_envelopes;
    use crate::config::types::memory::MemoryInjectionMode;
    use crate::error::AlephError;
    use crate::memory::notes::orientation::types::{
        IndexStats, LogEntry, OrientationSnapshot, TokenBudget,
    };
    use crate::memory::notes::orientation::NoteOrientation;
    use crate::sync_primitives::Arc;
    use crate::thinker::MemoryContextProvider;
    use async_trait::async_trait;

    struct FixedOrient;

    #[async_trait]
    impl NoteOrientation for FixedOrient {
        async fn bootstrap(&self, _: &str) -> Result<(), AlephError> {
            Ok(())
        }
        async fn read_snapshot(
            &self,
            _: &str,
            _: TokenBudget,
        ) -> Result<OrientationSnapshot, AlephError> {
            Ok(OrientationSnapshot {
                schema_text: "# Memory Schema".into(),
                index_text: "- [[learning/rust]] — orientation-index-marker".into(),
                recent_log_tail: "## [2026-07-17] ingest | touched=1".into(),
            })
        }
        async fn record_ingest(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_query(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_lint(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn record_session_end(&self, _: &str, _: LogEntry) -> Result<(), AlephError> {
            Ok(())
        }
        async fn rebuild_index(&self, _: &str) -> Result<IndexStats, AlephError> {
            Ok(IndexStats::default())
        }
        async fn rotate_log_if_needed(&self, _: &str) -> Result<bool, AlephError> {
            Ok(false)
        }
        fn invalidate(&self, _: &str, _: &str) {}
    }

    #[test]
    fn merge_passes_through_single_sides_and_joins_both() {
        assert_eq!(merge_stable_memory_envelopes(None, None), None);
        assert_eq!(
            merge_stable_memory_envelopes(Some("c".into()), None).as_deref(),
            Some("c")
        );
        assert_eq!(
            merge_stable_memory_envelopes(None, Some("o".into())).as_deref(),
            Some("o")
        );
        assert_eq!(
            merge_stable_memory_envelopes(Some("c".into()), Some("o".into())).as_deref(),
            Some("c\no")
        );
    }

    /// End-to-end over the exact wiring `build_system_prompt` performs:
    /// provider-owned mode gates the orientation builder (read through the
    /// frozen per-(agent, session) path), the envelope merges with curated,
    /// and `PromptBuilder::with_curated_envelope` lands it in the assembled
    /// prompt.
    #[tokio::test]
    async fn orientation_envelope_lands_in_assembled_prompt() {
        use crate::providers::message::UnifiedMessage;
        use crate::thinker::prompt_builder::{PromptBuilder, PromptConfig};

        let mcp = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Context)
            .with_orientation(Arc::new(FixedOrient));

        let orientation_text = mcp
            .build_orientation_message_cached("agent-1", "agent:agent-1:main", mcp.injection_mode())
            .await
            .unwrap()
            .as_ref()
            .map(UnifiedMessage::text_content);
        let envelope = merge_stable_memory_envelopes(None, orientation_text);
        assert!(envelope.is_some(), "context mode must produce an envelope");

        let prompt = PromptBuilder::new(PromptConfig::default())
            .with_curated_envelope(envelope)
            .build_system_prompt(&[]);

        assert!(
            prompt.contains("<NoteOrientation>"),
            "orientation envelope must land in the assembled prompt:\n{prompt}"
        );
        assert!(
            prompt.contains("orientation-index-marker"),
            "index snapshot content must survive assembly:\n{prompt}"
        );
    }

    /// Tools mode: the provider's own mode must gate the builder to `None`,
    /// keeping the assembled prompt free of the orientation envelope.
    #[tokio::test]
    async fn orientation_skipped_when_provider_mode_is_tools() {
        let mcp = MemoryContextProvider::new_for_test_empty_envelope(MemoryInjectionMode::Tools)
            .with_orientation(Arc::new(FixedOrient));

        let msg = mcp
            .build_orientation_message_cached("agent-1", "agent:agent-1:main", mcp.injection_mode())
            .await
            .unwrap();
        assert!(msg.is_none(), "Tools mode must not auto-inject orientation");
    }
}
