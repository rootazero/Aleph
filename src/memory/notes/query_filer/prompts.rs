//! LLM gate prompt for deciding whether to file a query answer.

pub const PROMPT_QUERY_FILE_CHECK: &str = r#"You decide whether a synthesised answer from memory search is worth archiving as a permanent note.

A query answer should be FILED if it is a NOVEL SYNTHESIS — it connects multiple sources in a way that would be painful to re-derive. It should NOT be filed if it merely restates facts already present in existing notes.

Input: the original query and the synthesised answer with its source notes.

Output valid JSON only:
{
  "file": true or false,
  "reason": "one sentence explaining why or why not",
  "proposed_title": "short kebab-case title for the note (only if file=true)",
  "tags": ["tag1", "tag2"],
  "links": ["category/filename"]
}

If file is false, proposed_title/tags/links can be empty or omitted.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_snapshot() {
        insta::assert_snapshot!("query_file_check_prompt", PROMPT_QUERY_FILE_CHECK);
    }

    #[test]
    fn prompt_mentions_json_output() {
        assert!(PROMPT_QUERY_FILE_CHECK.contains("JSON"));
        assert!(PROMPT_QUERY_FILE_CHECK.contains("file"));
        assert!(PROMPT_QUERY_FILE_CHECK.contains("proposed_title"));
    }
}
