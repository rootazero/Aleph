//! Structured parsing of the agent's `IDENTITY.md` rich-identity fields.
//!
//! `IDENTITY.md` is written at agent-creation time by
//! `config::agent_resolver::templates::default_identity`, which **seeds**
//! Role / Vibe / Emoji from the chosen [`SoulArchetype`] and leaves a
//! `**Name:**` line plus an empty `**Language:**` slot. Until this module
//! existed those seeded values had no reader anywhere in the tree: the file
//! was injected into the prompt as raw markdown by `IdentityFilesLayer`, and
//! the only structured consumer was a bespoke `**Name:**` scan inside
//! `config::agent_manager::crud`. Everything else was write-only.
//!
//! Two consequences motivated this parser, both mirrored from openclaw's
//! `src/agents/identity-file.ts`:
//!
//! 1. **Decoration.** The template writes invitations like
//!    `- **Role:** systems thinker _(edit to taste)_`. A naive `strip_prefix`
//!    reader hands back `systems thinker _(edit to taste)_` as if the
//!    parenthetical were part of the value.
//! 2. **Placeholders.** A freshly-seeded file carries
//!    `- **Language:** _(preferred language for conversation)_`. That is a
//!    prompt to the human, not a value; treating it as one would claim the
//!    agent prefers a language literally named "preferred language for
//!    conversation".
//!
//! The parser therefore normalizes markdown decoration away and rejects a
//! known placeholder set, returning [`None`] for any field the user has not
//! genuinely filled in. Fields are `Option` precisely so callers can tell
//! "unset" from "set to something" rather than papering over the difference
//! with an empty string.
//!
//! [`SoulArchetype`]: crate::thinker::soul_archetypes::SoulArchetype

use std::path::Path;

use serde::Serialize;

/// Maximum `IDENTITY.md` size this parser will read off disk.
///
/// Matches [`MAX_IDENTITY_FILE_SIZE`] so a file that the write surface accepts
/// is always one the read surface can parse back — a smaller limit here would
/// make `identity.set` able to write files `identity.get` then refuses.
///
/// [`MAX_IDENTITY_FILE_SIZE`]: super::identity_files::MAX_IDENTITY_FILE_SIZE
const MAX_IDENTITY_PARSE_BYTES: u64 = super::identity_files::MAX_IDENTITY_FILE_SIZE as u64;

/// Lowercased placeholder values shipped by the creation templates.
///
/// These are invitations addressed to the human, never real identity values.
/// Compared after [`normalize_value`], so entries are stored in their
/// already-normalized (decoration-stripped, whitespace-collapsed, lowercased)
/// form.
const PLACEHOLDER_VALUES: &[&str] = &[
    "preferred language for conversation",
    "edit to taste",
    "your signature - swap if you like",
    "your signature - pick one that feels right",
    "not set yet",
    "pick something you like",
];

/// Trailing editorial asides the templates append after a real value, e.g.
/// `systems thinker _(edit to taste)_`. Stripped from the tail of a value
/// before the placeholder check, so a seeded-but-real value survives while a
/// bare placeholder does not.
const TRAILING_ASIDES: &[&str] = &[
    "edit to taste",
    "your signature - swap if you like",
    "preferred language for conversation",
];

/// Rich identity fields parsed from an agent's `IDENTITY.md`.
///
/// Every field is `Option` and is `None` unless the user supplied a real
/// value: absent lines, blank values, and template placeholders all read as
/// `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AgentIdentityProfile {
    /// Display name (`**Name:**`).
    pub name: Option<String>,
    /// Self-described role (`**Role:**`), archetype-seeded at creation.
    pub role: Option<String>,
    /// Self-described demeanour (`**Vibe:**`), archetype-seeded at creation.
    pub vibe: Option<String>,
    /// Signature emoji (`**Emoji:**`), archetype-seeded at creation.
    pub emoji: Option<String>,
    /// Preferred conversation language (`**Language:**`), unseeded.
    pub language: Option<String>,
}

impl AgentIdentityProfile {
    /// True when no field carries a user-supplied value.
    ///
    /// Mirrors openclaw's `identityHasValues` (negated). Callers use this to
    /// decide whether a parsed profile is worth surfacing at all — an
    /// all-placeholder file is indistinguishable from a missing one.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.role.is_none()
            && self.vibe.is_none()
            && self.emoji.is_none()
            && self.language.is_none()
    }

    /// Parse rich identity fields from `IDENTITY.md` markdown content.
    ///
    /// Accepts the canonical template shape (`- **Label:** value`) as well as
    /// looser human-authored variants: no leading bullet, no bold markers,
    /// and arbitrary label casing all parse. The first occurrence of a label
    /// wins, so a user's edit placed above legacy prose takes precedence.
    #[must_use]
    pub fn from_markdown(content: &str) -> Self {
        let mut profile = Self::default();

        for line in content.lines() {
            let cleaned = line.trim().trim_start_matches("- ").trim();
            let Some((raw_label, raw_value)) = cleaned.split_once(':') else {
                continue;
            };

            let label = normalize_label(raw_label);
            // Only bind the first sighting of each label.
            let slot = match label.as_str() {
                "name" => &mut profile.name,
                "role" => &mut profile.role,
                "vibe" => &mut profile.vibe,
                "emoji" => &mut profile.emoji,
                "language" => &mut profile.language,
                _ => continue,
            };
            if slot.is_some() {
                continue;
            }

            if let Some(value) = clean_value(raw_value) {
                *slot = Some(value);
            }
        }

        profile
    }

    /// Parse `IDENTITY.md` from an agent directory.
    ///
    /// Returns an empty profile when the file is absent, unreadable, larger
    /// than [`MAX_IDENTITY_PARSE_BYTES`], or carries only placeholders. This
    /// is deliberately infallible: identity is decorative metadata, and no
    /// caller has a better answer to "the file is malformed" than "treat the
    /// agent as unnamed", so an error type would only push `unwrap_or_default`
    /// to every call site.
    ///
    /// The size guard reads metadata first so a pathological file is rejected
    /// without being pulled into memory.
    #[must_use]
    pub fn from_agent_dir(agent_dir: &Path) -> Self {
        let path = agent_dir.join("IDENTITY.md");
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() && meta.len() <= MAX_IDENTITY_PARSE_BYTES => {}
            _ => return Self::default(),
        }
        std::fs::read_to_string(&path).map_or_else(|_| Self::default(), |c| Self::from_markdown(&c))
    }
}

/// Normalize a label to a bare lowercase key: strips bullets, bold/italic/code
/// markers, and surrounding whitespace so `- **Role:**`, `*Role*:`, and
/// `role :` all key to `role`.
fn normalize_label(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c| matches!(c, '*' | '_' | '`' | '-' | ' '))
        .trim()
        .to_lowercase()
}

/// Normalize a value for comparison against [`PLACEHOLDER_VALUES`]: strips
/// markdown decoration, unwraps a fully-parenthesized value, folds en/em
/// dashes to ASCII hyphens, collapses whitespace runs, and lowercases.
///
/// Dash folding matters because the templates use a typographic em dash
/// (`your signature — swap if you like`) that would otherwise never match an
/// ASCII-authored placeholder constant.
fn normalize_value(raw: &str) -> String {
    let mut value = raw
        .trim()
        .trim_matches(|c| matches!(c, '*' | '_' | '`' | ' '))
        .trim()
        .to_string();

    if value.starts_with('(') && value.ends_with(')') && value.len() >= 2 {
        value = value
            .get(1..value.len() - 1)
            .unwrap_or_default()
            .trim()
            .to_string();
    }

    value = value.replace(['\u{2013}', '\u{2014}'], "-");
    value.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Strip decoration from a raw value and return it only if it is a genuine,
/// non-placeholder value.
///
/// Returns the value with original casing preserved (an emoji or a name is
/// not ours to lowercase) while matching placeholders case-insensitively.
fn clean_value(raw: &str) -> Option<String> {
    let mut value = raw
        .trim()
        .trim_matches(|c| matches!(c, '*' | '_' | '`' | ' '))
        .trim()
        .to_string();

    // Drop a trailing editorial aside such as `_(edit to taste)_` so a
    // seeded-but-real value ("systems thinker") survives intact.
    for aside in TRAILING_ASIDES {
        if let Some(cut) = find_trailing_aside(&value, aside) {
            value = value.get(..cut).unwrap_or_default().trim_end().to_string();
            break;
        }
    }

    let value = value
        .trim()
        .trim_matches(|c| matches!(c, '*' | '_' | '`' | ' '))
        .trim();

    if value.is_empty() {
        return None;
    }
    if PLACEHOLDER_VALUES.contains(&normalize_value(value).as_str()) {
        return None;
    }

    Some(value.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Locate the byte index at which a trailing `(aside)` parenthetical begins,
/// if the value ends with one matching `aside`.
///
/// Matching is done on the normalized form so decoration and dash style do
/// not defeat it, but the returned index is into the original string.
fn find_trailing_aside(value: &str, aside: &str) -> Option<usize> {
    let open = value.rfind('(')?;
    let tail = value.get(open..)?;
    let normalized = normalize_value(tail);
    (normalized == aside).then_some(open)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thinker::soul_archetypes::SoulArchetype;

    #[test]
    fn parses_canonical_template_shape() {
        let md = "# IDENTITY.md — Who Am I?\n\n\
                  - **Name:** Aleph\n\
                  - **Role:** systems thinker\n\
                  - **Vibe:** calm and precise\n\
                  - **Emoji:** 🜁\n\
                  - **Language:** English\n";
        let p = AgentIdentityProfile::from_markdown(md);
        assert_eq!(p.name.as_deref(), Some("Aleph"));
        assert_eq!(p.role.as_deref(), Some("systems thinker"));
        assert_eq!(p.vibe.as_deref(), Some("calm and precise"));
        assert_eq!(p.emoji.as_deref(), Some("🜁"));
        assert_eq!(p.language.as_deref(), Some("English"));
        assert!(!p.is_empty());
    }

    #[test]
    fn strips_edit_to_taste_aside_but_keeps_the_seeded_value() {
        // The creation template appends an invitation after a *real* seeded
        // value; the value must survive and the invitation must not.
        let md = "- **Role:** systems thinker _(edit to taste)_\n\
                  - **Emoji:** 🜁 _(your signature — swap if you like)_\n";
        let p = AgentIdentityProfile::from_markdown(md);
        assert_eq!(p.role.as_deref(), Some("systems thinker"));
        assert_eq!(p.emoji.as_deref(), Some("🜁"));
    }

    #[test]
    fn rejects_bare_placeholders() {
        // `**Language:**` ships with only an invitation and no value. Reading
        // it as a value would claim a language literally named "preferred
        // language for conversation".
        let md = "- **Language:** _(preferred language for conversation)_\n";
        let p = AgentIdentityProfile::from_markdown(md);
        assert_eq!(p.language, None);
        assert!(p.is_empty());
    }

    #[test]
    fn em_dash_placeholder_matches_ascii_constant() {
        // Regression: the template uses a typographic em dash. Without dash
        // folding this placeholder would parse as a real emoji value.
        let md = "- **Emoji:** _(your signature — swap if you like)_\n";
        assert_eq!(AgentIdentityProfile::from_markdown(md).emoji, None);
    }

    #[test]
    fn accepts_loose_human_authored_shapes() {
        let md = "Name: Ada\nrole : librarian\n**Vibe**: wry\n";
        let p = AgentIdentityProfile::from_markdown(md);
        assert_eq!(p.name.as_deref(), Some("Ada"));
        assert_eq!(p.role.as_deref(), Some("librarian"));
        assert_eq!(p.vibe.as_deref(), Some("wry"));
    }

    #[test]
    fn first_occurrence_of_a_label_wins() {
        let md = "- **Name:** First\n- **Name:** Second\n";
        assert_eq!(
            AgentIdentityProfile::from_markdown(md).name.as_deref(),
            Some("First")
        );
    }

    #[test]
    fn ignores_unknown_labels_and_colonless_prose() {
        let md = "- **Favourite colour:** blue\nJust some prose about identity.\n";
        assert!(AgentIdentityProfile::from_markdown(md).is_empty());
    }

    #[test]
    fn empty_and_missing_read_as_empty_profile() {
        assert!(AgentIdentityProfile::from_markdown("").is_empty());
        let dir = tempfile::tempdir().unwrap();
        assert!(AgentIdentityProfile::from_agent_dir(dir.path()).is_empty());
    }

    #[test]
    fn from_agent_dir_reads_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("IDENTITY.md"), "- **Name:** Disk\n").unwrap();
        assert_eq!(
            AgentIdentityProfile::from_agent_dir(dir.path()).name.as_deref(),
            Some("Disk")
        );
    }

    #[test]
    fn oversized_file_is_rejected_without_reading() {
        let dir = tempfile::tempdir().unwrap();
        let mut body = String::from("- **Name:** Huge\n");
        body.push_str(&"x".repeat(MAX_IDENTITY_PARSE_BYTES as usize + 1));
        std::fs::write(dir.path().join("IDENTITY.md"), body).unwrap();
        assert!(AgentIdentityProfile::from_agent_dir(dir.path()).is_empty());
    }

    /// The parser must read back exactly what the creation template writes.
    /// This is the seam that was previously broken: `default_identity` seeded
    /// Role/Vibe/Emoji from the archetype and nothing ever read them.
    #[test]
    fn round_trips_every_archetype_seeded_template() {
        for archetype in SoulArchetype::ALL {
            let md = crate::config::agent_resolver::templates::default_identity("Tester", archetype);
            let p = AgentIdentityProfile::from_markdown(&md);
            assert_eq!(p.name.as_deref(), Some("Tester"), "{archetype:?}");
            assert_eq!(
                p.role.as_deref(),
                Some(archetype.role_hint()),
                "role must round-trip for {archetype:?}"
            );
            assert_eq!(
                p.vibe.as_deref(),
                Some(archetype.vibe_hint()),
                "vibe must round-trip for {archetype:?}"
            );
            assert_eq!(
                p.emoji.as_deref(),
                Some(archetype.emoji_hint()),
                "emoji must round-trip for {archetype:?}"
            );
            // Language ships as a bare placeholder and must stay unset.
            assert_eq!(p.language, None, "{archetype:?}");
        }
    }
}
