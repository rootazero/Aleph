//! NoteExtractor types and extraction prompt builder.
//!
//! Defines the structured types for LLM-driven note extraction and provides
//! a prompt builder that instructs the LLM how to extract knowledge notes
//! from conversation context.

use serde::{Deserialize, Serialize};

/// Action to perform on a knowledge note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteAction {
    Create,
    Append,
    Update,
}

/// A single note update extracted by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteUpdate {
    pub note_title: String,
    pub action: NoteAction,
    #[serde(default)]
    pub new_facts: Vec<String>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

/// Response envelope for note extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteExtractionResponse {
    #[serde(default)]
    pub updates: Vec<NoteUpdate>,
}

/// Build the system prompt for LLM-based note extraction.
///
/// The prompt instructs the LLM to analyze conversation content and extract
/// actionable, memorable facts into structured note updates. The
/// `existing_titles` list lets the LLM decide whether to append to an
/// existing note or create a new one.
pub fn build_note_extraction_prompt(existing_titles: &[String]) -> String {
    let titles_section = if existing_titles.is_empty() {
        "No existing notes yet.".to_string()
    } else {
        let list = existing_titles
            .iter()
            .map(|t| format!("- {t}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("Existing notes:\n{list}")
    };

    format!(
        r#"You are a knowledge extraction engine. Analyze the conversation and extract noteworthy facts into structured note updates.

## Rules

1. Write every fact in THIRD PERSON (e.g. "He prefers Helix editor", not "I prefer Helix editor").
2. Each fact must be a single atomic statement — one idea per bullet.
3. Group related facts into the SAME note.
4. If an existing note already covers the topic, use action "append" with that note's title.
5. If no existing note fits, use action "create" with a clear, short title (2-4 words).
6. Use action "update" only to correct previously stored facts that are now wrong.
7. Add [[wikilinks]] in the `links` array to connect related notes (use exact note titles).
8. Focus on ACTIONABLE or MEMORABLE information — preferences, decisions, plans, learnings, skills.
9. Ignore greetings, small talk, transient information, and pleasantries.
10. Extract 0-10 facts max per batch. Quality over quantity.

## Note titles

Keep titles short: 2-4 words, title case (e.g. "Editor Preferences", "Rust Learning").

## Categories

Use exactly one of: preference, plan, learning, project, personal, tool, lesson, skill, wiki, other

## {titles_section}

## Output format

Respond with valid JSON only. No markdown fences, no commentary.

{{
  "updates": [
    {{
      "note_title": "Example Note",
      "action": "create",
      "new_facts": ["Fact one in third person", "Fact two in third person"],
      "links": ["Related Note"],
      "category": "preference",
      "tags": ["tag1", "tag2"]
    }}
  ]
}}

If there is nothing worth extracting, respond with:
{{"updates": []}}"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extraction_response() {
        let json = r#"{
            "updates": [
                {
                    "note_title": "Editor Preferences",
                    "action": "append",
                    "new_facts": ["The user started using Helix editor"],
                    "links": ["Rust Learning"],
                    "category": "preference",
                    "tags": ["editor"]
                }
            ]
        }"#;
        let response: NoteExtractionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.updates.len(), 1);
        assert_eq!(response.updates[0].note_title, "Editor Preferences");
        assert_eq!(response.updates[0].action, NoteAction::Append);
    }

    #[test]
    fn parses_empty_response() {
        let json = r#"{"updates": []}"#;
        let response: NoteExtractionResponse = serde_json::from_str(json).unwrap();
        assert!(response.updates.is_empty());
    }

    #[test]
    fn parses_create_action() {
        let json = r#"{
            "updates": [
                {
                    "note_title": "Rust Learning",
                    "action": "create",
                    "new_facts": ["He is learning async Rust"],
                    "links": [],
                    "category": "learning",
                    "tags": ["rust", "async"]
                }
            ]
        }"#;
        let response: NoteExtractionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.updates[0].action, NoteAction::Create);
        assert_eq!(response.updates[0].category.as_deref(), Some("learning"));
    }

    #[test]
    fn parses_minimal_update() {
        // Only required fields, all optional fields use defaults
        let json = r#"{
            "updates": [
                {
                    "note_title": "Test",
                    "action": "update"
                }
            ]
        }"#;
        let response: NoteExtractionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.updates[0].action, NoteAction::Update);
        assert!(response.updates[0].new_facts.is_empty());
        assert!(response.updates[0].links.is_empty());
        assert!(response.updates[0].category.is_none());
        assert!(response.updates[0].tags.is_none());
    }

    #[test]
    fn prompt_includes_existing_titles() {
        let titles = vec!["Editor Preferences".to_string(), "Rust Learning".to_string()];
        let prompt = build_note_extraction_prompt(&titles);
        assert!(prompt.contains("- Editor Preferences"));
        assert!(prompt.contains("- Rust Learning"));
    }

    #[test]
    fn prompt_handles_no_existing_titles() {
        let prompt = build_note_extraction_prompt(&[]);
        assert!(prompt.contains("No existing notes yet."));
    }
}
