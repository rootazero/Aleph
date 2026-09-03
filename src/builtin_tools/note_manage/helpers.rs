//! Shared plumbing for `note_manage`: identity/category resolution, the D4
//! destination receipt, and the free helpers every surface module reuses.
//!
//! Nothing here decides *what* an action does — it answers the questions every
//! action asks first (whose vault? which category? is this content safe to
//! persist?).

use crate::error::{AlephError, Result};
use crate::memory::notes::{canonicalize_category, KnowledgeNote, CATEGORY_DIRS};

use super::args::{NoteManageArgs, NoteRelationArg};
use super::NoteManageTool;

impl NoteManageTool {
    /// Default agent ID (used when `args.agent_id` is absent). Must match the
    /// system-wide `DEFAULT_AGENT_ID` ("main") that every note reader falls back
    /// to — panel graph, memory recall, dreaming, orientation. A stray literal
    /// here (the old `"default"`) silently misfiled chat-created notes into a
    /// namespace nothing reads, making them invisible everywhere.
    pub(super) const fn agent_id(&self) -> &str {
        crate::routing::DEFAULT_AGENT_ID
    }

    /// Test-only accessor for the underlying memory directory, so tests can
    /// assert against on-disk note paths without duplicating the tool's
    /// construction args (`indexer` is a private field).
    #[cfg(test)]
    pub(super) fn memory_dir(&self) -> &std::path::Path {
        self.indexer.memory_dir()
    }

    /// Resolve the effective `agent_id` (storage partition key) for this
    /// invocation. Priority: explicit `args.agent_id` → the active chat
    /// session's agent (turn context) → `DEFAULT_AGENT_ID` for non-gateway
    /// paths (cron / internal / tests).
    ///
    /// The base id is composed via `project_scope::session_write_id`: a
    /// personal-scoped session (`crate::scope::current_scope`) always wins,
    /// isolating notes to the user's own personal namespace; otherwise, when
    /// `project_scoped` is enabled and a project root is active for the run,
    /// the base id is composed with the project namespace so notes are
    /// isolated per project directory (the existing `note/{agent_id}/…` layout
    /// + `(agent_id, …)` table keys do the partitioning, no schema change).
    ///   With neither active, the base id is returned unchanged. This is the
    ///   only path callers should use when they need an agent-scoped operation.
    pub(super) fn resolve_agent_id(&self, args: &NoteManageArgs) -> Result<String> {
        if let Some(id) = args.agent_id.as_deref() {
            // `agent_id` is untrusted LLM input that is joined directly into a
            // filesystem path (memory_dir/<agent_id>/<category>/<file>.md).
            // Reject traversal and separators so it cannot escape the vault.
            if id.is_empty()
                || id.contains("..")
                || id.contains('/')
                || id.contains('\\')
                || id.starts_with('.')
                || id.contains('\0')
            {
                return Err(AlephError::tool(format!(
                    "invalid agent_id `{id}`: must not be empty, start with '.', \
                     contain NUL, or contain '..', '/', or '\\'"
                )));
            }

            // A composed id (`{base}__u-*` / `__p-*` / `__proj-*`) is the
            // OUTPUT of scope composition
            // (`project_scope::session_write_id`), never a value the model
            // was handed to type back in — `agent_id: "main__u-alice"`
            // would otherwise address u-alice's vault byte-for-byte.
            // Refuse it FIRST and unconditionally: this is the load-bearing
            // gate, because `partition_visible_to(_, None)` is
            // unconditionally `true` (see its doc), so it only catches a
            // composed id here by the accident that `ambient_actor()`'s
            // fallback arms happen to disagree with the named suffix. A
            // genuinely actor-less run (cron / heartbeat) has no ambient
            // actor at all and would sail straight through without this
            // arm — see `refuses_a_composed_agent_id_even_with_no_ambient_actor`.
            if crate::memory::project_scope::is_composed_id(id) {
                return Err(invalid_agent_id_partition_error(id));
            }

            // Defence in depth: even an id `is_composed_id` does not
            // recognize as a scoped-suffix family (e.g. an arbitrary
            // `__`-separated string) must still be visible to the actor
            // driving this turn. `ambient_actor` — not
            // `scope::ambient_owner` — is the resolver every tool-face
            // predicate in `gateway::visibility` uses, because inside a
            // project room it prefers the turn's SPEAKER over the room's
            // creator. `None` (no ambient actor: cron / internal / a bare
            // turn with no scope stamped) is unconditionally unrestricted,
            // matching every sibling predicate's convention.
            let actor = crate::gateway::visibility::ambient_actor();
            if !crate::gateway::visibility::partition_visible_to(id, actor.as_deref()) {
                return Err(invalid_agent_id_partition_error(id));
            }
        }
        // Resolution priority:
        //   1. explicit `args.agent_id` (validated above) — an intentional LLM
        //      override to target another agent's vault.
        //   2. the *active session's* agent — read from the per-tool-call turn
        //      context, which the dispatch chokepoint (`ScopedToolService::
        //      execute`) scopes around every tool execution, so a concurrent run
        //      of another agent cannot race it. Without this a note saved while
        //      chatting with a non-default agent lands in "main" and is invisible
        //      in that agent's own graph.
        //   3. `DEFAULT_AGENT_ID` — terminal fallback for non-gateway paths
        //      (cron / internal / tests) where no turn is scoped.
        //
        // The turn-context id comes from a parsed `SessionKey` whose agent_id is
        // always normalized (`[a-z0-9_-]`, ≤64 chars, no separators), so it is
        // path-safe by construction and needs no re-validation — the same trust
        // `memory_search` places in `current_agent_id()`.
        let session_agent = crate::tools::turn_context::current_agent_id();
        let base = args
            .agent_id
            .as_deref()
            .or(session_agent.as_deref())
            .unwrap_or_else(|| self.agent_id());
        Ok(crate::memory::project_scope::session_write_id(
            base,
            self.project_scoped,
            crate::projects::current_project_root().as_deref(),
        ))
    }

    /// Resolve the `category` argument for any action: canonicalize the
    /// spelling, then validate.
    ///
    /// One boundary for every handler. `create` used to be the only action that
    /// canonicalized, so `category: "projects"` created a note under
    /// `project/` and then failed to update, append to, or delete it — the same
    /// model, the same session, contradictory answers about the same category.
    pub(super) fn resolve_category(args: &NoteManageArgs, action: &str) -> Result<String> {
        let raw = args
            .category
            .as_deref()
            .ok_or_else(|| AlephError::tool(format!("category is required for {action}")))?;
        let canonical = canonicalize_category(raw);
        validate_category(&canonical)?;
        Ok(canonical)
    }

    /// D4 receipt data plane: where a note write landed, as a human-readable
    /// string — resolved on-disk file (home abbreviated to `~`) plus the tier
    /// label. Modelled on `CuratedMemoryStore::destination()`: the acknowledgment
    /// the model owes the user must be able to name the destination for whichever
    /// tier it wrote to, and reading it off the two writers' identically shaped
    /// receipts is what keeps the two acknowledgments comparable.
    ///
    /// `note_path` is the `{category}/{filename}` VFS path the write returned.
    pub(super) fn destination(&self, agent_id: &str, note_path: &str) -> String {
        let file = self
            .indexer
            .memory_dir()
            .join(agent_id)
            .join(format!("{note_path}.md"));
        let shown = crate::utils::paths::get_home_dir()
            .ok()
            .and_then(|home| {
                file.strip_prefix(&home)
                    .ok()
                    .map(|rel| format!("~/{}", rel.display()))
            })
            .unwrap_or_else(|| file.display().to_string());
        format!(
            "{shown} (durable notes — searchable, recalled on relevance; \
             not always in your prompt)"
        )
    }
}

/// Shared refusal text for both the `is_composed_id` gate and the
/// `partition_visible_to` gate in [`NoteManageTool::resolve_agent_id`], so a
/// caller cannot distinguish "refused outright as a composed id" from
/// "refused as invisible to this actor" from "the named vault does not
/// exist" — none of that is information a refusal should leak (P7: fail
/// closed, no existence oracle).
pub(super) fn invalid_agent_id_partition_error(id: &str) -> AlephError {
    AlephError::tool(format!(
        "invalid agent_id `{id}`: not a partition this caller may address"
    ))
}

/// Truncate to at most `max` characters on a char boundary (P7 UTF-8 safety),
/// with an honest marker carrying the omitted-char count.
pub(super) fn bound_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => {
            let omitted = s.chars().count() - max;
            format!("{}…(+{omitted} chars truncated)", &s[..byte_idx])
        }
        None => s.to_string(),
    }
}

/// Extract up to 4 significant keywords (length >= 4, lowercased, deduped,
/// input order preserved) from a note's title+content for the per-keyword
/// related-note FTS search after `create`.
pub(super) fn related_keywords(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if word.chars().count() < 4 {
            continue;
        }
        let lower = word.to_lowercase();
        if !out.contains(&lower) {
            out.push(lower);
            if out.len() >= 4 {
                break;
            }
        }
    }
    out
}

/// Merge tool-authored typed relations into the note's frontmatter set,
/// deduped by (to, rel_type). Tool-authored = explicit statement → confidence 1.0.
pub(super) fn merge_relations(note: &mut KnowledgeNote, rels: &[NoteRelationArg]) {
    for r in rels {
        let exists = note
            .relations
            .iter()
            .any(|x| x.to == r.to && x.rel_type == r.rel_type);
        if !exists {
            note.relations.push(
                crate::memory::notes::Relation {
                    to: r.to.clone(),
                    rel_type: r.rel_type.clone(),
                    confidence: 1.0,
                }
                .clamped(),
            );
        }
    }
}

/// Validate that the category is one of the known valid values.
///
/// Single source of truth: `CATEGORY_DIRS` (indexer.rs) — the exact set of
/// directories the indexer scans. A previous hand-copied list here drifted
/// (missing `feedback` / `goal-lessons` / `query`), locking the LLM out of
/// managing notes in those categories.
pub(super) fn validate_category(category: &str) -> Result<()> {
    if CATEGORY_DIRS.contains(&category) {
        Ok(())
    } else {
        Err(AlephError::tool(format!(
            "Unknown category '{category}'. Valid categories: {}",
            CATEGORY_DIRS.join(", ")
        )))
    }
}

/// Reject note content that carries a prompt-injection / exfiltration /
/// persistence payload before it is written to long-term memory.
///
/// A note is a *user-mediated write* in the `injection_patterns` scope model:
/// the model is persisting text it chose into a vault that is later recalled
/// into context as **trusted** memory — losing the `<<<EXTERNAL_UNTRUSTED…>>>`
/// fence the content carried while it was being read. Scanning here at
/// [`ThreatScope::Strict`] closes the *memory-poisoning* laundering vector
/// (untrusted web/MCP content → distilled into a note → replayed as a trusted
/// instruction). Strict is the right breadth because a false positive on this
/// path is interactively resolvable: the tool error is returned to the model,
/// which can rephrase or drop the offending literal (R9 — the loop's LLM, not a
/// deterministic recovery branch, decides what to do).
///
/// This is the production consumer the `first_threat_message` helper was
/// designed for; without it the entire Strict scope (and its persistence
/// patterns) was unreachable in production.
pub(crate) fn scan_note_for_threats(text: &str) -> Result<()> {
    scan_note_at_scope(
        text,
        crate::security::injection_patterns::ThreatScope::Strict,
    )
}

/// Exfiltration-only note scan (`ThreatScope::All`): flags classic
/// data-exfiltration payloads but NOT the SSH-backdoor / persistence / C2 /
/// hardcoded-credential patterns that would false-positive on legitimate
/// security-research prose. Used on the untrusted-content write paths (query
/// filer synthesis, panel node edits) where a Strict scan would silently drop
/// or reject a user's own security notes.
pub(crate) fn scan_note_for_exfiltration(text: &str) -> Result<()> {
    scan_note_at_scope(text, crate::security::injection_patterns::ThreatScope::All)
}

fn scan_note_at_scope(
    text: &str,
    scope: crate::security::injection_patterns::ThreatScope,
) -> Result<()> {
    // Canonicalize (fold homoglyphs + strip invisibles) before scanning: this is
    // a raw-text write path, so the scan must not be evadable by a zero-width- or
    // homoglyph-obfuscated payload that the model reconstructs on recall. The
    // stored note keeps its original bytes (body fidelity); only the scanned copy
    // is canonicalized.
    match crate::security::injection_patterns::first_threat_message_canonicalized(text, scope) {
        Some(reason) => Err(AlephError::tool(reason)),
        None => Ok(()),
    }
}
