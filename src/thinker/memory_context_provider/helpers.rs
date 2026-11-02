use crate::memory::notes::orientation::types::OrientationSnapshot;
use crate::thinker::xml_util::escape_xml;

/// Strip YAML frontmatter (`---\n…\n---\n`) from the start of `raw`.
/// If no frontmatter is present, returns `raw` unchanged.
#[must_use]
pub fn strip_frontmatter(raw: &str) -> &str {
    if let Some(rest) = raw.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---\n") {
            return &rest[end + 5..];
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
