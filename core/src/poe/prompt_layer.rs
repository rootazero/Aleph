//! PoePromptLayer — injects POE success criteria into the system prompt.
//!
//! This layer reads `LayerInput::poe` and, when present, appends a
//! structured block that tells the LLM about the active success contract,
//! current progress, and any step-level hints.

use crate::poe::types::ValidationRule;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};

/// All five assembly paths — this layer participates everywhere.
const ALL_PATHS: &[AssemblyPath] = &[
    AssemblyPath::Basic,
    AssemblyPath::Hydration,
    AssemblyPath::Soul,
    AssemblyPath::Context,
    AssemblyPath::Cached,
];

/// PromptLayer that injects POE success criteria into the system prompt.
///
/// Priority **505** places it right after `HydratedToolsLayer` (501)
/// and before `SecurityLayer` (600), so the LLM sees the success
/// contract early but after tool definitions.
pub struct PoePromptLayer;

impl PromptLayer for PoePromptLayer {
    fn name(&self) -> &'static str {
        "poe_success_criteria"
    }

    fn priority(&self) -> u32 {
        505
    }

    fn paths(&self) -> &'static [AssemblyPath] {
        ALL_PATHS
    }

    fn inject(&self, output: &mut String, input: &LayerInput) {
        let poe = match input.poe {
            Some(p) if p.has_content() => p,
            _ => return,
        };

        output.push_str("\n\n<poe_success_criteria>\n");

        // --- Manifest (objective + constraints + soft metrics) ---
        if let Some(manifest) = &poe.manifest {
            output.push_str("## Success Criteria\n\n");
            output.push_str("**Objective:** ");
            output.push_str(&manifest.objective);
            output.push('\n');

            if !manifest.hard_constraints.is_empty() {
                output.push_str("\n### Hard Constraints (ALL must pass)\n");
                for (i, rule) in manifest.hard_constraints.iter().enumerate() {
                    output.push_str(&format!("{}. {}\n", i + 1, format_rule(rule)));
                }
            }

            if !manifest.soft_metrics.is_empty() {
                output.push_str("\n### Soft Metrics (quality score)\n");
                for (i, metric) in manifest.soft_metrics.iter().enumerate() {
                    output.push_str(&format!(
                        "{}. {} (weight={:.1}, threshold={:.1})\n",
                        i + 1,
                        format_rule(&metric.rule),
                        metric.weight,
                        metric.threshold,
                    ));
                }
            }
        }

        // --- Progress summary ---
        if let Some(progress) = &poe.progress_summary {
            output.push_str("\n### Progress\n");
            output.push_str(progress);
            output.push('\n');
        }

        // --- Current hint ---
        if let Some(hint) = &poe.current_hint {
            output.push_str("\n### Current Step Guidance\n");
            output.push_str(hint);
            output.push('\n');
        }

        output.push_str("</poe_success_criteria>\n");
    }
}

/// Format a [`ValidationRule`] for human-readable display inside the prompt.
fn format_rule(rule: &ValidationRule) -> String {
    match rule {
        ValidationRule::FileExists { path } => {
            format!("File must exist: `{}`", path.display())
        }
        ValidationRule::FileNotExists { path } => {
            format!("File must NOT exist: `{}`", path.display())
        }
        ValidationRule::FileContains { path, pattern } => {
            format!(
                "File `{}` must contain pattern: `{}`",
                path.display(),
                pattern,
            )
        }
        ValidationRule::FileNotContains { path, pattern } => {
            format!(
                "File `{}` must NOT contain pattern: `{}`",
                path.display(),
                pattern,
            )
        }
        ValidationRule::DirStructureMatch { root, expected } => {
            format!(
                "Directory `{}` must match structure: {}",
                root.display(),
                expected,
            )
        }
        ValidationRule::CommandPasses {
            cmd,
            args,
            timeout_ms,
        } => {
            let args_str = if args.is_empty() {
                String::new()
            } else {
                format!(" {}", args.join(" "))
            };
            format!(
                "Command must pass: `{}{}` (timeout: {}ms)",
                cmd, args_str, timeout_ms,
            )
        }
        ValidationRule::CommandOutputContains {
            cmd,
            args,
            pattern,
            timeout_ms,
        } => {
            let args_str = if args.is_empty() {
                String::new()
            } else {
                format!(" {}", args.join(" "))
            };
            format!(
                "Command `{}{}` output must contain `{}` (timeout: {}ms)",
                cmd, args_str, pattern, timeout_ms,
            )
        }
        ValidationRule::JsonSchemaValid { path, schema } => {
            let schema_preview = if schema.len() > 60 {
                format!("{}...", &schema[..60])
            } else {
                schema.clone()
            };
            format!(
                "File `{}` must be valid JSON against schema: {}",
                path.display(),
                schema_preview,
            )
        }
        ValidationRule::SemanticCheck {
            target: _,
            prompt,
            passing_criteria: _,
            model_tier,
        } => {
            format!(
                "Semantic check ({}): {}",
                model_tier.name(),
                prompt,
            )
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::poe::prompt_context::PoePromptContext;
    use crate::poe::types::{SuccessManifest, ValidationRule};
    use crate::thinker::prompt_builder::PromptConfig;
    use crate::thinker::prompt_layer::AssemblyPath;
    use std::path::PathBuf;

    /// Helper: run inject and return the output string.
    fn run_inject(poe_ctx: &PoePromptContext) -> String {
        let config = PromptConfig::default();
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        let input = LayerInput::basic(&config, &tools).with_poe(poe_ctx);
        let mut output = String::new();
        PoePromptLayer.inject(&mut output, &input);
        output
    }

    #[test]
    fn test_no_injection_when_no_poe_context() {
        let config = PromptConfig::default();
        let tools: Vec<crate::agent_loop::ToolInfo> = vec![];
        // No POE context at all (poe = None)
        let input = LayerInput::basic(&config, &tools);
        let mut output = String::new();
        PoePromptLayer.inject(&mut output, &input);
        assert!(output.is_empty(), "Expected empty output when no POE context");
    }

    #[test]
    fn test_injects_manifest_objective() {
        let manifest = SuccessManifest::new("t1", "Create the auth module")
            .with_hard_constraint(ValidationRule::FileExists {
                path: PathBuf::from("src/auth.rs"),
            });
        let ctx = PoePromptContext::new().with_manifest(manifest);

        let output = run_inject(&ctx);

        assert!(
            output.contains("Success Criteria"),
            "Output should contain 'Success Criteria'"
        );
        assert!(
            output.contains("Create the auth module"),
            "Output should contain the objective"
        );
        assert!(
            output.contains("File must exist: `src/auth.rs`"),
            "Output should contain formatted hard constraint"
        );
    }

    #[test]
    fn test_injects_hint() {
        let ctx = PoePromptContext::new().with_hint("Focus on error handling first".into());

        let output = run_inject(&ctx);

        assert!(
            output.contains("Current Step Guidance"),
            "Output should contain 'Current Step Guidance'"
        );
        assert!(
            output.contains("Focus on error handling first"),
            "Output should contain the hint text"
        );
    }

    #[test]
    fn test_priority_is_505() {
        assert_eq!(PoePromptLayer.priority(), 505);
    }

    #[test]
    fn test_participates_in_all_paths() {
        let paths = PoePromptLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Cached));
        assert_eq!(paths.len(), 5, "Should participate in exactly 5 paths");
    }
}
