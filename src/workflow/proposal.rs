//! Gated `MetaSkill` proposals — the **draft tier** above the active workflow
//! store.
//!
//! The dream pipeline mines recurring skill co-occurrence
//! ([`crate::skill::cooccurrence`]) and drafts candidate workflows here. A
//! proposal is **never** an active workflow: it lives in a `proposals/`
//! subdirectory and runs only after an explicit `accept` (the "proposal gate" /
//! proposal gate). This keeps the loop R5-quiet — capabilities grow in the
//! background, but nothing auto-activates and nothing steals focus.
//!
//! ## Entropy reduction: pure reuse of the active store
//!
//! A proposal is just a [`WorkflowManifest`] that lives in the gated dir
//! instead of the active one, so this module is a thin set of functions over
//! [`crate::workflow::store`] anchored to [`proposals_dir`] — no new
//! persistence, no new atomic-write path, no new traversal guard. Provenance
//! (how many times the chain was observed, which skills) rides in the
//! workflow's `description`, so it survives `list`/`describe`/`accept`
//! unchanged.

use std::path::PathBuf;

use crate::error::Result;
use crate::json_canvas_io::sanitise_name;
use crate::routing::DEFAULT_AGENT_ID;
use crate::workflow::def::{WorkflowDef, WorkflowStepDef};
use crate::workflow::interop::manifest::WorkflowManifest;
use crate::workflow::store;

/// Name prefix for every auto-drafted `MetaSkill` proposal. Lets a human (and
/// the dedup check) tell auto-generated drafts from user-authored workflows.
pub const PROPOSAL_PREFIX: &str = "metaskill-";

/// Cap on the canonical name length so a long chain does not produce an
/// unwieldy filename. The dedup key stays stable because the truncation is
/// deterministic.
const MAX_NAME_LEN: usize = 80;

/// `$ALEPH_HOME/workflows/proposals/` — the gated draft directory, a sibling
/// concept to the active [`store::workflow_dir`].
#[must_use]
pub fn proposals_dir() -> PathBuf {
    store::workflow_dir().join("proposals")
}

/// Deterministic, dedup-stable name for a co-occurrence chain: the prefix plus
/// the skill ids **sorted** and joined by `-`. Sorting (not observed order)
/// makes the same skill SET map to the same name regardless of sequence, so a
/// second observation of `[b, a]` collides with a draft of `[a, b]`.
///
/// Skill ids are sanitised before joining because the resulting name must
/// round-trip through `store::list_at` (which keys on the on-disk `file_stem`
/// after `sanitise_name` rewriting). Joining raw ids like `["git status",
/// "code review"]` would yield `"metaskill-git status-code review"`, which
/// the file system rewrites to `"metaskill-git_status-code_review"` and
/// thereafter never matches the in-memory canonical_name — silently defeating
/// the proposal dedup gate.
pub fn canonical_name(skills: &[String]) -> String {
    let joined = canonical_set(skills).join("-");
    let mut name = format!("{PROPOSAL_PREFIX}{joined}");
    if name.len() > MAX_NAME_LEN {
        // Truncate on a char boundary to stay UTF-8 safe (P7).
        let mut end = MAX_NAME_LEN;
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }
    name
}

/// Canonical, comparable form of a name set: **sanitise first, then sort,
/// then dedup**.
///
/// The order matters and is the single derivation point for it (criterion
/// 12). `sanitise_name` maps every char outside `[A-Za-z0-9._-]` to `_`
/// (`0x5F`), which sorts ABOVE both `-` (`0x2D`) and space (`0x20`) — so
/// sorting before sanitising yields a different sequence than sorting after
/// (`"deep dive" < "deep-scan"` raw, `"deep-scan" < "deep_dive"` sanitised),
/// and deduping before sanitising leaves two entries where the sanitised
/// world has one. Every comparison of a skill/step name set in this module
/// goes through here so no two of them can disagree.
fn canonical_set(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names.iter().map(|s| sanitise_name(s)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Build a linear [`WorkflowDef`] skeleton from an observed skill chain. Each
/// skill becomes one agent step depending on the previous, so accepting the
/// draft yields a runnable template the user can re-target and refine.
///
/// Deterministic and reasoning-free (R7/R10): the system only *transcribes* the
/// observed habit into a template shell. The intelligence — what the steps
/// should really say, which agents own them — is added by the LLM/user when
/// the proposal is reviewed, not invented here. Returns `None` for a chain of
/// fewer than two skills (a single skill is not a `MetaSkill`).
#[must_use]
pub fn skeleton_from_chain(chain: &[String], observations: u32) -> Option<WorkflowDef> {
    if chain.len() < 2 {
        return None;
    }
    // Step ids are `sanitise_name(skill)`, which is not injective: two
    // distinct skills (`deploy:prod`, `deploy prod`) collapse onto one id.
    // `cluster_chains` dedups on the RAW skill name, so such a pair survives
    // to here — and minting both steps produced a def with a duplicate step
    // id, which `validate` rejects at `save_proposal`. Nothing records that
    // failure, so the miner re-drafted the same chain on every dream cycle,
    // forever. Keep the first occurrence and let the next step depend on the
    // survivor.
    let mut steps: Vec<WorkflowStepDef> = Vec::with_capacity(chain.len());
    for skill in chain {
        let id = sanitise_name(skill);
        if steps.iter().any(|s| s.id == id) {
            continue;
        }
        let depends_on = match steps.last() {
            None => Vec::new(),
            Some(prev) => vec![prev.id.clone()],
        };
        let prompt = if depends_on.is_empty() {
            format!("Apply the '{skill}' skill toward the goal: {{input}}")
        } else {
            format!("Continue with the '{skill}' skill, building on the previous step's output.")
        };
        steps.push(WorkflowStepDef {
            id,
            agent: DEFAULT_AGENT_ID.to_string(),
            prompt,
            depends_on,
            kind: Default::default(),
            choices: Vec::new(),
            review: false,
            require_grounding: false,
            tolerate_failed_deps: false,
            timeout_seconds: None,
            max_retries: None,
        });
    }
    if steps.len() < 2 {
        // Every skill in the chain collapsed onto one id — the same rule as a
        // one-skill chain, one step later: this is not a `MetaSkill`.
        tracing::warn!(
            chain = %chain.join(" → "),
            "MetaSkill skeleton: the whole chain collapsed to one step id after sanitising — not drafting"
        );
        return None;
    }

    let description = format!(
        "Auto-drafted MetaSkill from {observations} observed co-occurrence(s) of skills: {}. \
         Review the steps/agents and accept via workflow(action='accept_proposal') to activate.",
        chain.join(" → ")
    );

    Some(WorkflowDef {
        name: canonical_name(chain),
        description,
        steps,
    })
}

/// Persist a draft into the gated `proposals/` dir. Validates via the store
/// (an invalid skeleton never reaches disk).
pub fn save_proposal(def: &WorkflowDef) -> Result<PathBuf> {
    let manifest = WorkflowManifest::from_def(def);
    store::save_at(&proposals_dir(), &manifest)
}

/// List pending proposals (gated drafts), sorted by name. Carries the same
/// entry shape as the active store, so a draft can be judged from the listing
/// (its provenance rides in `description`) instead of one `describe_proposal`
/// per candidate.
pub fn list_proposals() -> Result<store::WorkflowListing> {
    store::list_at(&proposals_dir())
}

/// Load one pending proposal by name.
pub fn load_proposal(name: &str) -> Result<WorkflowManifest> {
    store::load_at(&proposals_dir(), name)
}

/// Delete a pending proposal (idempotent). Returns `true` if a draft was
/// removed — e.g. a user rejecting it.
pub fn delete_proposal(name: &str) -> Result<bool> {
    store::delete_at(&proposals_dir(), name)
}

/// **Gated accept**: promote a pending proposal to an active workflow.
///
/// Loads the draft, saves it into the active store, then removes the draft so
/// it is not re-offered. The active copy is what `workflow(action='run')`
/// executes — so a proposal only ever runs after this explicit step.
pub fn accept(name: &str) -> Result<PathBuf> {
    let manifest = load_proposal(name)?;
    let active_path = store::save(&manifest)?;
    // Draft removal is cleanup, not the semantic outcome — the workflow IS
    // active once the save above succeeded. Propagating a delete I/O error
    // here would report failure for an accept that worked (and invite a
    // confusing re-accept of the leftover draft), so log-and-continue.
    if let Err(e) = delete_proposal(name) {
        tracing::warn!(proposal = %name, error = %e, "accepted proposal but could not remove the draft; it may be re-listed until deleted");
    }
    Ok(active_path)
}

/// True when an active workflow OR a pending proposal already carries this
/// chain's canonical name. Used by the miner to avoid re-drafting the same
/// `MetaSkill` every dream cycle. Name-based dedup is deliberate: the canonical
/// name is the chain's identity, so it needs no step-by-step comparison.
#[must_use]
pub fn already_covered(chain: &[String]) -> bool {
    let name = canonical_name(chain);
    let in_active = store::list().is_ok_and(|l| l.entries.iter().any(|m| m.name == name));
    if in_active {
        return true;
    }
    list_proposals().is_ok_and(|l| l.entries.iter().any(|m| m.name == name))
}

/// True when the active store already contains a user-authored workflow whose
/// step set equals this chain's skill set — even under a different name. This
/// catches the case where the user already built (and renamed) the same
/// `MetaSkill` by hand, so the miner does not shadow it with a draft.
pub fn covered_by_step_set(chain: &[String]) -> bool {
    let want = canonical_set(chain);
    let Ok(listing) = store::list() else {
        return false;
    };
    for meta in listing.entries {
        let Ok(manifest) = store::load(&meta.name) else {
            continue;
        };
        let have: Vec<&str> = manifest.steps.iter().map(|s| s.id.as_str()).collect();
        if step_ids_cover_chain(&have, &want) {
            return true;
        }
    }
    false
}

/// Pure half of [`covered_by_step_set`] (the other half is store I/O), so the
/// ordering rule is unit-testable without an `$ALEPH_HOME`.
///
/// Sorted-vector compare rather than two `HashSet`s: two sets match iff they
/// hold the same elements, so equal sorted-and-deduped sequences are
/// equivalent, and this runs once per stored workflow.
///
/// **Design note (not a defect fixed here):** `step_ids` are the RAW stored
/// ids while `want` is canonical, so this only ever matches a workflow whose
/// step ids are already slug-shaped. Canonicalising the stored side too would
/// widen the match — and would also collapse two ids differing only in
/// punctuation into one, silently suppressing a draft — so that is left as an
/// open design question rather than changed under a bug fix.
fn step_ids_cover_chain(step_ids: &[&str], want: &[String]) -> bool {
    let mut have: Vec<&str> = step_ids.to_vec();
    have.sort_unstable();
    have.dedup();
    have.iter().copied().eq(want.iter().map(String::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_name_is_order_independent() {
        let a = canonical_name(&["git".into(), "pr".into()]);
        let b = canonical_name(&["pr".into(), "git".into()]);
        assert_eq!(a, b);
        assert!(a.starts_with(PROPOSAL_PREFIX));
    }

    #[test]
    fn canonical_name_dedups_repeats() {
        let n = canonical_name(&["git".into(), "git".into(), "pr".into()]);
        assert_eq!(n, "metaskill-git-pr");
    }

    #[test]
    fn canonical_name_is_length_capped_on_char_boundary() {
        let many: Vec<String> = (0..40).map(|i| format!("skill{i}")).collect();
        let n = canonical_name(&many);
        assert!(n.len() <= MAX_NAME_LEN);
        // Did not panic and is valid UTF-8 by construction.
        assert!(n.starts_with(PROPOSAL_PREFIX));
    }

    #[test]
    fn canonical_name_sanitises_skills_so_it_round_trips_through_store() {
        // Skills with whitespace / special chars must sanitise the same way
        // the on-disk filename stem does, otherwise dedup against list_at
        // (which keys on file_stem) silently fails.
        let raw: Vec<String> = ["git status", "code review"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let n = canonical_name(&raw);
        let expected = format!("{PROPOSAL_PREFIX}{}", sanitise_name("code review"));
        let expected = format!("{expected}-{}", sanitise_name("git status"));
        assert_eq!(n, expected);
        // The same chain re-ordered must dedup identically.
        let reordered: Vec<String> = raw.iter().rev().cloned().collect();
        assert_eq!(canonical_name(&reordered), n);
    }

    #[test]
    fn skeleton_rejects_single_skill() {
        assert!(skeleton_from_chain(&["solo".into()], 5).is_none());
    }

    #[test]
    fn skeleton_builds_linear_chain() {
        let def = skeleton_from_chain(&["research".into(), "write".into()], 3).unwrap();
        assert_eq!(def.steps.len(), 2);
        assert!(def.steps[0].depends_on.is_empty());
        assert_eq!(def.steps[1].depends_on, vec!["research".to_string()]);
        assert_eq!(def.steps[0].agent, DEFAULT_AGENT_ID);
        assert!(def.description.contains("3 observed"));
        // The skeleton must pass the store's own validator.
        def.validate().unwrap();
    }

    /// The set compare must be done in ONE alphabet. Sanitisation reorders
    /// (`"deep dive" < "deep-scan"` raw, `"deep-scan" < "deep_dive"`
    /// sanitised), so sorting before sanitising made two lists holding the
    /// same elements compare unequal — and the miner re-drafted a workflow
    /// the user had already built by hand, every dream cycle.
    #[test]
    fn step_set_compare_is_stable_under_sanitisation_reordering() {
        let chain: Vec<String> = vec!["deep dive".into(), "deep-scan".into()];
        assert!(
            step_ids_cover_chain(&["deep-scan", "deep_dive"], &canonical_set(&chain)),
            "the same set in the sanitised alphabet must compare equal"
        );
        assert!(
            !step_ids_cover_chain(&["deep-scan"], &canonical_set(&chain)),
            "a strictly smaller step set is not coverage"
        );
    }

    /// Second axis of the same rule: dedup must happen AFTER sanitising, or
    /// two raw skills that collapse to one id leave `want` one element longer
    /// than any stored workflow can ever be.
    #[test]
    fn step_set_compare_dedups_after_sanitising() {
        let chain: Vec<String> = vec!["deploy:prod".into(), "deploy prod".into()];
        assert_eq!(canonical_set(&chain), vec!["deploy_prod".to_string()]);
        assert!(step_ids_cover_chain(
            &["deploy_prod"],
            &canonical_set(&chain)
        ));
    }

    /// Two skills that sanitise to one id must not mint two steps with the
    /// same id: `validate` rejects the duplicate, `save_proposal` fails, and
    /// nothing records the failure — so the miner re-attempts the same chain
    /// on every dream cycle, forever.
    #[test]
    fn skeleton_dedups_steps_whose_ids_collide_after_sanitising() {
        let def = skeleton_from_chain(
            &["deploy:prod".into(), "deploy prod".into(), "verify".into()],
            3,
        )
        .unwrap();
        let ids: Vec<&str> = def.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["deploy_prod", "verify"]);
        assert_eq!(
            def.steps[1].depends_on,
            vec!["deploy_prod".to_string()],
            "the survivor of a collision must be what the next step depends on"
        );
        // The whole point: a skeleton that cannot be saved is a permanent
        // silent retry loop.
        def.validate().unwrap();
    }

    /// A chain whose every skill collapses onto one id is not a `MetaSkill` —
    /// same rule as a one-skill chain, one step later.
    #[test]
    fn skeleton_rejects_a_chain_that_collapses_to_one_step() {
        assert!(skeleton_from_chain(&["deploy:prod".into(), "deploy prod".into()], 5).is_none());
    }
}
