//! Plan phase — the read-only planning posture and its handoff into execution.
//!
//! ## What this is
//!
//! A session is in exactly one of two phases: [`PlanPhase::Building`] (the
//! default, byte-identical to every install that never heard of this module)
//! or [`PlanPhase::Planning`], in which **no tool that can change anything
//! outside the plan may run**. The model researches, writes a checklist through
//! `scratchpad`, and asks the person to approve it. Approval — and only
//! approval — moves the session to `Building`.
//!
//! ## Why this is not a fourth `SessionMode`
//!
//! [`super::session_mode::SessionMode`] documents, in its own module header and
//! in `MODE_SYSTEM.md`, that a mode "never grants or denies anything". A mode
//! that denies would make that sentence false at the moment it is read, and
//! that sentence has three copies — the doc, the module header, and the line
//! `SessionMode::prompt_line` ships to the model every turn.
//!
//! ## Why this is not a fourth `ExecTier`
//!
//! Two reasons, both structural rather than stylistic.
//!
//! 1. **The tier is not a floor.** [`super::effective_permission`] resolves an
//!    explicit `[policies.tool_permissions]` entry *before* consulting the tier,
//!    which is exactly right for Ask/Auto/Full (an operator who names a tool has
//!    decided) and exactly wrong here: a single `"bash" = "allow"` would hollow
//!    out a posture whose whole promise is "nothing you do can change anything".
//!    So the phase is checked **above** the explicit layer, like the sandbox
//!    hardline — see [`PlanPhase::admits`]'s call site.
//! 2. **A tier value would need a resume value.** `resolve_exec_tier` clamps the
//!    tier every turn (non-operator ceiling, channel clamp). If Plan were a tier,
//!    leaving it would mean restoring "the tier from before", i.e. persisting a
//!    permission value captured at an earlier moment under earlier clamps — a
//!    stale, escalation-shaped piece of state. Because the phase is orthogonal,
//!    **there is nothing to restore**: the latch lifts and the tier the turn
//!    already resolved (and already clamped) takes over unchanged.
//!
//! ## An enum, not a bool
//!
//! Deliberate. A bool is the cheaper spelling today and the wrong shape the
//! first time anyone wants a third posture (read-only-plus-memory, or a
//! post-approval "building, plan frozen" state). Adding a variant to an enum is
//! a compile error at every match; widening a bool is a silent semantic change.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Identity-metadata custom key under which a session's phase is persisted.
/// Same carrier as `EXEC_TIER_SESSION_KEY` / `MODE_SESSION_KEY`: written
/// through `sessions.patch` or stamped from a request-carried value, read per
/// turn by the execution engine.
///
/// Absent (the overwhelmingly common case) reads as [`PlanPhase::Building`].
pub const PLAN_PHASE_SESSION_KEY: &str = "plan_phase";

/// Tools whose entire purpose is to *produce* the plan or to *reach the human*,
/// and which therefore cannot be gated by the posture that exists to make the
/// human read a plan.
///
/// Name-keyed, and legitimately so: Aleph defines all three, so their names ARE
/// contracts (same argument as [`super::exec_tier`]'s `HUMAN_CONTACT_TOOLS`).
/// Nothing here touches anything outside the session's own scratchpad or the
/// conversation.
///
/// * `scratchpad` — the plan lives here. Denying it would leave the model in a
///   phase whose only exit it cannot write.
/// * `ask_user` — planning is a conversation. `ExecTier` already carves this
///   out for the same reason one layer down.
/// * `flag_user_correction` — records that the user corrected the model. Pure
///   bookkeeping about the conversation, and the phase most likely to attract a
///   correction is this one.
const PLANNING_TOOLS: &[&str] = &["scratchpad", "ask_user", "flag_user_correction"];

/// `file_ops` operations that only observe.
///
/// `file_ops` multiplexes `list` / `search` / `stats` *and* `delete` / `move`
/// behind one name, so it is non-idempotent as a whole and a name-keyed rule
/// can only deny it wholesale. Denying it wholesale would remove the repo-grep
/// a planning turn needs most. This is the mirror image of
/// [`super::exec_tier`]'s `DESTRUCTIVE_FILE_OPS` — the same tool, the same
/// argument, the opposite question — and the two are pinned against each other
/// by `read_and_destructive_file_ops_are_disjoint`.
///
/// Values are the serialized `FileOperation` variants
/// (`src/builtin_tools/file_ops/types.rs`).
const READ_ONLY_FILE_OPS: &[&str] = &["list", "search", "stats", "tree", "find_duplicates"];

/// The `scratchpad` action that asks the person to approve the plan. Spelled
/// once, here, because three places need it and none of them may disagree: the
/// tool that implements it, the gate that stops it for a human, and the
/// admission rule that lets it through the read-only floor at all.
pub const HANDOFF_ACTION: &str = "request_build";

/// What the read-only planning floor does with one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAdmission {
    /// Runs unchanged: the call cannot change anything the plan is about.
    Admitted,
    /// This is the handoff verb. It runs only after the person approves —
    /// enforced by `GateRule::PlanHandoff`, not here.
    Handoff,
    /// Refused while the session is planning.
    Refused,
}

/// What the floor can say about a tool from its **name alone**.
///
/// The floor is asked twice about every tool, in two places that must not
/// disagree: once with no arguments, when the tool surface is built (a tool
/// nothing could make admissible is hidden outright), and once with the real
/// arguments, at dispatch. Both answers are derived from this one function, so
/// "which tools disappear while planning" and "which calls are refused while
/// planning" cannot drift into two tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameVerdict {
    /// No argument shape can make this call change anything.
    AlwaysAdmitted,
    /// Some argument shapes are admissible and some are not — the tool stays
    /// visible and the decision moves to dispatch.
    ArgumentDependent,
    /// No argument shape is admissible. The tool is hidden while planning.
    Refused,
}

/// The floor's verdict on a tool name. See [`NameVerdict`].
fn name_verdict(name: &str, idempotent: bool) -> NameVerdict {
    // `scratchpad` writes the plan (always fine) AND carries the handoff verb
    // (a gate), so it is argument-dependent even though every one of its
    // actions is reachable while planning.
    if name == "scratchpad" || name == "file_ops" {
        return NameVerdict::ArgumentDependent;
    }
    if idempotent || PLANNING_TOOLS.contains(&name) {
        return NameVerdict::AlwaysAdmitted;
    }
    NameVerdict::Refused
}

/// Which phase of the plan → build cycle a session is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PlanPhase {
    /// Normal operation. The default, and what an absent session key means, so
    /// every existing session and every install that never touches this feature
    /// resolves exactly as it did before the phase existed.
    #[default]
    Building,
    /// Read-only planning. Research and checklist writing only; the exit is an
    /// approved handoff.
    Planning,
}

impl PlanPhase {
    /// Parse a phase from its serialized id (`"planning"` / `"building"`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "planning" => Some(Self::Planning),
            "building" => Some(Self::Building),
            _ => None,
        }
    }

    /// Serialized id of this phase.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Building => "building",
            Self::Planning => "planning",
        }
    }

    /// True while the read-only floor is engaged.
    #[must_use]
    pub const fn is_planning(self) -> bool {
        matches!(self, Self::Planning)
    }

    /// This phase's verdict on one call.
    ///
    /// `idempotent` is the tool's own DECLARED metadata — the same fact
    /// [`super::exec_tier::ExecTier::rule_for`] reads, from the same seam
    /// (`LoopTool::is_idempotent`), so "mutating" means one thing in this repo
    /// and not two. An unknown tool declares nothing, is therefore not
    /// idempotent, and is therefore refused: fail-closed, which is the only
    /// admissible direction for a posture that promises read-only.
    ///
    /// `Building` admits everything: the phase adds no rule when it is not
    /// engaged, so the hot path is one comparison and the behavior of every
    /// existing install is byte-identical.
    #[must_use]
    pub fn admits(self, name: &str, input: &Value, idempotent: bool) -> PlanAdmission {
        if !self.is_planning() {
            return PlanAdmission::Admitted;
        }
        match name_verdict(name, idempotent) {
            NameVerdict::AlwaysAdmitted => PlanAdmission::Admitted,
            NameVerdict::Refused => PlanAdmission::Refused,
            NameVerdict::ArgumentDependent => {
                if name == "scratchpad" {
                    // Every scratchpad action writes the plan and nothing else,
                    // so all of them are admissible; the one that is a *gate*
                    // is singled out here.
                    if scratchpad_action(input) == Some(HANDOFF_ACTION) {
                        PlanAdmission::Handoff
                    } else {
                        PlanAdmission::Admitted
                    }
                } else if file_ops_is_read_only(input) {
                    PlanAdmission::Admitted
                } else {
                    PlanAdmission::Refused
                }
            }
        }
    }

    /// True when the floor refuses this tool for **every** argument shape, so
    /// it should not appear in the turn's tool surface at all.
    ///
    /// Hiding rather than refusing is deliberate for this class: a model that
    /// can see `file_write` in its tool list while planning will reach for it,
    /// burn a turn on the refusal, and — worse — read the refusal as an
    /// obstacle to route around. A tool it cannot see is not a temptation.
    /// Tools with an admissible argument form (`file_ops`, `scratchpad`) stay
    /// visible; their refusal happens where the arguments are known.
    ///
    /// Derived from the same [`name_verdict`] as [`Self::admits`] — the two
    /// answers are one table, not two.
    #[must_use]
    pub fn hides(self, name: &str, idempotent: bool) -> bool {
        self.is_planning() && name_verdict(name, idempotent) == NameVerdict::Refused
    }

    /// One model-facing line describing the phase, for the system prompt
    /// (rendered by `OperatingEnvelopeLayer`, in the Dynamic zone — the phase
    /// flips mid-conversation and must never touch the cacheable prefix).
    ///
    /// The copy lives next to [`Self::admits`] — the single source of what the
    /// phase actually gates — so the rule and its description cannot drift.
    /// Model-facing prompt text is always English in this repo.
    ///
    /// `Building` renders nothing. A line that says "you may act" on every turn
    /// of every install is bytes spent to state the absence of a feature, and
    /// R9's first ruler rejects it.
    #[must_use]
    pub const fn prompt_line(self) -> Option<&'static str> {
        match self {
            Self::Building => None,
            Self::Planning => Some(
                "Plan phase: planning (read-only) — every tool that could change \
                 anything is refused, including `bash`. Research with the read-only \
                 tools (`file_read`, `file_ops` list/search/stats, `ctx_search`, \
                 `search`, `web_fetch`), write the plan with `scratchpad` \
                 (set_objective + set_plan), and ask questions with `ask_user`. \
                 When the plan is ready, call `scratchpad` with \
                 `action: \"request_build\"` to put it to the user; if they approve, \
                 execution unlocks and you carry the plan out. Do not describe the \
                 plan as done, and do not try to work around a refusal — the \
                 refusal is the point of this phase.",
            ),
        }
    }

    /// The sentence a refused call reports back to the model.
    ///
    /// One derivation, two audiences, same words: this is what the tool error
    /// says and what the approval-card reader would be told, so the model and
    /// the person are never told different stories about the same floor.
    #[must_use]
    pub fn refusal(tool: &str) -> String {
        format!(
            "`{tool}` cannot run while this session is in the read-only planning phase. \
             Finish the plan with `scratchpad` and call \
             `scratchpad {{ action: \"{HANDOFF_ACTION}\" }}` to ask the user to approve it; \
             execution unlocks only when they do. Do not retry this call and do not \
             attempt the same effect through another tool."
        )
    }
}

/// The `action` field of a `scratchpad` call, when it is a string.
fn scratchpad_action(input: &Value) -> Option<&str> {
    input.get("action").and_then(Value::as_str)
}

/// True when this `file_ops` call names an operation that only observes.
///
/// A call with no `operation` at all is NOT read-only: the tool decides the
/// default itself, and a floor must never let an omitted field pick the
/// permissive branch.
fn file_ops_is_read_only(input: &Value) -> bool {
    input
        .get("operation")
        .and_then(Value::as_str)
        .is_some_and(|op| READ_ONLY_FILE_OPS.contains(&op))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn admits(phase: PlanPhase, name: &str, idempotent: bool) -> PlanAdmission {
        phase.admits(name, &json!({}), idempotent)
    }

    #[test]
    fn default_is_building() {
        assert_eq!(PlanPhase::default(), PlanPhase::Building);
    }

    #[test]
    fn ids_round_trip() {
        for phase in [PlanPhase::Building, PlanPhase::Planning] {
            assert_eq!(PlanPhase::from_id(phase.id()), Some(phase));
        }
        assert_eq!(PlanPhase::from_id("nonsense"), None);
        assert_eq!(PlanPhase::from_id(""), None);
    }

    #[test]
    fn serde_uses_the_same_ids_as_from_id() {
        // The wire form and the metadata form must be one spelling: the session
        // key is written from `id()` and read back through `from_id`, while
        // `chat.send` carries the serde form.
        for phase in [PlanPhase::Building, PlanPhase::Planning] {
            let json = serde_json::to_string(&phase).unwrap();
            assert_eq!(json, format!("\"{}\"", phase.id()));
            let back: PlanPhase = serde_json::from_str(&json).unwrap();
            assert_eq!(back, phase);
        }
    }

    #[test]
    fn building_admits_everything() {
        assert_eq!(
            admits(PlanPhase::Building, "bash", false),
            PlanAdmission::Admitted
        );
        assert_eq!(
            admits(PlanPhase::Building, "file_write", false),
            PlanAdmission::Admitted
        );
        // Including the handoff verb: outside planning it is not a gate, it is
        // an ordinary (and, as the tool implements it, refused) call.
        assert_eq!(
            PlanPhase::Building.admits("scratchpad", &json!({"action": HANDOFF_ACTION}), false),
            PlanAdmission::Admitted
        );
    }

    #[test]
    fn planning_refuses_mutating_tools() {
        for name in [
            "bash",
            "code_exec",
            "file_write",
            "file_edit",
            "apply_patch",
        ] {
            assert_eq!(
                admits(PlanPhase::Planning, name, false),
                PlanAdmission::Refused,
                "{name} must not run while planning"
            );
        }
    }

    #[test]
    fn planning_admits_declared_reads() {
        for name in ["file_read", "ctx_search", "search", "web_fetch"] {
            assert_eq!(
                admits(PlanPhase::Planning, name, true),
                PlanAdmission::Admitted,
                "{name} declares itself idempotent and must stay callable"
            );
        }
    }

    #[test]
    fn unknown_tools_are_refused_while_planning() {
        // An unknown tool declares nothing, so `idempotent` is false and the
        // floor must hold. This is the property that makes the phase safe for
        // MCP servers nobody has classified.
        assert_eq!(
            admits(PlanPhase::Planning, "some__unheard_of_tool", false),
            PlanAdmission::Refused
        );
    }

    #[test]
    fn planning_admits_the_plan_writer_and_the_human_channel() {
        for name in PLANNING_TOOLS {
            assert_eq!(
                admits(PlanPhase::Planning, name, false),
                PlanAdmission::Admitted,
                "{name} is how planning happens"
            );
        }
    }

    #[test]
    fn the_handoff_verb_is_a_gate_not_an_admission() {
        assert_eq!(
            PlanPhase::Planning.admits("scratchpad", &json!({"action": HANDOFF_ACTION}), false),
            PlanAdmission::Handoff
        );
    }

    #[test]
    fn file_ops_splits_on_its_operation() {
        for op in READ_ONLY_FILE_OPS {
            assert_eq!(
                PlanPhase::Planning.admits("file_ops", &json!({"operation": op}), false),
                PlanAdmission::Admitted,
                "file_ops {op} only observes"
            );
        }
        for op in ["delete", "move", "batch_move", "organize", "copy"] {
            assert_eq!(
                PlanPhase::Planning.admits("file_ops", &json!({"operation": op}), false),
                PlanAdmission::Refused,
                "file_ops {op} changes things"
            );
        }
    }

    #[test]
    fn file_ops_with_no_operation_is_refused() {
        // An omitted field must not select the permissive branch: the tool's
        // own default is not this module's to guess.
        assert_eq!(
            PlanPhase::Planning.admits("file_ops", &json!({}), false),
            PlanAdmission::Refused
        );
        assert_eq!(
            PlanPhase::Planning.admits("file_ops", &json!({"operation": 3}), false),
            PlanAdmission::Refused
        );
    }

    #[test]
    fn read_and_destructive_file_ops_are_disjoint() {
        // The two lists answer opposite questions about the same argument. If a
        // future operation landed in both, one of the two gates would be a lie;
        // this pins them apart at compile-of-the-test-suite time rather than at
        // the moment somebody deletes a directory during a planning turn.
        for op in READ_ONLY_FILE_OPS {
            assert!(
                !super::super::exec_tier::destructive_file_ops().contains(op),
                "`{op}` is claimed by both the read-only and the destructive list"
            );
        }
    }

    #[test]
    fn only_planning_renders_a_prompt_line() {
        assert!(PlanPhase::Building.prompt_line().is_none());
        let planning = PlanPhase::Planning.prompt_line().expect("planning speaks");
        // The line must name the exit, or it describes a trap rather than a
        // phase (判据: 一句关于"什么被闸住"的话，发给模型的那份说了假话最贵).
        assert!(planning.contains(HANDOFF_ACTION));
    }

    #[test]
    fn hiding_never_contradicts_admission() {
        // The floor answers twice — once with no arguments (tool surface) and
        // once with them (dispatch). A tool that is hidden must have no
        // admissible call, and a tool that is visible must have at least one,
        // or one of the two surfaces is lying about the other.
        let cases: &[(&str, bool, &[Value])] = &[
            ("bash", false, &[]),
            ("file_read", true, &[]),
            ("ask_user", false, &[]),
            (
                "file_ops",
                false,
                &[
                    json!({"operation": "search"}),
                    json!({"operation": "delete"}),
                ],
            ),
            (
                "scratchpad",
                false,
                &[
                    json!({"action": "set_plan"}),
                    json!({"action": HANDOFF_ACTION}),
                ],
            ),
        ];
        for (name, idempotent, inputs) in cases {
            let hidden = PlanPhase::Planning.hides(name, *idempotent);
            let probes: Vec<Value> = if inputs.is_empty() {
                vec![json!({})]
            } else {
                inputs.to_vec()
            };
            let any_admissible = probes.iter().any(|i| {
                PlanPhase::Planning.admits(name, i, *idempotent) != PlanAdmission::Refused
            });
            assert_eq!(
                hidden, !any_admissible,
                "`{name}`: hidden={hidden} but any_admissible={any_admissible}"
            );
        }
    }

    #[test]
    fn building_hides_nothing() {
        for name in ["bash", "file_write", "some__unheard_of_tool"] {
            assert!(!PlanPhase::Building.hides(name, false));
        }
    }

    #[test]
    fn the_refusal_names_the_exit() {
        let said = PlanPhase::refusal("bash");
        assert!(said.contains("bash"));
        assert!(said.contains(HANDOFF_ACTION));
    }
}
