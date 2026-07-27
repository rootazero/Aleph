//! `SecurityLayer` — security constraints injection (priority 600)

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
        // Participate in every non-minimal path so the layer fires on the
        // harness `Basic` / `Cached` routes, where `ResolvedContext` is
        // threaded. The inject() guard keeps output empty when it is absent.
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let ctx = match input.context {
            Some(c) => c,
            None => return,
        };

        let disabled_tools = &ctx.disabled_tools;
        let security_notes = &ctx.environment_contract.security_notes;
        let sandbox_summary = ctx.sandbox_summary.as_ref();

        // The paradigm-derived approval posture yields to the resolved `ExecTier`
        // whenever one exists: both answer "does a mutating call pause for the
        // human?", only the tier is enforced, and for a Messaging turn at
        // `exec_tier = auto` they flatly contradicted each other three bullets
        // apart. `OperatingEnvelopeLayer` @1758 states the tier.
        let elevated_note = ctx
            .approval_tier
            .is_none()
            .then_some(ctx.environment_contract.elevated_policy_note.as_deref())
            .flatten();

        // Only add the section if something below can actually render. The gate
        // used to admit ANY non-empty `disabled_tools`, including the
        // `UnsupportedByChannel` / `Unhealthy` reasons that have no render arm —
        // which would emit a bare header with no body.
        let blocked_by_policy: Vec<_> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::BlockedByPolicy { .. }))
            .collect();
        let requires_approval: Vec<_> = disabled_tools
            .iter()
            .filter(|d| matches!(d.reason, DisableReason::RequiresApproval { .. }))
            .collect();
        if security_notes.is_empty()
            && blocked_by_policy.is_empty()
            && requires_approval.is_empty()
            && sandbox_summary.is_none()
            && elevated_note.is_none()
        {
            return;
        }

        output.push_str("## Security & Constraints\n\n");

        // Sandbox posture (codex-inspired): tells the LLM which enforcer
        // it is running under so it can plan accordingly instead of
        // discovering limits through trial-and-error.
        if let Some(summary) = sandbox_summary {
            for line in summary.to_prompt_lines() {
                let line = sanitize_for_prompt(&line, SanitizeLevel::Light);
                output.push_str(&format!("- {line}\n"));
            }
            output.push('\n');
        }

        // Security notes
        for note in security_notes {
            let note = sanitize_for_prompt(note, SanitizeLevel::Light);
            output.push_str(&format!("- {note}\n"));
        }
        // Fallback approval posture, only when no `ExecTier` was resolved.
        if let Some(note) = elevated_note {
            let note = sanitize_for_prompt(note, SanitizeLevel::Light);
            output.push_str(&format!("- {note}\n"));
        }
        if !security_notes.is_empty() || elevated_note.is_some() {
            output.push('\n');
        }

        if !blocked_by_policy.is_empty() {
            output.push_str("**Disabled by Policy**:\n");
            for tool in blocked_by_policy {
                if let DisableReason::BlockedByPolicy { ref reason } = tool.reason {
                    output.push_str(&format!("- `{}` — {}\n", tool.name, reason));
                }
            }
            output.push('\n');
        }

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
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Security & Constraints"));
        assert!(out.contains("macos/seatbelt"));
        assert!(out.contains("workspace-write"));
        assert!(out.contains("/ws/abc"));
        assert!(out.contains("512 MiB"));
    }

    /// The volatile envelope knobs belong to `OperatingEnvelopeLayer` @1758
    /// (Dynamic). This Stable layer must never restate them — that placement is
    /// what invalidated a whole conversation's prompt cache on a pill flip.
    #[test]
    fn never_renders_the_volatile_envelope_knobs() {
        use crate::config::types::policies::{ExecTier, SessionMode};
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::security_context::SecurityContext;
        use crate::thinker::InteractionManifest;
        use crate::thinker::InteractionParadigm;

        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.approval_tier = Some(ExecTier::Ask);
        ctx.session_mode = Some(SessionMode::Code);

        let layer = SecurityLayer;
        let config = PromptConfig::default();
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(!out.contains("Approval mode"), "{out}");
        assert!(!out.contains("Usage mode"), "{out}");
    }

    /// One approval voice, and it is the enforced one: the paradigm-derived
    /// `ElevatedPolicy` note yields whenever a real `ExecTier` was resolved.
    /// Regression for the Messaging + `exec_tier = auto` contradiction, where the
    /// prompt carried both "Approval mode: auto — routine tool calls run without
    /// interruption" and "Elevated Operations: Require user approval".
    #[test]
    fn elevated_policy_note_yields_to_a_resolved_tier() {
        use crate::config::types::policies::ExecTier;
        use crate::thinker::context::ContextAggregator;
        use crate::thinker::security_context::SecurityContext;
        use crate::thinker::InteractionManifest;
        use crate::thinker::InteractionParadigm;

        let layer = SecurityLayer;
        let config = PromptConfig::default();
        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Messaging),
            &SecurityContext::for_paradigm(InteractionParadigm::Messaging),
            &[],
        );

        // No tier resolved (internal / sub-agent dispatch) → the paradigm note is
        // the only approval signal available, so it renders.
        assert!(ctx.approval_tier.is_none());
        let mut out = String::new();
        layer.inject(
            &mut out,
            &LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx)),
        );
        assert!(
            out.contains("Elevated Operations: Require user approval"),
            "{out}"
        );

        // Tier resolved → the unenforced note must vanish.
        ctx.approval_tier = Some(ExecTier::Auto);
        let mut out = String::new();
        layer.inject(
            &mut out,
            &LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx)),
        );
        assert!(!out.contains("Elevated Operations"), "{out}");
        assert!(
            out.contains("Security Level"),
            "other notes must survive: {out}"
        );
    }

    /// The section gate must agree with the body: a `disabled_tools` list holding
    /// only reasons the body cannot render must not emit a bare header.
    #[test]
    fn unrenderable_disable_reasons_do_not_emit_a_bare_header() {
        use crate::thinker::context::{ContextAggregator, DisableReason, DisabledTool};
        use crate::thinker::security_context::SecurityContext;
        use crate::thinker::InteractionManifest;
        use crate::thinker::InteractionParadigm;

        let mut ctx = ContextAggregator::resolve(
            &InteractionManifest::new(InteractionParadigm::Background),
            &SecurityContext::permissive(),
            &[],
        );
        ctx.environment_contract.security_notes.clear();
        ctx.environment_contract.elevated_policy_note = None;
        ctx.disabled_tools = vec![DisabledTool {
            name: "canvas".to_string(),
            reason: DisableReason::UnsupportedByChannel,
        }];

        let layer = SecurityLayer;
        let config = PromptConfig::default();
        let mut out = String::new();
        layer.inject(
            &mut out,
            &LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx)),
        );
        assert!(out.is_empty(), "header with no renderable body: {out}");
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
        let input = LayerInput::basic(&config, &[]).with_resolved_context_opt(Some(&ctx));
        let mut out = String::new();
        layer.inject(&mut out, &input);

        // Header present (other notes exist) but no sandbox backend tag.
        assert!(!out.contains("macos/seatbelt"));
        assert!(!out.contains("linux/bwrap"));
        assert!(!out.contains("none/disabled"));
    }
}
