//! SecurityLayer — security constraints injection (priority 600)

use crate::thinker::context::DisableReason;
use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::prompt_sanitizer::{sanitize_for_prompt, SanitizeLevel};

pub struct SecurityLayer;

impl PromptLayer for SecurityLayer {
    fn name(&self) -> &'static str {
        "security"
    }
    fn priority(&self) -> u32 {
        600
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // Phase 2 wiring: participate in every non-minimal path so the
        // layer fires on the harness `Basic` path. The inject() guard
        // keeps output empty until a `ResolvedContext` is threaded into
        // `LayerInput::context` (Phase 3 work), so widening here is a
        // pure no-op today and ready to emit when context arrives.
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Context,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };

        let disabled_tools = &ctx.disabled_tools;
        let security_notes = &ctx.environment_contract.security_notes;
        let sandbox_summary = ctx.sandbox_summary.as_ref();

        // Only add section if there's something to report
        if security_notes.is_empty() && disabled_tools.is_empty() && sandbox_summary.is_none() {
            return;
        }

        output.push_str("## Security & Constraints\n\n");

        // Sandbox posture (codex-inspired): tells the LLM which enforcer
        // it is running under so it can plan accordingly instead of
        // discovering limits through trial-and-error.
        if let Some(summary) = sandbox_summary {
            for line in summary.to_prompt_lines() {
                let line = sanitize_for_prompt(&line, SanitizeLevel::Light);
                output.push_str(&format!("- {}\n", line));
            }
            output.push('\n');
        }

        // Security notes
        for note in security_notes {
            let note = sanitize_for_prompt(note, SanitizeLevel::Light);
            output.push_str(&format!("- {}\n", note));
        }
        if !security_notes.is_empty() {
            output.push('\n');
        }

        // Collect policy-blocked tools
        let blocked_by_policy: Vec<_> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::BlockedByPolicy { .. }))
            .collect();

        if !blocked_by_policy.is_empty() {
            output.push_str("**Disabled by Policy**:\n");
            for tool in blocked_by_policy {
                if let DisableReason::BlockedByPolicy { ref reason } = tool.reason {
                    output.push_str(&format!("- `{}` — {}\n", tool.name, reason));
                }
            }
            output.push('\n');
        }

        // Collect approval-required tools
        let requires_approval: Vec<_> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::RequiresApproval { .. }))
            .collect();

        if !requires_approval.is_empty() {
            output.push_str("**Requires User Approval**:\n");
            for tool in requires_approval {
                if let DisableReason::RequiresApproval {
                    prompt: ref approval_prompt,
                } = tool.reason
                {
                    output.push_str(&format!(
                        "- `{}` — available, but each invocation requires user confirmation ({})\n",
                        tool.name, approval_prompt
                    ));
                }
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
    fn test_security_no_context() {
        let layer = SecurityLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn test_security_paths() {
        let paths = SecurityLayer.paths();
        // Phase 2: layer now participates in every non-minimal path so it
        // fires on the harness `Basic` route — graceful no-op until a
        // ResolvedContext is threaded in.
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Soul));
        assert!(paths.contains(&AssemblyPath::Context));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Cached));
    }

    #[test]
    fn graceful_noop_on_basic_path_without_context() {
        let layer = SecurityLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        // Basic path doesn't carry a ResolvedContext; the layer must emit
        // nothing instead of a half-rendered "## Security & Constraints"
        // header.
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);
        assert!(out.is_empty());
    }

    #[test]
    fn renders_sandbox_summary_when_attached() {
        use crate::sandbox::{NetworkState, PolicyTier, SandboxSummary};
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::security_context::SecurityContext;
        use crate::thinker::InteractionManifest;
        use crate::thinker::InteractionParadigm;

        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.sandbox_summary = Some(SandboxSummary {
            backend: "macos/seatbelt",
            policy_tier: PolicyTier::WorkspaceWrite.as_str(),
            writable_roots: vec![std::path::PathBuf::from("/ws/abc")],
            network: NetworkState::AllowAll,
            max_memory_mb: Some(512),
        });

        let layer = SecurityLayer;
        let config = PromptConfig::default();
        let _tools: Vec<crate::tools::info::ToolInfo> = vec![];
        let input = LayerInput::context(&config, &ctx);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Security & Constraints"));
        assert!(out.contains("macos/seatbelt"));
        assert!(out.contains("workspace-write"));
        assert!(out.contains("/ws/abc"));
        assert!(out.contains("512 MiB"));
    }

    #[test]
    fn omits_sandbox_section_when_summary_is_none() {
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::security_context::SecurityContext;
        use crate::thinker::InteractionManifest;
        use crate::thinker::InteractionParadigm;

        let ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        // sandbox_summary defaults to None; security_notes is still
        // populated by `permissive` (one note), so the section still emits.
        assert!(ctx.sandbox_summary.is_none());

        let layer = SecurityLayer;
        let config = PromptConfig::default();
        let _tools: Vec<crate::tools::info::ToolInfo> = vec![];
        let input = LayerInput::context(&config, &ctx);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Header present (other notes exist) but no sandbox backend tag.
        assert!(!out.contains("macos/seatbelt"));
        assert!(!out.contains("linux/bwrap"));
        assert!(!out.contains("none/disabled"));
    }
}
