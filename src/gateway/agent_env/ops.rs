//! Workspace verbs — the one derivation both faces of `workspace.*` run.
//!
//! The family has two callers with two different shapes of answer:
//!
//! - `src/gateway/handlers/workspace.rs` — the JSON-RPC face reached by the
//!   Panel's `/settings/workspaces` page and by `aleph workspace …` over IPC.
//! - `src/builtin_tools/workspace_manage.rs` — the `workspace_manage` tool, so
//!   an operator can manage workspaces by talking (R8).
//!
//! Everything that is a *judgement* lives here: the partition gate, which
//! `None` from the store means "archived, read-only" versus "no such row", and
//! whether a create collision has a way back. The two callers do nothing but
//! parse their own argument shape and render this module's verdict in their own
//! envelope.
//!
//! # Why the verdict could not stay in the handlers
//!
//! Two reasons, and the second is the one that bites silently.
//!
//! 1. `src/gateway/CLAUDE.md`'s "一个动词有 N 个面时，'谁能看'要在每个面用同一个
//!    推导": a second copy of "an archived row is readable but not writable"
//!    drifts, and the drift is invisible — both faces keep answering, just
//!    differently.
//! 2. **The actor resolver is not the same one the handlers were using.**
//!    [`crate::gateway::visibility::partition_visible`] reads `CALLER_USER`,
//!    which is correct in gateway dispatch and **dead inside a spawned run** —
//!    and every tool call happens inside one. A tool that reached for the
//!    obvious predicate would get a silent always-true. This module uses
//!    [`crate::gateway::visibility::ambient_partition_visible`], whose resolver
//!    (`ambient_actor`) reads `CALLER_USER` **first** and is therefore
//!    byte-identical on the RPC face and correct on the tool face. One
//!    predicate, both surfaces, no per-caller argument to get wrong.
//!
//! # The events are not emitted here
//!
//! `WorkspaceChanged` is published by [`AgentEnvStore`] itself, inside the
//! mutating verbs (`AgentEnvStore::with_event_bus`). That is what makes the
//! tool face announce itself to open Panels without this module — or the tool —
//! knowing an event bus exists. The one thing a caller must not do is build its
//! own store: a hand-rolled `AgentEnvStore::with_defaults()` has no bus, and the
//! only symptom is a Panel that never refreshes.

use aleph_protocol::workspace::{
    WorkspaceCreateParams, WorkspaceDetail, WorkspaceRow, WorkspaceUpdateParams,
};

use super::{AgentEnv, AgentEnvError, AgentEnvStore};
use crate::gateway::visibility::ambient_partition_visible;

// ============================================================================
// Projection
// ============================================================================

/// Project the stored [`AgentEnv`] onto the detail shape this family promises.
///
/// Every read here used to serialize the whole `AgentEnv`, which put four
/// fields on the wire that Aleph never reads back: `env_vars`, `allowed_tools`,
/// `system_prompt_override` and `default_model`.
/// [`crate::gateway::agent_env::ActiveAgentEnv`] — the struct that actually
/// flows through the execution pipeline — carries `agent_id`, `profile`,
/// `memory_filter` and `agent_env_path`, and drops all four at the resolution
/// boundary. There was no writer for them either — none in any version, not
/// just this one — so what shipped was a permanently-empty configuration
/// surface that looked settable: the failure mode is a caller who sets
/// `default_model` on a workspace and gets silence. Per R10 a channel with zero
/// consumers is CUT, not connected. The four were dropped from `AgentEnv` and
/// from the `agent_envs` table on the same day this projection landed, so today
/// they are not reachable from here even by accident; this function's job is
/// now the general one, of being the single place that decides what a caller
/// sees.
///
/// Projecting also makes the contract enforceable in the direction that
/// matters. `aleph-cli` cannot depend on `alephcore`, so the only guard for
/// this wire is the shared type plus assertions on the server side; parsing
/// proves the response is a *superset* of the contract, never that it is equal
/// to it. Building the response FROM the contract type closes that gap by
/// construction — a new `AgentEnv` field cannot leak onto the wire by being
/// added, only by being added here on purpose.
#[must_use]
pub fn detail_of(ws: &AgentEnv) -> WorkspaceDetail {
    WorkspaceDetail {
        id: ws.id.clone(),
        name: ws.name.clone(),
        description: ws.description.clone(),
        icon: ws.icon.clone(),
        profile: ws.profile.clone(),
        created_at: ws.created_at,
        last_active_at: ws.last_active_at,
        is_archived: ws.is_archived,
    }
}

/// The list twin of [`detail_of`], onto the narrower row shape.
///
/// Deliberately not `detail_of(..)` minus fields: the two projections answer
/// different questions and [`WorkspaceRow`] documents why it is a second one.
#[must_use]
pub fn row_of(ws: &AgentEnv) -> WorkspaceRow {
    WorkspaceRow {
        id: ws.id.clone(),
        name: ws.name.clone(),
        description: ws.description.clone(),
        created_at: ws.created_at,
        is_archived: ws.is_archived,
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Why a workspace verb refused.
///
/// One variant per *honest answer*, not one per call site — the split between
/// [`Self::NotFound`] and [`Self::Archived`] is the whole rule this family
/// enforces, and putting it in the type is what keeps both faces drawing it in
/// the same place (`src/gateway/CLAUDE.md`, P2 mine E: **invisible →
/// not-found**, since existence is itself the secret; **visible but not
/// writable → forbidden**, since the caller can already read the row and a
/// "not found" would simply be false to their face).
///
/// Not `Clone`/`PartialEq`: [`AgentEnvError`] is neither, and deriving them
/// would mean either wrapping the store error in a `String` (losing the variant
/// the caller may want to match on) or making the store's error type derive
/// traits it has no use for. Compare the rendered [`Self::text`] instead — that
/// is what a caller actually sees.
#[derive(Debug)]
pub enum WorkspaceOpError {
    /// No such row, **or** a partition this caller may not see. One variant on
    /// purpose: the two must be indistinguishable, or the refusal becomes an
    /// existence oracle.
    NotFound(String),
    /// The row exists and this caller can read it, but archived rows are
    /// read-only.
    Archived(String),
    /// `create` only: the id is held. `archived` says the holder is archived,
    /// i.e. there is a way back — see [`Self::text`] for why the way back is
    /// spelled by the caller and not here.
    IdTaken { id: String, archived: bool },
    /// The store failed. `context` is the "Failed to …" prefix of the verb that
    /// was running, kept as a `&'static str` so the wording cannot vary per
    /// call.
    Store {
        context: &'static str,
        source: AgentEnvError,
    },
}

impl WorkspaceOpError {
    /// The full text for a caller's own envelope.
    ///
    /// `restore_verb` is how **this surface** spells the way back from an
    /// archived collision — `` `workspace.unarchive` `` on the wire,
    /// `action="unarchive"` for the model. It is the only part of the sentence
    /// that differs between the two faces, so it is the only part passed in;
    /// a second copy of the sentence is a second thing to reword. Every other
    /// variant ignores it.
    ///
    /// Note what stays constant: with `archived == false` the text is
    /// [`Self::id_taken`] and nothing else, whatever `restore_verb` says. That
    /// is what keeps a partition denial (which never reads the store) and a
    /// genuine collision byte-identical instead of leaving it to discipline.
    #[must_use]
    pub fn text(&self, restore_verb: &str) -> String {
        match self {
            Self::NotFound(id) => format!("Workspace '{id}' not found"),
            Self::Archived(id) => format!("Workspace '{id}' is archived and cannot be modified"),
            Self::IdTaken {
                id,
                archived: false,
            } => Self::id_taken(id),
            Self::IdTaken { id, archived: true } => format!(
                "{} — it is archived, not gone. Restore it with {restore_verb}; an archived \
                 workspace keeps its id, and its memory and notes are still on disk under that id.",
                Self::id_taken(id)
            ),
            Self::Store { context, source } => format!("{context}: {source}"),
        }
    }

    /// The one wording `create` has for "that id is not available".
    ///
    /// Built from the real [`AgentEnvError::AlreadyExists`] value rather than a
    /// copy of the store's wording, for the same reason the partition denial
    /// reuses this shape at all: the two must agree byte for byte, and two
    /// literals that merely look alike are one reword away from becoming an
    /// existence oracle.
    #[must_use]
    pub fn id_taken(id: &str) -> String {
        format!(
            "Failed to create workspace: {}",
            AgentEnvError::AlreadyExists(id.to_string())
        )
    }
}

type OpResult<T> = Result<T, WorkspaceOpError>;

fn store_err(context: &'static str) -> impl Fn(AgentEnvError) -> WorkspaceOpError {
    move |source| WorkspaceOpError::Store { context, source }
}

// ============================================================================
// Verbs
// ============================================================================

/// List workspaces visible to the current actor.
///
/// # P1 partition isolation — and what it does NOT cover
///
/// Each row is filtered by [`ambient_partition_visible`] on its id, the same
/// predicate the `memory.*`/`graph.*` family uses, so an id composed with the
/// partition grammar (`<base>__u-alice`) is invisible to everyone else.
///
/// That is defense in depth, NOT a closed boundary on its own, and the
/// distinction is recorded here rather than implied by the presence of a check:
/// a workspace id is a user-chosen name (`"project-aleph"`), it encodes no
/// owner, and the `agent_envs` table has no owner column — so an ordinary
/// workspace passes this predicate for every caller who gets this far.
///
/// # The residual was write, and it is closed at the caller's gate
///
/// The 2026-08-08 real-machine QA exercised the write half: a member renamed
/// and then archived a workspace the operator had just created, both returning
/// `ok`. The fix was to gate the whole family rather than add an owner column —
/// admin-gated on the RPC face
/// ([`crate::gateway::method_admin`]'s `ADMIN_PREFIXES`), operator-gated on the
/// tool face ([`crate::gateway::method_authz`]'s `OPERATOR_TOOLS`). Both refuse
/// a `"member"` role, so the surviving case for this predicate is one operator
/// addressing another user's partition — not vacuous, which is why it stays.
///
/// (That QA also produced a *false* pass on the list side: this verb looked
/// filtered only because the member had archived the row one call earlier and
/// `include_archived = false` skips archived.)
pub async fn list(store: &AgentEnvStore, include_archived: bool) -> OpResult<Vec<WorkspaceRow>> {
    let all = store
        .list(include_archived)
        .await
        .map_err(store_err("Failed to list workspaces"))?;
    Ok(all
        .iter()
        .filter(|w| ambient_partition_visible(&w.id))
        .map(row_of)
        .collect())
}

/// Read one workspace by id.
///
/// # Archived workspaces are visible here
///
/// Reads through [`AgentEnvStore::get_including_archived`] rather than `get`.
/// The default read filters `archived = 0` because its callers resolve the env
/// a run executes under; this one is addressed by exact id, is read-only, and
/// reports `is_archived` in the answer.
///
/// Filtering here would reintroduce, one level down, the complaint that
/// `include_archived` was added to [`list`] to fix: the list would print a row
/// this verb then swears does not exist. "Readable, not writable" is the whole
/// rule — [`update`] refuses the same rows, and `AgentEnvStore::update`
/// enforces it below them both.
pub async fn get(store: &AgentEnvStore, id: &str) -> OpResult<WorkspaceDetail> {
    if !ambient_partition_visible(id) {
        return Err(WorkspaceOpError::NotFound(id.to_string()));
    }
    match store
        .get_including_archived(id)
        .await
        .map_err(store_err("Failed to get workspace"))?
    {
        Some(ws) => Ok(detail_of(&ws)),
        None => Err(WorkspaceOpError::NotFound(id.to_string())),
    }
}

/// Create a workspace.
///
/// P1 partition isolation on the WRITE side, with exactly the coverage boundary
/// [`list`] documents and no more. What the check buys is the composed-id half:
/// without it a caller can plant `main__u-alice`, a row that then shows up in
/// ALICE's filtered [`list`] under a name and description he chose. Reads were
/// gated first; a write into a partition you cannot read is the strictly worse
/// half.
///
/// The denial reuses this verb's own "that id is not available" shape
/// ([`WorkspaceOpError::id_taken`]) and returns **before reading the store**, so
/// it always carries the plain form and cannot become an existence oracle.
///
/// A genuine collision has two forms. When the id is held by an **archived**
/// workspace the refusal names the way back, because "already exists" is true
/// but unactionable and sends the operator off to pick a different id. The
/// probe only ever upgrades a refusal that is ALREADY TRUE into a more specific
/// one — the same shape [`update`] uses — so a probe that cannot answer falls
/// back to what it was refining rather than inventing something.
pub async fn create(
    store: &AgentEnvStore,
    params: WorkspaceCreateParams,
) -> OpResult<WorkspaceDetail> {
    if !ambient_partition_visible(&params.id) {
        return Err(WorkspaceOpError::IdTaken {
            id: params.id,
            archived: false,
        });
    }

    let mut ws = match store
        .create(&params.id, "default", params.description.as_deref())
        .await
    {
        Ok(ws) => ws,
        Err(AgentEnvError::AlreadyExists(taken)) => {
            let archived = matches!(
                store.get_including_archived(&taken).await,
                Ok(Some(ws)) if ws.is_archived
            );
            return Err(WorkspaceOpError::IdTaken {
                id: taken,
                archived,
            });
        }
        Err(source) => {
            return Err(WorkspaceOpError::Store {
                context: "Failed to create workspace",
                source,
            })
        }
    };

    // `AgentEnvStore::create` takes neither name nor icon, so they are a second
    // write.
    ws.name = params.name;
    ws.icon = params.icon.clone();
    let persisted = store
        .update(&params.id, Some(&ws.name), None, params.icon.as_deref())
        .await;

    // Answer with the row that is ON DISK, not the one just asked for.
    //
    // Until 2026-08-09 this returned the locally-mutated `ws`, which made the
    // answer a **statement of intent** wearing the shape of an observation: the
    // write above only `warn!`s on failure, so a workspace whose name never
    // persisted was reported back carrying that name, and the caller learned
    // otherwise on their next read. That is the same "said ok, wrote nothing"
    // shape [`update`] and `AgentEnvStore::update` were fixed for a day earlier.
    //
    // It also silently answered a different question about time. `create`
    // builds `created_at` from `Utc::now()` while the store persists
    // `timestamp()` — whole seconds — so the answer carried sub-second
    // precision that no later read would ever reproduce.
    //
    // A failed read-back falls back to the constructed value: the row does
    // exist (the INSERT succeeded), and refusing the whole call would be a
    // worse lie than an imprecise success.
    let ws = match (persisted, store.get(&params.id).await) {
        (Ok(_), Ok(Some(stored))) => stored,
        (persisted, read_back) => {
            tracing::warn!(
                workspace = %params.id,
                persist_error = ?persisted.err(),
                read_back = ?read_back.map(|r| r.is_some()),
                "workspace create: could not confirm the stored row; \
                 answering with the requested values"
            );
            ws
        }
    };
    Ok(detail_of(&ws))
}

/// Patch a workspace's display metadata. Absent fields mean "leave it alone".
///
/// # Archived workspaces are refused, and the refusal says which refusal it is
///
/// `AgentEnvStore::update` filters the write to active rows, so an archived id
/// reaches the `None` arm below with nothing written — see its doc for why the
/// write was narrowed rather than the read-back widened, and for the shape this
/// replaced (the row was really rewritten and the caller was told it did not
/// exist).
///
/// That arm then asks which `None` it is, because the two have different honest
/// answers — see [`WorkspaceOpError`] for the split. The no-oracle property is
/// untouched: a partition-invisible id returns from the check ABOVE and never
/// reaches the store, so it cannot land on this branch and cannot learn
/// anything from it.
pub async fn update(
    store: &AgentEnvStore,
    params: WorkspaceUpdateParams,
) -> OpResult<WorkspaceDetail> {
    if !ambient_partition_visible(&params.id) {
        return Err(WorkspaceOpError::NotFound(params.id));
    }

    let updated = store
        .update(
            &params.id,
            params.name.as_deref(),
            params.description.as_deref(),
            params.icon.as_deref(),
        )
        .await
        .map_err(store_err("Failed to update workspace"))?;

    if let Some(ws) = updated {
        return Ok(detail_of(&ws));
    }

    // Which `None` is it? An archived row is visible to this caller, so saying
    // "not found" would contradict the read they can run in the next breath.
    // Anything else genuinely is not there.
    //
    // Gated on the OBSERVED flag, not on the row merely existing: the message
    // states a fact about the row, so it has to be one we read rather than one
    // we inferred from which arm we are on. Everything else — no row, the read
    // failing, or a live row that somehow did not match the write — falls back
    // to what it was refining rather than inventing something.
    match store.get_including_archived(&params.id).await {
        Ok(Some(ws)) if ws.is_archived => Err(WorkspaceOpError::Archived(params.id)),
        _ => Err(WorkspaceOpError::NotFound(params.id)),
    }
}

/// Archive (soft-delete) a workspace.
///
/// The most destructive verb in this module, and reversible since 2026-08-09 —
/// see [`unarchive`]. What has NOT changed is that the id stays taken:
/// [`create`] still refuses it, with a refusal that names the way back.
///
/// Deliberately returns nothing. The result is that the row left the default
/// view; there is nothing useful left to show, and [`unarchive`]'s doc records
/// why the asymmetry is on purpose.
pub async fn archive(store: &AgentEnvStore, id: &str) -> OpResult<()> {
    if !ambient_partition_visible(id) {
        return Err(WorkspaceOpError::NotFound(id.to_string()));
    }
    if store
        .archive(id)
        .await
        .map_err(store_err("Failed to archive workspace"))?
    {
        Ok(())
    } else {
        Err(WorkspaceOpError::NotFound(id.to_string()))
    }
}

/// Restore an archived workspace — the inverse of [`archive`].
///
/// # Why archive stopped being terminal
///
/// It was terminal by omission dressed as design. "Readable, not writable" is a
/// sound rule for an archived row, but with no way back a single mistyped
/// archive was permanent — and permanent in the expensive direction, because
/// the id stays taken (`AgentEnvStore::create` is a plain INSERT against a
/// primary key) so the operator cannot even start over under the same name. See
/// [`AgentEnvStore::unarchive`] for why the id staying taken is the right half
/// to keep.
///
/// # This one returns the workspace, unlike [`archive`]
///
/// Deliberate asymmetry, recorded here so it is not "fixed" into symmetry
/// later. This verb's result IS a row, and a caller that has to re-read it with
/// a follow-up `get` is racing anything else holding the store.
pub async fn unarchive(store: &AgentEnvStore, id: &str) -> OpResult<WorkspaceDetail> {
    if !ambient_partition_visible(id) {
        return Err(WorkspaceOpError::NotFound(id.to_string()));
    }
    // Unambiguous, unlike [`update`]'s `None`: the store's UPDATE carries no
    // `archived` predicate, so this is "no such row" and nothing else. No
    // follow-up probe — there is nothing to disambiguate, and a probe that
    // cannot change the answer reads like one that can.
    match store
        .unarchive(id)
        .await
        .map_err(store_err("Failed to unarchive workspace"))?
    {
        Some(ws) => Ok(detail_of(&ws)),
        None => Err(WorkspaceOpError::NotFound(id.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plain half of an id collision must not depend on how the caller
    /// spells "unarchive" — the partition denial (which never reads the store)
    /// and a genuine collision both go through it, and they have to stay
    /// byte-identical or the refusal becomes an existence oracle.
    #[test]
    fn a_plain_collision_reads_the_same_whatever_the_surface_calls_unarchive() {
        let plain = WorkspaceOpError::IdTaken {
            id: "crypto".to_string(),
            archived: false,
        };
        let wire = plain.text("`workspace.unarchive`");
        let tool = plain.text("action=\"unarchive\"");
        assert_eq!(wire, tool);
        assert_eq!(wire, WorkspaceOpError::id_taken("crypto"));
        assert!(
            !wire.contains("unarchive"),
            "a plain collision must not hint at a way back that does not exist: {wire}"
        );
    }

    /// ...and the archived half must, because "already exists" alone sends the
    /// operator off to pick a different id.
    #[test]
    fn an_archived_collision_names_the_surface_s_own_way_back() {
        let held = WorkspaceOpError::IdTaken {
            id: "crypto".to_string(),
            archived: true,
        };
        let tool = held.text("action=\"unarchive\"");
        assert!(tool.starts_with(&WorkspaceOpError::id_taken("crypto")));
        assert!(tool.contains("action=\"unarchive\""));
        assert!(
            !tool.contains("workspace.unarchive"),
            "the tool face must not be told to call an RPC method it cannot reach: {tool}"
        );
    }

    /// The two `None`s of a write are different answers. Pinned on the text so
    /// a future edit cannot quietly collapse them into one.
    #[test]
    fn not_found_and_archived_are_different_answers() {
        let missing = WorkspaceOpError::NotFound("ghost".to_string()).text("");
        let archived = WorkspaceOpError::Archived("crypto".to_string()).text("");
        assert_eq!(missing, "Workspace 'ghost' not found");
        assert_eq!(
            archived,
            "Workspace 'crypto' is archived and cannot be modified"
        );
        assert_ne!(missing, archived);
    }
}
