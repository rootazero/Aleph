//! Request-time rewriter that implements progressive tool disclosure:
//! non-core tools get their `input_schema` collapsed to an open placeholder
//! (name + description stay visible) so the model can still discover them but
//! pays no schema tokens until it loads the full schema via `get_tool_schema`.
//!
//! This is a STATIC partition (core set is config, decided ahead of any
//! message) applied at the tool-presentation layer — not per-message intent
//! filtering. See CLAUDE.md R10 (第2不 例外注).

use std::collections::BTreeSet;

use serde_json::json;

use crate::sync_primitives::Arc;
use crate::tools::scoped::ToolDefinitionRewriter;
use crate::tools::service::ToolDefinition;

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
        Self { core, truncate_desc }
    }

    /// Build from config. Returns `None` (⇒ attach nothing ⇒ old behavior)
    /// when `core` is empty or contains the `"*"` wildcard sentinel.
    #[must_use]
    pub fn from_config(core: &[String], truncate_desc: bool) -> Option<Arc<dyn ToolDefinitionRewriter>> {
        if core.is_empty() || core.iter().any(|c| c == "*") {
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
            if let Some((head, _)) = def.description.split_once(". ") {
                def.description = head.to_string();
            }
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
}
