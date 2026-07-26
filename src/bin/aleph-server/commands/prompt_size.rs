//! `aleph-server prompt-size` — offline prompt-budget introspection.
//!
//! hermes `prompt_size.py` analogue. Builds the prompt pipeline offline (no
//! network, no daemon, no instance lock) and prints a per-layer byte / char /
//! token breakdown of the fixed system-prompt scaffold, so the prompt budget is
//! visible without parsing a saved session by hand.
//!
//! **The input is shaped like production, not like `Default::default()`.** The
//! main loop always threads a `ResolvedContext` (paradigm + security posture +
//! session mode), a resolved provider-behavior name, and an iteration cap; a
//! report built without them silently omitted `security`, `protocol_tokens`,
//! `operational_guidelines`, `session_budget` and `runtime_context` — always-
//! present layers — while listing a `tools` section production never emitted.
//! Numbers from that shape were not the numbers the model sees. `--bare`
//! reproduces the old shape, for comparison only.
//!
//! Per-session content (identity files, curated memory, skills, MCP
//! instructions, per-family behavior deltas) is still excluded: it is user data,
//! not scaffold, and the scaffold is the tuning target. Every layer that
//! contributes nothing is listed by name under "silent", so an absent section is
//! always explainable.

use alephcore::thinker::context::{ContextAggregator, ResolvedContext};
use alephcore::thinker::interaction::{InteractionManifest, InteractionParadigm};
use alephcore::thinker::prompt_layer::{AssemblyPath, LayerInput};
use alephcore::thinker::prompt_mode::PromptMode;
use alephcore::thinker::prompt_pipeline::{LayerSize, PromptPipeline};
use alephcore::thinker::security_context::SecurityContext;
use alephcore::thinker::PromptConfig;

type CmdResult = Result<(), Box<dyn std::error::Error>>;

/// Representative Think→Act cap (the shipped `[execution] max_iterations`
/// default). `SessionBudgetLayer` renders the number, so only its digit count
/// affects the size report.
const SAMPLE_ITERATION_CAP: u32 = 1000;

/// Representative provider family. `ProviderGuidanceLayer` gates on the name
/// being present; the per-family `.md` delta is per-deployment content and is
/// deliberately left unloaded (it needs disk + config).
const SAMPLE_BEHAVIOR: &str = "anthropic";

/// Entry point for the `prompt-size` subcommand.
pub fn run(path: &str, mode: &str, paradigm: &str, bare: bool, json: bool) -> CmdResult {
    let assembly_path = parse_path(path)?;
    let prompt_mode = parse_mode(mode)?;
    let paradigm_enum = parse_paradigm(paradigm)?;

    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools = Vec::new();
    let context: Option<ResolvedContext> = if bare {
        None
    } else {
        Some(ContextAggregator::resolve(
            &InteractionManifest::new(paradigm_enum),
            &SecurityContext::for_paradigm(paradigm_enum),
            &[],
        ))
    };

    let mut input = LayerInput::basic(&config, &tools)
        .with_mode(prompt_mode)
        .with_resolved_context_opt(context.as_ref());
    if !bare {
        input = input
            .with_behavior_name(SAMPLE_BEHAVIOR)
            .with_iteration_cap(SAMPLE_ITERATION_CAP);
    }

    let breakdown = pipeline.layer_breakdown(assembly_path, &input, prompt_mode);
    let silent = pipeline.silent_layers(assembly_path, &input, prompt_mode);
    let shape = Shape {
        path,
        mode,
        paradigm,
        bare,
    };

    if json {
        println!("{}", render_json(shape, &breakdown, &silent));
    } else {
        print!("{}", render_table(shape, &breakdown, &silent));
    }
    Ok(())
}

/// The input shape a report was produced under — echoed in the output so a
/// pasted number always carries the assumptions that produced it.
#[derive(Clone, Copy)]
struct Shape<'a> {
    path: &'a str,
    mode: &'a str,
    paradigm: &'a str,
    bare: bool,
}

fn parse_path(s: &str) -> Result<AssemblyPath, String> {
    match s.to_ascii_lowercase().as_str() {
        "basic" => Ok(AssemblyPath::Basic),
        "cached" => Ok(AssemblyPath::Cached),
        other => Err(format!(
            "unknown --path '{other}' (expected: basic | cached)"
        )),
    }
}

fn parse_mode(s: &str) -> Result<PromptMode, String> {
    match s.to_ascii_lowercase().as_str() {
        "full" => Ok(PromptMode::Full),
        "compact" => Ok(PromptMode::Compact),
        "minimal" => Ok(PromptMode::Minimal),
        other => Err(format!(
            "unknown --mode '{other}' (expected: full | compact | minimal)"
        )),
    }
}

fn parse_paradigm(s: &str) -> Result<InteractionParadigm, String> {
    match s.to_ascii_lowercase().as_str() {
        "background" => Ok(InteractionParadigm::Background),
        "cli" => Ok(InteractionParadigm::CLI),
        "messaging" => Ok(InteractionParadigm::Messaging),
        "webrich" => Ok(InteractionParadigm::WebRich),
        "embedded" => Ok(InteractionParadigm::Embedded),
        other => Err(format!(
            "unknown --paradigm '{other}' \
             (expected: background | cli | messaging | webrich | embedded)"
        )),
    }
}

const fn zone(size: &LayerSize) -> &'static str {
    use alephcore::thinker::prompt_layer::LayerStability;
    match size.stability {
        LayerStability::Stable => "stable",
        LayerStability::Dynamic => "dynamic",
    }
}

fn fmt_kb(bytes: usize) -> String {
    format!("{:.1} KB", bytes as f64 / 1024.0)
}

/// Human-readable table, sorted largest-first by bytes.
fn render_table(shape: Shape<'_>, breakdown: &[LayerSize], silent: &[&'static str]) -> String {
    let total_bytes: usize = breakdown.iter().map(|l| l.bytes).sum();
    let total_chars: usize = breakdown.iter().map(|l| l.chars).sum();
    let total_tokens: usize = breakdown.iter().map(|l| l.tokens).sum();
    let stable_bytes: usize = breakdown
        .iter()
        .filter(|l| zone(l) == "stable")
        .map(|l| l.bytes)
        .sum();
    let dynamic_bytes = total_bytes.saturating_sub(stable_bytes);

    let mut sorted: Vec<&LayerSize> = breakdown.iter().collect();
    sorted.sort_by_key(|l| std::cmp::Reverse(l.bytes));

    let Shape {
        path,
        mode,
        paradigm,
        bare,
    } = shape;
    let shape_note = if bare {
        "bare config, no ResolvedContext — UNDER-REPORTS the live prompt"
    } else {
        "production-shaped input, fixed scaffold only"
    };

    let mut out = String::new();
    out.push_str(&format!(
        "Prompt-size breakdown (path={path}, mode={mode}, paradigm={paradigm})\n  {shape_note}\n\n"
    ));
    out.push_str(&format!(
        "  System prompt total : {total_bytes:>8} B  ({}, {total_chars} chars, ~{total_tokens} tokens)\n",
        fmt_kb(total_bytes)
    ));
    out.push_str(&format!(
        "    stable zone       : {stable_bytes:>8} B  ({})\n",
        fmt_kb(stable_bytes)
    ));
    out.push_str(&format!(
        "    dynamic zone      : {dynamic_bytes:>8} B  ({})\n\n",
        fmt_kb(dynamic_bytes)
    ));

    out.push_str(&format!(
        "  {:>8}  {:>7}  {:>7}  {:<7}  {:>5}  {}\n",
        "bytes", "chars", "tokens", "zone", "prio", "layer"
    ));
    for l in &sorted {
        out.push_str(&format!(
            "  {:>8}  {:>7}  {:>7}  {:<7}  {:>5}  {}\n",
            l.bytes,
            l.chars,
            l.tokens,
            zone(l),
            l.priority,
            l.name
        ));
    }
    out.push_str(&format!(
        "\n  {} of {} layers contributed.\n",
        breakdown.len(),
        breakdown.len() + silent.len()
    ));
    if !silent.is_empty() {
        out.push_str(&format!(
            "  silent ({}): {}\n",
            silent.len(),
            silent.join(", ")
        ));
        out.push_str(
            "  (filtered out by path/mode, or gated on per-session content this offline \
             report does not load)\n",
        );
    }
    out
}

/// JSON envelope (in assembly order, not size-sorted, so it is stable).
fn render_json(shape: Shape<'_>, breakdown: &[LayerSize], silent: &[&'static str]) -> String {
    let total_bytes: usize = breakdown.iter().map(|l| l.bytes).sum();
    let total_chars: usize = breakdown.iter().map(|l| l.chars).sum();
    let total_tokens: usize = breakdown.iter().map(|l| l.tokens).sum();

    let layers = serde_json::Value::Array(
        breakdown
            .iter()
            .map(|l| {
                serde_json::json!({
                    "name": l.name,
                    "priority": l.priority,
                    "zone": zone(l),
                    "bytes": l.bytes,
                    "chars": l.chars,
                    "tokens": l.tokens,
                })
            })
            .collect(),
    );

    let envelope = serde_json::json!({
        "path": shape.path,
        "mode": shape.mode,
        "paradigm": shape.paradigm,
        "scope": if shape.bare { "bare_scaffold" } else { "static_scaffold" },
        "total": { "bytes": total_bytes, "chars": total_chars, "tokens": total_tokens },
        "layers": layers,
        "silent": silent,
    });
    serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
}
