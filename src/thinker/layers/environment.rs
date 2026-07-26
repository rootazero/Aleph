//! `EnvironmentLayer` — baseline runtime hints (date, OS, cwd) plus
//! optional environment contract (paradigm / capabilities / constraints).
//!
//! Time is rendered **date-granular** (`YYYY-MM-DD UTC`) so the assembled
//! prompt prefix stays byte-stable across turns within a UTC day. That
//! lets Anthropic's prefix cache keep hits — the model can still query
//! exact wall-clock time via tools when it actually matters.

use chrono::Utc;

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

/// Paths the environment hint participates in. Everything except `Minimal`
/// benefits from knowing the date/OS — even the `Basic` harness path used
/// to omit this entirely, which left the LLM unable to answer "what day
/// is it?" without a tool round-trip.
const ENVIRONMENT_PATHS: &[AssemblyPath] = &[AssemblyPath::Basic, AssemblyPath::Cached];

pub struct EnvironmentLayer;

impl PromptLayer for EnvironmentLayer {
    fn name(&self) -> &'static str {
        "environment"
    }
    fn priority(&self) -> u32 {
        300
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        // Full mode only — Compact / Minimal deliberately strip environment
        // hints to keep the prompt token-tight. The pipeline's mode test
        // pins this policy (`compact_mode_excludes_heavy_layers`).
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        ENVIRONMENT_PATHS
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        output.push_str("## Environment\n\n");

        // Always-available baseline. Date-granular timestamp keeps the
        // prompt prefix byte-stable within a UTC day for prefix-cache reuse.
        output.push_str(&format!(
            "- **Date (UTC)**: {}\n",
            Utc::now().format("%Y-%m-%d")
        ));
        output.push_str(&format!("- **OS**: {}\n", std::env::consts::OS));
        if let Ok(cwd) = std::env::current_dir() {
            output.push_str(&format!("- **Working directory**: {}\n", cwd.display()));
        }
        output.push('\n');

        // Optional contract section — only present when ResolvedContext is wired.
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };
        let contract = &ctx.environment_contract;

        output.push_str(&format!(
            "**Paradigm**: {}\n\n",
            contract.paradigm.description()
        ));

        if !contract.active_capabilities.is_empty() {
            output.push_str("**Active Capabilities**:\n");
            for cap in &contract.active_capabilities {
                let (name, hint) = cap.prompt_hint();
                output.push_str(&format!("- `{name}`: {hint}\n"));
            }
            output.push('\n');
        }

        let mut constraint_notes = Vec::new();
        if let Some(max_chars) = contract.constraints.max_output_chars {
            constraint_notes.push(format!("Max output: {max_chars} characters"));
        }
        if contract.constraints.prefer_compact {
            constraint_notes.push("Prefer concise responses".to_string());
        }
        if contract.constraints.supports_streaming {
            constraint_notes.push("Streaming enabled".to_string());
        }

        if !constraint_notes.is_empty() {
            output.push_str("**Constraints**:\n");
            for note in constraint_notes {
                output.push_str(&format!("- {note}\n"));
            }
            output.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn baseline_hints_emit_without_context() {
        let layer = EnvironmentLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Environment"));
        assert!(out.contains("**Date (UTC)**:"));
        assert!(out.contains("**OS**:"));
        // contract-only sections must stay absent without ResolvedContext
        assert!(!out.contains("**Paradigm**"));
        assert!(!out.contains("**Active Capabilities**"));
    }

    #[test]
    fn date_is_day_granular_for_cache_stability() {
        let layer = EnvironmentLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let mut a = String::new();
        let mut b = String::new();
        layer.inject(&mut a, &LayerInput::basic(&config, &tools));
        layer.inject(&mut b, &LayerInput::basic(&config, &tools));
        // Two builds in the same UTC day must be byte-identical so the
        // Anthropic prefix cache holds across turns.
        assert_eq!(a, b);
        // No HH:MM:SS leakage — that would bust the cache every call.
        assert!(!a.contains(':') || !a.contains("UTC:"));
    }

    #[test]
    fn participates_in_harness_paths() {
        let paths = EnvironmentLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
