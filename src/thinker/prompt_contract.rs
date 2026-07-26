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
const CONDITIONALLY_SILENT: &[(&str, &str)] = &[
    ("soul", "agent SOUL.md on disk"),
    ("agent_role", "a registered sub-agent's AgentDef"),
    ("curated_memory", "a non-empty MEMORY.md hot zone"),
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
    (
        "runtime_context",
        "RuntimeContext::collect on the resolved context",
    ),
    ("tool_runtime_state", "a tool-health probe result"),
    ("agent_catalog", "at least one switchable agent registered"),
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
    ContextAggregator::resolve(
        &InteractionManifest::new(paradigm),
        &SecurityContext::for_paradigm(paradigm),
        &[],
    )
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
    let config = PromptConfig::default();

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
/// History: 4,904 B measured 2026-07-26 (`aleph-server prompt-size --path
/// cached --paradigm background`, same shape as the test below). The
/// pi-leanness round that set it removed 2,597 B net — `special_actions`
/// 1,234 → 313 and `memory_protocol` 2,938 → 1,187, both by moving per-tool
/// how-to into the tool `DESCRIPTION`s that already stated it, less 75 B for
/// the parallel-dispatch fact rescued into `role`.
const SCAFFOLD_CEILING_BYTES: usize = 4_904;

/// The fixed Background-paradigm scaffold must not grow past the ceiling.
///
/// Background is the measurement paradigm: it is the always-on daemon default
/// and the widest, enabling protocol tokens *and* operational awareness.
#[test]
fn scaffold_bytes_ratchet() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let context = resolve(InteractionParadigm::Background);
    let input = production_shaped(&config, &context);

    let breakdown = pipeline.layer_breakdown(AssemblyPath::Cached, &input, PromptMode::Full);
    let total: usize = breakdown.iter().map(|l| l.bytes).sum();

    let mut largest: Vec<(&str, usize)> = breakdown.iter().map(|l| (l.name, l.bytes)).collect();
    largest.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    largest.truncate(5);

    assert!(
        total <= SCAFFOLD_CEILING_BYTES,
        "always-on prompt scaffold grew to {total} B (ceiling {SCAFFOLD_CEILING_BYTES}). \
         Largest layers: {largest:?}. Answer the three questions documented on \
         SCAFFOLD_CEILING_BYTES before raising it."
    );
}

/// No sentence may be stated by two different layers.
///
/// Cross-layer duplication is how the prompt grew without anyone adding a
/// section: the D4 acknowledgment contract was, at its peak, stated in
/// `memory_protocol`, `special_actions`, and two tool descriptions. Each copy
/// costs tokens on every request and is one more place the rule can drift out
/// of sync with the others.
#[test]
fn no_sentence_is_stated_twice() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let context = resolve(InteractionParadigm::Background);
    let input = production_shaped(&config, &context);

    let mut seen: Vec<(String, &'static str)> = Vec::new();
    let mut dupes: Vec<String> = Vec::new();

    for (name, section) in pipeline.layer_sections(AssemblyPath::Cached, &input, PromptMode::Full) {
        for sentence in section.split(['.', '\n']) {
            let norm = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
            // Short fragments (headers, list markers, "Rules:") collide by
            // coincidence, not by duplication.
            if norm.split_whitespace().count() < 8 {
                continue;
            }
            if let Some((_, other)) = seen.iter().find(|(s, _)| *s == norm) {
                dupes.push(format!("{other} and {name} both say: {norm:?}"));
            } else {
                seen.push((norm, name));
            }
        }
    }

    assert!(
        dupes.is_empty(),
        "the same sentence is emitted by two layers — pick one home:\n{}",
        dupes.join("\n")
    );
}
