//! Parses LLM-generated session reflection markdown into structured categories.

use crate::skill::SkillExtraction;

/// Structured output from session-end reflection parsing.
#[derive(Debug, Clone, Default)]
pub struct ReflectionOutput {
    pub invariants: Vec<String>,
    pub derived: Vec<String>,
    pub lessons: Vec<LessonItem>,
    pub skills: Vec<String>,
    pub skills_structured: Vec<SkillExtraction>,
    pub open_loops: Vec<String>,
}

/// A single lesson extracted from the reflection.
#[derive(Debug, Clone)]
pub struct LessonItem {
    pub symptom: String,
    pub cause: String,
    pub resolution: String,
}

/// Which section we are currently collecting items for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Invariants,
    Derived,
    Lessons,
    Skills,
    OpenLoops,
    Unknown,
}

/// Parse a markdown reflection into four structured categories.
///
/// Recognises `## ` headers (case-insensitive):
/// - `## Invariants`
/// - `## Derived`
/// - `## Lessons` / `## Lessons & pitfalls`
/// - `## Open Loops` / `## Open Loops / next actions`
///
/// Bullet items (`- `) under each header are collected.
/// Placeholder values like "(none)" are skipped.
///
/// In the Skills section, ` ```yaml ` fences are detected and the enclosed
/// block is parsed as a list of structured skill entries.
pub fn parse_reflection(text: &str) -> ReflectionOutput {
    let mut out = ReflectionOutput::default();
    let mut section = Section::Unknown;
    let mut in_yaml_block = false;
    let mut yaml_lines: Vec<&str> = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // Handle YAML block termination
        if in_yaml_block {
            if trimmed == "```" {
                // End of YAML block — parse accumulated lines
                let yaml_text = yaml_lines.join("\n");
                if let Some(structured) = parse_skill_yaml(&yaml_text) {
                    out.skills_structured.extend(structured);
                }
                in_yaml_block = false;
                yaml_lines.clear();
            } else {
                yaml_lines.push(line);
            }
            continue;
        }

        // Detect section headers
        if let Some(header) = trimmed.strip_prefix("## ") {
            let lower = header.to_lowercase();
            section = if lower == "invariants" {
                Section::Invariants
            } else if lower == "derived" {
                Section::Derived
            } else if lower.starts_with("lessons") {
                Section::Lessons
            } else if lower.starts_with("skills") {
                Section::Skills
            } else if lower.starts_with("open loops") {
                Section::OpenLoops
            } else {
                Section::Unknown
            };
            continue;
        }

        // Detect YAML fence opening inside Skills section
        if section == Section::Skills && (trimmed == "```yaml" || trimmed == "``` yaml") {
            in_yaml_block = true;
            yaml_lines.clear();
            continue;
        }

        // Collect bullet items
        if let Some(item) = trimmed.strip_prefix("- ") {
            let item = item.trim();
            if is_placeholder(item) {
                continue;
            }
            match section {
                Section::Invariants => out.invariants.push(item.to_string()),
                Section::Derived => out.derived.push(item.to_string()),
                Section::Lessons => out.lessons.push(parse_lesson(item)),
                Section::Skills => out.skills.push(item.to_string()),
                Section::OpenLoops => out.open_loops.push(item.to_string()),
                Section::Unknown => {}
            }
        }
    }

    out
}

/// Parse a YAML block containing a list of skill definitions.
///
/// Each entry must have `name`, `category`, `description`, and `content` fields.
/// Returns `None` if the YAML cannot be parsed.
fn parse_skill_yaml(yaml_text: &str) -> Option<Vec<SkillExtraction>> {
    #[derive(serde::Deserialize)]
    struct RawSkill {
        name: String,
        category: String,
        description: String,
        content: String,
    }

    let skills: Vec<RawSkill> = serde_yaml::from_str(yaml_text).ok()?;
    Some(
        skills
            .into_iter()
            .map(|s| SkillExtraction {
                name: s.name,
                category: s.category,
                description: s.description,
                content: s.content.trim().to_string(),
                is_update: false,
            })
            .collect(),
    )
}

/// Returns `true` for placeholder values that should be skipped.
fn is_placeholder(s: &str) -> bool {
    let lower = s.to_lowercase();
    let stripped = lower.trim_matches(|c: char| c == '(' || c == ')');
    matches!(stripped, "none" | "none captured" | "")
}

/// Try to parse a lesson line in "symptom: cause → fix" format.
///
/// Falls back to using the whole line as `symptom` with empty cause/resolution.
fn parse_lesson(line: &str) -> LessonItem {
    // Try splitting on arrow first (→ or ->)
    let arrow_pos = line
        .find('→')
        .map(|p| (p, '→'.len_utf8()))
        .or_else(|| line.find("->").map(|p| (p, 2)));

    if let Some((arrow_byte, arrow_len)) = arrow_pos {
        let before_arrow = &line[..arrow_byte];
        let resolution = line[arrow_byte + arrow_len..].trim().to_string();

        // Try splitting the part before the arrow on ':'
        if let Some(colon) = before_arrow.find(':') {
            let symptom = before_arrow[..colon].trim().to_string();
            let cause = before_arrow[colon + 1..].trim().to_string();
            return LessonItem {
                symptom,
                cause,
                resolution,
            };
        }

        // Arrow but no colon: everything before arrow is symptom
        return LessonItem {
            symptom: before_arrow.trim().to_string(),
            cause: String::new(),
            resolution,
        };
    }

    // Fallback: whole line is symptom
    LessonItem {
        symptom: line.to_string(),
        cause: String::new(),
        resolution: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_reflection() {
        let md = "\
## Invariants
- User prefers Chinese dialogue
- Rust core is the single source of truth

## Derived
- Session focused on memory optimization
- Token budget was tight

## Lessons
- UTF-8 slicing: byte index panics on CJK → use char_indices
- Lock poisoning: unwrap cascades panics → use unwrap_or_else

## Open Loops
- Finish compression daemon tuning
- Benchmark retrieval latency
";
        let out = parse_reflection(md);
        assert_eq!(out.invariants.len(), 2);
        assert_eq!(out.derived.len(), 2);
        assert_eq!(out.lessons.len(), 2);
        assert_eq!(out.open_loops.len(), 2);

        assert_eq!(out.lessons[0].symptom, "UTF-8 slicing");
        assert_eq!(out.lessons[0].cause, "byte index panics on CJK");
        assert_eq!(out.lessons[0].resolution, "use char_indices");
    }

    #[test]
    fn parse_skips_placeholders() {
        let md = "\
## Invariants
- (none)
- Real item
- (none captured)
";
        let out = parse_reflection(md);
        assert_eq!(out.invariants.len(), 1);
        assert_eq!(out.invariants[0], "Real item");
    }

    #[test]
    fn parse_lesson_fallback() {
        let item = parse_lesson("just a plain observation");
        assert_eq!(item.symptom, "just a plain observation");
        assert!(item.cause.is_empty());
        assert!(item.resolution.is_empty());
    }

    #[test]
    fn parse_skills_section() {
        let md = "\
## Invariants
- User prefers Chinese dialogue

## Skills
- Cross-session FTS5 search: build FTS5 index on messages table, group results by session, return context window
- Atomic file writes: use tempfile + rename pattern to prevent corruption

## Open Loops
- Finish compression daemon
";
        let out = parse_reflection(md);
        assert_eq!(out.invariants.len(), 1);
        assert_eq!(out.skills.len(), 2);
        assert!(out.skills[0].contains("FTS5"));
        assert!(out.skills[1].contains("Atomic file writes"));
        assert_eq!(out.open_loops.len(), 1);
    }

    #[test]
    fn parse_empty_returns_default() {
        let out = parse_reflection("");
        assert!(out.invariants.is_empty());
        assert!(out.derived.is_empty());
        assert!(out.lessons.is_empty());
        assert!(out.skills.is_empty());
        assert!(out.open_loops.is_empty());
    }

    #[test]
    fn parse_structured_skill_yaml() {
        let md = r#"## Skills
```yaml
- name: rust-lifetime-debugging
  category: coding
  description: Debug lifetime errors efficiently
  content: |
    # Rust Lifetime Debugging

    ## When to Use
    When the compiler reports lifetime errors.

    ## Steps
    1. Check borrow scope
    2. Use explicit lifetime annotations
```
"#;
        let out = parse_reflection(md);
        assert_eq!(out.skills_structured.len(), 1);
        assert!(out.skills.is_empty());

        let skill = &out.skills_structured[0];
        assert_eq!(skill.name, "rust-lifetime-debugging");
        assert_eq!(skill.category, "coding");
        assert_eq!(skill.description, "Debug lifetime errors efficiently");
        assert!(skill.content.contains("Check borrow scope"));
        assert!(!skill.is_update);
    }

    #[test]
    fn parse_old_format_skill_still_works() {
        let md = "\
## Skills
- Cross-session FTS5 search: build FTS5 index on messages table
- Atomic file writes: use tempfile + rename pattern
";
        let out = parse_reflection(md);
        assert_eq!(out.skills.len(), 2);
        assert!(out.skills_structured.is_empty());
        assert!(out.skills[0].contains("FTS5"));
        assert!(out.skills[1].contains("Atomic file writes"));
    }

    #[test]
    fn parse_mixed_skills_yaml_and_bullets() {
        let md = r#"## Skills
- quick-note: simple bullet skill

```yaml
- name: structured-skill
  category: workflow
  description: A structured skill
  content: |
    ## Steps
    1. Do the thing
```

- another-bullet: second bullet
"#;
        let out = parse_reflection(md);
        assert_eq!(out.skills.len(), 2);
        assert_eq!(out.skills_structured.len(), 1);
        assert!(out.skills[0].contains("quick-note"));
        assert!(out.skills[1].contains("another-bullet"));
        assert_eq!(out.skills_structured[0].name, "structured-skill");
        assert_eq!(out.skills_structured[0].category, "workflow");
    }
}
