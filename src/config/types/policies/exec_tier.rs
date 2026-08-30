//! Execution permission tier — the single user-facing dial over tool permissions.
//!
//! Four tiers: **Plan** / **Ask** / **Auto** / **Full**, strictest first. The
//! last three are the install's posture (codex-style); `Plan` is a
//! per-conversation posture that ends when a human approves a plan — see the
//! variant doc and [`session_tiers`] vs [`builtin_tiers`]. A tier is *not* a
//! second policy axis and *not* a second enforcement mechanism: it is a rule
//! consulted by the one chokepoint every gate already funnels through
//! ([`crate::tools::scoped::ScopedToolService::permission_for`]) whenever no
//! explicit `[policies.tool_permissions]` entry names the tool. Explicit
//! entries win over the tier, which is what makes a Panel "advanced overrides"
//! section coherent.
//!
//! ## The rules read declared metadata, never the tool's name
//!
//! Tool names are not a contract. MCP tools are registered as
//! `{server_id}__{tool}` (`github__delete_repo`), browser tools as `browser_*`,
//! and every future tool as whatever its author picked — so any table of name
//! globs silently lets whole families through the gate it claims to hold.
//!
//! The property that already declares what a tool *does* is idempotency, read
//! at the enforcement chokepoint through
//! [`crate::tools::runtime::LoopTool::is_idempotent`]: the maintained pure-read
//! allowlist ([`crate::tools::retry::is_idempotent_builtin_name`], which
//! delegates to `READ_ONLY_TOOLS`) for builtins, the
//! server's own `readOnlyHint` / `idempotentHint` for MCP tools, `false` for
//! anything that declares nothing. Hence the rule: **a tool that is not
//! idempotent is a mutating tool**. Unknown tools are non-idempotent, so `Ask`
//! is fail-closed for anything new.
//!
//! The tier axis is orthogonal to `[sandbox.command_policy]`. No tier — not
//! even `Full` — can lower the command-policy hardline floor (fork bombs,
//! `rm -rf /`, device wipes); see the unit test at the bottom of this file.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::tool_permissions::{restrictive_min, ToolPermissionsConfig};
use crate::extension::PermissionAction;

/// Identity-metadata custom key under which a session's per-session tier
/// override is persisted (written through the existing `sessions.patch` RPC,
/// read per turn by the execution engine). Same carrier pattern as
/// `custom["project_root"]`.
pub const EXEC_TIER_SESSION_KEY: &str = "exec_tier";

/// What a tier rule is allowed to know about a tool: its declared metadata.
/// Filled at the enforcement chokepoint from the tool's own `ToolDefinition`.
#[derive(Debug, Clone, Copy)]
pub struct ToolFacts<'a> {
    /// Registry name. Only consulted for the curated destructive set below,
    /// never to guess whether the tool mutates.
    pub name: &'a str,
    /// `LoopTool::is_idempotent` — the tool declares itself a read-only / pure
    /// query (builtin allowlist, or an MCP server's `readOnlyHint`). Everything
    /// else mutates, including every tool Aleph has never heard of.
    pub idempotent: bool,
    /// `ToolDefinitionMetadata.requires_approval` — for MCP tools this carries
    /// the server's `destructiveHint`.
    pub requires_approval: bool,
}

/// `file_ops` operations that destroy or relocate data irreversibly.
///
/// `file_ops` multiplexes `list` / `search` / `stats` *and* `delete` / `move`
/// behind one tool name, so no name-keyed rule can tell them apart: under
/// `Auto` a delete would never ask, contradicting the tier's own promise. This
/// is the tier system's only argument-level rule — a deterministic safety hard
/// filter (explicitly permitted by R7), not a judgement about intent. Values
/// are the serialized `FileOperation` variants (`src/builtin_tools/file_ops/types.rs`).
const DESTRUCTIVE_FILE_OPS: &[&str] = &["delete", "move", "batch_move", "organize"];

/// Tools whose entire effect is to contact the human, and which therefore can
/// never be gated behind contacting the human.
///
/// `ask_user` is not idempotent (asking twice asks twice), so the `Ask` rule
/// below would gate it — meaning that to put a question to the user, the model
/// must first ask the user for permission to ask a question. Circular by
/// construction, and it buys no safety: the tool touches nothing outside the
/// conversation. Worse, the extra prompt is pure noise, and noise is what
/// trains a user to approve without reading — which is how a confirmation gate
/// stops being a safety mechanism.
///
/// Deliberately NOT solved by declaring `ask_user` idempotent
/// ([`crate::tools::retry::is_idempotent_builtin_name`]): that predicate answers "is it
/// safe to auto-retry after a transient failure?", and auto-retrying a question
/// means prompting the human twice. "Safe to retry" and "needs no approval" are
/// two different questions; collapsing them into one field is a coupling that
/// bites the next person who edits either.
///
/// Name-keyed, like [`is_destructive`]'s curated set, and for the same reason:
/// Aleph itself defines this tool, so its name IS a contract. Nothing here may
/// touch the user's system.
const HUMAN_CONTACT_TOOLS: &[&str] = &["ask_user"];

/// The two tools that stay reachable by name while the session is PLANNING
/// ([`ExecTier::Plan`]), despite declaring themselves non-idempotent.
///
/// This is not a "tools planning finds handy" list and must not become one. A
/// mutating tool the model wants mid-plan is a signal that the plan is
/// finished, not that this list is short. Each entry needs an argument of the
/// shape below — a reason the tier would be *incoherent* without it — and
/// there are exactly two:
///
/// * **`scratchpad`** — the plan itself. It is where the execution list lives
///   (`## Objective` / `## Plan` / `## Notes`, the single on-disk shape every
///   plan surface reads, FEATURE_LOCATOR §3.13), so denying it would deny the
///   one deliverable the tier exists to produce. It also carries
///   `action='request_approval'`, the handoff that LEAVES this tier — gating
///   that behind the gate it opens is the circularity [`HUMAN_CONTACT_TOOLS`]
///   documents one const up. Its writes never leave the agent's own markdown.
///
/// * **`subagent`** — read-only delegation. A spawned child does not get a
///   tool service of its own: `subagent_spawner` wraps the PARENT's
///   `ScopedToolService` (`parent_view_for_children`), which carries this tier
///   and the very same `PlanGate` `Arc`. So every tool call a child makes is
///   refused by the same gate that refuses the parent's, and one human
///   approval releases both. Delegated exploration is how a plan for a large
///   codebase gets written, and there is no hole to trade for it.
///
///   Note what is deliberately NOT here for the same reason: `session_send`,
///   `task_manage` and the `team_*` family delegate to a SEPARATE run, which
///   resolves its own tier against its own session key and would not be
///   planning at all. Those stay refused — a two-step path whose composition
///   equals the gated one-step call is the escape hatch this tier would
///   otherwise have.
///
/// Name-keyed for the same reason the other curated sets are: Aleph defines
/// both of these tools, so their names ARE contracts.
///
/// Visible to the crate because the side-question floor is DERIVED from this
/// set (`ScopedToolService::permission_for`): a side question revokes exactly
/// these carve-outs. Keeping a second list would be the very drift this
/// constant exists to prevent.
pub(crate) const PLAN_REACHABLE_TOOLS: &[&str] = &["scratchpad", "subagent"];

/// Execution permission tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecTier {
    /// **Planning.** Nothing that mutates runs at all — not "asks", *refuses*.
    /// The conversation reads, searches, and writes its plan; the first
    /// mutating call is the one that would start doing the work, and holding
    /// that back until a human has seen the plan is the whole point.
    ///
    /// Three things make it a tier rather than a sixth session dial:
    ///
    /// 1. it answers the tier's own question ("does this call run, ask, or
    ///    refuse?") from the same DECLARED metadata every other tier reads;
    /// 2. it composes through [`ExecTier::most_restrictive`] as the strictest
    ///    rung (`Plan` < `Ask` < `Auto` < `Full`), so every ceiling that
    ///    already exists — the channel clamp, the non-operator ceiling — keeps
    ///    holding without a second lattice; and
    /// 3. it is per-conversation and reversible, which is exactly what the
    ///    `custom["exec_tier"]` carrier already is.
    ///
    /// Deliberately NOT offerable as the install-wide `[policies] exec_tier`
    /// default — see [`builtin_tiers`] vs [`session_tiers`]. A global `plan`
    /// would have nothing to hand back to when a plan is approved.
    ///
    /// Carve-outs: [`HUMAN_CONTACT_TOOLS`] (asking the user is how planning
    /// resolves what exploration cannot) and [`PLAN_REACHABLE_TOOLS`] (the plan
    /// file plus the handoff that leaves this tier, and read-only delegation,
    /// which runs through this very gate).
    ///
    /// The per-CALL half lives at the enforcement chokepoint rather than here:
    /// [`ExecTier::rule_for`] only sees a tool's name-level facts, so a
    /// read/write multiplexer like `file_ops` (`list`/`search`/`stats` vs
    /// `move`/`delete`/…) is refused here and re-admitted for its READ arm by
    /// `ScopedToolService`, which holds the arguments and asks the one
    /// per-call read classifier this repo already maintains
    /// (`LoopTool::concurrency_claim(input) == Shared`).
    Plan,
    /// Every mutating / side-effecting tool needs human confirmation.
    /// Read-only tools stay allowed so the model can still investigate.
    Ask,
    /// Today's de-facto behavior plus a real guard on the destructive tail
    /// (deletions, credential writes, team destruction). The default: an
    /// unconfigured install behaves as it did before tiers existed, except
    /// irreversible operations now stop for a human.
    #[default]
    Auto,
    /// The tier itself asks nothing. **Three floors survive it**, and none is
    /// reachable by any configuration:
    ///
    /// 1. the `[sandbox.command_policy]` hardline;
    /// 2. a tool's own `requires_confirmation` declaration
    ///    (`CONFIRMATION_REQUIRED_TOOLS` + MCP `destructiveHint`), which
    ///    `ScopedToolService::check_confirmation_gate` reads independently of
    ///    the tier — so `vault_store`, `agent_delete`, `team_disband` and
    ///    friends still raise a card here, and in an unattended run still
    ///    fail closed; and
    /// 3. [`ExecTier::floor_asks_for_arguments`] — a `self_config` write that
    ///    can reach the approval settings themselves. That one is here because
    ///    this tier is a **per-request** knob and the config change is
    ///    permanent: without it, one message at `full` retires a gate for every
    ///    later session at every later tier.
    ///
    /// "Full" therefore means "the tier gates nothing", not "nothing is
    /// gated". Both the variant doc and
    /// [`ExecTier::approval_prompt_line`] used to claim the latter.
    Full,
}

impl ExecTier {
    /// Parse a tier from its serialized id (`"ask"` / `"auto"` / `"full"`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "plan" => Some(Self::Plan),
            "ask" => Some(Self::Ask),
            "auto" => Some(Self::Auto),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// Serialized id of this tier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::Full => "full",
        }
    }

    /// How permissive this tier is: `Plan` (0) < `Ask` (1) < `Auto` (2) <
    /// `Full` (3).
    ///
    /// Deliberately a method rather than a derived `Ord`. `ExecTier` is
    /// `Serialize`/`Deserialize`/`JsonSchema`, and a derived `Ord` would make
    /// `a < b` compile everywhere with a meaning ("declaration order") that
    /// happens to coincide today and carries no promise to keep coinciding.
    /// The one comparison anybody needs is [`Self::most_restrictive`].
    #[must_use]
    const fn permissiveness(self) -> u8 {
        match self {
            Self::Plan => 0,
            Self::Ask => 1,
            Self::Auto => 2,
            Self::Full => 3,
        }
    }

    /// The stricter of two tiers — the composition rule for a ceiling.
    ///
    /// Same shape as [`restrictive_min`] one level up: composing permissions
    /// may only ever tighten, so a ceiling cannot accidentally grant.
    #[must_use]
    pub const fn most_restrictive(a: Self, b: Self) -> Self {
        if a.permissiveness() <= b.permissiveness() {
            a
        } else {
            b
        }
    }

    /// This tier's verdict on a tool with these declared facts.
    ///
    /// `None` = the tier has nothing to say; the caller falls back to the
    /// configured `default`. The tier never *widens* a permission — it raises
    /// tools to `Ask`, or, under [`Self::Plan`] alone, to `Deny`.
    #[must_use]
    pub fn rule_for(self, facts: ToolFacts<'_>) -> Option<PermissionAction> {
        // Contacting the human is never gated behind contacting the human.
        if HUMAN_CONTACT_TOOLS.contains(&facts.name) {
            return None;
        }
        // Planning is the one tier that REFUSES rather than asks: the point of
        // the mode is that no work starts, and an approval card per mutating
        // call would turn "hold everything until the plan is reviewed" into
        // "review the plan one edit at a time". Same input as every other
        // tier — the tool's DECLARED idempotency, never its name.
        if self == Self::Plan {
            if PLAN_REACHABLE_TOOLS.contains(&facts.name) {
                return None;
            }
            return (!facts.idempotent).then_some(PermissionAction::Deny);
        }
        let asks = match self {
            // Not idempotent = mutating. Destructive is folded in so a tool
            // that lies about idempotency but declares itself destructive
            // still stops.
            Self::Ask => !facts.idempotent || is_destructive(facts),
            Self::Auto => is_destructive(facts),
            // Answered above; listed so a new variant is a compile error here
            // rather than a silent `false`.
            Self::Plan | Self::Full => false,
        };
        asks.then_some(PermissionAction::Ask)
    }

    /// `true` when this *call* must ask because of its arguments, whatever the
    /// name-keyed rules said. See [`DESTRUCTIVE_FILE_OPS`].
    ///
    /// Two layers, and the difference matters:
    ///
    /// * The **tier-scoped** rules below only `Auto` needs: `Ask` already gates
    ///   these tools wholesale (they are not idempotent) and `Full` never asks
    ///   (its documented contract — the residual "model writes a root reference
    ///   under Full" risk is the same trust decision Full makes everywhere,
    ///   noted in GRAPH_LAYER.md).
    /// * The **floor** ([`Self::floor_asks_for_arguments`]) applies under every
    ///   tier, including `Full`.
    #[must_use]
    pub fn asks_for_arguments(self, name: &str, input: &Value) -> bool {
        if Self::floor_asks_for_arguments(name, input) {
            return true;
        }
        if self != Self::Auto {
            return false;
        }
        match name {
            "file_ops" => input
                .get("operation")
                .and_then(Value::as_str)
                .is_some_and(|op| DESTRUCTIVE_FILE_OPS.contains(&op)),
            // Loop-graph governance: any write touching a `root:` or `frozen:`
            // node pauses for the person. Root references are human-supplied
            // BY DEFINITION (the store already enforces origin=human); this
            // argument-level ask is the channel that makes "a human confirmed
            // the exact text" true, and a background session with no approval
            // transport fails closed — the machine is structurally unable to
            // touch the graph's ground.
            "loop_graph" => loop_graph_touches_protected(input),
            _ => false,
        }
    }

    /// The argument-level rules **no tier can stand down** — the third floor,
    /// alongside the command-policy hardline and a tool's own
    /// `requires_confirmation`.
    ///
    /// Exactly one rule today: a `self_config` write that can reach the
    /// configuration deciding whether these gates fire at all.
    ///
    /// # Why this cannot be a tier rule
    ///
    /// [`self_config_touches_the_gate`] exists because "write the override,
    /// then act freely" is two legal steps whose composition equals the gated
    /// one-step write. It was written as an `Auto`-only rule, which left the
    /// composition open on the tier where it is cheapest: under `Full` the
    /// model could retire the gate in one un-carded call — and the removal is
    /// **install-wide and permanent**, while the tier that permitted it is a
    /// per-request knob (`chat.send{exec_tier}`). One message at `full` bought
    /// a config change that outlives it, in every later session at every later
    /// tier. A gate whose own justification is "cover the verb that removes the
    /// gate" cannot be switched off by the thing it is protecting.
    ///
    /// `Ask` and `Auto` reach the same call through their own rules, so this
    /// changes the gated set at `Full` only. It is deliberately narrow: it does
    /// not fire for `self_config` generally, only for a write whose path
    /// intersects [`GATE_DECIDING_CONFIG_PATHS`].
    ///
    /// An explicit `[policies.tool_permissions]` entry still stands it down
    /// (`ScopedToolServiceBuilder::explicitly_named`) — that entry is a
    /// decision a person wrote, and the first write that creates one cards
    /// through this very rule.
    #[must_use]
    pub fn floor_asks_for_arguments(name: &str, input: &Value) -> bool {
        name == "self_config" && self_config_touches_the_gate(input)
    }

    /// One model-facing line describing this tier's approval regime, for the
    /// system prompt (rendered by `SecurityLayer`). Codex surfaces the same
    /// fact as `<approval_policy>` inside its `<environment_context>` so the
    /// model can pace itself against the human backstop instead of discovering
    /// it through interrupted tool calls (R9: intelligence in the prompt).
    ///
    /// The copy lives next to `rule_for` — the single source of what each tier
    /// actually gates — so a change to the enforcement rule and its description
    /// cannot drift apart. Model-facing prompt text is always English (unlike a
    /// user-surface label, which follows the reader's locale — see
    /// [`TierPreset`]).
    #[must_use]
    pub const fn approval_prompt_line(self) -> &'static str {
        match self {
            // Describes the whole PROTOCOL, not just the current state. A line
            // that said only "everything is read-only" would still be on the
            // wire, unchanged, for the rest of the turn in which the plan gets
            // approved and the gate lifts — and 判据 §0 says the copy the model
            // reads is the most expensive of the three places a claim about
            // what is blocked can go stale. Stating the exit here means the
            // sentence stays true on both sides of the handoff.
            Self::Plan => {
                "Approval mode: plan — this conversation is PLANNING. Every tool that \
                 does not declare itself read-only is refused, so no work starts yet; \
                 reading, searching and `ask_user` all run freely, and `scratchpad` is \
                 writable because the plan is the deliverable. Explore first and ask \
                 only what exploration cannot answer, then write the objective and an \
                 ordered, decision-complete checklist with `scratchpad` (`set_objective` \
                 + `set_plan`) and call `scratchpad` with `action='request_approval'`. \
                 If the user approves, the read-only gate lifts IMMEDIATELY — in this \
                 same turn — and you continue straight into implementing the plan you \
                 just had approved. If they ask for changes, revise the checklist and \
                 request approval again."
            }
            Self::Ask => {
                "Approval mode: ask — every mutating or side-effecting tool call \
                 pauses for the user's confirmation before it runs; read-only tools run \
                 freely. Plan ahead and batch related changes, and state what you intend to \
                 do before a run of edits, so the user is not interrupted step by step."
            }
            Self::Auto => {
                "Approval mode: auto — routine tool calls run without interruption; \
                 only irreversible or destructive actions (deletions, moves, credential \
                 writes, disbanding a team) pause for the user's confirmation."
            }
            Self::Full => {
                "Approval mode: full — the tier gates nothing. You are the last line of \
                 defense: double-check destructive or irreversible actions yourself before \
                 running them. Three floors still apply under every mode: the command-policy \
                 hardline (fork bombs, `rm -rf /`, device wipes), the handful of tools \
                 that declare their own confirmation gate (credential writes, deleting an \
                 agent, disbanding a team, installing a skill), and a `self_config` write \
                 that changes the approval settings themselves — those still pause for the \
                 user."
            }
        }
    }
}

/// The effective permission for a tool: a refusing tier's floor, else the
/// operator's explicit decision, else their configured baseline TIGHTENED by
/// the tier.
///
/// Precedence, most binding first:
/// 0. a tier verdict of **`Deny`** — a floor. Nothing below may outrank it;
/// 1. an **explicit** entry (exact name, then glob) in the merged
///    [`ToolPermissionsConfig`] — an operator who names a tool has decided;
/// 2. the configured `default` (`Allow` when no policy is attached), tightened
///    by [`ExecTier::rule_for`] through the restrictiveness lattice.
///
/// # Why rung 0 exists
///
/// Rungs 1 and 2 encode "more specific wins", and for `Ask` / `Auto` / `Full`
/// that is exactly right: those tiers only ever raise a tool to *ask*, so an
/// operator who named the tool has answered the very question the tier was
/// going to pose. [`ExecTier::Plan`] asks nothing — its whole promise, stated
/// to the model on every turn of a planning conversation, is that **nothing
/// runs**. Under "more specific wins" a single `"bash" = "allow"` sitting in
/// `[policies.tool_permissions]` — written months earlier, for an unrelated
/// reason — would hollow that promise out silently, on the one install shape
/// where it matters most: the one whose operator bothered to configure tools
/// at all. A floor that an entry can outrank is not a floor, it is a default.
///
/// # Why it is keyed on the verdict, not on the tier
///
/// `Some(Deny)` is *by construction* "a tier that refuses rather than asks":
/// [`ExecTier::rule_for`]'s contract is that a tier yields at most `Ask`, and
/// its `match` is exhaustive, so a new refusing tier is a compile error there
/// and inherits this floor here for free. A `tier == Some(ExecTier::Plan)` test
/// would instead describe only the tiers that existed the day it was written.
///
/// # What rung 0 does NOT change
///
/// It fires only where `rule_for` itself answered `Deny`, never on a composed
/// `Deny`. So `default = "deny"` still loses to an explicit `allow` (rung 1),
/// `Ask` / `Auto` / `Full` are byte-identical to before it existed, and the
/// `Plan` carve-outs (`HUMAN_CONTACT_TOOLS`, [`PLAN_REACHABLE_TOOLS`], and
/// every idempotent tool) answer `None` and fall straight through to rung 1 —
/// an explicit `deny` on `read_file` still denies while planning.
///
/// Consulting the tier *before* the `default` for the non-`Deny` verdicts would
/// invert a `default = "deny"` install into ask-by-default for exactly the tools
/// the tier wanted to guard, which is why rung 2 composes rather than short-
/// circuits.
///
/// The single composition point: `ScopedToolService::permission_for` (the loop's
/// enforcement chokepoint) and the gateway slash-command fast path both call it,
/// so neither surface can drift into its own precedence.
#[must_use]
pub fn effective_permission(
    permissions: Option<&ToolPermissionsConfig>,
    tier: Option<ExecTier>,
    facts: ToolFacts<'_>,
) -> PermissionAction {
    let tier_verdict = tier.and_then(|t| t.rule_for(facts));
    // Rung 0 — see "Why rung 0 exists" above. Must stay ahead of the explicit
    // lookup; moving it below turns the read-only phase back into a suggestion.
    if tier_verdict == Some(PermissionAction::Deny) {
        return PermissionAction::Deny;
    }
    if let Some(explicit) = permissions.and_then(|p| p.resolve_explicit(facts.name)) {
        return explicit;
    }
    let base = permissions.map_or(PermissionAction::Allow, |p| p.default);
    match tier_verdict {
        Some(tier_action) => restrictive_min(base, tier_action),
        None => base,
    }
}

/// The `Auto` tier's guarded tail: irreversible operations.
///
/// Irreversibility is not a property any tool declares, so unlike "mutating"
/// it cannot be inverted out of an existing allowlist. What we do have is the
/// server-declared destructive bit (`requires_approval`, an MCP server's
/// `destructiveHint` — free coverage of destructive MCP tools) plus a small
/// curated set of builtin families whose name *is* their contract, because
/// Aleph itself defines them.
/// A `loop_graph` call that **names** a `root:` or `frozen:` node in its
/// arguments and writes — including `pair`, which writes the same `watches`
/// edge onto its `to_id` that `link` would — **or** an `unlink` of an
/// `owns_reference` edge, which removes the objective ACL itself while naming
/// no protected id at all. Other writes to ordinary loop/anchor nodes never
/// match: the gate is exactly the graph's ground layer plus the one verb that
/// can take that layer's authority away.
///
/// Two write actions are deliberately OUT of scope, because the mechanism is
/// argument-level and neither call carries a protected id to match on:
/// - `enable_audit` fans `audits` edges onto every frozen node it finds. Those
///   edges are purely additive governance — they never touch a frozen rule's
///   `body`, and an audit ring that cannot see the frozen rules is the failure
///   this layer exists to prevent.
/// - `gc` deletes only structurally dead rows (an endpoint that no longer
///   exists), which the audit template is explicitly licensed to clear.
///
/// Both remain operator-only on channels (`method_authz::OPERATOR_TOOLS`).
/// Do NOT "fix" the scope by having this pure config predicate read the store
/// to discover whether the graph happens to contain a frozen node — that is a
/// layering cost for an additive write. If it ever needs closing, close it in
/// the tool (an explicit `confirm` argument), not here.
///
/// Whatever this predicate answers is also the argument-level floor on
/// surfaces that cannot raise a card at all
/// (`dangerous_tools::is_denied_on_gateway_surface` reads it directly), so
/// widening it here silently narrows those surfaces too.
fn loop_graph_touches_protected(input: &Value) -> bool {
    let action = input.get("action").and_then(Value::as_str);
    let is_write =
        action.is_some_and(|a| matches!(a, "node" | "drop_node" | "link" | "unlink" | "pair"));
    if !is_write {
        return false;
    }
    // Cutting an `owns_reference` edge dissolves the objective ACL itself, and
    // the ids involved carry no protected prefix — the governed loop is
    // `goal:<session>` and its governor is an ordinary `cron:`/`daemon:` node.
    // Without this arm the §6.2 write protection is removable by the very loop
    // it governs, in one un-carded call whose exact arguments the refusal
    // message in `builtin_tools/goal.rs` prints for the model. GRAPH_LAYER.md
    // has always described that escape hatch as "用户确认后 unlink" — this is
    // the confirmation.
    if action == Some("unlink")
        && input.get("edge").and_then(Value::as_str) == Some("owns_reference")
    {
        return true;
    }
    ["id", "from_id", "to_id"].iter().any(|k| {
        input
            .get(*k)
            .and_then(Value::as_str)
            .is_some_and(|v| v.starts_with("root:") || v.starts_with("frozen:"))
    })
}

/// The config subtrees a `self_config` write may never touch without a
/// card, at every execution tier including `Full`.
///
/// The membership rule is not a count and not "important settings" — it is
/// what a write here can DO to the confirmation chain. A path belongs on
/// this list if writing it either STANDS DOWN an argument-level card the
/// chain would otherwise raise, or OPENS an execution surface no card was
/// ever watching in the first place. Both are "write the override, then act
/// freely" turning two individually legal steps into the one-step,
/// un-carded removal of a human checkpoint — see
/// [`self_config_touches_the_gate`]'s doc for the composition this whole
/// mechanism exists to close.
///
/// - `policies.tool_permissions` — one entry named after a tool disarms that
///   tool's card via `ScopedToolServiceBuilder::explicitly_named`, whose whole
///   justification is "the operator already decided about this tool". Stands
///   down a card.
/// - `policies.exec_tier` — `Full` never asks, by contract. Stands down every
///   tier-raised card at once.
/// - `policies.terminal` — `[policies.terminal] enabled = true` turns on
///   `pty.*`, a raw shell the command policy does not see and the exec tier
///   does not gate (`gateway::handlers::pty`'s module doc). This entry
///   stands down no OTHER card; it opens a new execution surface no card
///   was ever watching.
const GATE_DECIDING_CONFIG_PATHS: &[&str] = &[
    "policies.tool_permissions",
    "policies.exec_tier",
    "policies.terminal",
];

/// Whether this `self_config` call can reach the configuration that decides
/// whether [`ExecTier::asks_for_arguments`] fires.
///
/// # Why this arm exists
///
/// `explicitly_named` is exact-match (round 11 made it so, after `"*" = "allow"`
/// was found switching off the last card in front of `file_ops delete` and
/// `root:`/`frozen:` writes). But exactness only answers *which* entry counts;
/// it assumes the entry was written by a person. `self_config` breaks that
/// assumption: it raises no card of its own — it is absent from
/// `CONFIRMATION_REQUIRED_TOOLS`, `is_destructive` is false for it, and an
/// operator connection sails through `check_operator_gate`. So on a default
/// install the model could, in one un-carded call, write
/// `policies.tool_permissions.overrides = {"loop_graph": "allow"}` and thereby
/// retire — permanently, for every later turn — the only human checkpoint this
/// repo's governance layer has. Two legal steps, and the composition is
/// equivalent to the gated one-step write.
///
/// The predicate is a subtree-intersection rather than a prefix test, because a
/// write at `policies` (or at the root) carries the same payload one level up.
/// `rollback_config` restores a whole snapshot and therefore always intersects.
///
/// Note this covers its own removal: the first call that writes an override for
/// `self_config` itself is a write to `policies.tool_permissions`, so it cards.
fn self_config_touches_the_gate(input: &Value) -> bool {
    match input.get("action").and_then(Value::as_str) {
        // A snapshot restore reinstates whatever the snapshot held; there is no
        // path argument to narrow it by.
        Some("rollback_config") => true,
        Some("update_config") => {
            let path = input
                .get("config_path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            GATE_DECIDING_CONFIG_PATHS
                .iter()
                .any(|target| dot_paths_intersect(path, target))
        }
        _ => false,
    }
}

/// Whether two dot-paths name overlapping config subtrees — equal, or one an
/// ancestor of the other. Segment-aware, so `policies.exec_tier_legacy` does not
/// count as touching `policies.exec_tier`. An empty path is the whole config.
fn dot_paths_intersect(a: &str, b: &str) -> bool {
    a.is_empty() || a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

fn is_destructive(facts: ToolFacts<'_>) -> bool {
    facts.requires_approval
        || facts.name.ends_with("_delete")
        || facts.name == "team_disband"
        || facts.name.starts_with("vault_")
}

/// A tier as offered to a user surface (Panel / CLI / bot).
///
/// Core owns the tier IDENTITY — the id set, its order, and every `rule_for`
/// verdict behind it — so every surface offers the same three choices with the
/// same meaning (R6). It does NOT own the COPY: a label is presentation, it has
/// to follow the reader's locale, and a surface that cannot resolve it in its
/// own locale files is structurally unable to be localized (R4: surfaces render,
/// core decides). Ship ids; let the surface author the words for its user.
///
/// An alias since the fourth dial arrived: the shape is identical across the
/// five session knobs, so it lives once in [`super::DialPreset`].
pub type TierPreset = super::DialPreset;

/// The three tiers an INSTALL may sit at, ordered least → most permissive.
///
/// `plan` is deliberately absent: it is a per-conversation posture that ENDS
/// when a human approves a plan, and ending it means falling back to whatever
/// the install's own `[policies] exec_tier` says. An install whose default was
/// `plan` would have nothing to fall back to — every conversation would return
/// from its approved plan straight into planning again. See [`session_tiers`]
/// for the list a composer offers.
#[must_use]
pub const fn builtin_tiers() -> &'static [TierPreset] {
    &[
        TierPreset { id: "ask" },
        TierPreset { id: "auto" },
        TierPreset { id: "full" },
    ]
}

/// The tiers a single CONVERSATION may sit at — [`builtin_tiers`] plus `plan`,
/// which leads because it is the strictest.
///
/// Two lists rather than one because the two questions genuinely differ: "what
/// posture does this machine run at" admits no answer that expires, and "what
/// is this conversation doing right now" does. Both are served from one RPC
/// (`config.get_tool_permissions`) so a surface decodes them together and
/// cannot pair a session dial with a stale install dial.
#[must_use]
pub const fn session_tiers() -> &'static [TierPreset] {
    &[
        TierPreset { id: "plan" },
        TierPreset { id: "ask" },
        TierPreset { id: "auto" },
        TierPreset { id: "full" },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::command_policy::{CommandPolicy, EnforcementMode};
    use serde_json::json;

    /// Facts as the chokepoint would build them for a tool nobody declared
    /// anything about — an MCP tool, a browser tool, a tool shipped tomorrow.
    fn unknown(name: &str) -> ToolFacts<'_> {
        ToolFacts {
            name,
            idempotent: false,
            requires_approval: false,
        }
    }

    /// Facts for a builtin, read from the same allowlist the chokepoint reads.
    fn builtin(name: &str) -> ToolFacts<'_> {
        ToolFacts {
            name,
            idempotent: crate::tools::retry::is_idempotent_builtin_name(name),
            requires_approval: false,
        }
    }

    #[test]
    fn full_tier_leaves_the_command_policy_hardline_floor_intact() {
        // THE invariant of the tier system: a tier is a rule on the TOOL
        // permission axis only. `Full` opens that axis to the maximum (it never
        // asks for anything) and still cannot reach the sandbox command-policy
        // hardline — fork bombs, `rm -rf /`, device wipes stay blocked under
        // every tier and every enforcement mode.
        for facts in [
            unknown("bash"),
            builtin("bash"),
            unknown("github__delete_repo"),
        ] {
            assert_eq!(ExecTier::Full.rule_for(facts), None);
        }
        assert!(!ExecTier::Full.asks_for_arguments("file_ops", &json!({"operation": "delete"})));

        for enforcement in [
            EnforcementMode::Block,
            EnforcementMode::Warn,
            EnforcementMode::Off,
        ] {
            let policy = CommandPolicy::defaults(enforcement);
            for cmd in ["rm -rf /", "rm -rf / --no-preserve-root", ":(){ :|:& };:"] {
                assert!(
                    !policy.evaluate(cmd).blocked.is_empty(),
                    "hardline floor must block `{cmd}` (enforcement {enforcement:?}) regardless of exec tier"
                );
            }
        }
    }

    #[test]
    fn default_tier_is_auto() {
        assert_eq!(ExecTier::default(), ExecTier::Auto);
    }

    #[test]
    fn ask_tier_asks_for_every_non_idempotent_tool() {
        let ask = ExecTier::Ask;
        // Builtins that mutate.
        for name in ["bash", "file_write", "file_ops", "system", "self_config"] {
            assert_eq!(
                ask.rule_for(builtin(name)),
                Some(PermissionAction::Ask),
                "`{name}` mutates and must ask under Ask"
            );
        }
        // The families a name-glob table missed: MCP tools are registered as
        // `{server_id}__{tool}`, browser tools as `browser_*`.
        for name in [
            "github__create_issue",
            "slack__send_message",
            "browser_evaluate",
        ] {
            assert_eq!(
                ask.rule_for(unknown(name)),
                Some(PermissionAction::Ask),
                "`{name}` must ask under Ask"
            );
        }
    }

    #[test]
    fn ask_tier_is_fail_closed_for_unknown_tools() {
        // A tool shipped tomorrow declares nothing → non-idempotent → asks.
        assert_eq!(
            ExecTier::Ask.rule_for(unknown("brand_new_tool")),
            Some(PermissionAction::Ask)
        );
    }

    #[test]
    fn ask_tier_leaves_the_read_only_surface_open() {
        for name in [
            "search",
            "memory_search",
            "web_fetch",
            "file_read",
            "recall_context",
        ] {
            assert_eq!(
                ExecTier::Ask.rule_for(builtin(name)),
                None,
                "`{name}` is a declared pure read and must stay allowed under Ask"
            );
        }
    }

    /// `ask_user` is non-idempotent, so the Ask rule would gate it — forcing
    /// the model to ask the user for permission to ask the user something.
    /// The exemption exists to break that circle, and must not widen: a tool
    /// that touches the system still asks, under every tier.
    #[test]
    fn human_contact_is_never_gated_behind_human_contact() {
        for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            assert_eq!(
                tier.rule_for(builtin("ask_user")),
                None,
                "{tier:?} must not gate `ask_user` behind an approval prompt"
            );
        }

        // The exemption is one name, not a hole: everything that reaches the
        // user's system still asks under Ask.
        for name in [
            "bash",
            "file_ops",
            "browser_evaluate",
            "github__delete_repo",
        ] {
            assert_eq!(
                ExecTier::Ask.rule_for(unknown(name)),
                Some(PermissionAction::Ask),
                "`{name}` touches the system and must still ask under Ask"
            );
        }
    }

    #[test]
    fn auto_tier_only_guards_the_destructive_tail() {
        let auto = ExecTier::Auto;
        assert_eq!(auto.rule_for(builtin("bash")), None);
        assert_eq!(auto.rule_for(builtin("file_write")), None);
        assert_eq!(auto.rule_for(builtin("search")), None);
        assert_eq!(
            auto.rule_for(builtin("agent_delete")),
            Some(PermissionAction::Ask)
        );
        assert_eq!(
            auto.rule_for(builtin("team_disband")),
            Some(PermissionAction::Ask)
        );
        assert_eq!(
            auto.rule_for(builtin("vault_store")),
            Some(PermissionAction::Ask)
        );
    }

    #[test]
    fn auto_tier_guards_server_declared_destructive_mcp_tools() {
        // `requires_approval` is where an MCP server's `destructiveHint` lands.
        let destructive = ToolFacts {
            name: "github__delete_repo",
            idempotent: false,
            requires_approval: true,
        };
        assert_eq!(
            ExecTier::Auto.rule_for(destructive),
            Some(PermissionAction::Ask)
        );
        // A non-destructive MCP tool runs freely under Auto — that is the tier.
        assert_eq!(
            ExecTier::Auto.rule_for(unknown("github__create_issue")),
            None
        );
    }

    #[test]
    fn auto_tier_asks_for_destructive_file_ops_arguments() {
        let auto = ExecTier::Auto;
        // The name alone says nothing — `file_ops` is not destructive per se.
        assert_eq!(auto.rule_for(builtin("file_ops")), None);
        for op in ["delete", "move", "batch_move", "organize"] {
            assert!(
                auto.asks_for_arguments("file_ops", &json!({"operation": op, "path": "/tmp/x"})),
                "file_ops `{op}` destroys or relocates data and must ask under Auto"
            );
        }
        for op in ["list", "search", "stats", "copy", "mkdir"] {
            assert!(
                !auto.asks_for_arguments("file_ops", &json!({"operation": op, "path": "/tmp/x"}))
            );
        }
        // Malformed / missing operation: the tool itself rejects it, no ask.
        assert!(!auto.asks_for_arguments("file_ops", &json!({})));
        // Other tools are unaffected by the argument filter.
        assert!(!auto.asks_for_arguments("bash", &json!({"operation": "delete"})));
    }

    #[test]
    fn loop_graph_root_and_frozen_writes_ask_under_auto() {
        let auto = ExecTier::Auto;
        // Writes touching the graph's ground layer pause for the person.
        for (action, key) in [
            ("node", "id"),
            ("drop_node", "id"),
            ("link", "from_id"),
            ("link", "to_id"),
            ("unlink", "from_id"),
            // `pair` writes a `watches` edge onto its `to_id` — the identical
            // edge write `link` gates. (Deliberately flipped from the earlier
            // pin that exempted `pair`.)
            ("pair", "to_id"),
        ] {
            for prefix in ["root:aleph", "frozen:budget-ratchet"] {
                assert!(
                    auto.asks_for_arguments("loop_graph", &json!({"action": action, key: prefix})),
                    "loop_graph {action} on {prefix} must ask under Auto"
                );
            }
        }
        // Ordinary loop/anchor writes and all read actions run freely.
        assert!(!auto.asks_for_arguments(
            "loop_graph",
            &json!({"action": "node", "id": "daemon:dreaming"})
        ));
        assert!(!auto.asks_for_arguments(
            "loop_graph",
            &json!({"action": "link", "from_id": "cron:w", "to_id": "goal:s"})
        ));
        // Pairing a watcher onto an ordinary node runs freely too.
        assert!(!auto.asks_for_arguments(
            "loop_graph",
            &json!({"action": "pair", "to_id": "goal:s", "label": "w", "prompt": "p"})
        ));
        for action in ["status", "list", "gc", "enable_audit"] {
            assert!(!auto.asks_for_arguments("loop_graph", &json!({"action": action})));
        }
        // Cutting the objective ACL asks, even though no id here is protected:
        // this is the "用户确认后 unlink" the design has always specified, and
        // `builtin_tools/goal.rs` prints these exact arguments to the model when
        // it refuses an objective rewrite.
        assert!(auto.asks_for_arguments(
            "loop_graph",
            &json!({"action": "unlink", "from_id": "cron:steward",
                    "to_id": "goal:s1", "edge": "owns_reference"})
        ));
        // Other verbs on the same ordinary ids stay free — the gate is the ACL,
        // not `unlink`.
        assert!(!auto.asks_for_arguments(
            "loop_graph",
            &json!({"action": "unlink", "from_id": "cron:w",
                    "to_id": "goal:s1", "edge": "watches"})
        ));
        // Ask gates the tool wholesale by the name-keyed rule; Full never asks
        // (its documented contract).
        assert!(!ExecTier::Full
            .asks_for_arguments("loop_graph", &json!({"action": "node", "id": "root:aleph"})));
    }

    /// The documented precedence end-to-end THROUGH the merge every turn runs:
    /// an operator's explicit `allow` — even one equal to the default — beats
    /// the tier after `ToolPermissionsConfig::merge`. The former merge
    /// "compression" dropped such entries, so the Ask tier re-gated tools the
    /// operator had deliberately named.
    #[test]
    fn explicit_allow_survives_merge_and_beats_the_tier() {
        let global = ToolPermissionsConfig {
            default: PermissionAction::Allow,
            overrides: [("bash".to_string(), PermissionAction::Allow)]
                .into_iter()
                .collect(),
        };
        let merged = ToolPermissionsConfig::merge(&global, &ToolPermissionsConfig::default());
        assert_eq!(
            effective_permission(Some(&merged), Some(ExecTier::Ask), builtin("bash")),
            PermissionAction::Allow,
            "the operator named `bash` — the tier has nothing to say"
        );
        // An unnamed mutating tool is still tightened by the tier.
        assert_eq!(
            effective_permission(Some(&merged), Some(ExecTier::Ask), builtin("file_write")),
            PermissionAction::Ask
        );
    }

    #[test]
    fn tier_id_roundtrip() {
        for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            assert_eq!(ExecTier::from_id(tier.id()), Some(tier));
        }
        assert_eq!(ExecTier::from_id("nonsense"), None);
    }

    #[test]
    fn approval_prompt_line_is_distinct_and_names_the_tier() {
        // Each tier renders a non-empty, tier-specific line that leads with its
        // id so the model can key on it. The three lines must be distinct — a
        // copy-paste that collapsed two tiers would hide the regime from the
        // model.
        let ask = ExecTier::Ask.approval_prompt_line();
        let auto = ExecTier::Auto.approval_prompt_line();
        let full = ExecTier::Full.approval_prompt_line();
        assert!(ask.contains("Approval mode: ask"));
        assert!(auto.contains("Approval mode: auto"));
        assert!(full.contains("Approval mode: full"));
        assert_ne!(ask, auto);
        assert_ne!(auto, full);
        assert_ne!(ask, full);
        // The `Full` line must warn that there is no human backstop — that is
        // the whole reason to surface the regime at this tier.
        assert!(full.contains("last line of defense"));
    }

    #[test]
    fn builtin_tiers_cover_every_variant() {
        let ids: Vec<&str> = builtin_tiers().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec!["ask", "auto", "full"]);
        assert!(builtin_tiers()
            .iter()
            .all(|p| ExecTier::from_id(p.id).is_some()));
    }

    /// The two lists are one list plus `plan`, in that order, and every id in
    /// both parses. A census rather than a literal on each side: the failure
    /// this pins is a fourth tier reaching only ONE of the two surfaces.
    #[test]
    fn session_tiers_are_the_install_tiers_plus_plan() {
        let session: Vec<&str> = session_tiers().iter().map(|p| p.id).collect();
        let install: Vec<&str> = builtin_tiers().iter().map(|p| p.id).collect();
        assert_eq!(session.first(), Some(&"plan"));
        assert_eq!(&session[1..], install.as_slice());
        assert!(session_tiers()
            .iter()
            .all(|p| ExecTier::from_id(p.id).is_some()));
        // Every variant reaches a user surface. `Plan` reaches exactly one of
        // them, which is the whole asymmetry — an install cannot sit at it.
        assert!(!install.contains(&ExecTier::Plan.id()));
    }

    /// The strictest rung. Every ceiling in the repo composes through
    /// `most_restrictive`, so a `Plan` that were not the minimum would be
    /// silently raised by the channel clamp / non-operator ceiling.
    #[test]
    fn plan_is_the_most_restrictive_rung() {
        for other in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            assert_eq!(
                ExecTier::most_restrictive(ExecTier::Plan, other),
                ExecTier::Plan
            );
            assert_eq!(
                ExecTier::most_restrictive(other, ExecTier::Plan),
                ExecTier::Plan
            );
        }
    }

    /// `Plan` refuses; it does not ask. An approval card per mutating call
    /// would turn "hold everything until the plan is reviewed" into "review
    /// the plan one edit at a time".
    #[test]
    fn plan_denies_every_tool_that_does_not_declare_itself_read_only() {
        assert_eq!(
            ExecTier::Plan.rule_for(unknown("some_mcp__write")),
            Some(PermissionAction::Deny)
        );
        assert_eq!(
            ExecTier::Plan.rule_for(unknown("file_write")),
            Some(PermissionAction::Deny)
        );
        // Read-only by DECLARATION runs freely — same input every tier reads.
        let read = ToolFacts {
            name: "file_read",
            idempotent: true,
            requires_approval: false,
        };
        assert_eq!(ExecTier::Plan.rule_for(read), None);
    }

    /// The carve-outs, and the reason each exists: without them the tier is
    /// circular — you would need approval to ask for approval, you could not
    /// write the plan the approval is about, and you could not send a reader
    /// into the codebase you are planning against.
    #[test]
    fn plan_keeps_the_human_channel_the_plan_file_and_delegation_open() {
        for name in ["ask_user", "scratchpad", "subagent"] {
            assert_eq!(
                ExecTier::Plan.rule_for(unknown(name)),
                None,
                "`{name}` must stay reachable while planning"
            );
        }
    }

    /// The delegation carve-out is exactly one verb wide. `subagent` is
    /// admissible only because its child runs through the PARENT's gated tool
    /// service; every other delegation verb starts a SEPARATE run that
    /// resolves its own tier and would not be planning at all — a two-step
    /// path whose composition equals the call this tier refuses.
    #[test]
    fn plan_refuses_every_delegation_that_starts_a_separate_run() {
        for name in [
            "session_send",
            "task_manage",
            "team_create",
            "team_dispatch",
            "node_invoke",
            "a2a_send",
        ] {
            assert_eq!(
                ExecTier::Plan.rule_for(unknown(name)),
                Some(PermissionAction::Deny),
                "`{name}` delegates to a run this gate cannot reach"
            );
        }
    }

    /// A `Deny` verdict survives composition: the tier only ever tightens, and
    /// `restrictive_min` must not let a `default = "allow"` install soften it.
    #[test]
    fn the_plan_denial_survives_an_allow_default() {
        let perms = ToolPermissionsConfig::default();
        assert_eq!(perms.default, PermissionAction::Allow);
        assert_eq!(
            effective_permission(Some(&perms), Some(ExecTier::Plan), unknown("file_write")),
            PermissionAction::Deny
        );
    }

    /// …and unlike at every OTHER tier, an operator who NAMED the tool does
    /// not win. The planning refusal is rung 0.
    ///
    /// This deliberately inverts what this tier shipped with. The original
    /// reasoning — "an explicit entry is a decision a human wrote, and the
    /// tier never gets a word in about a tool it names" — is correct for
    /// `Ask` / `Auto` / `Full`, which only ever *ask*: naming the tool answers
    /// the very question the tier was going to pose. `Plan` poses no question.
    /// Its promise, restated to the model on every turn of a planning
    /// conversation, is that nothing runs. A `"file_write" = "allow"` written
    /// months earlier for an unrelated reason would retract that promise with
    /// no surface saying so — and only on installs whose operator configured
    /// tools at all, i.e. the ones with the most to lose.
    #[test]
    fn an_explicit_allow_cannot_hollow_out_the_planning_floor() {
        let mut perms = ToolPermissionsConfig::default();
        perms
            .overrides
            .insert("file_write".to_string(), PermissionAction::Allow);
        assert_eq!(
            effective_permission(Some(&perms), Some(ExecTier::Plan), unknown("file_write")),
            PermissionAction::Deny,
            "a named tool must not outrank the floor the phase is made of"
        );
    }

    /// The scope proof for rung 0: every tier that ASKS is byte-identical to
    /// before the floor existed. Without this assertion, "the floor works" and
    /// "the floor ate the permission model" look the same from the suite.
    #[test]
    fn the_planning_floor_leaves_every_asking_tier_untouched() {
        let mut perms = ToolPermissionsConfig::default();
        perms
            .overrides
            .insert("file_write".to_string(), PermissionAction::Allow);
        for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            assert_eq!(
                effective_permission(Some(&perms), Some(tier), unknown("file_write")),
                PermissionAction::Allow,
                "{tier:?}: an explicit entry still decides — no tier but `Plan` refuses"
            );
        }
    }

    /// Rung 0 keys on the tier's OWN `Deny`, never on a composed one. So a
    /// `default = "deny"` install is still a baseline an explicit `allow`
    /// outranks — at every tier, `Plan` included, for a tool the floor has
    /// nothing to say about.
    #[test]
    fn a_denying_default_is_a_baseline_not_a_floor() {
        let mut perms = ToolPermissionsConfig {
            default: PermissionAction::Deny,
            ..Default::default()
        };
        perms
            .overrides
            .insert("ctx_search".to_string(), PermissionAction::Allow);
        let facts = builtin("ctx_search");
        assert!(
            facts.idempotent,
            "fixture: `ctx_search` must read as idempotent, else this asserts nothing"
        );
        for tier in [
            ExecTier::Plan,
            ExecTier::Ask,
            ExecTier::Auto,
            ExecTier::Full,
        ] {
            assert_eq!(
                effective_permission(Some(&perms), Some(tier), facts),
                PermissionAction::Allow,
                "{tier:?}: a denying DEFAULT is not a floor"
            );
        }
    }

    /// The other direction, which is the one a floor gets wrong quietly: it
    /// adds refusals, it never grants. `Plan` answers `None` for every
    /// idempotent tool, so those fall through to rung 1 and an operator's
    /// explicit `deny` still denies while planning.
    #[test]
    fn the_planning_floor_never_grants_what_the_operator_denied() {
        let mut perms = ToolPermissionsConfig::default();
        perms
            .overrides
            .insert("ctx_search".to_string(), PermissionAction::Deny);
        assert_eq!(
            effective_permission(Some(&perms), Some(ExecTier::Plan), builtin("ctx_search")),
            PermissionAction::Deny,
            "the floor must not become a grant for tools it is silent about"
        );
    }

    /// The prompt line must describe the EXIT, not only the current state:
    /// it is still on the wire, unchanged, for the rest of the turn in which
    /// the gate lifts.
    #[test]
    fn the_plan_prompt_line_states_the_handoff() {
        let line = ExecTier::Plan.approval_prompt_line();
        assert!(line.contains("request_approval"));
        assert!(line.contains("scratchpad"));
        assert!(line.contains("same turn"));
        // Distinct from every other tier's line.
        for other in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            assert_ne!(line, other.approval_prompt_line());
        }
    }

    /// The composition this arm exists to break: `self_config` raises no card
    /// of its own, so without it the model can retire the `root:`/`frozen:`
    /// checkpoint in ONE un-carded call and every later write sails through.
    #[test]
    fn writing_a_tool_permission_override_asks() {
        let write = serde_json::json!({
            "action": "update_config",
            "config_path": "policies.tool_permissions.overrides",
            "config_value": {"loop_graph": "allow"},
        });
        assert!(ExecTier::Auto.asks_for_arguments("self_config", &write));
    }

    /// A write one (or several) levels up carries the same payload.
    #[test]
    fn an_ancestor_path_write_asks_too() {
        for path in ["policies", "", "policies.exec_tier"] {
            let write = serde_json::json!({
                "action": "update_config",
                "config_path": path,
                "config_value": {"exec_tier": "full"},
            });
            assert!(
                ExecTier::Auto.asks_for_arguments("self_config", &write),
                "a write at '{path}' reaches the gate-deciding config"
            );
        }
    }

    /// A snapshot restore reinstates whatever it held, path-blind.
    #[test]
    fn rolling_back_a_snapshot_asks() {
        let rollback = serde_json::json!({"action": "rollback_config", "timestamp": "x"});
        assert!(ExecTier::Auto.asks_for_arguments("self_config", &rollback));
    }

    /// …and it asks under **every** tier, `full` included.
    ///
    /// The composition above ("write the override, then act freely") was left
    /// open on the tier where it costs least: `asks_for_arguments` opened with
    /// `if self != Auto { return false }`, so at `full` the model could retire
    /// the checkpoint in one un-carded call. The tier is a per-request knob
    /// (`chat.send{exec_tier}`) and the config change is install-wide and
    /// permanent, so one message at `full` bought a gate removal that outlived
    /// it — in every later session, at every later tier.
    #[test]
    fn the_gate_removal_write_asks_under_every_tier() {
        let writes = [
            serde_json::json!({
                "action": "update_config",
                "config_path": "policies.tool_permissions.overrides",
                "config_value": {"loop_graph": "allow"},
            }),
            serde_json::json!({
                "action": "update_config",
                "config_path": "policies.exec_tier",
                "config_value": "full",
            }),
            serde_json::json!({"action": "rollback_config", "timestamp": "x"}),
        ];
        for tier in [ExecTier::Ask, ExecTier::Auto, ExecTier::Full] {
            for write in &writes {
                assert!(
                    tier.asks_for_arguments("self_config", write),
                    "{tier:?} must not stand down the gate-removal floor"
                );
                assert!(ExecTier::floor_asks_for_arguments("self_config", write));
            }
        }
        // Narrow: the floor is about the gate-deciding paths, not about
        // `self_config`. Ordinary configuration work is untouched at `full`.
        let ordinary = serde_json::json!({"action": "update_config", "config_path": "memory"});
        assert!(!ExecTier::Full.asks_for_arguments("self_config", &ordinary));
        assert!(!ExecTier::floor_asks_for_arguments(
            "self_config",
            &ordinary
        ));
        // And nothing else joins the floor by accident.
        assert!(!ExecTier::floor_asks_for_arguments(
            "file_ops",
            &serde_json::json!({"operation": "delete"})
        ));
    }

    /// The model is told what still stops at `full`, and the copy has to name
    /// the floors that exist. Adding a third floor without touching this line
    /// is the "三份拷贝，其中一份是发给模型的" failure.
    #[test]
    fn the_full_tier_prompt_line_names_every_floor() {
        let line = ExecTier::Full.approval_prompt_line();
        assert!(line.contains("Three floors"), "{line}");
        assert!(line.contains("self_config"), "{line}");
        assert!(line.contains("command-policy"), "{line}");
        assert!(line.contains("confirmation gate"), "{line}");
    }

    /// The cost has to stay narrow, or the card becomes noise and gets turned
    /// off — which is the same failure by another route.
    #[test]
    fn ordinary_self_config_work_still_does_not_ask() {
        for (action, path) in [
            ("update_config", "memory"),
            ("update_config", "providers.openai"),
            ("update_config", "policies.exec_tier_legacy"),
            ("read_config", "policies.tool_permissions"),
            ("list_files", ""),
        ] {
            let call = serde_json::json!({"action": action, "config_path": path});
            assert!(
                !ExecTier::Auto.asks_for_arguments("self_config", &call),
                "{action} at '{path}' must not raise a card"
            );
        }
    }

    /// Segment-aware, in both directions — the prefix test that is not one.
    #[test]
    fn dot_path_intersection_is_segment_aware() {
        assert!(dot_paths_intersect("policies", "policies.exec_tier"));
        assert!(dot_paths_intersect("policies.exec_tier", "policies"));
        assert!(dot_paths_intersect(
            "policies.exec_tier",
            "policies.exec_tier"
        ));
        assert!(dot_paths_intersect("", "policies.exec_tier"));
        assert!(!dot_paths_intersect("policies_x", "policies"));
        assert!(!dot_paths_intersect(
            "policies.exec_tier_legacy",
            "policies.exec_tier"
        ));
        assert!(!dot_paths_intersect("memory", "policies"));
    }

    #[test]
    fn deserializes_from_policies_toml() {
        let toml_str = r#"exec_tier = "ask""#;
        let cfg: super::super::PoliciesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.exec_tier, ExecTier::Ask);
        // Unconfigured installs stay on today's behavior.
        let empty: super::super::PoliciesConfig = toml::from_str("").unwrap();
        assert_eq!(empty.exec_tier, ExecTier::Auto);
    }
}
