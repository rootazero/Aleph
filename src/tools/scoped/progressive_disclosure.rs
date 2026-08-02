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

/// Tools whose description must survive `truncate_tool_descriptions` intact:
/// the three memory writers, each of which closes with the D4 acknowledgment
/// contract (tell the user in one short sentence what was recorded, in their
/// language, and don't quote it back).
///
/// Truncation keeps only the FIRST sentence, and no writer states that contract
/// in its first sentence — so with the flag on, a writer outside the configured
/// core set lost the whole contract silently, with every test green.
/// `flag_user_correction` is not in `default_core_tools`, so `rewrite`'s
/// early return never covered it and the default config was already affected.
///
/// Same shape as `session_mode.rs`'s `NEVER_DEFER`: one named list plus one
/// predicate, so the exemption is stated once. The exemption is the
/// *description* only — the schema is still collapsed, so the token saving that
/// motivates the flag is untouched.
///
/// Measured caveat, so nobody reads more protection into this than it delivers:
/// the description the rewriter actually sees for these three is their
/// `BUILTIN_TOOL_DEFINITIONS` entry, and all three entries are terse one-line
/// literals that shadow the rich `AlephTool::DESCRIPTION` const (the five file
/// tools point at their consts; these do not). So *today* there is no contract
/// text left to lose — this exemption is pre-positioned, and becomes
/// load-bearing the moment `definitions.rs` points those entries at their
/// consts. `memory_writers_keep_their_whole_description_under_truncation`
/// asserts both halves and fails loudly when that changes.
const NEVER_TRUNCATE: &[&str] = &["remember", "note_manage", "flag_user_correction"];

/// Whether `name`'s description carries a contract that truncation must not
/// cut. Single source of truth for [`NEVER_TRUNCATE`].
fn preserves_full_description(name: &str) -> bool {
    NEVER_TRUNCATE.contains(&name)
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

        if self.truncate_desc && !preserves_full_description(&def.name) {
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

    /// The one substring all three memory writers state verbatim in their D4
    /// acknowledgment contract. Their tails differ by tier on purpose
    /// ("always-loaded hot memory" / "where it landed" / "where the correction
    /// was recorded"), so the tier-specific half is deliberately NOT matched.
    const D4_MARKER: &str = "short sentence, in the user's language";

    /// How many registered catalog entries currently ship [`D4_MARKER`].
    ///
    /// Three: `remember`, `note_manage`, `flag_user_correction`. This number is
    /// the load-bearing part of the test — it was 0 until the three entries in
    /// `BUILTIN_TOOL_DEFINITIONS` stopped restating their tools in prose and
    /// started pointing at their `AlephTool::DESCRIPTION` consts. A catalog
    /// entry written as a literal SHADOWS its const (`agent_init` only appends
    /// registry tools whose name the catalog does not already carry), so a
    /// future "let's shorten this entry" edit silently un-ships the contract
    /// again while `memory_protocol.rs`'s const-side assertions stay green.
    /// That is what this count exists to catch.
    const D4_SHIPPING_ENTRIES: usize = 3;

    /// Every builtin as the model is offered it — same static catalog
    /// `agent_init` maps into the LLM tool list, which is where the rewriter's
    /// descriptions come from at request time.
    fn registered_defs() -> Vec<ToolDefinition> {
        crate::executor::BUILTIN_TOOL_DEFINITIONS
            .iter()
            .map(|d| ToolDefinition {
                name: d.name.to_string(),
                description: d.description.to_string(),
                input_schema: json!({"type":"object","properties":{"x":{"type":"string"}}}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            })
            .collect()
    }

    /// Contract test for [`NEVER_TRUNCATE`], run over the real catalog rather
    /// than a hand-built double — the drift being guarded against is precisely
    /// between the exemption list and what is actually registered.
    ///
    /// The writer set is derived from `NEVER_TRUNCATE` / [`preserves_full_description`]
    /// alone; re-listing the three names here would recreate the second
    /// hand-maintained list this whole round exists to remove.
    #[test]
    fn memory_writers_keep_their_whole_description_under_truncation() {
        // Truncation ON with an EMPTY core set: the worst case the config can
        // express, since `rewrite`'s core early-return then spares nothing.
        // It is also the DEFAULT case for `flag_user_correction`, which
        // `default_core_tools()` does not list.
        let rw = ProgressiveDisclosureRewriter::new(BTreeSet::new(), true);
        let before = registered_defs();
        let mut after = before.clone();
        for d in &mut after {
            rw.rewrite(d);
        }

        // An exemption for a tool nobody registers protects nothing; it is a
        // stale claim about a tool no reader can find. Same ghost check
        // `prompt_contract.rs` runs on CONDITIONALLY_SILENT.
        for name in NEVER_TRUNCATE {
            assert!(
                before.iter().any(|d| d.name == *name),
                "NEVER_TRUNCATE names `{name}`, which is not a registered builtin"
            );
        }

        // The contract itself: an exempted writer's registered description
        // reaches the model whole — the rewriter may only APPEND its
        // schema-loading hint, never cut.
        for (b, a) in before.iter().zip(&after) {
            if !preserves_full_description(&b.name) {
                continue;
            }
            assert!(
                a.description.starts_with(&b.description),
                "`{}` lost description text to truncation — the exemption is not \
                 reaching it. Got: {:?}",
                b.name,
                a.description
            );
        }

        // Anti-vacuity: the loop above passes trivially if truncation did
        // nothing at all this run (inverted flag, broken `first_sentence_head`).
        // Prove the flag is live by finding a non-exempt tool that WAS cut.
        let shortened = before
            .iter()
            .zip(&after)
            .filter(|(b, a)| {
                !preserves_full_description(&b.name) && !a.description.starts_with(&b.description)
            })
            .count();
        assert!(
            shortened > 0,
            "truncation shortened no description at all — this test would pass just as \
             happily with the exemption deleted, so it is measuring nothing"
        );

        // ...and the clause the exemption exists for, counted on the surface
        // that actually ships.
        let shipping: Vec<&str> = before
            .iter()
            .filter(|d| d.description.contains(D4_MARKER))
            .map(|d| d.name.as_str())
            .collect();
        assert_eq!(
            shipping.len(),
            D4_SHIPPING_ENTRIES,
            "the number of catalog entries shipping the D4 acknowledgment clause changed \
             (now: {shipping:?}). If entries gained it, raise D4_SHIPPING_ENTRIES and drop \
             its note. If they lost it, a writer's contract just stopped reaching the model."
        );
        for name in &shipping {
            let a = after
                .iter()
                .find(|d| d.name == *name)
                .expect("rewritten set has the same names");
            assert!(
                a.description.contains(D4_MARKER),
                "`{name}` ships the D4 clause but truncation cut it: {:?}",
                a.description
            );
        }
    }

    /// The one sentence that decides whether a governance audit verdict is
    /// right must reach the model, and the only surface that proves that is
    /// the registered catalog — the same shadowing trap [`D4_SHIPPING_ENTRIES`]
    /// guards, one tool over.
    ///
    /// `synthesis_sum` is 0 on every `consolidate` run *by design*. An auditor
    /// that does not know this reads a perfectly healthy nightly distillation
    /// as "dreaming stopped producing" and files a `stale` verdict against it.
    /// Asserting on `GovernanceMetricsTool::DESCRIPTION` instead would stay
    /// green through exactly the regression this exists to catch (a
    /// hand-written catalog literal silently shadows the const).
    #[test]
    fn governance_metrics_ships_its_synthesis_sum_caveat_to_the_model() {
        let entry = registered_defs()
            .into_iter()
            .find(|d| d.name == "governance_metrics")
            .expect("governance_metrics is a registered builtin");
        assert!(
            entry.description.contains("`synthesis_sum` is 0"),
            "governance_metrics' registered description lost the synthesis_sum caveat — \
             the audit ring now reads a healthy consolidate as a dead one: {:?}",
            entry.description
        );
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
