//! Serialization and utility helpers for knowledge notes.

use sha2::{Digest, Sha256};

/// Compute SHA-256 hex digest of content.
pub fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Return `true` if `s`, emitted unquoted in YAML flow context, would parse
/// as a non-string scalar (null, bool, number, etc.) and break a round-trip
/// to `Vec<String>`.
fn is_yaml_implicit_scalar(s: &str) -> bool {
    matches!(
        s,
        "" | "~"
            | "null"
            | "Null"
            | "NULL"
            | "true"
            | "True"
            | "TRUE"
            | "false"
            | "False"
            | "FALSE"
            | "yes"
            | "Yes"
            | "YES"
            | "no"
            | "No"
            | "NO"
            | "on"
            | "On"
            | "ON"
            | "off"
            | "Off"
            | "OFF"
            | ".inf"
            | ".Inf"
            | ".INF"
            | "-.inf"
            | "-.Inf"
            | "-.INF"
            | ".nan"
            | ".NaN"
            | ".NAN"
    ) || s.parse::<i64>().is_ok()
        || s.parse::<f64>().is_ok()
        || s.starts_with("0x")
        || s.starts_with("-0x")
        || s.starts_with("+0x")
        || s.starts_with("0o")
        || s.starts_with("-0o")
        || s.starts_with("+0o")
}

/// Whether a string needs quoting to survive a YAML round-trip unchanged.
fn needs_yaml_quote(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '\'' | '"'
                | ','
                | ':'
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '%'
                | '@'
                | '`'
        )
    }) || s.starts_with(' ')
        || s.ends_with(' ')
        || s.is_empty()
        || is_yaml_implicit_scalar(s)
}

/// Emit a YAML scalar value, single-quoting when the raw form would not
/// round-trip (reserved chars, implicit scalars, edge whitespace). Used for
/// frontmatter `title:` / `type:` / `category:` — an unquoted `title: [wip] x`
/// makes the whole note permanently unparseable.
pub(crate) fn yaml_scalar(s: &str) -> String {
    if needs_yaml_quote(s) {
        format!("'{}'", s.replace('\'', "''"))
    } else {
        s.to_string()
    }
}

/// Emit a YAML flow-style array, quoting any element that contains a YAML
/// reserved character so the round-trip survives `serde_yml::from_str`.
pub(crate) fn yaml_inline_array(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let parts: Vec<String> = items.iter().map(|s| yaml_scalar(s)).collect();
    format!("[{}]", parts.join(", "))
}

/// Render passthrough frontmatter keys (see
/// [`super::parsing::ExtraFrontmatter`]) as YAML block lines, ready to be
/// appended inside a note's `---` fence.
///
/// Serialization goes through `serde_yml` rather than hand-built strings:
/// the values are arbitrary YAML read back from disk (nested maps, sequences,
/// multi-line scalars), and only the emitter that produced them can be trusted
/// to quote them back correctly.
///
/// Returns `""` for an empty map, so a note without passthrough keys
/// serializes byte-for-byte as it did before this existed.
pub(crate) fn yaml_extra_block(extra: &super::parsing::ExtraFrontmatter) -> String {
    if extra.is_empty() {
        return String::new();
    }
    let mut map = serde_yml::Mapping::new();
    for (k, v) in extra {
        // rust-doctor-disable-next-line excessive-clone
        map.insert(serde_yml::Value::String(k.clone()), v.clone());
    }
    let Ok(rendered) = serde_yml::to_string(&serde_yml::Value::Mapping(map)) else {
        // Unrepresentable value: drop the passthrough block rather than emit a
        // half-serialized header that would make the whole note unparseable.
        // The known fields — the ones the note layer actually reasons about —
        // still round-trip.
        return String::new();
    };
    // Some serde_yml versions frame a document with `---` / `...`; those
    // markers would close the note's own frontmatter fence early.
    let body = rendered
        .strip_prefix("---\n")
        .unwrap_or(&rendered)
        .trim_end_matches('\n')
        .trim_end_matches("\n...")
        .to_string();
    if body.is_empty() {
        return String::new();
    }
    format!("{body}\n")
}

/// Sanitize a `category/filename` note path for safe use as a filesystem path.
/// Each path component is run through [`sanitize_title`] and joined with the
/// platform separator, preventing traversal out of the agent memory directory.
#[must_use]
pub fn sanitize_note_path(note_path: &str) -> String {
    note_path
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(sanitize_title)
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_default()
        .join("/")
}

/// Sanitize a note title for safe use as a filename.
///
/// Strips path separators, null bytes, and filesystem-unsafe characters
/// to prevent path traversal attacks from LLM-generated titles.
///
/// Returns `Err(AlephError::Validation)` if the title contains a `..` (any
/// path-traversal hint, before stripping — the lossy `replace("..", "")` made
/// `..foo` collapse to `foo` and silently collide with an existing `foo.md`)
/// or if the result is empty / all-dots / all-whitespace. Callers should
/// reject the operation rather than write a note with an unstable filename.
pub fn sanitize_title(title: &str) -> Result<String, crate::error::AlephError> {
    // Reject any path-traversal hint up front: the previous `replace("..", "")`
    // was lossy AND collision-prone. A legitimate note title never contains
    // `..`; any occurrence is either an LLM mistake or an attack. Fail closed
    // and let the caller re-prompt.
    if title.contains("..") {
        return Err(crate::error::AlephError::Validation(format!(
            "note title contains '..' (path traversal): {title:?}"
        )));
    }
    let cleaned: String = title
        .replace(['/', '\\', '\0', ':', '*', '?', '"', '<', '>', '|'], "")
        .trim()
        .to_string();
    // Titles are stored extensionless; a trailing ".md" leaking in (e.g. a
    // filename passed as a title) would otherwise produce a doubled "*.md.md"
    // file on disk. Strip it at this single filename chokepoint.
    let cleaned = crate::memory::notes::store::strip_md_ext(&cleaned).to_string();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '.' || c.is_whitespace()) {
        return Err(crate::error::AlephError::Validation(format!(
            "note title sanitizes to empty: {title:?}"
        )));
    }
    Ok(cleaned)
}

/// Strip markdown-significant characters from a wikilink target after
/// `sanitize_title` has already removed path-traversal hazards. Obsidian-style
/// `[[target|alias]]` allows `|` as a separator; we strip both `|` and `]`
/// (and the bracket pair used to delimit the wikilink) so a malicious
/// `link_target` cannot break out of the `[[…]]` wrapping in the rendered
/// body. The stripped chars are dropped silently — the link is then
/// unambiguous to the markdown reader and to the link extractor.
#[must_use]
pub fn sanitize_wikilink_target(s: &str) -> String {
    s.replace(['|', ']', '[', '\n', '\r', '\0'], "")
}
