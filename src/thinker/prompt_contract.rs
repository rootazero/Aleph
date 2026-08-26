//! Structural guards on the assembled system prompt.
//!
//! Two failure modes have cost this module more than any bug in any single
//! layer, and neither is visible in a layer's own unit test — a layer's tests
//! construct exactly the input that makes it speak, so a layer that nothing
//! feeds in production still passes every one of them:
//!
//! 1. **Silent by omission.** A layer is registered, tested, and unreachable —
//!    its `paths()` names a path no caller requests, or its gate reads a
//!    `PromptConfig` field no production code writes. Found and removed by hand
//!    four times (`InboundContextLayer`, `SessionResumeLayer`,
//!    `HydratedToolsLayer`, `SkillModeLayer`), then five more on 2026-07-26
//!    (`ToolsLayer`, `ToolUsageGrammarLayer`, `ThinkingGuidanceLayer`,
//!    `GenerationModelsLayer`, `CustomInstructionsLayer`).
//! 2. **Silent growth.** No single commit adds much; the always-on prompt
//!    drifts up a few hundred bytes at a time until someone measures it again.
//!
//! [`reachable_layers`] catches the first, [`scaffold_bytes_ratchet`] and
//! [`no_sentence_is_stated_twice`] the second. All three are cheap and run in
//! the normal `--lib` pass.

use crate::thinker::context::{ContextAggregator, ResolvedContext};
use crate::thinker::interaction::{InteractionManifest, InteractionParadigm};
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_pipeline::PromptPipeline;
use crate::thinker::security_context::SecurityContext;
use crate::thinker::PromptConfig;

/// Layers that legitimately stay silent under the production-shaped input
/// below, each with the per-session content that would wake it.
///
/// **This list is the point of the test.** Adding a name is a deliberate act
/// that says "no fixed input makes this layer speak, and here is the session
/// content that does". A layer that cannot be justified in one line does not
/// belong in the pipeline.
///
/// **What an entry here does NOT say — and the hole that opens under a Dynamic
/// layer.** It says only "no *fixed* input wakes this". It says nothing about
/// how big the layer gets when session content *does* wake it, and
/// [`dynamic_tail_bytes_ratchet`] measures the same fixed input, so for a
/// Dynamic layer on this list the answer is always 0 B — a number that cannot
/// distinguish "renders one short sentence" from "renders whatever the user
/// pasted". `graph_topology` sat here for two weeks interpolating uncapped
/// human-authored root bodies straight into the zone that re-writes at 1.25x,
/// and the placement was defended as correct — which it is; it was the
/// *unboundedness* that nothing had ever measured.
///
/// So a Dynamic entry on this list owes a bound of its own, asserted wherever
/// the waking input is cheap to construct — which is the layer's own module when
/// the layer builds its own text, and the *producer's* module when the layer is
/// a pass-through. Both shapes exist:
///
/// * `layers::memory_window::worst_case_render_is_bounded` — bounded by
///   construction; the layer picks one of three consts and interpolates nothing.
/// * `loop_graph::service::render_is_bounded_against_oversized_graph_rows` —
///   bounded by cap. `GraphTopologyLayer` only escapes and emits whatever
///   `render_session_topology` handed it, so the bound has to live at the
///   producer; asserting it on the layer would only pin that a `String` is as
///   long as it is.
const CONDITIONALLY_SILENT: &[(&str, &str)] = &[
    ("soul", "agent SOUL.md on disk"),
    ("agent_role", "a registered sub-agent's AgentDef"),
    ("curated_memory", "a non-empty MEMORY.md hot zone"),
    (
        "memory_window",
        "a non-empty MEMORY.md hot zone, or an auto-recalled <memory-context>",
    ),
    ("strategy", "an active StraTA strategy"),
    ("strategy_pointer", "an active strategy's guardrails"),
    ("chain_context", "a sub-agent delegation chain (depth > 0)"),
    (
        "mcp_instructions",
        "a connected MCP server advertising instructions",
    ),
    ("voice_mode", "a voice-transcribed turn"),
    ("profile", "agent AGENTS.md on disk"),
    (
        "runtime_capabilities",
        "detected Python / Node / FFmpeg runtimes",
    ),
    ("tool_runtime_state", "a tool-health probe result"),
    ("provider_guidance", "a model_behaviors/{family}.md delta"),
    ("skill_instructions", "an eligible skill in the snapshot"),
    (
        "doctor_repair_hint",
        "a WebRich session with a failing doctor check",
    ),
    (
        "identity_files",
        "IDENTITY.md / TOOLS.md / HEARTBEAT.md on disk",
    ),
    (
        "extra_files",
        "[prompt.extra_files] configured and non-empty",
    ),
    (
        "session_context_guide",
        "a history carrying compaction summaries",
    ),
    ("timer_loop", "an active watch loop in this session"),
    ("graph_topology", "a session governed by a loop-graph"),
    ("standing_goal", "an active standing goal"),
    ("execution_plan", "a scratchpad plan with at least one item"),
    (
        "room_roster",
        "a project-room session with two or more members",
    ),
    ("language", "[general] language configured"),
];

/// Every paradigm, because several layers are paradigm-exclusive: Background
/// alone enables protocol tokens and operational awareness, while the
/// interactive paradigms alone get multi-step narration. Requiring a layer to
/// speak under *some* paradigm is the honest bar.
const PARADIGMS: &[InteractionParadigm] = &[
    InteractionParadigm::Background,
    InteractionParadigm::CLI,
    InteractionParadigm::Messaging,
    InteractionParadigm::WebRich,
    InteractionParadigm::Embedded,
];

fn resolve(paradigm: InteractionParadigm) -> ResolvedContext {
    let mut ctx = ContextAggregator::resolve(
        &InteractionManifest::new(paradigm),
        &SecurityContext::for_paradigm(paradigm),
    );
    // The four fields below are present on EVERY gateway turn, so a guard that
    // leaves them unset is not measuring the always-on prompt. `runtime_context`
    // was even excused in `CONDITIONALLY_SILENT` — and that excuse is exactly how
    // the `EnvironmentLayer` / `RuntimeContextLayer` OS+cwd duplication survived
    // unmeasured, alongside `Approval mode:` (206 B), `Usage mode:` (353 B) and
    // the sandbox bullets: an arbitrarily long sentence could be added to any of
    // them with both the byte ratchet and the duplicate-sentence guard green.
    //
    // The rule this encodes: **present on every production turn ⇒ must be
    // measured; genuinely conditional ⇒ may go in `CONDITIONALLY_SILENT`.**
    //
    // All four use FIXED stand-ins rather than live collection, so the ceiling is
    // machine-independent — a developer's long `cwd`, hostname, or workspace path
    // must not move the measured number.
    ctx.runtime_context = Some(fixed_runtime_context());
    ctx.approval_tier = Some(crate::config::types::policies::ExecTier::default());
    ctx.session_mode = Some(crate::config::types::policies::SessionMode::default());
    ctx.sandbox_summary = Some(fixed_sandbox_summary());
    ctx
}

/// Sandbox posture frozen at representative widths, for the same
/// machine-independence reason as [`fixed_runtime_context`].
fn fixed_sandbox_summary() -> crate::sandbox::SandboxSummary {
    crate::sandbox::SandboxSummary {
        // Deliberately names a DIFFERENT os than `fixed_runtime_context().os`:
        // `no_environment_fact_is_stated_twice` matches on the fact's value, and a
        // `linux/bwrap` backend would collide with `os = "linux"` as a substring —
        // a false positive about a genuinely different fact.
        backend: "macos/seatbelt",
        policy_tier: crate::sandbox::PolicyTier::WorkspaceWrite.as_str(),
        // Same width as `fixed_runtime_context().working_dir`, deliberately a
        // DIFFERENT value. In production the two usually coincide, but they are
        // different facts — "where you may write" vs "where you are" — and the
        // guard matches on value, so equal strings would read as a duplicate.
        writable_roots: vec![std::path::PathBuf::from("/home/u/projects/demo-wsp")],
        network: crate::sandbox::NetworkState::AllowAll,
        max_memory_mb: Some(512),
        // `None` here keeps the `Permission profile:` bullet out of the
        // scaffold measurement; adding the field to production dispatch
        // wiring automatically starts measuring it on every paradigm where
        // it lights up. See §2.3 round.
        permission_profile_id: None,
    }
}

/// Machine facts frozen at representative widths, so `scaffold_bytes_ratchet`
/// measures the layer set rather than the machine it runs on.
fn fixed_runtime_context() -> crate::thinker::runtime_context::RuntimeContext {
    crate::thinker::runtime_context::RuntimeContext {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        shell: "bash".to_string(),
        working_dir: Some(std::path::PathBuf::from("/home/u/.aleph/workspaces/main")),
        repo_root: None,
        // Model-ID shaped, deliberately NOT a provider name. In production
        // this is `TurnEnvelope::serving_model` (= `runner_impl`'s
        // `gauge_model`). It used to fall back to `provider.name()`, which is
        // `"failover"` on every real stack — and a fixture holding a provider
        // name would have mirrored that defect rather than exposing it.
        current_model: "claude-sonnet-4-5-20250929".to_string(),
        hostname: "aleph-host".to_string(),
        current_time: "2026-07-26 12:00".to_string(),
        timezone: "UTC".to_string(),
    }
}

/// The `PromptConfig` half of the production-shaped input.
///
/// Only `available_agents` is populated, and only with the **builtin** sub-agent
/// set — the compiled-in floor every install has, so the measurement stays
/// machine-independent for the same reason [`fixed_runtime_context`] does.
/// Registry and plugin agents fold into the same field in production
/// (`harness_bridge::prompt_build`) but vary per install, so they are left out:
/// this measures the floor, not one developer's machine.
///
/// **Why it is here at all.** `AgentCatalogLayer` sat in [`CONDITIONALLY_SILENT`]
/// excused as "at least one switchable agent registered" — but the production
/// builder seeds the field from `builtin_agents()`, which is never empty, so the
/// layer is *always on* and its ~1.7 KB was outside the ratchet's field of view.
/// Same shape as the `runtime_context` excuse this module's history already
/// records: an entry on that list is a claim that no fixed input makes the layer
/// speak, and the claim was false.
fn production_config() -> PromptConfig {
    use crate::agents::AgentMode;
    let available_agents: Vec<crate::thinker::prompt_layer::AgentCatalogEntry> =
        crate::agents::builtin_agents()
            .into_iter()
            .filter(|a| a.mode == AgentMode::SubAgent)
            .map(|a| crate::thinker::prompt_layer::AgentCatalogEntry {
                id: a.id,
                description: a.description,
                when_to_use: a.when_to_use,
            })
            .collect();
    assert!(
        !available_agents.is_empty(),
        "the builtin sub-agent set is empty, so this fixture no longer measures the \
         always-on agent catalog. Either the builtins moved (update this) or the catalog \
         really is conditional (put it back in CONDITIONALLY_SILENT with the reason)."
    );
    PromptConfig {
        available_agents: Some(available_agents),
        ..Default::default()
    }
}

/// The input the main loop builds, minus per-session content — the same shape
/// `aleph-server prompt-size` reports on.
fn production_shaped<'a>(config: &'a PromptConfig, context: &'a ResolvedContext) -> LayerInput<'a> {
    LayerInput::basic(config, &[])
        .with_resolved_context_opt(Some(context))
        .with_behavior_name("anthropic")
        .with_iteration_cap(1000)
}

/// Every registered layer must either contribute to a production-shaped prompt
/// under at least one paradigm, or be listed in [`CONDITIONALLY_SILENT`] with
/// the session content that wakes it.
///
/// A layer that is neither is dead weight: it costs a file, a registration, a
/// slot in every `paths()` review, and a reader's attention — and renders
/// nothing. Delete it, or wire the input that makes it speak.
#[test]
fn reachable_layers() {
    let pipeline = PromptPipeline::default_layers();
    let config = production_config();

    let mut ever_spoke: Vec<&'static str> = Vec::new();
    for &paradigm in PARADIGMS {
        let context = resolve(paradigm);
        let input = production_shaped(&config, &context);
        for l in pipeline.layer_breakdown(AssemblyPath::Cached, &input, PromptMode::Full) {
            if !ever_spoke.contains(&l.name) {
                ever_spoke.push(l.name);
            }
        }
    }

    let registered: Vec<&'static str> = pipeline
        .layer_info()
        .into_iter()
        .map(|(_, name, _)| name)
        .collect();

    let unexplained: Vec<&str> = registered
        .iter()
        .copied()
        .filter(|name| !ever_spoke.contains(name))
        .filter(|name| !CONDITIONALLY_SILENT.iter().any(|(n, _)| n == name))
        .collect();
    assert!(
        unexplained.is_empty(),
        "these layers render nothing for any paradigm under a production-shaped input and are \
         not declared conditionally-silent: {unexplained:?}\n\
         Either delete the layer, wire the input that feeds it, or add it to \
         CONDITIONALLY_SILENT with the session content that wakes it."
    );

    // The allowlist must not outlive its entries: a name listed here that no
    // longer exists is a stale claim about a layer nobody can find.
    let ghosts: Vec<&str> = CONDITIONALLY_SILENT
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !registered.contains(n))
        .collect();
    assert!(
        ghosts.is_empty(),
        "CONDITIONALLY_SILENT names layers that are no longer registered: {ghosts:?}"
    );
}

/// Byte ceiling for the always-on prompt scaffold, mirroring the ratchet that
/// guards `src/harness/`'s line budget.
///
/// **Measured, never hand-computed. Only ever lowered.** Raising it is allowed
/// but is a decision, not a formality — put the three answers in the commit
/// message:
///
///   1. Is this a runtime fact the model cannot know, or am I teaching a strong
///      model how to think? Only the first earns bytes.
///   2. Does a single tool own this sentence? Then it belongs in that tool's
///      `DESCRIPTION`, which ships with its schema — not in every request.
///   3. Would a stronger model still need it next quarter? If not it is a cage,
///      and cages get worse as models improve.
///
/// History: **5,913 B measured 2026-07-26 (§2.3 envelope round, via this very test with the ceiling temporarily set to 1)** — worst
/// paradigm still WebRich. The jump from 5,140 is **not new prompt content**: it
/// is the same bytes production always sent, now finally inside the ratchet's
/// field of view. `production_shaped` left `runtime_context`, `approval_tier`,
/// `session_mode` and `sandbox_summary` unset — all four are set on every gateway
/// turn — so `RuntimeContextLayer` in full, `Approval mode:` (206 B),
/// `Usage mode:` (353 B) and the sandbox bullets were unmeasured, and an
/// arbitrarily long sentence could be added to any of them with this test green.
/// The three answers for the raise:
///   1. **Runtime fact, not teaching.** Every added byte is something the model
///      cannot know: which directory it is in, which shell it will get, whether a
///      mutating call pauses for a human, which tool families are deferred.
///   2. **No single tool owns them.** They are cross-tool operating constraints —
///      exactly what a system prompt is for; a per-tool `DESCRIPTION` cannot state
///      "this whole session is in Ask mode".
///   3. **A stronger model still needs them.** They are environment state, not
///      scaffolding for weak reasoning; a better model uses them better.
///
/// **7,495 B measured 2026-08-03 (§2.18 ledger item 8), worst paradigm still
/// WebRich.** Same kind of raise as the one below and for the same reason: not
/// one new byte of prompt content, just bytes production always sent finally
/// inside the ratchet's field of view. `AgentCatalogLayer` — measured **1,705 B**,
/// the single largest layer in the prompt — was excused in `CONDITIONALLY_SILENT`
/// as "at least one switchable agent registered", but the production builder
/// seeds `available_agents` from `builtin_agents()`, which is never empty. The
/// layer is always on; the excuse was simply false, and an arbitrarily long
/// agent catalog could have been added with this test green. `production_config`
/// now supplies the builtin floor.
/// The three answers for the raise:
///   1. **Runtime fact, not teaching.** Which sub-agents this install has, and
///      what each is for, is state the model cannot derive. Without it the model
///      discovered agents reactively — guess an id, read the error.
///   2. **No single tool owns it.** `delegate` is the nearest candidate, but a
///      tool `DESCRIPTION` is a `const &str` and this is a per-install registry
///      (builtins ∪ user/project defs ∪ plugin agents). A constant cannot
///      enumerate it.
///   3. **A stronger model still needs it.** It is an inventory, not reasoning
///      scaffolding; a better model uses a catalog better, it does not infer one.
///
/// The ceiling is now honest, so the next real growth is catchable. Prior entry:
/// 5,140 B measured 2026-07-26 — the **worst paradigm**, WebRich
/// (`aleph-server prompt-size --path cached --paradigm webrich`); Background,
/// the daemon default, is 4,904 B. The ceiling is the max rather than one
/// chosen paradigm because no paradigm dominates: Background alone gets
/// `protocol_tokens` + `operational_guidelines`, WebRich alone gets
/// `multi_step_conduct` + `doctor_repair_hint` and a fuller `environment`. Any
/// fixed pick would let growth hide in the paradigms it does not measure.
///
/// The pi-leanness round that set this removed 2,597 B net —
/// `special_actions` 1,234 → 313 and `memory_protocol` 2,938 → 1,187, both by
/// moving per-tool how-to into the tool `DESCRIPTION`s that already stated it,
/// less 75 B for the parallel-dispatch fact rescued into `role`.
///
/// **Updated 2026-08-06 (§2.3 round)**: `RuntimeContext::to_dynamic_line` →
/// `RuntimeContext::to_environment_context_block`. The new XML format is
/// 8 elements (`<environment_context> <cwd> <repo> <git> <model> <time>`,
/// plus the open/close pair), where the previous markdown format used
/// `<key>=<value>` pairs in a single-line pipe-separated block. The byte
/// delta is approximately `+2 × 8 ≈ 16` per element × 6 fixed elements
/// = ~96 bytes per render, of which ~69 hit the `production_shaped`
/// fixture below (cwd/repo/git are filled in by the test, model/time are
/// constant strings). **Content is unchanged**: the same facts the model
/// saw before, now inside a tag-delimited region downstream tooling can
/// match on (`<environment_context>` start/end, codex parity). No `prompt`
/// prose was added.
const SCAFFOLD_CEILING_BYTES: usize = 7_600;

/// No paradigm's fixed scaffold may grow past the ceiling.
#[test]
fn scaffold_bytes_ratchet() {
    let pipeline = PromptPipeline::default_layers();
    let config = production_config();

    let mut worst: Option<(InteractionParadigm, usize, Vec<(&str, usize)>)> = None;
    for &paradigm in PARADIGMS {
        let context = resolve(paradigm);
        let input = production_shaped(&config, &context);
        let breakdown = pipeline.layer_breakdown(AssemblyPath::Cached, &input, PromptMode::Full);
        let total: usize = breakdown.iter().map(|l| l.bytes).sum();
        if worst.as_ref().is_none_or(|(_, w, _)| total > *w) {
            let mut largest: Vec<(&str, usize)> =
                breakdown.iter().map(|l| (l.name, l.bytes)).collect();
            largest.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
            largest.truncate(5);
            worst = Some((paradigm, total, largest));
        }
    }

    let (paradigm, total, largest) = worst.expect("PARADIGMS is non-empty");
    assert!(
        total <= SCAFFOLD_CEILING_BYTES,
        "always-on prompt scaffold grew to {total} B under {paradigm:?} \
         (ceiling {SCAFFOLD_CEILING_BYTES}). Largest layers: {largest:?}. Answer the three \
         questions documented on SCAFFOLD_CEILING_BYTES before raising it."
    );
}

/// Byte ceiling for the **dynamic** system block — the half that no
/// `cache_control` marker of its own ever covers.
///
/// This ratchet exists because "it's only in the dynamic tail" was used as a
/// reason to stop worrying about a layer's size, and that reasoning is wrong.
/// Anthropic builds its prefix tools → system → messages, and
/// `split_system_blocks_for_cache` stamps the marker on the **stable** block
/// only. So the dynamic block is covered by nothing but the message-level
/// breakpoints, every one of which sits *after* it: unchanged bytes parked here
/// do not *cause* a cache miss, but they are re-written at 1.25x every time any
/// genuinely volatile neighbour moves. A big session-stable layer in this zone
/// pays its neighbours' volatility tax forever.
///
/// That is not hypothetical — `agent_catalog`, `identity_files` (default cap
/// 100 000 chars) and `extra_files` all sat here until 2026-08-03, and
/// `memory_protocol`'s `stability()` carried a comment asserting the placement
/// was free, cited as precedent by another layer. See FEATURE_LOCATOR §2.18
/// ledger item 10.
///
/// **Measured, never hand-computed. Only ever lowered** — with one honest
/// exception: raising it because a layer genuinely varies per turn and had to
/// move *out* of the stable prefix (that is a correctness fix, and the raise is
/// the price). Raising it to park static content here is the thing this guard
/// forbids; move the content below priority 1700 instead.
/// Prior entry: 2,054 B measured 2026-08-03, worst paradigm WebRich, immediately
/// after `agent_catalog` / `identity_files` / `extra_files` moved out.
/// `memory_protocol` (1,037 B) and `operating_envelope` (633 B) were most of
/// what was left.
///
/// **1,017 B measured 2026-08-03**, same day, worst paradigm still WebRich —
/// the follow-up the entry above deferred, now taken. `memory_protocol` carried
/// two things with opposite cache profiles: a constant destination ladder and a
/// per-turn window claim. A layer gets one `stability()` for both, so the pair
/// was rated by its volatile half and the constant rode the unmarked block. It
/// is now two layers — the ladder at @1105/Stable keeping the name, the claim at
/// @1745/Dynamic as `memory_window`. **Not one byte of prompt content changed**;
/// this is the same text billed differently.
///
/// The measurement is worth recording because it inverted the intuition that
/// justified the split: under this test's input **both** window-claim gates are
/// false, so the entire 1,037 B was the constant, and the volatile sentence that
/// earned the layer its Dynamic rating contributed **nothing** to the number it
/// was blamed for. A layer's rating is about the bytes that *can* vary; its
/// measured size here is about the bytes that *do* render. Those are different
/// questions, and this ratchet only ever answers the second one.
///
/// **Updated 2026-08-06 (§2.3 round)**: the `<environment_context>` XML
/// block now renders into the dynamic tail where the old
/// `## Runtime Environment` line did. Same content, same cache lifetime;
/// the byte delta tracks `SCAFFOLD_CEILING_BYTES` and is +~69 B for the
/// reason noted there.
const DYNAMIC_TAIL_CEILING_BYTES: usize = 1_100;

/// The uncached half of the system prompt may not grow past its ceiling.
#[test]
fn dynamic_tail_bytes_ratchet() {
    let pipeline = PromptPipeline::default_layers();
    let config = production_config();

    let mut worst: Option<(InteractionParadigm, usize, Vec<&'static str>)> = None;
    for &paradigm in PARADIGMS {
        let context = resolve(paradigm);
        let input = production_shaped(&config, &context);
        let total = pipeline
            .execute_dynamic_with_mode(AssemblyPath::Cached, &input, PromptMode::Full)
            .len();
        if worst.as_ref().is_none_or(|(_, w, _)| total > *w) {
            let dynamic_names: Vec<&'static str> = pipeline
                .layer_info()
                .into_iter()
                .filter(|(_, _, stability)| {
                    *stability == crate::thinker::prompt_layer::LayerStability::Dynamic
                })
                .map(|(_, name, _)| name)
                .collect();
            worst = Some((paradigm, total, dynamic_names));
        }
    }

    let (paradigm, total, dynamic_names) = worst.expect("PARADIGMS is non-empty");
    assert!(
        total <= DYNAMIC_TAIL_CEILING_BYTES,
        "the uncached dynamic system block grew to {total} B under {paradigm:?} \
         (ceiling {DYNAMIC_TAIL_CEILING_BYTES}). Dynamic layers: {dynamic_names:?}. \
         Before raising this: does the layer that grew actually vary per turn? If it is \
         session-stable, give it a priority below 1700 and declare \
         `LayerStability::Stable` — in this zone it is re-written at 1.25x every time a \
         volatile neighbour moves."
    );
}

// ---------------------------------------------------------------------------
// Per-layer bounds for conditionally-silent Dynamic layers
// ---------------------------------------------------------------------------
//
// `dynamic_tail_bytes_ratchet` measures the FIXED production-shaped input, so
// every Dynamic layer on `CONDITIONALLY_SILENT` reads 0 B there — a number
// that cannot distinguish "renders one short sentence" from "renders whatever
// the user pasted". Each such layer therefore owes a bound of its own (the
// list's doc says so); this section is that rule mechanized. Self-rendering
// layers are asserted HERE against oversized waking input; pass-through
// layers name the producer that owns their bound, and the cheap ones are
// asserted against that producer directly.

/// How a conditionally-silent Dynamic layer's bound is established.
enum DynamicLayerBound {
    /// The layer builds its own text; rendered below with deliberately
    /// oversized waking input and asserted against this byte ceiling.
    SelfRendered(usize),
    /// The layer only forwards a producer's render; the bound lives at the
    /// named producer. Listed so the completeness check cannot lose track of
    //  it — naming the place IS the assertion for the expensive-to-wake ones.
    ProducerBounded(&'static str),
}

use DynamicLayerBound::{ProducerBounded, SelfRendered};

const DYNAMIC_LAYER_BOUNDS: &[(&str, DynamicLayerBound)] = &[
    ("chain_context", SelfRendered(600)),
    ("tool_runtime_state", SelfRendered(4_000)),
    ("mcp_instructions", SelfRendered(2_400)),
    ("voice_mode", SelfRendered(1_500)),
    ("doctor_repair_hint", SelfRendered(500)),
    ("session_context_guide", SelfRendered(600)),
    ("memory_window", SelfRendered(600)),
    (
        "timer_loop",
        ProducerBounded(
            "harness_bridge::context_blocks::active_timer_loop (watch prompt clamped to 400 chars)",
        ),
    ),
    (
        "standing_goal",
        ProducerBounded(
            "harness_bridge::context_blocks::render_goal_summary (objective 400 / task 100; asserted below)",
        ),
    ),
    (
        "execution_plan",
        ProducerBounded("memory::scratchpad::render_progress_bounded (PROMPT_PLAN_LIMITS)"),
    ),
    (
        "strategy_pointer",
        ProducerBounded(
            "strategy::render::render_guardrails_only (10 × 300; asserted below)",
        ),
    ),
    (
        "graph_topology",
        ProducerBounded("loop_graph::service::render_is_bounded_against_oversized_graph_rows"),
    ),
];

/// Every conditionally-silent Dynamic layer has a registered bound (and the
/// registry has no ghosts), and each self-rendering one respects its ceiling
/// under deliberately oversized waking input.
#[test]
fn conditionally_silent_dynamic_layers_are_bounded() {
    use crate::thinker::prompt_layer::PromptLayer;

    let pipeline = PromptPipeline::default_layers();
    let dynamic_names: Vec<&'static str> = pipeline
        .layer_info()
        .into_iter()
        .filter(|(_, _, stability)| {
            *stability == crate::thinker::prompt_layer::LayerStability::Dynamic
        })
        .map(|(_, name, _)| name)
        .collect();
    let silent_dynamic: Vec<&str> = CONDITIONALLY_SILENT
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| dynamic_names.contains(n))
        .collect();

    // Completeness, both directions: a silent Dynamic layer without a bound
    // is the graph_topology hole; a bound entry naming a layer that left the
    // list (or went Stable) is a stale claim.
    for name in &silent_dynamic {
        assert!(
            DYNAMIC_LAYER_BOUNDS.iter().any(|(n, _)| n == name),
            "{name} is a conditionally-silent Dynamic layer with no registered byte bound — \
             add it to DYNAMIC_LAYER_BOUNDS (SelfRendered ceiling, or ProducerBounded naming \
             the producer that caps its input)"
        );
    }
    for (name, _) in DYNAMIC_LAYER_BOUNDS {
        assert!(
            silent_dynamic.contains(name),
            "DYNAMIC_LAYER_BOUNDS entry {name} is not a conditionally-silent Dynamic layer \
             (removed, renamed, or re-rated) — drop or move the entry"
        );
    }

    let config = production_config();
    let assert_ceiling = |name: &str, rendered: String| {
        let bound = DYNAMIC_LAYER_BOUNDS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, b)| b);
        let ceiling = match bound {
            Some(SelfRendered(c)) => *c,
            Some(ProducerBounded(where_it_lives)) => panic!(
                "{name} is ProducerBounded({where_it_lives}) — assert it at the producer, not here"
            ),
            None => panic!("{name} is missing from DYNAMIC_LAYER_BOUNDS"),
        };
        assert!(
            rendered.len() <= ceiling,
            "{name} rendered {} B under oversized waking input (ceiling {ceiling} B) — \
             the unbounded-growth hole this table exists to close",
            rendered.len()
        );
    };

    // chain_context — depth/max are small ints; the render is near-const.
    let chain = crate::harness::chain_context::ChainContext::new()
        .child()
        .expect("root always has a child at depth 1");
    let input = LayerInput::basic(&config, &[]).with_chain_context_opt(Some(&chain));
    let mut out = String::new();
    crate::thinker::layers::ChainContextLayer.inject(&mut out, &input);
    assert!(!out.is_empty(), "fixture must wake chain_context");
    assert_ceiling("chain_context", out);

    // tool_runtime_state — 50 unhealthy tools (a downed MCP server exposing
    // many tools) is the realistic worst case.
    let mut ctx = resolve(InteractionParadigm::Background);
    ctx.runtime_state_blocks = (0..50)
        .map(|i| {
            crate::tools::runtime_state::RuntimeStateFragment::unavailable(
                format!("mcp_server_tool_{i}"),
                "dependency down",
            )
        })
        .collect();
    let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
    let mut out = String::new();
    crate::thinker::layers::ToolRuntimeStateLayer.inject(&mut out, &input);
    assert!(!out.is_empty(), "fixture must wake tool_runtime_state");
    assert_ceiling("tool_runtime_state", out);

    // mcp_instructions — a hostile / sloppy server advertising a 50 KB block.
    let instructions = vec![crate::thinker::prompt_layer::McpServerInstruction {
        server_name: "srv".to_string(),
        instructions: "x".repeat(50_000),
    }];
    let input = LayerInput::basic(&config, &[]).with_mcp_instructions(&instructions);
    let mut out = String::new();
    crate::thinker::layers::McpInstructionsLayer.inject(&mut out, &input);
    assert!(!out.is_empty(), "fixture must wake mcp_instructions");
    assert_ceiling("mcp_instructions", out);

    // voice_mode — transcribed turn with a 25 KB operator vocabulary list.
    let mut ctx = resolve(InteractionParadigm::Background);
    ctx.voice = crate::thinker::context::VoiceContext::SpokenTranscribed;
    ctx.voice_vocabulary = Some("term".repeat(5_000));
    let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
    let mut out = String::new();
    crate::thinker::layers::VoiceModeLayer.inject(&mut out, &input);
    assert!(!out.is_empty(), "fixture must wake voice_mode");
    assert_ceiling("voice_mode", out);

    // doctor_repair_hint — const text under WebRich.
    let ctx = resolve(InteractionParadigm::WebRich);
    let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
    let mut out = String::new();
    crate::thinker::layers::DoctorRepairHintLayer.inject(&mut out, &input);
    assert!(!out.is_empty(), "fixture must wake doctor_repair_hint");
    assert_ceiling("doctor_repair_hint", out);

    // session_context_guide — const text gated on a flag.
    let input = LayerInput::basic(&config, &[]).with_session_summaries(true);
    let mut out = String::new();
    crate::thinker::layers::SessionContextGuideLayer.inject(&mut out, &input);
    assert!(!out.is_empty(), "fixture must wake session_context_guide");
    assert_ceiling("session_context_guide", out);

    // memory_window — one of three consts; wake via the recall flag.
    let input = LayerInput::basic(&config, &[]).with_recalled_memory(true);
    let mut out = String::new();
    crate::thinker::layers::MemoryWindowLayer.inject(&mut out, &input);
    assert!(!out.is_empty(), "fixture must wake memory_window");
    assert_ceiling("memory_window", out);

    // Producer bounds asserted where the producer is cheap to drive:
    // render_goal_summary with a 20 KB user-authored objective.
    let goal = crate::goal::Goal::new("session", &"objective ".repeat(2_500), 0, 0);
    let rendered = crate::orchestrator::harness_bridge::render_goal_summary(&goal);
    assert!(
        rendered.len() <= 600,
        "render_goal_summary rendered {} B from an oversized objective (ceiling 600 B)",
        rendered.len()
    );
    // render_guardrails_only with 100 guardrails of 1,000 chars each.
    let strategy = crate::strategy::Strategy {
        objective: "o".into(),
        approach: "a".into(),
        phases: vec![],
        guardrails: (0..100).map(|_| "g".repeat(1_000)).collect(),
        success_criteria: "s".into(),
        goal_id: None,
    };
    let rendered = crate::strategy::render_guardrails_only(&strategy);
    assert!(
        rendered.len() <= 3_200,
        "render_guardrails_only rendered {} B from an oversized list (ceiling 3,200 B)",
        rendered.len()
    );
}

/// No sentence may be stated twice in one request — across prompt layers *and*
/// the tool descriptions that ship beside them.
///
/// Cross-layer duplication is how the prompt grew without anyone adding a
/// section: the D4 acknowledgment contract was, at its peak, stated in
/// `memory_protocol`, `special_actions`, and two tool descriptions. Each copy
/// costs tokens on every request and is one more place the rule can drift out
/// of sync with the others.
///
/// Half of that sentence used to be unmeasurable. The guard walked
/// `layer_sections` only, so the two tool-description copies it names sat
/// outside its field of view — it certified non-duplication over a surface that
/// excluded where the duplication was. That is the same shape as
/// `production_shaped` leaving `runtime_context` unset: an excuse that makes
/// the number look clean by not looking. A tool's description ships with its
/// schema in the same request as the layers, on the same token budget, with the
/// same drift risk, so it belongs in the same scan.
///
/// The tool text ingested is `BUILTIN_TOOL_DEFINITIONS` — the LLM-facing
/// catalog `agent_init` maps straight into the model's tool list. That choice
/// is about *what ships*, not about where the text is written: reading the
/// `AlephTool::DESCRIPTION` consts directly would repeat the very mistake above
/// in mirror image, measuring text production never sends. It stays correct now
/// that every catalog entry references its tool's const, and it stays correct
/// if one ever stops.
///
/// The corollary is easy to misread, so: this guard counts **how many times a
/// sentence is sent**, not how many times it is written. Text hoisted into one
/// shared const and referenced by N tools still ships N times and is still
/// flagged. Deduplicating the source is not the fix; sending it once is.
///
/// History, for the record. When the tool half was first ingested it found no
/// duplication at all — and reported that the D4 acknowledgment clause shipped
/// *zero* times, because all three memory writers' catalog entries were terse
/// literals shadowing the `AFTER A SUCCESSFUL WRITE` paragraph in each tool's
/// own const. The mirror-image failure of triplication, invisible from the
/// layer side exactly as triplication was. The 2026-08-04 sweep pointed all 155
/// entries at their consts, and this guard immediately earned its keep: six
/// real duplicates surfaced the moment the text started shipping — the AX
/// platform-support sentence in four sibling tools, the heartbeat "find the id
/// first" pointer in three, and one hard-wrapped `working_dir` line identical
/// across `bash` and `code_exec`. All three now state it once.
///
/// The tool half must also stay non-empty — see the ingest assertion below. A
/// guard that quietly narrows back to layer-only would keep passing while
/// measuring the same partial surface this doc comment exists to condemn.
#[test]
fn no_sentence_is_stated_twice() {
    let pipeline = PromptPipeline::default_layers();
    let config = production_config();
    let context = resolve(InteractionParadigm::Background);
    let input = production_shaped(&config, &context);

    let mut surfaces: Vec<(String, String)> = pipeline
        .layer_sections(AssemblyPath::Cached, &input, PromptMode::Full)
        .into_iter()
        .map(|(name, section)| (format!("layer `{name}`"), section))
        .collect();
    surfaces.extend(
        crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|def| (format!("tool `{}`", def.name), def.description.to_string())),
    );
    // ...plus the ten tools the registry constructor registers without a
    // catalog entry. They ship exactly as a catalog entry's description does
    // (`agent_init` completes the model's tool list from the registry map),
    // so leaving them out measured ~85% of the surface — the same blind spot
    // the byte ratchet had until 2026-08-10, in the same place, found by the
    // same question: *which block shape does this scanner recognise?* The
    // table is shared with the ratchet rather than restated here; a second
    // list is precisely the failure it exists to prevent.
    surfaces.extend(crate::executor::REGISTRY_ONLY_DESCRIPTIONS.iter().map(
        |(name, description)| {
            (
                format!("tool `{name}` (registry-only)"),
                (*description).to_string(),
            )
        },
    ));

    // ...and the per-request injected surface: the tool the ScopedToolService
    // pushes onto the model's list without it ever passing through the
    // registry. Third shape, same question — a scanner is only as wide as the
    // registration forms it recognises, and this one reached the model for its
    // whole life inside both scanners' blind spot. Shared table again, for the
    // reason the note above gives.
    surfaces.extend(crate::executor::INJECTED_TOOL_DESCRIPTIONS.iter().map(
        |(name, description, _)| {
            (
                format!("tool `{name}` (injected)"),
                (*description).to_string(),
            )
        },
    ));

    // ...and the MCP bridge surface, the fourth shape: registered straight into
    // the ToolHandlerRegistry that run_loop snapshots, so it is text the model
    // reads on every request whose capability gate is open. Six tools of one
    // family is exactly where a near-repeat is cheapest to write and hardest to
    // notice, which is the argument for scanning them here rather than only
    // bounding their bytes.
    surfaces.extend(crate::executor::BRIDGE_TOOL_DESCRIPTIONS.iter().map(
        |(name, description, _)| {
            (
                format!("tool `{name}` (mcp bridge)"),
                (*description).to_string(),
            )
        },
    ));

    // Whitespace-normalized sentences long enough to be a claim rather than a
    // header, list marker or "Rules:" — short fragments collide by coincidence,
    // not by duplication. One definition, so the ingest check below counts
    // exactly what the duplicate scan compares.
    fn measured_sentences(text: &str) -> impl Iterator<Item = String> + '_ {
        text.split(['.', '\n'])
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|norm| norm.split_whitespace().count() >= 8)
    }

    // The tool half of the surface must actually carry text. Zero here means
    // the guard silently reverted to layer-only scope — the exact blindness
    // that let the D4 copies sit outside its field of view — and every
    // "no duplicates" verdict below would again be a claim about half a request.
    let tool_sentences = surfaces
        .iter()
        .filter(|(origin, _)| origin.starts_with("tool `"))
        .flat_map(|(_, text)| measured_sentences(text))
        .count();
    assert!(
        tool_sentences > 0,
        "the tool half of the surface contributed no measurable sentence — this guard is \
         back to certifying non-duplication across layers alone. Check that \
         BUILTIN_TOOL_DEFINITIONS is still the catalog agent_init maps into the model's \
         tool list, and that this test still ingests it."
    );

    // Same check for the registry-only half, and for the same reason: it was
    // added because its absence made the verdict a claim about 85% of a
    // request. An empty table here would restore that silently.
    let registry_only_sentences = surfaces
        .iter()
        .filter(|(origin, _)| origin.ends_with("(registry-only)"))
        .flat_map(|(_, text)| measured_sentences(text))
        .count();
    assert!(
        registry_only_sentences > 0,
        "the registry-only half of the surface contributed no measurable sentence. Those \
         ten descriptions ship on every request without a catalog entry; if the table \
         emptied or this test stopped ingesting it, the verdict below is once again about \
         part of the request only."
    );

    let mut seen: Vec<(String, String)> = Vec::new();
    let mut dupes: Vec<String> = Vec::new();

    for (origin, text) in &surfaces {
        for norm in measured_sentences(text) {
            if let Some((_, other)) = seen.iter().find(|(s, _)| *s == norm) {
                dupes.push(format!("{other} and {origin} both say: {norm:?}"));
            } else {
                seen.push((norm, origin.clone()));
            }
        }
    }

    assert!(
        dupes.is_empty(),
        "the same sentence ships twice in one request — pick one home (a rule that \
         ranks tools against each other belongs in a layer; a rule about one tool \
         belongs in that tool's DESCRIPTION):\n{}",
        dupes.join("\n")
    );
}

/// No environment *fact* may be stated by two layers.
///
/// [`no_sentence_is_stated_twice`] cannot catch this class: it compares whole
/// normalized sentences, so the same fact rendered in two different shapes —
/// `- **OS**: linux` in `environment` and `os=linux` in `runtime_context` — slips
/// through as two distinct "sentences". That is precisely how the environment
/// envelope came to state OS, the working directory, and the date twice per
/// request, with the per-run-varying `cwd` copy sitting in the *cacheable* prefix.
///
/// This guard asserts on the fact's **value**, which is shape-independent, and it
/// is the regression test for the §2.3 two-zone split: `environment` @300 (Stable)
/// owns the process-invariant half, `runtime_context` @1720 (Dynamic) owns the
/// per-run half, and neither repeats the other.
#[test]
fn no_environment_fact_is_stated_twice() {
    let pipeline = PromptPipeline::default_layers();
    let config = production_config();
    let context = resolve(InteractionParadigm::WebRich);
    let input = production_shaped(&config, &context);

    // The fact list is DERIVED from the two types that own the facts, not
    // written out here. The hand-written version listed six `RuntimeContext`
    // values and no sandbox values at all, so the `Network:` sentence that
    // `SecurityLayer` @600 and `OperatingEnvelopeLayer` @1758 both rendered —
    // from the same `SandboxSummary`, in the same turn — was outside its field
    // of view for four rounds of green. Each census is an exhaustive
    // destructure of its struct, so a new field is a compile error here until
    // someone has said whether it is model-visible.
    let mut facts: Vec<(&'static str, String)> = fixed_runtime_context().fact_census();
    facts.extend(fixed_sandbox_summary().fact_census());

    // Values short enough to appear inside unrelated prose would report
    // collisions between genuinely different facts. Skipping them is a stated
    // limit of the guard, not an exemption list that can grow into a licence:
    // the threshold is a property of the string, re-derived every run, and
    // nothing names a layer or a fact.
    const MIN_DISTINCTIVE_LEN: usize = 5;

    let sections = pipeline.layer_sections(AssemblyPath::Cached, &input, PromptMode::Full);
    let mut checked = 0usize;
    for (fact, value) in facts {
        if value.len() < MIN_DISTINCTIVE_LEN {
            continue;
        }
        checked += 1;
        let stating: Vec<&'static str> = sections
            .iter()
            .filter(|(_, section)| section.contains(&value))
            .map(|(name, _)| *name)
            .collect();
        assert!(
            stating.len() <= 1,
            "environment fact {fact} ({value:?}) is stated by {stating:?} — exactly one \
             layer must own it. Process-invariant facts belong in `environment` (Stable), \
             per-run facts in `runtime_context` / `operating_envelope` (Dynamic); see \
             RuntimeContext's module docs for the split."
        );
    }
    assert!(
        checked >= 8,
        "only {checked} facts were distinctive enough to check — the censuses shrank or \
         the fixtures went short, and this guard is now measuring almost nothing"
    );
}

// ---------------------------------------------------------------------------
// Stable-prefix determinism
// ---------------------------------------------------------------------------
//
// The two guards below are the assembly-side twin of
// `providers::protocols::anthropic::adapter_tests::prefix_stability`, which
// pins the same property on raw request bodies but only ever sees a
// hand-written fixture — never the real layer set.
//
// They exist because `PromptLayer::stability()` is a required method — a
// layer that picks `Stable` without thinking rides inside the cacheable
// prefix, and every byte it renders then re-keys the provider's prompt cache
// for the whole conversation behind it. That has already happened once here:
// `ToolRuntimeStateLayer` sat at priority 502 back when `stability()` still
// DEFAULTED to `Stable`, so a 30-second-TTL tool-health probe silently
// invalidated entire sessions. It was caught by a human reading code; making
// the method required moved the catch to the compiler, and these guards are
// what watch the answer every layer now has to give.
//
// DeepSeek-Reasonix guards the same invariant with a runtime
// `verifyFingerprint()` that throws when a mutation path bypasses cache
// invalidation. Aleph does carry a runtime prefix hash —
// `cache_monitor.rs::stable_prefix_hash`, consumed by `MeteringProvider` for
// miss attribution at the watchdog's alarm edge — but it annotates alarms,
// it never gates assembly. The gating half of that mechanism is the
// assertion, so it lives here as a test.

/// Building the stable prefix twice from identical input must produce
/// identical bytes.
///
/// Catches any `Stable` layer that reads a clock, a counter, a process global,
/// or an unsorted collection. Note the honest limit: this input carries FIXED
/// stand-ins for machine facts, so it proves determinism *inside* the layers —
/// `stable_prefix_ignores_per_run_facts` below is what catches a per-run input
/// being threaded into a stable layer.
#[test]
fn stable_prefix_is_byte_identical_when_built_twice() {
    let pipeline = PromptPipeline::default_layers();
    let config = production_config();

    for &paradigm in PARADIGMS {
        let context = resolve(paradigm);
        let input = production_shaped(&config, &context);
        let first =
            pipeline.execute_stable_with_mode(AssemblyPath::Cached, &input, PromptMode::Full);
        let second =
            pipeline.execute_stable_with_mode(AssemblyPath::Cached, &input, PromptMode::Full);
        assert_eq!(
            first, second,
            "the cacheable prefix is not deterministic under {paradigm:?} — some Stable \
             layer renders a clock, a counter, a global, or an unsorted collection. \
             Every byte of drift re-keys the provider prompt cache for the entire \
             conversation behind it."
        );
    }
}

/// Facts that vary within a session must not reach the cacheable prefix.
///
/// Two contexts differing ONLY in per-run facts (time, cwd, repo root, serving
/// model, **sandbox writable roots**) must produce a byte-identical stable
/// prefix. The dynamic suffix is where those belong, and it is asserted to
/// actually differ so the test cannot pass by the facts having been dropped
/// everywhere.
///
/// This is the guard that fails the moment someone welds a per-run input into a
/// `Stable` layer — the exact regression class that is otherwise invisible until
/// a monthly bill.
///
/// **It shifted only `runtime_context` for its first two months, and that gap
/// cost a real defect** (§2.18 ledger item 9): `SandboxSummary::writable_roots`
/// is *also* per-run — `isolated_worktree` mints a fresh UUID path on every
/// isolated run — and `SecurityLayer` @600 was rendering it from inside the
/// cacheable prefix the whole time. The guard was present, watching the wrong
/// half of the input. Anything added to `resolve()` because "every production
/// turn sets it" must be shifted here too, or it re-opens the same blind spot.
#[test]
fn stable_prefix_ignores_per_run_facts() {
    let pipeline = PromptPipeline::default_layers();
    let config = production_config();

    for &paradigm in PARADIGMS {
        let baseline = resolve(paradigm);

        let mut shifted = resolve(paradigm);
        shifted.runtime_context = Some(crate::thinker::runtime_context::RuntimeContext {
            // Per-run / per-hour facts: all four must live in the dynamic zone.
            working_dir: Some(std::path::PathBuf::from("/home/u/.aleph/workspaces/other")),
            repo_root: Some(std::path::PathBuf::from("/home/u/src/other")),
            current_model: "openai".to_string(),
            current_time: "2026-07-26 13:00".to_string(),
            // Process-invariant facts held equal — those legitimately belong to
            // the stable `environment` layer.
            ..fixed_runtime_context()
        });
        // Per-run sandbox identity: an isolated run gets its own worktree, so
        // `writable_roots` differs run to run while the posture around it does
        // not. Held-equal posture is the point — if the whole summary were
        // swapped, a layer that legitimately renders the backend tag would fail
        // this and the test would be asserting the wrong invariant.
        shifted.sandbox_summary = Some(crate::sandbox::SandboxSummary {
            writable_roots: vec![std::path::PathBuf::from(
                "/home/u/.aleph/worktrees/6f1c2e9a-4b77-4d51-9a0e-2c8b5f3d17ab",
            )],
            ..fixed_sandbox_summary()
        });

        let input_a = production_shaped(&config, &baseline);
        let input_b = production_shaped(&config, &shifted);

        let stable_a =
            pipeline.execute_stable_with_mode(AssemblyPath::Cached, &input_a, PromptMode::Full);
        let stable_b =
            pipeline.execute_stable_with_mode(AssemblyPath::Cached, &input_b, PromptMode::Full);
        assert_eq!(
            stable_a, stable_b,
            "a per-run fact (cwd / repo / model / time / sandbox writable roots) reached \
             the cacheable prefix under {paradigm:?}. It belongs in the dynamic suffix — \
             see RuntimeContext's module docs for the machine facts and \
             OperatingEnvelopeLayer's for the sandbox half."
        );

        let dynamic_a =
            pipeline.execute_dynamic_with_mode(AssemblyPath::Cached, &input_a, PromptMode::Full);
        let dynamic_b =
            pipeline.execute_dynamic_with_mode(AssemblyPath::Cached, &input_b, PromptMode::Full);
        assert_ne!(
            dynamic_a, dynamic_b,
            "under {paradigm:?} the per-run facts vanished from the dynamic suffix too — \
             the stable-prefix assertion above would then pass vacuously. The model must \
             still be told its cwd and time somewhere."
        );
    }
}

/// A subagent's whole system prompt must be byte-identical across two spawns
/// of the same shape.
///
/// **The two guards above cannot see this path.** Both hardcode
/// `AssemblyPath::Cached`, `production_shaped` never threads a
/// `chain_context`, and `chain_context` sits in [`CONDITIONALLY_SILENT`] — so
/// the byte ratchet reads 0 B for it. Three exemptions stacked, and the only
/// place `ChainContextLayer` actually renders — a subagent on
/// `AssemblyPath::Basic` — fell through all of them.
///
/// The assertion is on the WHOLE assembled string, not the stable half. Doing
/// it on `execute_stable_with_mode` would be vacuous by construction:
/// `ChainContextLayer` is `Dynamic`, so the stable half excludes it and would
/// pass green while the defect was live. It would also be testing something no
/// production caller does — `build_system_prompt` runs `pipeline.execute`, and
/// Basic never splits.
///
/// That distinction is the whole point. Basic produces ONE unsplit system
/// block, and the Anthropic adapter's lone cache breakpoint covers all of it,
/// so a single per-spawn byte anywhere in here re-keys `tools` (~82 KB of
/// builtin descriptions alone) plus the entire system prompt on every spawn:
/// `cache_creation` at 1.25×, `cache_read` pinned at 0, and a fresh
/// OpenAI `pck_` bucket each time. That is exactly what the `Chain id:` line
/// used to do. Nothing errors, no test goes red, and the model behaves
/// perfectly — the symptom appears only on the bill.
///
/// Goes through `PromptBuilder::build_system_prompt`, the same entry
/// `agents::subagent_spawner::spawn` calls, so post-pipeline welds are covered
/// too.
#[test]
fn basic_path_prefix_is_stable_across_spawns() {
    use crate::harness::chain_context::ChainContext;
    use crate::thinker::prompt_builder::PromptBuilder;

    // Two different parent runs ⇒ two different chain ids, same depth/budget.
    let a = ChainContext::new().child().unwrap();
    let b = ChainContext::new().child().unwrap();
    assert_ne!(
        a.chain_id, b.chain_id,
        "the fixture must really differ, or this guard proves nothing"
    );

    let build = |chain: &ChainContext| {
        PromptBuilder::new(PromptConfig::default())
            .with_chain_context(chain.clone())
            .build_system_prompt(&[])
    };

    assert_eq!(
        build(&a),
        build(&b),
        "a subagent's system prompt varies between spawns. Basic is one unsplit \
         block under a single cache breakpoint, so every spawn now re-pays the \
         whole tools+system prefix at cache-creation rates with zero cache_read. \
         Find the per-spawn byte (a uuid, a clock, a counter, an unsorted map) \
         and move it out of the Basic path."
    );
}
