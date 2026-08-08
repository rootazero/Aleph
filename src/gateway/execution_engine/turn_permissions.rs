//! This turn's execution permissions: the effective [`ExecTier`] and the merged
//! explicit [`ToolPermissionsConfig`].
//!
//! One resolution, two consumers. The agent loop feeds the pair into
//! `ScopedToolService` (`run_loop::inner`), and the slash-command fast path
//! (`slash_command::execute_direct_tool`) consults it to decide whether it is
//! allowed to dispatch at all. The fast path used to answer that question by not
//! asking it: it called `ToolRegistry::execute_tool` directly, so a chat-tier
//! channel's `/bash …` ran with no tier, no permission policy, no operator gate
//! and no approval card. Both surfaces now resolve the same way, here.

use tracing::{info, warn};

use crate::config::types::policies::{ExecTier, ToolPermissionsConfig};
use crate::executor::ToolRegistry;
use crate::gateway::agent_instance::AgentInstance;
use crate::thinker::ProviderRegistry as ThinkerProviderRegistry;

use super::engine::ExecutionEngine;
use super::RunRequest;

/// Resolve this turn's execution tier.
///
/// Precedence: the tier the REQUEST carries (the composer's pick, possibly made
/// in a conversation whose session did not exist yet) > the session's stored
/// tier > the fallback tier. An explicit choice REPLACES the fallback and may
/// RAISE it as well as lower it — an authorized caller's deliberate decision,
/// safe because the undisableable `[sandbox.command_policy]` floor holds under
/// every tier.
///
/// ## The non-operator ceiling (2026-08-08)
///
/// "An explicit choice REPLACES the fallback and may RAISE it" is true for an
/// operator and was true for everyone. For a **non-operator** it is now bounded:
/// the resolved tier is clamped to the operator's own `[policies.exec_tier]`,
/// so a member may go stricter than the install's posture and never looser.
///
/// This closes a hole the paragraph below used to paper over. The P1 hardening
/// gave a member the `Ask` *default* and wrote that "the clamp below and the
/// undisableable `[sandbox.command_policy]` floor are what actually bound
/// member tool execution" — but the clamp below is
/// [`channel_permission_level_from_role`](crate::gateway::channel_policy::channel_permission_level_from_role),
/// which maps `"guest"` and `"operator"` and returns `None` for everything
/// else. It **never fires for `"member"`.** So the sentence named a predicate
/// that was structurally unreachable for exactly the role it claimed to bound,
/// and `chat.send { exec_tier: "full" }` — a plain per-request parameter any
/// member can send — resolved to `Full`, at which the tier gates nothing.
///
/// The ceiling is the existing global dial rather than a new
/// `member_max_exec_tier` knob: `[policies]` is a server-global section, so
/// setting it to `Full` is already an install-wide statement that this tier
/// axis gates nothing here. **What that makes harder:** an operator who raises
/// the global tier for their own convenience raises every member's ceiling with
/// it, and there is no way to say "Full for me, Auto for everyone else" without
/// introducing that second knob. That is the trade accepted here — one dial
/// with one meaning beats two dials nobody has asked to set differently, and
/// the second knob layers on top of this clamp cleanly if the day comes.
///
/// The fallback is normally the global `[policies.exec_tier]`. P1 member
/// hardening (spec §11): when the caller is NOT an operator and neither rung
/// above named a tier, the fallback is `Ask` instead — a non-operator never
/// silently inherits an operator-configured `Auto`/`Full` default. The
/// predicate is the repo-wide [`role_is_operator`] (absent role = trusted
/// local/internal, `"operator"` = operator, everything else is not), NOT a
/// `role == Some("member")` equality test: that spelling put `"guest"` — the
/// role chat-tier channels default to — on the OPERATOR side of the branch.
/// It is not exploitable today (the login wall admits guests to no method but
/// `connect`), but a gate that reads "member" where it means "not operator" is
/// a wrong precedent for the next person to copy, and `tools_invoke.rs` already
/// uses the canonical predicate for the same question. This is a DEFAULT, not a
/// clamp: an explicit member pick (which lands in `requested` or `stored`) still
/// wins over `Ask` and may raise above it — up to the ceiling above. Operator /
/// absent-role callers are byte-identical to before (fallback stays `global`,
/// and clamping an operator to the global tier is a no-op on their own dial).
///
/// Both clamps run AFTER the three rungs, never before: an untrusted (`Chat`)
/// channel must not run at `Full` with nobody at the keyboard, and a member must
/// not exceed the install's posture, whichever rung asked for it. Panel / CLI /
/// cron turns carry no `caller_role` and are subject to neither.
pub(super) fn resolve_exec_tier(
    global: ExecTier,
    requested: Option<ExecTier>,
    stored: Option<ExecTier>,
    caller_role: Option<&str>,
) -> ExecTier {
    let is_operator = crate::tools::turn_context::role_is_operator(caller_role);
    let fallback = if is_operator { global } else { ExecTier::Ask };
    let tier = requested.or(stored).unwrap_or(fallback);

    // Ceiling first: a non-operator may never resolve looser than the install's
    // own posture. `role_is_operator` treats an ABSENT role as trusted
    // (loopback / CLI / cron), so this touches only connections that carry a
    // role word and it is not one of the operator spellings.
    let tier = if is_operator {
        tier
    } else {
        ExecTier::most_restrictive(tier, global)
    };

    match caller_role.and_then(crate::gateway::channel_policy::channel_permission_level_from_role) {
        Some(level) => crate::gateway::channel_policy::clamp_tier_for_channel(level, tier),
        None => tier,
    }
}

impl<P: ThinkerProviderRegistry + 'static, R: ToolRegistry + 'static> ExecutionEngine<P, R> {
    /// Effective execution tier + explicit tool permission policy for this turn.
    /// Two inputs to one enforcement chokepoint
    /// ([`crate::config::types::policies::effective_permission`]): the explicit
    /// policy decides the tools it names, the tier decides everything else from
    /// each tool's declared metadata.
    ///
    /// The tier is resolved by [`resolve_exec_tier`]; the global rung is read
    /// LIVE from the shared config (not a boot snapshot) so a tier change takes
    /// effect on the very next tool call with no restart, and a request-carried
    /// tier is stamped onto the session so turns 2+ (which carry nothing) and a
    /// page reload both keep it — the same "stamped on the first message"
    /// contract as `project_root`.
    ///
    /// Explicit policy: global `[policies.tool_permissions]` merged with the
    /// agent's override and the originating channel's override (stamped into
    /// metadata by the inbound router — absent for Panel / CLI / cron turns);
    /// most restrictive wins at both layers. `None` when everything is
    /// all-default, so the `ScopedToolService` hot path stays a no-op.
    pub(super) async fn resolve_turn_permissions(
        &self,
        request: &RunRequest,
        agent: &AgentInstance,
    ) -> (ExecTier, Option<ToolPermissionsConfig>) {
        let (global_tier, explicit) = match self.app_config.as_ref() {
            Some(cfg) => {
                let guard = cfg.read().await;
                (
                    guard.policies.exec_tier,
                    guard.policies.tool_permissions.clone(),
                )
            }
            None => Default::default(),
        };
        let requested = request
            .metadata
            .get(crate::config::types::policies::EXEC_TIER_SESSION_KEY)
            .map(String::as_str)
            .and_then(ExecTier::from_id);
        let stored = self.session_exec_tier(&request.session_key).await;
        if let Some(t) = requested.filter(|t| stored != Some(*t)) {
            self.persist_session_exec_tier(&request.session_key, t)
                .await;
        }
        let tier = resolve_exec_tier(
            global_tier,
            requested,
            stored,
            request.metadata.get("caller_role").map(String::as_str),
        );

        let mut merged =
            ToolPermissionsConfig::merge(&explicit, &agent.config().tool_permissions());
        if let Some(raw) = request.metadata.get(super::CHANNEL_TOOL_PERMISSIONS_KEY) {
            match serde_json::from_str::<ToolPermissionsConfig>(raw) {
                Ok(channel_perms) => merged = ToolPermissionsConfig::merge(&merged, &channel_perms),
                Err(e) => warn!(
                    run_id = %request.run_id,
                    error = %e,
                    "Malformed channel tool_permissions metadata — channel layer skipped"
                ),
            }
        }
        let is_all_default = merged.default == crate::extension::PermissionAction::Allow
            && merged.overrides.is_empty();
        info!(
            run_id = %request.run_id,
            exec_tier = tier.id(),
            default = ?merged.default,
            overrides = merged.overrides.len(),
            "Execution permissions resolved for this turn"
        );
        (tier, (!is_all_default).then_some(merged))
    }

    /// Per-session execution tier, carried in the session's identity metadata
    /// under `custom["exec_tier"]` (same carrier as `custom["project_root"]`,
    /// written through the existing `sessions.patch` RPC).
    ///
    /// A malformed or unknown value is ignored — the turn falls back to the
    /// global tier rather than failing.
    async fn session_exec_tier(
        &self,
        session_key: &crate::gateway::router::SessionKey,
    ) -> Option<ExecTier> {
        use crate::config::types::policies::EXEC_TIER_SESSION_KEY;

        let store = self.session_manager.as_ref()?;
        let meta = match store.get_metadata(session_key).await {
            Ok(meta) => meta?,
            Err(e) => {
                warn!(error = %e, "Failed to read session metadata — session exec tier skipped");
                return None;
            }
        };
        let raw = meta
            .identity_meta?
            .custom
            .get(EXEC_TIER_SESSION_KEY)?
            .as_str()?
            .to_string();
        match ExecTier::from_id(&raw) {
            Some(tier) => Some(tier),
            None => {
                warn!(
                    value = %raw,
                    "Unknown session exec_tier — falling back to the global tier"
                );
                None
            }
        }
    }

    /// Stamp a request-carried tier onto the session, so the choice outlives
    /// the turn that carried it (later turns send nothing; a page reload reads
    /// the session back). Best-effort: a store failure must not fail the run —
    /// the tier for THIS turn is already resolved and enforced either way.
    async fn persist_session_exec_tier(
        &self,
        session_key: &crate::gateway::router::SessionKey,
        tier: ExecTier,
    ) {
        use crate::config::types::policies::EXEC_TIER_SESSION_KEY;
        use crate::gateway::session_store::types::SessionPatch;

        let Some(store) = self.session_manager.as_ref() else {
            return;
        };
        let patch = SessionPatch {
            metadata: Some(serde_json::json!({ EXEC_TIER_SESSION_KEY: tier.id() })),
            ..Default::default()
        };
        if let Err(e) = store.patch_session(session_key, &patch).await {
            warn!(error = %e, tier = tier.id(), "Failed to persist session exec tier");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_exec_tier;
    use crate::config::types::policies::ExecTier;

    #[test]
    fn request_tier_outranks_session_and_global() {
        assert_eq!(
            resolve_exec_tier(
                ExecTier::Auto,
                Some(ExecTier::Ask),
                Some(ExecTier::Full),
                None
            ),
            ExecTier::Ask
        );
    }

    #[test]
    fn session_tier_outranks_global_when_the_request_carries_none() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Auto, None, Some(ExecTier::Ask), None),
            ExecTier::Ask
        );
    }

    #[test]
    fn global_tier_governs_when_neither_request_nor_session_names_one() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Ask, None, None, None),
            ExecTier::Ask
        );
    }

    /// The behavior two doc comments used to disagree about — pin it. An
    /// explicit per-chat choice REPLACES the global tier and may raise it.
    #[test]
    fn an_explicit_tier_may_raise_above_the_global_tier() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Ask, None, Some(ExecTier::Full), None),
            ExecTier::Full
        );
        assert_eq!(
            resolve_exec_tier(ExecTier::Ask, Some(ExecTier::Full), None, None),
            ExecTier::Full
        );
    }

    /// The clamp runs AFTER the three rungs, not before: a chat-tier channel
    /// that asks for Full lands on Auto even though Full won the precedence
    /// contest. This is what keeps the raise above from being a hole.
    #[test]
    fn the_channel_clamp_applies_after_resolution_not_before() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Auto, Some(ExecTier::Full), None, Some("guest")),
            ExecTier::Auto
        );
        assert_eq!(
            resolve_exec_tier(ExecTier::Auto, None, Some(ExecTier::Full), Some("guest")),
            ExecTier::Auto
        );
        // The clamp is a ceiling, not an override: a chat channel that asks for
        // Ask still gets Ask.
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, Some(ExecTier::Ask), None, Some("guest")),
            ExecTier::Ask
        );
    }

    #[test]
    fn operator_and_absent_roles_are_not_clamped() {
        // Config-tier channel: an operator surface, runs at the resolved tier.
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, None, None, Some("operator")),
            ExecTier::Full
        );
        // Panel / CLI / cron carry no role at all.
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, None, None, None),
            ExecTier::Full
        );
        // An operator may still raise above their own global dial, because the
        // ceiling is theirs: clamping an operator to their own configuration
        // would turn a default into a cage.
        assert_eq!(
            resolve_exec_tier(ExecTier::Ask, Some(ExecTier::Full), None, Some("operator")),
            ExecTier::Full
        );
        assert_eq!(
            resolve_exec_tier(ExecTier::Ask, Some(ExecTier::Full), None, None),
            ExecTier::Full
        );
    }

    /// An unrecognized role word is not an operator, so it is ceilinged like a
    /// member. This flipped on 2026-08-08 and the flip is the point: the old
    /// pin asserted "an unknown role is not a channel — no clamp, same as no
    /// role", which put every future role word on the *unbounded* side of the
    /// branch by default. `None` (no role at all) means local/internal and is
    /// trusted; a role STRING nobody recognizes means a caller we cannot place,
    /// and those two must not share an answer.
    #[test]
    fn an_unrecognized_role_is_ceilinged_like_any_other_non_operator() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Ask, Some(ExecTier::Full), None, Some("bogus")),
            ExecTier::Ask
        );
    }

    /// Final-review minor: the non-operator default is keyed on the repo-wide
    /// `role_is_operator` predicate, not a `== Some("member")` equality test.
    /// The spelling matters for exactly the roles that are neither: `"guest"`
    /// (what chat-tier channels default to) and anything unrecognized used to
    /// land on the OPERATOR side of the branch and inherit an operator's
    /// configured `Full`. Now they fail closed to `Ask`, like a member.
    #[test]
    fn every_non_operator_role_defaults_to_ask_not_just_the_literal_member() {
        for role in ["member", "guest", "bogus", ""] {
            assert_eq!(
                resolve_exec_tier(ExecTier::Full, None, None, Some(role)),
                ExecTier::Ask,
                "role {role:?} is not an operator and must not inherit the global tier"
            );
        }
        // The two operator spellings are unaffected, byte-identical to before.
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, None, None, Some("operator")),
            ExecTier::Full
        );
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, None, None, None),
            ExecTier::Full
        );
    }

    // -------------------------------------------------------------------
    // P1 member hardening (Task 9): member default tier = Ask.
    // -------------------------------------------------------------------

    /// A member with no explicit pick anywhere (this request and no
    /// persisted session choice) falls back to `Ask`, not the operator-
    /// configured global tier — the actual defect this task closes.
    #[test]
    fn member_with_no_explicit_tier_defaults_to_ask() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, None, None, Some("member")),
            ExecTier::Ask
        );
        assert_eq!(
            resolve_exec_tier(ExecTier::Auto, None, None, Some("member")),
            ExecTier::Ask
        );
    }

    /// Operator resolves the global tier exactly as before — byte-identical.
    #[test]
    fn operator_with_no_explicit_tier_still_resolves_the_global_tier() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, None, None, Some("operator")),
            ExecTier::Full
        );
    }

    /// This is a DEFAULT, not a clamp: an explicit member pick (composer
    /// pill, landing in `requested`) still wins over the `Ask` default and
    /// may raise above it — **up to the install's own posture**.
    #[test]
    fn a_members_explicit_pick_still_wins_over_the_member_default() {
        // Global `Full` — the operator has said the tier axis gates nothing
        // here, so the member's pick stands.
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, Some(ExecTier::Auto), None, Some("member")),
            ExecTier::Auto
        );
    }

    /// Same for a session-stored tier from an earlier explicit pick: it
    /// outranks the member default exactly like it outranks the global one.
    #[test]
    fn a_members_stored_session_tier_still_wins_over_the_member_default() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Auto, None, Some(ExecTier::Auto), Some("member")),
            ExecTier::Auto
        );
    }

    // -------------------------------------------------------------------
    // Round 2 (2026-08-08): the non-operator ceiling.
    // -------------------------------------------------------------------

    /// The defect these pins close. `chat.send { exec_tier: "full" }` is a
    /// plain per-request parameter (`handlers/agent.rs`, rejected only if the
    /// id is unknown), and it landed in `requested`, which outranked
    /// everything. The clamp the doc named as the thing bounding members —
    /// `channel_permission_level_from_role` — returns `None` for `"member"`,
    /// so it never ran. A member could turn the tier off entirely.
    #[test]
    fn a_member_cannot_raise_above_the_operators_global_tier() {
        for global in [ExecTier::Ask, ExecTier::Auto] {
            assert_eq!(
                resolve_exec_tier(global, Some(ExecTier::Full), None, Some("member")),
                global,
                "a member asking for Full under a global {global:?} must land on \
                 {global:?} — the tier axis is the operator's to set"
            );
            // The stored rung is the same door with a longer hinge: pick Full
            // once and every later turn reads it back from the session row.
            assert_eq!(
                resolve_exec_tier(global, None, Some(ExecTier::Full), Some("member")),
                global
            );
        }
    }

    /// The ceiling only ever tightens. A member who deliberately picks a
    /// STRICTER tier than the install's keeps it — arming your own gate is
    /// never something a ceiling should undo.
    #[test]
    fn the_ceiling_never_raises_a_members_own_stricter_choice() {
        assert_eq!(
            resolve_exec_tier(ExecTier::Full, Some(ExecTier::Ask), None, Some("member")),
            ExecTier::Ask
        );
        assert_eq!(
            resolve_exec_tier(ExecTier::Auto, Some(ExecTier::Ask), None, Some("member")),
            ExecTier::Ask
        );
    }

    /// An operator's own resolution is byte-identical to before the ceiling
    /// existed — including the case where they raise above their global dial.
    #[test]
    fn the_ceiling_is_invisible_to_operators_and_to_local_callers() {
        for role in [Some("operator"), None] {
            for global in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
                for pick in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
                    assert_eq!(
                        resolve_exec_tier(global, Some(pick), None, role),
                        pick,
                        "role {role:?} must resolve its explicit pick unchanged"
                    );
                }
                assert_eq!(resolve_exec_tier(global, None, None, role), global);
            }
        }
    }
}
