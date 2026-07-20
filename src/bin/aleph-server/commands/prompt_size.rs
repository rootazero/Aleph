//! `aleph-server prompt-size` — offline prompt-budget introspection.
//!
//! hermes `prompt_size.py` analogue. Builds the prompt pipeline with default
//! config (no network, no daemon, no instance lock) and prints a per-layer
//! byte / char / token breakdown of the static system-prompt scaffold, so the
//! fixed prompt budget is visible without parsing a saved session by hand.
//!
//! Session-specific layers (tool schemas, retrieved memory, dynamic runtime
//! context) emit nothing under default input and are therefore excluded — this
//! reports the fixed scaffold cost, which is exactly the budget tuning target.

use alephcore::thinker::prompt_layer::{AssemblyPath, LayerInput};
use alephcore::thinker::prompt_mode::PromptMode;
use alephcore::thinker::prompt_pipeline::{LayerSize, PromptPipeline};
use alephcore::thinker::PromptConfig;

type CmdResult = Result<(), Box<dyn std::error::Error>>;

/// Entry point for the `prompt-size` subcommand.
pub fn run(path: &str, mode: &str, json: bool) -> CmdResult {
    let assembly_path = parse_path(path)?;
    let prompt_mode = parse_mode(mode)?;

    let pipeline = PromptPipeline::default_layers();
    let config = PromptConfig::default();
    let tools = Vec::new();
    let input = LayerInput::basic(&config, &tools);

    let breakdown = pipeline.layer_breakdown(assembly_path, &input, prompt_mode);

    if json {
        println!("{}", render_json(path, mode, &breakdown));
    } else {
        print!("{}", render_table(path, mode, &breakdown));
    }
    Ok(())
}

fn parse_path(s: &str) -> Result<AssemblyPath, String> {
    match s.to_ascii_lowercase().as_str() {
        "basic" => Ok(AssemblyPath::Basic),
        "hydration" => Ok(AssemblyPath::Hydration),
        "soul" => Ok(AssemblyPath::Soul),
        "cached" => Ok(AssemblyPath::Cached),
        other => Err(format!(
            "unknown --path '{other}' (expected: basic | hydration | soul | cached)"
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
fn render_table(path: &str, mode: &str, breakdown: &[LayerSize]) -> String {
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

    let mut out = String::new();
    out.push_str(&format!(
        "Prompt-size breakdown (path={path}, mode={mode}, static scaffold only)\n\n"
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
        "\n  {} layers (static scaffold; session content excluded)\n",
        breakdown.len()
    ));
    out
}

/// JSON envelope (in assembly order, not size-sorted, so it is stable).
fn render_json(path: &str, mode: &str, breakdown: &[LayerSize]) -> String {
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
        "path": path,
        "mode": mode,
        "scope": "static_scaffold",
        "total": { "bytes": total_bytes, "chars": total_chars, "tokens": total_tokens },
        "layers": layers,
    });
    serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
}
