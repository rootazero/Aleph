use crate::memory::notes::orientation::types::OrientationSnapshot;
use crate::thinker::xml_util::escape_xml;

/// Strip YAML frontmatter (`---\n…\n---\n`) from the start of `raw`.
/// If no frontmatter is present, returns `raw` unchanged.
#[must_use]
pub fn strip_frontmatter(raw: &str) -> &str {
    let rest = raw
        .strip_prefix("---\r\n")
        .or_else(|| raw.strip_prefix("---\n"));
    if let Some(rest) = rest {
        if let Some((_frontmatter, body)) = rest
            .split_once("\r\n---\r\n")
            .or_else(|| rest.split_once("\n---\n"))
            .or_else(|| rest.split_once("\r\n---\n"))
            .or_else(|| rest.split_once("\n---\r\n"))
        {
            return body;
        }
        // Handle closing fence at EOF without trailing newline
        if let Some(stripped) = rest
            .strip_suffix("\r\n---")
            .or_else(|| rest.strip_suffix("\n---"))
        {
            if !stripped.contains("\n---") && !stripped.contains("\r\n---") {
                return "";
            }
        }
    }
    raw
}

#[must_use]
pub fn render_orientation_envelope(s: &OrientationSnapshot) -> String {
    format!(
        "<NoteOrientation>\n<schema>\n{}\n</schema>\n<index_snapshot>\n{}\n</index_snapshot>\n<recent_log>\n{}\n</recent_log>\n</NoteOrientation>",
        escape_xml(&s.schema_text),
        escape_xml(&s.index_text),
        escape_xml(&s.recent_log_tail)
    )
}
