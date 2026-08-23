//! Argument, action, and result types for the `note_manage` tool.
//!
//! Pure data shapes: what the LLM may ask for and what it gets back. The
//! decision logic that *fills* these shapes lives in the surface modules
//! (`write` / `read` / `lifecycle` / `analysis`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a `query` actually did, as opposed to what it attempted.
///
/// The mode label used to be a claim about configuration: any query with an
/// embedder wired reported `"hybrid"`, including one whose vector leg returned
/// nothing because the index was empty or dimension-mismatched. The model could
/// not tell "semantic search found nothing relevant" from "semantic search did
/// not run", which are opposite instructions about whether to trust the result.
/// Same discipline as `note_graph_query`'s `QueryAdvisory`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SearchAdvisory {
    /// `hybrid` (both legs contributed), `semantic` (vector only),
    /// `full-text` (keyword only).
    pub mode: String,
    /// Candidates the vector leg contributed. Zero under `mode: "hybrid"`
    /// means the vector index held nothing for this agent.
    pub vector_candidates: usize,
    /// Candidates the full-text leg contributed.
    pub fts_candidates: usize,
    /// Present only when the semantic leg was skipped, saying why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
    /// Result bodies dropped to stay inside the response content budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bodies_omitted: Option<usize>,
}

/// Actions supported by the `note_manage` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum NoteManageAction {
    /// Create a new note (fails if filename already exists).
    Create,
    /// Replace the body content of an existing note (markdown preserved verbatim).
    Update,
    /// Append bullet-point facts (and optional links) to an existing or new note.
    Append,
    /// Hybrid (semantic + full-text) search across all indexed notes; falls
    /// back to full-text only when no embedder is configured.
    Query,
    /// List all notes, optionally filtered by category.
    List,
    /// Read one note **by address** — its full markdown body, untruncated by
    /// ranking. `query` is a survey (ranked, and every hit's body is cut at
    /// 4,000 chars); `get` is an address. Anything that rewrites a note's body
    /// must read it through here first, because `update` replaces the body
    /// wholesale and a body reconstructed from a truncated search hit silently
    /// loses everything past the cut.
    Get,
    /// Delete a note file and remove it from the index.
    Delete,
    /// Rename a note (change its filename/title) and rewrite every inbound
    /// `[[wikilink]]` that referenced the old name. Uses `filename` (current
    /// name) + `new_title` (target name).
    Rename,
    /// Read materialized graph-health insights (knowledge gaps, bridges,
    /// surprising connections). Read-only.
    Insights,
    /// Read the memory-evolution gate state: recent dream cycles' health
    /// score (before/after), best-ever score, accepted/rejected verdict,
    /// merges the gate rejected, and any churn-pathology cooldown. Lets the
    /// model explain *why* memory changed (or didn't) last night. Read-only.
    Evolution,
}

/// Arguments for the `note_manage` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoteManageArgs {
    /// Action to perform: create, update, append, query, list, delete, rename.
    pub action: NoteManageAction,

    /// Note category: preference, plan, learning, project, personal, tool,
    /// lesson, goal-lessons, skill, reference, feedback, transcript, query,
    /// contradiction, other, or the subagent-* family.
    /// Required for create/update/append/delete; optional filter for list;
    /// optional for get (omit it and the filename is resolved through the
    /// index — a name held by two categories is refused, not guessed).
    #[serde(default)]
    pub category: Option<String>,

    /// Note filename in kebab-case or title-case, without the `.md` suffix.
    /// Required for create, update, append (as target), delete, get.
    #[serde(default)]
    pub filename: Option<String>,

    /// Note title displayed in the frontmatter (required for create).
    #[serde(default)]
    pub title: Option<String>,

    /// Markdown body content — the full body text for create/update.
    #[serde(default)]
    pub content: Option<String>,

    /// Bullet-point facts to append (for `append` action).
    #[serde(default)]
    pub facts: Option<Vec<String>>,

    /// Wikilinks to related notes (e.g. ["Rust Learning", "Dev Environment"]).
    #[serde(default)]
    pub links: Option<Vec<String>>,

    /// Tags to attach to the note (used on create).
    #[serde(default)]
    pub tags: Option<Vec<String>>,

    /// Search query text (required for `query` action).
    #[serde(default)]
    pub query: Option<String>,

    /// Maximum number of results for query/list (default: 20).
    #[serde(default)]
    pub limit: Option<usize>,

    /// Target name for the `rename` action. `filename` carries the note's
    /// current name; `new_title` carries the name to rename it to. The
    /// note's category is located automatically — no need to pass `category`.
    #[serde(default)]
    pub new_title: Option<String>,

    /// Typed semantic relations to declare on this note (create/update/append).
    /// Each entry is `{to, type}`: `to` is a note path or wikilink-style
    /// target, `type` is a free-form relationship verb (e.g. "refers",
    /// "derives"). `supersedes` / `superseded_by` / `contradicts` are
    /// structural-strong edges force-surfaced at retrieval regardless of score.
    #[serde(default)]
    pub relations: Option<Vec<NoteRelationArg>>,

    /// Agent ID to scope the note operation to. If absent, the note is scoped
    /// to the *active chat session's* agent (read from the turn context) so it
    /// lands in that agent's own vault, falling back to the system default
    /// agent (`"main"`) outside a gateway turn (cron / internal). Pass this
    /// explicitly only to target a *different* agent's per-agent vault than the
    /// one driving the current turn.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// A single typed semantic relation declared by the LLM at write time (via
/// `NoteManageArgs::relations`). Mirrors [`crate::memory::notes::Relation`]
/// minus `confidence` — tool-authored relations are an explicit statement,
/// so confidence is fixed at 1.0 by the merge helpers rather than accepted
/// as caller input.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NoteRelationArg {
    /// Target note path ("category/filename") or raw wikilink text.
    pub to: String,
    /// Free-form relationship verb (no fixed taxonomy — R7 LLM sovereignty).
    #[serde(rename = "type")]
    pub rel_type: String,
}

/// A lightweight note entry returned by list/query.
#[derive(Debug, Clone, Serialize)]
pub struct NoteListEntry {
    /// Relative path within the agent directory: "{category}/{filename}".
    pub path: String,
    pub category: String,
    pub filename: String,
    pub tags: Vec<String>,
}

/// Result of a `note_manage` operation.
#[derive(Debug, Clone, Serialize)]
pub struct NoteManageResult {
    pub success: bool,
    pub message: String,
    /// VFS path of the note affected (create/update/append/delete).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_path: Option<String>,
    /// D4 receipt: resolved on-disk note path + tier label, so the model can
    /// tell the user exactly where the note lives. Sibling of
    /// `RememberOutput.destination` / `FlagUserCorrectionOutput.destination`.
    /// `None` — and absent from the serialized shape — for every action that
    /// did not land content in a note: the read actions, and `delete` (whose
    /// note no longer lives anywhere). A receipt is proof that a write landed;
    /// stamping one on anything else is how a model ends up telling the user
    /// their note is filed away when nothing was filed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// File content (query action returns matching note bodies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Notes returned by list/query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<Vec<NoteListEntry>>,
    /// Related existing notes surfaced after a create, so the model can
    /// weave the new note into the wiki (via `links`) instead of leaving an
    /// orphan island.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_notes: Option<Vec<NoteListEntry>>,
    /// What the `query` action actually ran — which retrieval legs took part,
    /// how much each contributed, and why the semantic leg was skipped when it
    /// was. Absent for every other action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchAdvisory>,
}
