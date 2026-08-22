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
    let mut sorted: Vec<String> = skills.iter().map(|s| sanitise_name(s)).collect();
    sorted.sort_unstable();
    sorted.dedup();
    let joined = sorted.join("-");
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
    let steps: Vec<WorkflowStepDef> = chain
        .iter()
        .enumerate()
        .map(|(i, skill)| {
            let id = sanitise_name(skill);
            let depends_on = if i == 0 {
                Vec::new()
            } else {
                vec![sanitise_name(&chain[i - 1])]
            };
            let prompt = if i == 0 {
                format!("Apply the '{skill}' skill toward the goal: {{input}}")
            } else {
                format!(
                    "Continue with the '{skill}' skill, building on the previous step's output."
                )
            };
            WorkflowStepDef {
                id,
                agent: DEFAULT_AGENT_ID.to_string(),
                prompt,
                depends_on,
                kind: Default::default(),
                choices: Vec::new(),
                review: false,
                require_grounding: false,
                timeout_seconds: None,
                max_retries: None,
            }
        })
        .collect();

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
    // Sort once into a side-allocated `Vec` and compare sorted vectors — avoids
    // allocating two `HashSet`s per stored workflow (`store::load` already
    // does the heavy lifting). Two sets match iff they have the same elements,
    // so equal sorted sequences are equivalent.
    let mut want: Vec<&str> = chain.iter().map(String::as_str).collect();
    want.sort_unstable();
    want.dedup();
    let want_sanitised: Vec<String> = want.iter().map(|s| sanitise_name(s)).collect();
    let Ok(listing) = store::list() else {
        return false;
    };
    for meta in listing.entries {
        let Ok(manifest) = store::load(&meta.name) else {
            continue;
        };
        let mut have: Vec<&str> = manifest.steps.iter().map(|s| s.id.as_str()).collect();
        have.sort_unstable();
        have.dedup();
        if have == want_sanitised {
            return true;
        }
    }
    false
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
}
