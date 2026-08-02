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
    }
}

/// Machine facts frozen at representative widths, so `scaffold_bytes_ratchet`
/// measures the layer set rather than the machine it runs on.
fn fixed_runtime_context() -> crate::thinker::runtime_context::RuntimeContext {
    crate::thinker::runtime_context::RuntimeContext {
        os: "linux".to_string(),
        arch: "x86_64".to_string(),
        shell: "bash".to_string(),
        working_dir: std::path::PathBuf::from("/home/u/.aleph/workspaces/main"),
        repo_root: None,
        current_model: "anthropic".to_string(),
        hostname: "aleph-host".to_string(),
        current_time: "2026-07-26 12:00".to_string(),
        timezone: "UTC".to_string(),
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
const SCAFFOLD_CEILING_BYTES: usize = 5_913;

/// No paradigm's fixed scaffold may grow past the ceiling.
#[test]
fn scaffold_bytes_ratchet() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();

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
/// catalog `agent_init` maps straight into the model's tool list — deliberately
/// **not** the `AlephTool::DESCRIPTION` consts, several of which are richer than
/// the catalog entry that paraphrases them. Measuring the consts would repeat
/// the very mistake above in mirror image: measuring text production never
/// sends. (Where an entry and its tool's const have drifted apart, that is a
/// separate bug in `definitions.rs`; this guard reports on what ships, and will
/// see those sentences the moment the catalog points at the consts.)
///
/// What widening it actually found, for the record: **no duplication at all —
/// and the D4 clause named above ships zero times.** All three memory writers'
/// catalog entries are terse one-line literals, so the `AFTER A SUCCESSFUL
/// WRITE` paragraph in each tool's own const never reaches the model. The
/// mirror-image failure of triplication, and invisible from the layer side
/// exactly as triplication was. Fix is in `definitions.rs` (point those three
/// entries at their consts, as the five file tools already do); this guard will
/// start measuring the clause the moment it does.
///
/// The tool half must also stay non-empty — see the ingest assertion below. A
/// guard that quietly narrows back to layer-only would keep passing while
/// measuring the same partial surface this doc comment exists to condemn.
#[test]
fn no_sentence_is_stated_twice() {
    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
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
    let config = PromptConfig::default();
    let rt = fixed_runtime_context();
    let context = resolve(InteractionParadigm::WebRich);
    let input = production_shaped(&config, &context);

    // Distinctive values only: a fact whose value is a common substring (e.g. a
    // one-word arch on some hosts) would produce coincidental hits.
    let facts: [(&str, &str); 6] = [
        ("os", rt.os.as_str()),
        ("arch", rt.arch.as_str()),
        ("shell", rt.shell.as_str()),
        ("hostname", rt.hostname.as_str()),
        ("working_dir", "/home/u/.aleph/workspaces/main"),
        ("time", rt.current_time.as_str()),
    ];

    for (fact, value) in facts {
        let stating: Vec<&'static str> = pipeline
            .layer_sections(AssemblyPath::Cached, &input, PromptMode::Full)
            .into_iter()
            .filter(|(_, section)| section.contains(value))
            .map(|(name, _)| name)
            .collect();
        assert!(
            stating.len() <= 1,
            "environment fact {fact} ({value:?}) is stated by {stating:?} — exactly one \
             layer must own it. Stable facts belong in `environment`, per-run facts in \
             `runtime_context`; see RuntimeContext's module docs for the split."
        );
    }
}
