//! Prompt strings and LLM-backed helpers for the wiki orientation layer.

use crate::error::AlephError;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;
use crate::providers::AiProvider;
use crate::sync_primitives::Arc;

/// System prompt used by `schema_via_llm` to generate an opinionated
/// initial SCHEMA.md.
pub const PROMPT_ORIENTATION_BOOTSTRAP: &str = r#"You produce the initial SCHEMA.md for an Aleph personal-memory workspace.
Output MUST be a single markdown document with the following exact shape:

---
schema_version: 1
updated: "YYYY-MM-DD"
---
# Memory Schema

## Domain
<1-3 sentences describing what this agent's memory is about. Use the user's hint if present.>

## Categories (fixed by Aleph)
preference | plan | learning | project | personal | tool | lesson | skill | reference | other
Special: synthesis (weekly dream output). skill/reference carry extra frontmatter.

## Tag Taxonomy
<!-- LLM maintained. New tag MUST appear here before use. -->
- <seed 5-10 tags relevant to the domain hint>

## Page Thresholds
- create: a topic appearing in 2+ sources, or centrally in one source.
- append: an existing page already covers the topic.
- contradict: new content conflicts with existing — mark, never silently overwrite.

## Update Policy
- Conflict → keep both claims with dates and sources; frontmatter `contradictions: [path]`.
- Supersede → append `## Superseded by [[path]] (YYYY-MM-DD)` to the old page.
- New tags → add to Tag Taxonomy first, then use.

No commentary, no markdown fences around the whole document, no prose before or after.
"#;

/// Ask the provider for a fresh SCHEMA.md tailored to `domain_hint` (may be empty).
pub async fn schema_via_llm(
    provider: &Arc<dyn AiProvider>,
    domain_hint: &str,
) -> Result<String, AlephError> {
    let user = if domain_hint.trim().is_empty() {
        "Produce the initial SCHEMA.md. No domain hint — use a general-purpose setup.".to_string()
    } else {
        format!("Domain hint: {domain_hint}\n\nProduce the initial SCHEMA.md for this domain.")
    };
    let msgs = [UnifiedMessage::user(&user)];
    let resp = provider
        .process(RequestPayload::new(&msgs).with_system(Some(PROMPT_ORIENTATION_BOOTSTRAP)))
        .await
        .map_err(|e| AlephError::other(format!("schema LLM call: {e}")))?;
    Ok(resp.text_content())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::recording_mock::RecordingMockProvider;

    #[test]
    fn prompt_has_five_fixed_sections() {
        let p = PROMPT_ORIENTATION_BOOTSTRAP;
        for name in [
            "Domain",
            "Categories",
            "Tag Taxonomy",
            "Page Thresholds",
            "Update Policy",
        ] {
            assert!(p.contains(&format!("## {name}")), "missing section {name}");
        }
    }

    #[test]
    fn prompt_snapshot() {
        insta::assert_snapshot!("orientation_bootstrap_prompt", PROMPT_ORIENTATION_BOOTSTRAP);
    }

    #[tokio::test]
    async fn schema_via_llm_passes_system_prompt() {
        let mock =
            RecordingMockProvider::new("---\nschema_version: 1\n---\n# Memory Schema\n".into());
        let recorded = mock.recorded_system_prompt();
        let provider: Arc<dyn AiProvider> = Arc::new(mock);
        let _ = schema_via_llm(&provider, "Rust backend development")
            .await
            .unwrap();
        let got = recorded.lock().unwrap().clone().unwrap();
        assert!(got.contains("initial SCHEMA.md"));
    }
}
