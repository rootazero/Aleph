//! Parses LLM-generated session reflection markdown into structured categories.

/// Structured output from session-end reflection parsing.
#[derive(Debug, Clone, Default)]
pub struct ReflectionOutput {
    pub invariants: Vec<String>,
    pub derived: Vec<String>,
    pub lessons: Vec<LessonItem>,
    pub skills: Vec<String>,
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
pub fn parse_reflection(text: &str) -> ReflectionOutput {
    let mut out = ReflectionOutput::default();
    let mut section = Section::Unknown;

    for line in text.lines() {
        let trimmed = line.trim();

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
}
