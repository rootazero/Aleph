//! SkillDistill stage — extracts reusable skill-notes from synthesis output.
//!
//! Runs after NoteSynthesis in the Synthesize strategy. Reads synthesis notes
//! produced in the current cycle and asks an LLM to extract actionable
//! patterns as `skill`-category knowledge notes.

use async_trait::async_trait;

use crate::error::AlephError;
use crate::memory::dreaming::DreamContext;
use crate::memory::notes::KnowledgeNote;
use crate::providers::adapter::RequestPayload;
use crate::providers::message::UnifiedMessage;

use super::DreamStage;

pub struct SkillDistillStage {
    pub max_per_cycle: usize,
}

impl Default for SkillDistillStage {
    fn default() -> Self {
        Self {
            max_per_cycle: crate::config::types::memory::default_skill_distill_max_per_cycle(),
        }
    }
}

#[async_trait]
impl DreamStage for SkillDistillStage {
    fn name(&self) -> &'static str {
        "skill_distill"
    }

    async fn should_run(&self, ctx: &DreamContext) -> bool {
        // Only run if there are synthesis notes to distill from
        ctx.notes.iter().any(|n| n.category == "synthesis")
    }

    async fn execute(&self, mut ctx: DreamContext) -> Result<DreamContext, AlephError> {
        let synthesis_notes: Vec<_> = ctx
            .notes
            .iter()
            .filter(|n| n.category == "synthesis")
            .map(|n| n.path.clone())
            .collect();

        let mut distilled_count = 0usize;

        for path in &synthesis_notes {
            let content = match ctx.load_content(path).await {
                Some(c) => c,
                None => continue,
            };

            let category = path
                .split('/')
                .next()
                .map(|p| {
                    // Extract the original category from synthesis title
                    // e.g., "synthesis/learning-synthesis" → "learning"
                    p.strip_suffix("-synthesis").unwrap_or(p)
                })
                .unwrap_or("general");

            let prompt = build_distill_prompt(&content, category, self.max_per_cycle);
            let system = "You are a skill extraction engine. Extract actionable, reusable patterns from synthesis notes. Return a JSON array.";

            let msgs = vec![UnifiedMessage::user(&prompt)];
            let response = match ctx
                .provider
                .process(RequestPayload::new(&msgs).with_system(Some(system)))
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(path, error = %e, "SkillDistill LLM call failed");
                    continue;
                }
            };

            let skills = parse_distilled_skills(&response.text_content());

            for skill in &skills {
                let note = KnowledgeNote {
                    title: skill.title.clone(),
                    category: "skill".to_string(),
                    tags: vec!["distilled".to_string(), category.to_string()],
                    facts: skill.facts.clone(),
                    links: vec![format!("[[{}]]", path)],
                    created_at: chrono::Utc::now().timestamp(),
                    updated_at: chrono::Utc::now().timestamp(),
                    content_hash: String::new(),
                    ..Default::default()
                };

                match ctx.indexer.write_note(&ctx.agent_id, "skill", &note).await {
                    Ok(_) => {
                        distilled_count += 1;
                        tracing::info!(title = %skill.title, "Distilled skill-note");
                    }
                    Err(e) => {
                        tracing::warn!(title = %skill.title, error = %e, "Failed to write skill-note");
                    }
                }
            }
        }

        // Store distilled count in extras for the report
        ctx.report
            .extra
            .insert("skill_distill_count".into(), distilled_count.to_string());

        tracing::info!(distilled_count, "SkillDistill completed");
        Ok(ctx)
    }
}

/// Build the LLM prompt for skill extraction from synthesis content.
pub fn build_distill_prompt(synthesis_text: &str, source_category: &str, max_per_cycle: usize) -> String {
    format!(
        "Analyze this synthesis note from the '{source_category}' category and extract reusable skill patterns.\n\n\
         Synthesis:\n{synthesis_text}\n\n\
         Extract 0-{max_per_cycle} actionable skill patterns. For each, provide:\n\
         - A kebab-case title (e.g., \"async-error-handling\")\n\
         - 2-5 concise fact bullets (third person, actionable)\n\n\
         Return as JSON array:\n\
         ```json\n\
         [\n\
           {{\"title\": \"skill-name\", \"facts\": [\"fact 1\", \"fact 2\"]}}\n\
         ]\n\
         ```\n\
         Return `[]` if no actionable patterns found."
    )
}

/// Parsed skill from LLM response.
#[derive(Debug, Clone)]
pub struct DistilledSkill {
    pub title: String,
    pub facts: Vec<String>,
}

/// Parse LLM response into distilled skills. Tolerant of formatting issues.
pub fn parse_distilled_skills(response: &str) -> Vec<DistilledSkill> {
    // Try to find JSON array in response (may be wrapped in markdown code block)
    let json_str = response
        .find('[')
        .and_then(|start| response.rfind(']').map(|end| &response[start..=end]))
        .unwrap_or("[]");

    let parsed: Vec<serde_json::Value> = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    parsed
        .into_iter()
        .filter_map(|v| {
            let title = v.get("title")?.as_str()?.to_string();
            let facts: Vec<String> = v
                .get("facts")?
                .as_array()?
                .iter()
                .filter_map(|f| f.as_str().map(String::from))
                .collect();
            if title.is_empty() || facts.is_empty() {
                return None;
            }
            Some(DistilledSkill { title, facts })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_name() {
        assert_eq!(SkillDistillStage::default().name(), "skill_distill");
    }

    #[test]
    fn stage_default_uses_config_default_cap() {
        assert_eq!(SkillDistillStage::default().max_per_cycle, 3);
    }

    #[test]
    fn prompt_contains_synthesis_content() {
        let synthesis_text = "Cross-cutting theme: async patterns are preferred.";
        let prompt = build_distill_prompt(synthesis_text, "learning", 3);
        assert!(prompt.contains("async patterns"));
        assert!(prompt.contains("learning"));
        assert!(prompt.contains("skill"));
    }

    #[test]
    fn prompt_uses_configured_cap() {
        let prompt = build_distill_prompt("text", "general", 7);
        assert!(prompt.contains("Extract 0-7"));
        assert!(!prompt.contains("Extract 0-3"));
    }

    #[test]
    fn prompt_with_zero_cap_disables_extraction() {
        let prompt = build_distill_prompt("text", "general", 0);
        assert!(prompt.contains("Extract 0-0"));
    }

    #[test]
    fn parse_distilled_skills_valid_json() {
        let response = r#"[
            {"title": "async-error-handling", "facts": ["Always use ? for propagation", "Wrap spawned tasks in catch_unwind"]},
            {"title": "trait-design", "facts": ["Keep traits small", "Prefer associated types over generics"]}
        ]"#;
        let skills = parse_distilled_skills(response);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].title, "async-error-handling");
        assert_eq!(skills[0].facts.len(), 2);
    }

    #[test]
    fn parse_distilled_skills_invalid_json_returns_empty() {
        let response = "This is not valid JSON at all.";
        let skills = parse_distilled_skills(response);
        assert!(skills.is_empty());
    }

    #[test]
    fn parse_distilled_skills_empty_array() {
        let response = "[]";
        let skills = parse_distilled_skills(response);
        assert!(skills.is_empty());
    }
}
