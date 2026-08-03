//! `AgentCatalogLayer` — sub-agent catalog index for primary agent awareness (priority 1060)
//!
//! **Stable, and it sits next to `SkillInstructionsLayer` @1050 on purpose**:
//! "which sub-agents can I delegate to" and "which skills do I have" are the same
//! kind of index, and both are fixed for the life of a prompt build.
//!
//! It used to sit at 1704 — inside the Dynamic tail — for adjacency reasons, and
//! that placement cost real money (FEATURE_LOCATOR §2.18 ledger item 10). The
//! dynamic system block carries no `cache_control` marker of its own: it is
//! covered only by the message-level breakpoints, which all sit *after* it. So a
//! byte-identical block parked there is not free — it is re-written at 1.25x
//! every time any *other* dynamic layer changes (`runtime_context`'s clock,
//! `execution_plan`'s per-turn edits, a pill flip). Session-stable content pays
//! its neighbours' volatility tax. In the Stable block it rides the one
//! breakpoint that is guaranteed to hit.
//!
//! The rule that follows, and the one that put this layer in the wrong zone:
//! **`stability()` states whether the content varies, not where the author
//! wanted it to render.** Priority is for reading order; stability is a fact
//! about the bytes.

use crate::thinker::prompt_layer::{
    AgentCatalogEntry, AssemblyPath, LayerInput, LayerStability, PromptLayer,
};
use crate::thinker::prompt_mode::PromptMode;
use crate::thinker::xml_util::escape_xml;

pub struct AgentCatalogLayer;

impl PromptLayer for AgentCatalogLayer {
    fn name(&self) -> &'static str {
        "agent_catalog"
    }
    fn priority(&self) -> u32 {
        1060
    }
    fn stability(&self) -> LayerStability {
        // Declared, not inherited. The default IS `Stable`, but a layer that
        // rides the cacheable prefix by omission is the failure this module set
        // has already suffered once (`ToolRuntimeStateLayer`), so every layer in
        // the prefix says so out loud.
        LayerStability::Stable
    }
    fn supports_mode(&self, mode: PromptMode) -> bool {
        matches!(mode, PromptMode::Full)
    }
    fn paths(&self) -> &'static [AssemblyPath] {
        &[AssemblyPath::Basic, AssemblyPath::Cached]
    }
    fn inject(&self, output: &mut String, input: &LayerInput) {
        let agents = match input.config.available_agents {
            Some(ref agents) if !agents.is_empty() => agents,
            _ => return,
        };

        let visible: Vec<&AgentCatalogEntry> = agents
            .iter()
            .filter(|a| !a.description.is_empty())
            .collect();

        if visible.is_empty() {
            return;
        }

        output.push_str("## Available Agents\n\n");
        output.push_str(
            "Delegate tasks to specialized sub-agents with the `delegate` tool; call \
             `agent_info(agent_id)` first to see a candidate's full capabilities.\n\n",
        );
        output.push_str(&build_agent_catalog_xml(&visible));
        output.push_str("\n\n");
    }
}

fn build_agent_catalog_xml(agents: &[&AgentCatalogEntry]) -> String {
    let mut buf = String::from("<available_agents>\n");
    for agent in agents {
        buf.push_str("  <agent>\n");
        buf.push_str("    <id>");
        buf.push_str(&escape_xml(&agent.id));
        buf.push_str("</id>\n");
        buf.push_str("    <description>");
        buf.push_str(&escape_xml(&agent.description));
        buf.push_str("</description>\n");
        if let Some(ref when) = agent.when_to_use {
            buf.push_str("    <when>");
            buf.push_str(&escape_xml(when));
            buf.push_str("</when>\n");
        }
        buf.push_str("  </agent>\n");
    }
    buf.push_str("</available_agents>");
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::prompt_builder::PromptConfig;

    fn make_entry(id: &str, desc: &str, when: Option<&str>) -> AgentCatalogEntry {
        AgentCatalogEntry {
            id: id.to_string(),
            description: desc.to_string(),
            when_to_use: when.map(|s| s.to_string()),
        }
    }

    #[test]
    fn injects_agent_catalog() {
        let layer = AgentCatalogLayer;
        let entries = vec![
            make_entry("coder", "Writes code", Some("When coding tasks arise")),
            make_entry("researcher", "Searches the web", None),
        ];
        let config = PromptConfig {
            available_agents: Some(entries),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("## Available Agents"));
        assert!(out.contains("<available_agents>"));
        assert!(out.contains("<id>coder</id>"));
        assert!(out.contains("<description>Writes code</description>"));
        assert!(out.contains("<when>When coding tasks arise</when>"));
        assert!(out.contains("<id>researcher</id>"));
        assert!(out.contains("<description>Searches the web</description>"));
        // researcher has no when_to_use
        assert!(!out.contains("<when>Searches"));
    }

    #[test]
    fn filters_empty_descriptions() {
        let layer = AgentCatalogLayer;
        let entries = vec![
            make_entry("visible", "Has a description", None),
            make_entry("hidden", "", None),
        ];
        let config = PromptConfig {
            available_agents: Some(entries),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("<id>visible</id>"));
        assert!(!out.contains("<id>hidden</id>"));
    }

    #[test]
    fn empty_agents_no_output() {
        let layer = AgentCatalogLayer;
        let config = PromptConfig {
            available_agents: Some(vec![]),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn none_agents_no_output() {
        let layer = AgentCatalogLayer;
        let config = PromptConfig {
            available_agents: None,
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.is_empty());
    }

    #[test]
    fn xml_escaping() {
        let layer = AgentCatalogLayer;
        let entries = vec![make_entry(
            "test&id",
            "handles <angle> & ampersand",
            Some("when > threshold"),
        )];
        let config = PromptConfig {
            available_agents: Some(entries),
            ..Default::default()
        };
        let input = LayerInput::basic(&config, &[]);
        let mut out = String::new();
        layer.inject(&mut out, &input);

        assert!(out.contains("<id>test&amp;id</id>"));
        assert!(out.contains("handles &lt;angle&gt; &amp; ampersand"));
        assert!(out.contains("when &gt; threshold"));
    }

    /// Session-stable content must sit in the cacheable prefix, not the dynamic
    /// tail where it pays 1.25x every time a volatile neighbour moves.
    #[test]
    fn lives_in_the_cacheable_prefix_next_to_the_skill_index() {
        assert_eq!(AgentCatalogLayer.priority(), 1060);
        assert_eq!(AgentCatalogLayer.stability(), LayerStability::Stable);
        assert!(
            AgentCatalogLayer.priority() < 1700,
            "the Stable zone runs below 1700; see stable_layers_come_before_dynamic"
        );
    }

    #[test]
    fn full_mode_only() {
        let layer = AgentCatalogLayer;
        assert!(layer.supports_mode(PromptMode::Full));
        assert!(!layer.supports_mode(PromptMode::Compact));
        assert!(!layer.supports_mode(PromptMode::Minimal));
    }
}
