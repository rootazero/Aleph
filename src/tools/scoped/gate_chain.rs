//! The ordered rule chain that decides whether a tool call runs, is refused,
//! or stops for a human — and **which rule decided**.
//!
//! [`super::dispatch`] used to answer that question with a bare disjunction:
//!
//! ```ignore
//! if self.inner.requires_confirmation(name)
//!     || self.is_permission_ask(name)
//!     || self.tier_asks_for_arguments(name, input) { … }
//! ```
//!
//! A boolean is enough to *stop* the call and not enough to *explain* it. Every
//! surface downstream — the card the operator reads, the error the model reads,
//! the signed ledger row an auditor reads — got the same sentence ("Tool `x`
//! requires your confirmation to run"), which is true of all three arms and
//! actionable for none. That gap is not cosmetic:
//!
//! * `vault_store` under `exec_tier = "full"` still raises a card, because
//!   [`LoopTool::requires_confirmation`] is a floor no tier and no explicit
//!   `allow` can lower. The old sentence invited exactly the wrong repair
//!   ("set it to allow and it will stop asking"), and this repo has already
//!   had to fix three *other* copies of that same claim (see the `Full` variant
//!   doc and `approval_prompt_line`).
//! * A glob the operator forgot about (`"*_delete" = "ask"`) is indistinguishable
//!   from the tier speaking, so "why does it keep asking me" is a three-file
//!   investigation the card could have answered in one line.
//!
//! So the chain is named. [`GateRule`] is the *one* enumeration of the reasons a
//! call can be gated, [`GateRule::reason`] is the *one* prose rendering of each,
//! and [`GateRule::id`] is the stable token the ledger and the tests key on.
//!
//! ## Order
//!
//! [`ScopedToolService::confirmation_rule`] returns the FIRST matching rule, and
//! the order is chosen by one criterion: **a reason must never mislead the
//! reader about what would change the outcome.** So the immovable floor is
//! reported first, the argument-level filter (which is about *this call*, not
//! the tool) next, the operator's own entry next, and the tier — the most
//! general statement — last. Two of the four pairs are mutually exclusive by
//! construction (see the tests), so the ordering only actually arbitrates
//! between a declared gate and everything else.
//!
//! ## Not a policy engine
//!
//! This module adds no rule and removes none. It is a pure, synchronous
//! classification of predicates that already existed, in the same order they
//! already ran, so the set of gated calls is byte-identical. R7 is untouched:
//! nothing here judges intent, and every input is a declared fact
//! (`is_idempotent`, `requires_confirmation`, `destructiveHint`) or a config
//! entry a human wrote.
//!
//! [`LoopTool::requires_confirmation`]: crate::tools::runtime::LoopTool::requires_confirmation

use serde_json::Value;

use crate::extension::PermissionAction;

use super::ScopedToolService;

/// Why a tool call stopped. One variant per rule in the chain.
///
/// `Copy` on purpose: the matched override key is borrowed from the merged
/// [`ToolPermissionsConfig`] the service already owns, so classifying a call
/// allocates nothing — only [`Self::reason`] does, and only when a card or an
/// error is actually being built.
///
/// [`ToolPermissionsConfig`]: crate::config::types::policies::ToolPermissionsConfig
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GateRule<'a> {
    /// The merged `[policies.tool_permissions]` denies the tool outright.
    /// The only rule in the chain that refuses instead of asking.
    PolicyDeny {
        /// The override key that denied it, as written in the config.
        pattern: &'a str,
    },
    /// The tool declares its own confirmation gate — `CONFIRMATION_REQUIRED_TOOLS`
    /// for builtins, an MCP server's `destructiveHint` for external tools.
    ///
    /// Reported before every other ask-rule because it is the only one no
    /// configuration can switch off: it is read independently of the exec tier
    /// and of any explicit `allow`. A reason that named a softer rule here
    /// would send the reader to a setting that will not help them.
    ToolDeclared,
    /// This *call's arguments* trip the tier's destructive-argument filter
    /// (`file_ops` delete/move, a `loop_graph` write touching `root:`/`frozen:`,
    /// a `self_config` write that could remove the gate itself).
    ///
    /// Distinct from every other rule in the chain because it is about the
    /// arguments, not the tool: the same tool with different arguments runs
    /// without a card.
    DestructiveArguments,
    /// This call can reach the configuration that decides whether the approval
    /// gates fire at all — a `self_config` write intersecting
    /// `policies.tool_permissions` / `policies.exec_tier`, or a config
    /// rollback.
    ///
    /// The second **floor**: like [`Self::ToolDeclared`] it asks under every
    /// tier including `full`, so its reason must not point the reader at a
    /// setting, and its card must not offer to retire it
    /// ([`crate::exec::allowed_decisions::for_confirm_gate`]). Reported before
    /// [`Self::DestructiveArguments`] for that reason alone: the two are the
    /// same predicate family, but only one of them is removable, and naming
    /// the removable one would invite a repair that cannot work.
    GateRemoval,
    /// An explicit `[policies.tool_permissions]` entry resolves the tool to
    /// `ask`. The operator named it — by exact name or by glob — and no tier
    /// that ASKS gets a word in (see
    /// [`effective_permission`](crate::config::types::policies::effective_permission)).
    ///
    /// One tier does: `Plan` refuses rather than asks, and its refusal is rung
    /// 0 — above the entry, not beneath it. So a mutating tool carrying an
    /// explicit `ask` arrives here as [`Self::PlanMode`] while planning and as
    /// `PolicyAsk` at every other tier. That is the honest split: quoting the
    /// entry during a planning turn would point the reader at a setting whose
    /// edit changes nothing until the plan is approved.
    PolicyAsk {
        /// The override key that asked, as written in the config.
        pattern: &'a str,
        /// `false` when a glob matched. Worth saying out loud: a `"*" = "ask"`
        /// blanket looks nothing like a decision about this tool when you are
        /// staring at the card.
        exact: bool,
    },
    /// Nobody named this tool, and the execution tier raised the configured
    /// default to `ask` from the tool's DECLARED metadata (not from its name).
    TierRaised,
    /// The tool changes Aleph's own configuration and the requesting connection
    /// is not operator-tier, so the call is escalated to whoever is
    /// (`check_operator_gate`).
    ///
    /// Never returned by [`ScopedToolService::confirmation_rule`] — it is
    /// decided one gate earlier, from the caller's role rather than from the
    /// tool's facts — but it lives in this enum for the reason the enum exists:
    /// it is a reason a call can stop, its sentence reaches a human and a model,
    /// and the trail has to be able to name it. It was the one gate whose prose
    /// was still hand-written at its call site. Same shape as
    /// [`Self::PolicyDeny`], which [`ScopedToolService::deny_rule`] answers.
    OperatorRequired,
    /// The conversation is PLANNING ([`ExecTier::Plan`]) and this call is not
    /// a declared read.
    ///
    /// The second rule in the chain that REFUSES rather than asks, and the
    /// only one that is temporary: the exact same call runs the moment a human
    /// approves the plan. That is why it needs its own name — reported as
    /// [`Self::PolicyDeny`] it would send the reader to a
    /// `[policies.tool_permissions]` entry that does not exist, and send the
    /// MODEL to a dead end instead of to the two-call handoff that is sitting
    /// right there.
    ///
    /// [`ExecTier::Plan`]: crate::config::types::policies::ExecTier::Plan
    PlanMode,
    /// A `BeforeToolCall` interceptor answered `Ask` for this call.
    ///
    /// Also outside [`ScopedToolService::confirmation_rule`] — the decision
    /// belongs to a plugin, not to the tool facts — and here for the same
    /// reason as [`Self::OperatorRequired`]: the trail has to be able to say
    /// that a *hook* stopped this call and not the tier, which is the first
    /// thing an operator wants to know when a card appears for a tool their
    /// configuration allows.
    HookRequested,
}

impl GateRule<'_> {
    /// Stable token for logs, ledger details and tests. Never localized and
    /// never reworded — a surface that wants different words uses [`Self::reason`],
    /// and a test that wants to pin the chain uses this.
    pub(super) const fn id(self) -> &'static str {
        match self {
            Self::PolicyDeny { .. } => "policy_deny",
            // One spelling, shared with the decision-set derivation that must
            // refuse to offer a persistent grant on this rule's card
            // (`exec::allowed_decisions::for_confirm_gate`). A rename here is a
            // compile error there, not a silently widened button set.
            Self::ToolDeclared => crate::exec::allowed_decisions::DECLARED_FLOOR_RULE,
            Self::DestructiveArguments => "destructive_arguments",
            // Second spelling shared with the decision-set derivation, same as
            // `ToolDeclared` above: a rename here is a compile error there.
            Self::GateRemoval => crate::exec::allowed_decisions::GATE_REMOVAL_RULE,
            Self::PolicyAsk { .. } => "policy_ask",
            Self::TierRaised => "tier_raised",
            Self::PlanMode => "plan_mode",
            Self::OperatorRequired => "operator_required",
            Self::HookRequested => "hook_requested",
        }
    }

    /// Whether this rule is a **floor**: a gate no execution tier and no
    /// `allow` entry can lower. Read for exactly one thing — whether the card
    /// may offer a persistent grant.
    ///
    /// Deliberately `#[cfg(test)]` and deliberately an exhaustive `match`.
    /// Production asks
    /// [`allowed_decisions::rule_is_a_floor`](crate::exec::allowed_decisions::rule_is_a_floor)
    /// — the one derivation, which the gate reaches through
    /// [`Self::id`] — so a second production predicate here would be the second
    /// source that `FLOOR_RULES` exists to avoid. What this *is* for is the
    /// forcing function: adding a variant is a compile error on this `match`,
    /// and `every_floor_variant_is_declared_a_floor_on_both_sides` then makes
    /// the two answers agree. A `matches!` would have quietly answered `false`
    /// for the new one, which is the shape of every enumeration that goes
    /// stale (判据 §0).
    #[cfg(test)]
    pub(super) const fn is_floor(self) -> bool {
        match self {
            Self::ToolDeclared | Self::GateRemoval => true,
            // `PlanMode` is emphatically NOT a floor **in this sense**: it is
            // the one rule in the chain that a single human "approved" retires
            // for the rest of the turn. (It also never reaches a card — it
            // refuses — so the persistent-grant question it would otherwise
            // answer is moot.)
            //
            // ⚠️ "Floor" means two different things in this subsystem and they
            // are not in conflict. HERE it means "no card this rule raises may
            // offer a persistent grant" — `PlanMode` raises no card, and its
            // exit is one human `approved`, so `false` is right. In
            // `effective_permission` it means "rung 0: an explicit
            // `[policies.tool_permissions]` entry may not outrank this" — and
            // there the plan denial IS the floor, which is what lets
            // `denied_only_by_plan` above resolve `true` for a tool carrying an
            // explicit `allow` instead of never being asked at all. One rule,
            // two axes: unrelaxable by config, retired by a person.
            Self::PolicyDeny { .. }
            | Self::DestructiveArguments
            | Self::PolicyAsk { .. }
            | Self::TierRaised
            | Self::PlanMode
            | Self::OperatorRequired
            | Self::HookRequested => false,
        }
    }

    /// One sentence naming the rule and — where one exists — the knob that
    /// would change it. Rendered into the approval card the human reads and
    /// into the refusal the model reads, from this single source, so the two
    /// audiences are never told different stories about the same gate.
    ///
    /// English on purpose: this text reaches the model every time it reaches a
    /// person, and model-facing prompt text in this repo is always English
    /// (same rule as [`ExecTier::approval_prompt_line`]). A surface that wants
    /// to localize the card keys off [`Self::id`].
    ///
    /// [`ExecTier::approval_prompt_line`]: crate::config::types::policies::ExecTier::approval_prompt_line
    pub(super) fn reason(self, tool: &str) -> String {
        match self {
            Self::PolicyDeny { pattern } => format!(
                "`{tool}` is denied by `{pattern}` in the merged tool permission policy \
                 (`[policies.tool_permissions]`, global → agent → channel, most restrictive \
                 wins)."
            ),
            Self::ToolDeclared => format!(
                "`{tool}` declares its own confirmation gate, so it asks under every \
                 execution tier — including `full` — and an `allow` entry does not \
                 switch it off."
            ),
            Self::DestructiveArguments => format!(
                "These specific arguments to `{tool}` are irreversible or touch protected \
                 state, so this call stops even though the tool itself runs freely. Naming \
                 `{tool}` explicitly in `[policies.tool_permissions.overrides]` stands this \
                 filter down."
            ),
            Self::GateRemoval => format!(
                "This `{tool}` call can change the settings that decide which tool calls \
                 stop for you (`policies.tool_permissions` / `policies.exec_tier`), so it \
                 asks under every execution tier — including `full`. Raising the tier does \
                 not stand it down: the tier lasts one turn and this change would outlive it."
            ),
            Self::PolicyAsk {
                pattern,
                exact: true,
            } => format!("`[policies.tool_permissions]` sets `{pattern}` = ask."),
            Self::PolicyAsk {
                pattern,
                exact: false,
            } => format!(
                "`[policies.tool_permissions]` sets the pattern `{pattern}` = ask, which \
                 matches `{tool}`."
            ),
            Self::TierRaised => format!(
                "The execution tier stops for `{tool}` because the tool does not declare \
                 itself read-only. Nothing in `[policies.tool_permissions]` names it, so \
                 the tier decides."
            ),
            Self::PlanMode => format!(
                "This conversation is PLANNING, so `{tool}` does not run yet — nothing \
                 that mutates does. Finish the plan instead: write the objective and an \
                 ordered checklist with `scratchpad` (`set_objective` + `set_plan`), then \
                 call `scratchpad` with `action='request_approval'`. If the user approves, \
                 planning ends immediately and `{tool}` runs on your next call."
            ),
            Self::OperatorRequired => format!(
                "A chat-tier device asked to run `{tool}`, which changes Aleph's own \
                 configuration. Approve to allow this change."
            ),
            // The only rule whose sentence is not authored here: a hook's
            // `Ask` carries the plugin author's own words, and replacing them
            // with a generic line would throw away the only thing that says
            // WHICH hook and why. The `{tool}` suffix keeps the card readable
            // when the plugin wrote a bare phrase.
            Self::HookRequested => {
                format!("A `BeforeToolCall` hook asked for confirmation before `{tool}` runs.")
            }
        }
    }

    /// What the model should DO about a refusal, appended to [`Self::reason`]
    /// on the deny path.
    ///
    /// This sentence used to be a literal at the one call site, which was fine
    /// while that path had exactly one rule to explain. It has two now, and
    /// they want opposite advice: an operator's `deny` is permanent and the
    /// only way through it is a human editing config, while plan mode is one
    /// approval away and retrying is exactly right *after* that approval. A
    /// blanket "do not retry" on the second one would tell the model to give
    /// up on the work it just got permission to do.
    /// Exhaustive on purpose, with no catch-all: only two variants reach the
    /// deny path today, and the wrong half of this fork is a sentence that
    /// tells the model to give up on work it just got permission to do. A new
    /// refusing rule should be a compile error here, not a silent inheritance
    /// of "go edit your config".
    pub(super) const fn deny_advice(self) -> &'static str {
        match self {
            Self::PlanMode => {
                " Do not retry this call before the plan is approved — request approval first."
            }
            Self::PolicyDeny { .. }
            | Self::ToolDeclared
            | Self::DestructiveArguments
            | Self::GateRemoval
            | Self::PolicyAsk { .. }
            | Self::TierRaised
            | Self::OperatorRequired
            | Self::HookRequested => {
                " Do not retry; ask the user to adjust `[policies.tool_permissions]` if this \
                 tool is needed."
            }
        }
    }
}

impl ScopedToolService {
    /// The rule that gates this call, or `None` when it may run unasked.
    ///
    /// The ordered chain. Equivalent to the disjunction it replaced — same
    /// predicates, same order, same set of gated calls — and additionally says
    /// which arm matched. See the module doc for why the order is what it is.
    pub(super) fn confirmation_rule<'a>(
        &'a self,
        name: &str,
        input: &Value,
    ) -> Option<GateRule<'a>> {
        // 1. The floor. Read independently of the tier and of any explicit
        //    `allow`, exactly as `check_confirmation_gate` has always read it.
        if self.inner.requires_confirmation(name) {
            return Some(GateRule::ToolDeclared);
        }
        // 2. The second floor: a write that can retire the gates themselves.
        //    Read before rule 3 because `tier_asks_for_arguments` answers `true`
        //    for it as well (the floor is folded into the tier predicate), and
        //    the two must not be reported with the same sentence — one names a
        //    tier setting that would change the outcome, the other names one
        //    that would not.
        if self.gate_removal_floor(name, input) {
            return Some(GateRule::GateRemoval);
        }
        // 3. This call's arguments. Already stands down for an exactly-named
        //    tool (`tier_asks_for_arguments`), which is what keeps it and rule 4
        //    mutually exclusive.
        if self.tier_asks_for_arguments(name, input) {
            return Some(GateRule::DestructiveArguments);
        }
        // 4/5. `Ask` from the merged policy — either because an entry says so,
        //      or because the tier tightened the default. `permission_for` is
        //      the chokepoint that composes them; `explain` says which one it
        //      was, without recomputing the composition.
        if self.permission_for(name) != PermissionAction::Ask {
            return None;
        }
        Some(match self.explicit_match(name) {
            Some(m) if m.action == PermissionAction::Ask => GateRule::PolicyAsk {
                pattern: m.pattern,
                exact: m.exact,
            },
            // An explicit entry that is NOT `ask` cannot have produced this
            // `Ask` (the tier is skipped entirely when an entry names the tool),
            // so the tier is the only remaining author.
            _ => GateRule::TierRaised,
        })
    }

    /// The rule behind a hard refusal, for the one gate that refuses rather
    /// than asking. `None` when the tool is not denied.
    ///
    /// Separate from [`Self::confirmation_rule`] because it answers a different
    /// question at a different point in `execute_inner`, and collapsing the two
    /// would make every ask-path caller pattern-match a variant it can never
    /// see.
    pub(super) fn deny_rule<'a>(&'a self, name: &str) -> Option<GateRule<'a>> {
        if self.permission_for(name) != PermissionAction::Deny {
            return None;
        }
        // A denial the PLAN tier created is reported as itself. Checked before
        // the policy arm because the policy arm is unconditionally true here
        // and would otherwise quote `pattern: "default"` — a config entry the
        // operator never wrote, pointing at a fix that cannot work. The
        // predicate is derived by difference (`denied_only_by_plan`), so an
        // operator's real `deny` on the same tool keeps reporting as one.
        if self.denied_only_by_plan(name) {
            return Some(GateRule::PlanMode);
        }
        Some(GateRule::PolicyDeny {
            // A `Deny` with no explicit entry came from `default = "deny"`;
            // quote that rather than inventing a pattern that is not in the
            // config.
            pattern: self
                .explicit_match(name)
                .filter(|m| m.action == PermissionAction::Deny)
                .map_or("default", |m| m.pattern),
        })
    }

    /// The merged policy's explicit entry for `name`, if any — the borrow the
    /// two functions above quote. `None` when no policy is attached at all,
    /// which is the common case and costs one `Option` check.
    fn explicit_match<'a>(
        &'a self,
        name: &str,
    ) -> Option<crate::config::types::policies::PermissionMatch<'a>> {
        self.tool_permissions.as_ref()?.explain(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::policies::{ExecTier, ToolPermissionsConfig};
    use crate::tools::runtime::{LoopTool, LoopToolRegistry, ToolResult};
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeSet;
    use tokio_util::sync::CancellationToken;

    /// A tool that declares exactly the two facts the chain reads.
    struct Declared {
        name: &'static str,
        idempotent: bool,
        confirm: bool,
    }

    #[async_trait]
    impl LoopTool for Declared {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "test"
        }
        fn schema(&self) -> Value {
            json!({"type": "object"})
        }
        fn is_idempotent(&self) -> bool {
            self.idempotent
        }
        fn requires_confirmation(&self) -> bool {
            self.confirm
        }
        async fn execute(&self, _input: Value, _cancel: CancellationToken) -> ToolResult {
            ToolResult::Success { output: json!({}) }
        }
    }

    fn service(
        tools: Vec<Declared>,
        tier: ExecTier,
        perms: Option<ToolPermissionsConfig>,
    ) -> ScopedToolService {
        let mut registry = LoopToolRegistry::new();
        for t in tools {
            registry.register(Box::new(t));
        }
        let mut svc =
            ScopedToolService::new(crate::sync_primitives::Arc::new(registry), BTreeSet::new())
                .with_exec_tier(tier);
        if let Some(p) = perms {
            svc = svc.with_tool_permissions(p);
        }
        svc
    }

    fn perms(
        default: PermissionAction,
        entries: &[(&str, PermissionAction)],
    ) -> ToolPermissionsConfig {
        ToolPermissionsConfig {
            default,
            overrides: entries
                .iter()
                .map(|(k, v)| ((*k).to_string(), *v))
                .collect(),
        }
    }

    /// The floor outranks everything: a tool that declares its own gate reports
    /// `tool_declared` even when an explicit `ask` entry ALSO names it. The
    /// alternative would hand the reader a setting that cannot help them.
    #[test]
    fn a_declared_gate_outranks_an_explicit_ask_entry() {
        let svc = service(
            vec![Declared {
                name: "vault_store",
                idempotent: false,
                confirm: true,
            }],
            ExecTier::Auto,
            Some(perms(
                PermissionAction::Allow,
                &[("vault_store", PermissionAction::Ask)],
            )),
        );
        let rule = svc.confirmation_rule("vault_store", &json!({})).unwrap();
        assert_eq!(rule.id(), "tool_declared");
        assert!(
            rule.reason("vault_store").contains("including `full`"),
            "the floor must say it survives the tier"
        );
    }

    /// …and even when the operator explicitly ALLOWED it. This is the case the
    /// three now-corrected copies of the `Full` claim were about.
    #[test]
    fn a_declared_gate_survives_an_explicit_allow() {
        let svc = service(
            vec![Declared {
                name: "vault_store",
                idempotent: false,
                confirm: true,
            }],
            ExecTier::Full,
            Some(perms(
                PermissionAction::Allow,
                &[("vault_store", PermissionAction::Allow)],
            )),
        );
        assert_eq!(
            svc.confirmation_rule("vault_store", &json!({}))
                .map(GateRule::id),
            Some("tool_declared")
        );
    }

    /// The argument filter names itself, and says the call — not the tool — is
    /// what stopped.
    #[test]
    fn destructive_arguments_are_reported_as_such() {
        let svc = service(
            vec![Declared {
                name: "file_ops",
                idempotent: false,
                confirm: false,
            }],
            ExecTier::Auto,
            None,
        );
        assert_eq!(
            svc.confirmation_rule("file_ops", &json!({"operation": "delete"}))
                .map(GateRule::id),
            Some("destructive_arguments")
        );
        // Same tool, harmless arguments: no rule at all.
        assert_eq!(
            svc.confirmation_rule("file_ops", &json!({"operation": "list"})),
            None
        );
    }

    /// An exactly-named entry stands the argument filter down (round 11), so
    /// these two rules can never both match — the ordering between them is
    /// documentation, not arbitration.
    #[test]
    fn an_exact_entry_replaces_the_argument_filter_rather_than_racing_it() {
        let svc = service(
            vec![Declared {
                name: "file_ops",
                idempotent: false,
                confirm: false,
            }],
            ExecTier::Auto,
            Some(perms(
                PermissionAction::Allow,
                &[("file_ops", PermissionAction::Ask)],
            )),
        );
        let rule = svc
            .confirmation_rule("file_ops", &json!({"operation": "delete"}))
            .unwrap();
        assert_eq!(rule.id(), "policy_ask");
        assert!(rule.reason("file_ops").contains("`file_ops` = ask"));
    }

    /// A glob says so: the reason quotes the pattern, because "I never
    /// configured this tool" is the reader's default and it is wrong here.
    #[test]
    fn a_glob_entry_quotes_the_pattern_it_matched() {
        let svc = service(
            vec![Declared {
                name: "github__delete_repo",
                idempotent: false,
                confirm: false,
            }],
            ExecTier::Full,
            Some(perms(
                PermissionAction::Allow,
                &[("github__*", PermissionAction::Ask)],
            )),
        );
        let rule = svc
            .confirmation_rule("github__delete_repo", &json!({}))
            .unwrap();
        assert_eq!(rule.id(), "policy_ask");
        let reason = rule.reason("github__delete_repo");
        assert!(reason.contains("github__*"), "{reason}");
    }

    /// Nobody named it and the tier spoke — the one rule whose repair is "change
    /// the tier", so it must not be confused with the policy rules.
    #[test]
    fn the_tier_names_itself_when_no_entry_matches() {
        let svc = service(
            vec![Declared {
                name: "some_mcp__write",
                idempotent: false,
                confirm: false,
            }],
            ExecTier::Ask,
            None,
        );
        assert_eq!(
            svc.confirmation_rule("some_mcp__write", &json!({}))
                .map(GateRule::id),
            Some("tier_raised")
        );
        // A declared read-only tool is not gated by `Ask` at all.
        let svc = service(
            vec![Declared {
                name: "some_mcp__read",
                idempotent: true,
                confirm: false,
            }],
            ExecTier::Ask,
            None,
        );
        assert_eq!(svc.confirmation_rule("some_mcp__read", &json!({})), None);
    }

    /// The chain agrees with the chokepoint it explains. Any call the chain
    /// classifies must be `Ask`-or-declared at `permission_for`, and any call
    /// `permission_for` puts at `Ask` must get a rule — otherwise a gate would
    /// fire with no reason, or a reason would exist for a gate that never fires.
    #[test]
    fn the_chain_and_the_chokepoint_never_disagree() {
        // Every variant, `Plan` included: it refuses rather than asks, so the
        // interesting half of the equality there is that it produces NO
        // confirmation rule while `permission_for` says `Deny` — a card for a
        // call that will never run is exactly the disagreement this pins.
        for tier in [
            ExecTier::Plan,
            ExecTier::Ask,
            ExecTier::Auto,
            ExecTier::Full,
        ] {
            for confirm in [false, true] {
                for idempotent in [false, true] {
                    let svc = service(
                        vec![Declared {
                            name: "t",
                            idempotent,
                            confirm,
                        }],
                        tier,
                        None,
                    );
                    let gated = svc.confirmation_rule("t", &json!({})).is_some();
                    let chokepoint = svc.permission_for("t") == PermissionAction::Ask || confirm;
                    assert_eq!(
                        gated, chokepoint,
                        "tier={tier:?} confirm={confirm} idempotent={idempotent}"
                    );
                }
            }
        }
    }

    /// A `Deny` from the configured default has no pattern to quote; saying
    /// `default` beats inventing a config line the operator never wrote.
    #[test]
    fn a_default_deny_quotes_the_default_not_a_pattern() {
        let svc = service(
            vec![Declared {
                name: "t",
                idempotent: false,
                confirm: false,
            }],
            ExecTier::Auto,
            Some(perms(PermissionAction::Deny, &[])),
        );
        let rule = svc.deny_rule("t").unwrap();
        assert_eq!(rule.id(), "policy_deny");
        assert!(rule.reason("t").contains("`default`"));
        // A denied tool is a refusal, never an ask — the two chains are disjoint.
        assert_eq!(svc.confirmation_rule("t", &json!({})), None);
    }

    #[test]
    fn a_denying_glob_is_quoted_verbatim() {
        let svc = service(
            vec![Declared {
                name: "agent_delete",
                idempotent: false,
                confirm: false,
            }],
            ExecTier::Auto,
            Some(perms(
                PermissionAction::Allow,
                &[("*_delete", PermissionAction::Deny)],
            )),
        );
        assert!(svc
            .deny_rule("agent_delete")
            .unwrap()
            .reason("agent_delete")
            .contains("*_delete"));
        assert_eq!(svc.deny_rule("agent_create"), None);
    }

    /// Every rule renders a non-empty sentence that names the tool, and every
    /// id is distinct. Cheap, and it is what stops a new variant shipping with
    /// an empty card.
    #[test]
    fn every_rule_renders_and_has_a_distinct_id() {
        let rules = [
            GateRule::PolicyDeny { pattern: "*" },
            GateRule::ToolDeclared,
            GateRule::GateRemoval,
            GateRule::DestructiveArguments,
            GateRule::PolicyAsk {
                pattern: "bash",
                exact: true,
            },
            GateRule::PolicyAsk {
                pattern: "b*",
                exact: false,
            },
            GateRule::TierRaised,
            GateRule::OperatorRequired,
            GateRule::HookRequested,
        ];
        let mut ids = std::collections::HashSet::new();
        for rule in rules {
            let text = rule.reason("bash");
            assert!(text.contains("bash"), "{text}");
            assert!(text.ends_with('.'), "{text}");
            ids.insert(rule.id());
        }
        // Both `PolicyAsk` shapes share one id — they are one rule with two
        // renderings, which is the point of separating `id` from `reason`.
        assert_eq!(ids.len(), rules.len() - 1);
    }

    /// The two halves of "is this a floor" live in two crates-worth of module
    /// apart — the chain names the rule, `exec::allowed_decisions` decides what
    /// the card may offer — and a floor that is only declared on one side ships
    /// a card that offers to permanently retire a gate whose own sentence just
    /// said nothing can. Pinned by id, over every variant, so adding one forces
    /// the decision instead of inheriting a default.
    #[test]
    fn every_floor_variant_is_declared_a_floor_on_both_sides() {
        for (rule, expected) in [
            (GateRule::ToolDeclared, true),
            (GateRule::GateRemoval, true),
            (GateRule::DestructiveArguments, false),
            (GateRule::TierRaised, false),
            (GateRule::OperatorRequired, false),
            (GateRule::HookRequested, false),
            (
                GateRule::PolicyAsk {
                    pattern: "bash",
                    exact: true,
                },
                false,
            ),
            (GateRule::PolicyDeny { pattern: "*" }, false),
        ] {
            assert_eq!(rule.is_floor(), expected, "{:?}", rule.id());
            assert_eq!(
                crate::exec::allowed_decisions::rule_is_a_floor(rule.id()),
                expected,
                "`{}` disagrees between the chain and the decision-set derivation",
                rule.id()
            );
        }
        // And nothing is on the floor list that the chain cannot produce.
        for id in crate::exec::allowed_decisions::FLOOR_RULES {
            assert!(
                [GateRule::ToolDeclared, GateRule::GateRemoval]
                    .iter()
                    .any(|r| r.id() == *id),
                "`{id}` is listed as a floor but no `GateRule` variant emits it"
            );
        }
    }
}
