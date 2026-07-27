//! Request-time rewriter that implements progressive tool disclosure:
//! non-core tools get their `input_schema` collapsed to an open placeholder
//! (name + description stay visible) so the model can still discover them but
//! pays no schema tokens until it loads the full schema via `get_tool_schema`.
//!
//! This is a STATIC partition (core set is config, decided ahead of any
//! message) applied at the tool-presentation layer — not per-message intent
//! filtering. See CLAUDE.md R10 (the Progressive Disclosure exception).

use std::collections::BTreeSet;

use serde_json::json;

use crate::sync_primitives::Arc;
use crate::tools::scoped::ToolDefinitionRewriter;
use crate::tools::service::ToolDefinition;

/// First-sentence head for description truncation. Returns `Some(head)` only
/// when a clean sentence boundary exists (a `". "` whose preceding token is
/// not a common abbreviation); otherwise `None` (caller keeps the full text).
fn first_sentence_head(desc: &str) -> Option<&str> {
    const ABBREVS: &[&str] = &["e.g", "i.e", "etc", "vs", "cf", "al", "no", "fig"];
    let mut search_from = 0;
    while let Some(rel) = desc[search_from..].find(". ") {
        let end = search_from + rel; // index of the '.'
        let head = &desc[..end];
        let last_word = head
            .rsplit(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("");
        if head.is_empty() || ABBREVS.iter().any(|a| last_word.eq_ignore_ascii_case(a)) {
            search_from = end + 2;
            continue;
        }
        return Some(head);
    }
    None
}

/// Collapses non-core tools' schemas at request time. Deterministic per
/// `(name, core, truncate)`, so it is safe under the `metadata_schema()`
/// generation cache.
pub struct ProgressiveDisclosureRewriter {
    core: BTreeSet<String>,
    truncate_desc: bool,
}

impl ProgressiveDisclosureRewriter {
    /// Construct directly from a resolved core set.
    #[must_use]
    pub fn new(core: BTreeSet<String>, truncate_desc: bool) -> Self {
        Self {
            core,
            truncate_desc,
        }
    }

    /// Whether progressive disclosure is active for this core set. False when
    /// `core` is empty or contains the `"*"` wildcard (escape hatch — feature
    /// fully off, tool surface byte-identical to pre-feature). Single source of
    /// truth shared by `from_config` and the request-path registration gate.
    #[must_use]
    pub fn is_enabled(core: &[String]) -> bool {
        !core.is_empty() && !core.iter().any(|c| c == "*")
    }

    /// Build from config. Returns `None` (⇒ attach nothing ⇒ old behavior)
    /// when `core` is empty or contains the `"*"` wildcard sentinel.
    #[must_use]
    pub fn from_config(
        core: &[String],
        truncate_desc: bool,
    ) -> Option<Arc<dyn ToolDefinitionRewriter>> {
        if !Self::is_enabled(core) {
            return None;
        }
        let set: BTreeSet<String> = core.iter().cloned().collect();
        Some(Arc::new(Self::new(set, truncate_desc)))
    }
}

impl ToolDefinitionRewriter for ProgressiveDisclosureRewriter {
    fn rewrite(&self, def: &mut ToolDefinition) {
        if self.core.contains(&def.name) {
            return; // keep full schema + description
        }
        // Collapse to an open object so the eventual real call is accepted
        // by the provider (the model supplies args learned via get_tool_schema).
        def.input_schema = json!({ "type": "object", "additionalProperties": true });

        if self.truncate_desc {
            if let Some(head) = first_sentence_head(&def.description) {
                def.description = head.to_string();
            }
            // else: no clean boundary → keep full description (fail-safe, never garbled)
        }
        def.description.push_str(&format!(
            " [Parameters collapsed — call get_tool_schema(tool_name=\"{}\") to load the full input schema before calling this tool.]",
            def.name
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolSource};
    use serde_json::json;

    fn def(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: "Does a thing. Second sentence.".to_string(),
            input_schema: json!({"type":"object","properties":{"x":{"type":"string"}},"required":["x"]}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        }
    }

    #[test]
    fn collapses_non_core_keeps_core() {
        let rw = ProgressiveDisclosureRewriter::new(["bash".into()].into_iter().collect(), false);
        let mut core = def("bash");
        let mut other = def("browser_navigate");
        rw.rewrite(&mut core);
        rw.rewrite(&mut other);
        // core untouched
        assert!(core.input_schema.get("properties").is_some());
        assert_eq!(core.description, "Does a thing. Second sentence.");
        // non-core collapsed + hint, name never renamed
        assert_eq!(other.name, "browser_navigate");
        assert!(other.input_schema.get("properties").is_none());
        assert_eq!(other.input_schema["additionalProperties"], json!(true));
        assert!(other.description.contains("get_tool_schema"));
    }

    #[test]
    fn truncate_desc_option_shortens_first_sentence() {
        let rw = ProgressiveDisclosureRewriter::new(std::collections::BTreeSet::new(), true);
        let mut d = def("x");
        rw.rewrite(&mut d);
        assert!(d.description.starts_with("Does a thing"));
        assert!(!d.description.contains("Second sentence"));
    }

    #[test]
    fn from_config_disabled_on_wildcard_or_empty() {
        assert!(ProgressiveDisclosureRewriter::from_config(&["*".into()], false).is_none());
        assert!(ProgressiveDisclosureRewriter::from_config(&[], false).is_none());
        assert!(ProgressiveDisclosureRewriter::from_config(&["bash".into()], false).is_some());
    }

    #[test]
    fn truncate_does_not_garble_abbreviations() {
        let rw = ProgressiveDisclosureRewriter::new(std::collections::BTreeSet::new(), true);
        let mut d = ToolDefinition {
            name: "test_cmd".to_string(),
            description: "Executes a command, e.g. ls -la. Use carefully.".to_string(),
            input_schema: json!({"type":"object"}),
            source: ToolSource::Builtin,
            metadata: ToolDefinitionMetadata::default(),
        };
        rw.rewrite(&mut d);
        // Should NOT contain the garbled fragment
        assert!(!d.description.contains("Executes a command, e.g ["));
        // Should NOT start with garbled "e.g"
        assert!(!d.description.starts_with("Executes a command, e.g "));
        // Should contain the hint about get_tool_schema
        assert!(d.description.contains("get_tool_schema"));
    }

    #[test]
    fn mixed_wildcard_disables() {
        let result =
            ProgressiveDisclosureRewriter::from_config(&["bash".into(), "*".into()], false);
        assert!(result.is_none());
    }

    #[test]
    fn is_enabled_matches_from_config_gate() {
        assert!(!ProgressiveDisclosureRewriter::is_enabled(&[]));
        assert!(!ProgressiveDisclosureRewriter::is_enabled(&[
            "*".to_string()
        ]));
        assert!(!ProgressiveDisclosureRewriter::is_enabled(&[
            "bash".to_string(),
            "*".to_string()
        ]));
        assert!(ProgressiveDisclosureRewriter::is_enabled(&[
            "bash".to_string()
        ]));
    }

    #[test]
    fn collapsing_shrinks_serialized_tools_by_half() {
        // 20 fat tools (schema-heavy) + 2 core.
        let core: std::collections::BTreeSet<String> = ["bash".into(), "get_tool_schema".into()]
            .into_iter()
            .collect();
        let rw = ProgressiveDisclosureRewriter::new(core.clone(), false);
        let fat_schema = json!({
            "type":"object",
            "properties": (0..12).map(|i| (format!("field_{i}"), json!({"type":"string","description":"a reasonably long description of this parameter that costs tokens"}))).collect::<serde_json::Map<_,_>>(),
        });
        let mut defs: Vec<ToolDefinition> = (0..22)
            .map(|i| ToolDefinition {
                name: if i < 2 {
                    ["bash", "get_tool_schema"][i].to_string()
                } else {
                    format!("tool_{i}")
                },
                description: "does a thing".to_string(),
                input_schema: fat_schema.clone(),
                source: crate::tools::service::ToolSource::Builtin,
                metadata: crate::tools::service::ToolDefinitionMetadata::default(),
            })
            .collect();
        let before = serde_json::to_string(&defs).unwrap().len();
        for d in &mut defs {
            rw.rewrite(d);
        }
        let after = serde_json::to_string(&defs).unwrap().len();
        assert!(
            after * 2 < before,
            "expected >50% shrink, got {before} -> {after}"
        );
    }
}
