//! `RoleLayer` — core role definition (priority 100)

use crate::thinker::prompt_layer::{AssemblyPath, LayerInput, PromptLayer};
use crate::thinker::prompt_mode::PromptMode;

pub struct RoleLayer;

impl PromptLayer for RoleLayer {
    fn name(&self) -> &'static str {
        "role"
    }
    fn priority(&self) -> u32 {
        100
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        !matches!(mode, PromptMode::Minimal)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        // `Cached` is the live main-agent-loop path. This base role framing
        // complements persona/identity and belongs in every prompt, including
        // the cached production one where it was previously absent. Stable, so
        // it joins the cacheable prefix.
        &[
            AssemblyPath::Basic,
            AssemblyPath::Hydration,
            AssemblyPath::Soul,
            AssemblyPath::Cached,
        ]
    }
    fn inject(&self, output: &mut String, _input: &LayerInput) {
        // Lean: the "Observe → decide the SINGLE next action → execute" cycle
        // was cut (§1.1 prune-the-prompt). The harness supports parallel tool
        // dispatch (`act.rs::can_parallel_dispatch`), so "single next action"
        // was stale and limiting; a capable model runs the loop natively.
        output.push_str(
            "You are an AI assistant that works through tasks using the tools available to you, \
             continuing until the task is complete or you need the user's input.\n\n",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    #[test]
    fn test_role_content() {
        let layer = RoleLayer;
        let config = PromptConfig::default();
        let tools = vec![];
        let input = LayerInput::basic(&config, &tools);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("You are an AI assistant"));
        assert!(out.contains("tools available to you"));
        // The Observe/Decide-SINGLE-action/Execute manual was cut — the harness
        // supports parallel tool dispatch, so "single next action" was stale.
        assert!(!out.contains("SINGLE next action"));
        assert!(!out.contains("## Your Role"));
    }

    #[test]
    fn test_role_priority() {
        assert_eq!(RoleLayer.priority(), 100);
    }

    #[test]
    fn test_role_paths() {
        let paths = RoleLayer.paths();
        assert!(paths.contains(&AssemblyPath::Basic));
        assert!(paths.contains(&AssemblyPath::Hydration));
        assert!(paths.contains(&AssemblyPath::Soul));
        // Core role framing must reach the live cached production path.
        assert!(paths.contains(&AssemblyPath::Cached));
    }
}
