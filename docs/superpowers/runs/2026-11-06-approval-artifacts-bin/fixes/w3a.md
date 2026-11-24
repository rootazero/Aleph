# Wave 3A — fix report (W3A-DC-01, W3A-DC-02)

Branch: `fix-wave3a` (base `907d92942` — "fix(review): merge fix-wave2b second-pass review fixes")
Scope: `src/approval/callback_sink.rs`, `src/approval/config.rs`, `src/exec/manager.rs`
Brief: `.review/rd-2026-11-06-approval-artifacts-bin/fixes-briefs/w3a.md`
No cargo / clippy / rustfmt / test run was performed here (the worker contract forbids it; the orchestrator owns the single unified `cargo check`).

---

## W3A-DC-01 — Originator callback authorization was not atomic across approval-ID reuse

### Verdict

**FIXED.** The finding reproduces on the base commit: the sink took the originator decision from its own `get_pending` snapshot (`src/approval/callback_sink.rs`), while the manager's locked path (`src/exec/manager.rs`, `resolve_by_id`) rechecked liveness and the operator-only exclusion but **never** the originator. An approval id is reusable once its card is resolved (`await_registered` removes the pending entry, `register_pending` only refuses a duplicate id while the old entry is still present), so a delayed callback could pass the snapshot check as Alice, and then resolve a card that had meanwhile been registered under the same id for Bob.

### Files

- `src/exec/manager.rs`
- `src/approval/callback_sink.rs`

### Change

1. `src/exec/manager.rs:425` — new `pub(crate) enum ApprovalResolveOutcome { Resolved { decision }, NotFound, OperatorOnly, NoOriginator, WrongOriginator, NotLive, NotOffered }`: the locked path now reports **what it did** instead of a bare `false`, so the caller can tell the user why a card was refused and can report the *effective* (post-clamp) decision.
2. `src/exec/manager.rs:455` — new private `enum ResolutionAuthority<'a> { Operator, Member, ChannelUser(&'a str) }`: one place names, per resolution path, whether the operator-only exclusion applies and whether the answering user must be the card's originator. This replaces the old `owner_only: bool` parameter.
3. `src/exec/manager.rs:806` — `resolve_by_id` takes the authority and returns `ApprovalResolveOutcome`. Under the **same write lock**, the order is: not found → operator-only → not live → **originator** → decision not offered → clamp → resolve → cascade → audit. The originator gate compares the answering user with `entry.record.originator_user_id` **inside** that locked step: a card raised by somebody else is `WrongOriginator`, a card that recorded **no** originator is `NoOriginator` (refused for a channel button — only the operator path may answer it).
4. `src/exec/manager.rs:791` — `resolve_for_channel(id, decision, channel_user) -> ApprovalResolveOutcome` (see the signature note below). The answering user is now an argument rather than a `resolved_by` label; the old `resolved_by` was only written into the audit fields and is now derived from the answering user.
5. `src/approval/callback_sink.rs:67` — the sink no longer reads `get_pending` for the originator gate and no longer snapshots `allowed_decisions` for clamping. It calls `resolve_for_channel(&id, decision, user_id)` and maps the outcome to the **existing** message strings (`WrongOriginator` → "只有发起该操作的用户可以在此审批。", `NoOriginator` → "此审批没有关联的发起人，只能由操作员通过 operator RPC 处理。", `OperatorOnly` → "此审批仅可由操作员通过 operator RPC 处理。", `NotFound | NotLive | NotOffered` → "This approval has expired or already been processed."). The reply text is now derived from the manager's returned (post-clamp) decision, so the "✅ Allowed (always)" echo cannot diverge from what was actually applied — the TOCTOU between the snapshot and the write is gone.

**Preserved deliberately:** the early pre-parse operator check (`is_operator_approval`), so an operator card is refused even for an unparsable action token; the operator RPC path (`resolve_with_reason`) and the member RPC path (`resolve_for_channel_with_reason`, called from `src/gateway/handlers/exec_approvals.rs:264`) — both keep their signatures and their behaviour, including the operator-only exclusion and the cascade scope (`Originator` for member/channel answers, `Any` for the operator); the audit row; every existing message string.

**Signature change (flagged loudly):** `ExecApprovalManager::resolve_for_channel` was `pub(crate) fn (&self, id: &str, decision: ApprovalDecisionType, resolved_by: Option<String>) -> bool` and is now `pub(crate) fn (&self, id: &str, decision: ApprovalDecisionType, channel_user: &str) -> ApprovalResolveOutcome`. Its only production caller is the callback sink (updated); the two manager tests that called it were updated. `resolve_for_channel_with_reason` keeps its name and its `bool` result even though it is the *member* RPC path — a pre-existing naming smell left alone on purpose (renaming it would touch `src/gateway/handlers/exec_approvals.rs`, outside this wave's scope). No public API grew: the new enum is `pub(crate)`.

### Risk

- **LOW** for the operator and member RPC paths: their inputs, outputs and decisions are unchanged (`bool` in, `bool` out); only the internal parameter changed from a bool to an enum.
- **LOW–MEDIUM** for the channel path: the two refusals the sink used to make itself (another user's card, a card with no originator) are now made by the manager under the lock — same outcomes, same messages. The intended behaviour change is the removal of the TOCTOU window: a card swapped under the same id between the check and the write can no longer be answered by the wrong user.
- The reply text is now the manager's *effective* decision rather than the sink's snapshot of `allowed_decisions`. For a stable card the two are identical (`clamp_decision` ≡ `decision.clamped_for(allowed)`, `src/exec/manager.rs:1328`); they could differ only in the replaced-card case, which is exactly the case being fixed.
- **Not changed on purpose:** the member RPC path still does **not** check the record's originator (that path resolves ownership through the session the card belongs to, not through the record) — the brief scoped the fix to the channel callback path.

### Tests added (not executed here)

- `src/exec/manager.rs:2609` `a_channel_callback_cannot_answer_another_users_card` — bob is refused (`WrongOriginator`), the card stays pending, alice answers it.
- `src/exec/manager.rs:2645` `a_reused_approval_id_is_answered_by_the_card_of_the_current_author` — alice's card is resolved and leaves the live set, bob registers the **same id**, alice's delayed callback is refused and bob's card stays pending, bob answers it.
- `src/approval/callback_sink.rs:266` `a_delayed_callback_does_not_answer_a_reused_approval_id` — the same race through the sink's public entry point.
- Updated: `src/exec/manager.rs:1539` and `src/exec/manager.rs:2576` (the two existing `resolve_for_channel` call sites now assert on the outcome enum).

---

## W3A-DC-02 — URL case folding changed case-sensitive path and non-URL text

### Verdict

**FIXED.** The finding reproduces on the base commit: `decide` folded the target of **every** action (`src/approval/config.rs:559`), the rule pattern was folded at compile time for **every** action (`src/approval/config.rs:253`), and `canonicalize_url_case` looped over **every** `://` span in the string (`src/approval/config.rs:756`) — so `https://example.com/a://BLAH` became `https://example.com/a://blah` (the path text after the authority was folded), and a script/selector/cookie-value target that merely contained `://` was folded as well.

### Files

- `src/approval/config.rs`

### Change

1. `src/approval/config.rs:772` — new `fn target_is_url(action: &ActionType) -> bool`: true for `BrowserNavigate` and `BrowserOpen`, and for an action that **inherits** a URL action's rules (`BrowserSwitchEngine` → `BrowserOpen`), so the rule side and the target side always fold alike.
2. `src/approval/config.rs:571` (`decide`) — the target is folded **only** for a URL action; every other target (a script, a selector, a field, a cookie value, a file path) is compared literally.
3. `src/approval/config.rs:257` (`compile_rules_grouped`) — the rule pattern is folded **only** for a URL action, so a non-URL rule is not silently case-folded either. `CompiledRule::pattern` still keeps the operator's original text (it is what the deny reason and the audit log quote).
4. `src/approval/config.rs:798` (`canonicalize_url_case`) — folds **only the first** scheme/authority span: the scheme token and the host are lower-cased (the userinfo before the last `@` keeps its case, it is a credential), and everything after the authority — path, query, fragment, and any text that merely *contains* `://` — is copied unchanged. The old loop over all `://` spans is gone.

### Risk

- **LOW.** The scheme+host folding for URL actions is preserved, so the existing expectations still hold (`a_mixed_case_url_scheme_and_host_still_hit_the_blocklist_rule`, `the_url_path_stays_case_sensitive`, `a_rule_pattern_never_reaches_the_log_in_the_clear`).
- The behaviour change is narrow and is the point of the finding: a non-URL target/rule is no longer case-folded, and a `://` after the authority is no longer folded. A rule that **relied** on the old over-folding (e.g. a `BrowserEvaluate` rule written in upper case matching a lower-case script) now stops matching — that is the defect being removed.
- `target_is_url` mirrors `applicable_rules`' inheritance, so a rule inherited from a URL action and the target of that action are folded identically; a rule inherited from a non-URL action (`BrowserIdentityOverride` / `BrowserSessionState` → `BrowserCookiesWrite`) is unfolded on both sides.
- The policy file format, the deny reason text and the audit fields are unchanged.

### Tests added (not executed here)

- `src/approval/config.rs:1537` `only_the_scheme_and_the_host_of_a_url_are_folded` — a blocklist rule `https://example.com/a://BLAH*` must not deny the target `https://example.com/a://blah-payload` (under the old folding it did).
- `src/approval/config.rs:1561` `a_non_url_target_is_compared_literally` — a `BrowserEvaluate` rule `*://EXAMPLE.COM/*` must not deny the script target `fetch('https://example.com/x')` (under the old folding it did).

---

## What was NOT done

- **No build, lint or test run.** The worker contract forbids cargo / clippy / rustfmt / tests here; the orchestrator runs the single unified `cargo check`. The new code was reviewed by hand, line lengths were checked (every added line ≤ 100 columns) and the two language-level questions — match ergonomics on a `&ActionType` scrutinee and the rewritten `canonicalize_url_case` — were checked with standalone `rustc` snippets outside the repository (no repo build was triggered).
- **Pre-existing naming smell left in place:** `resolve_for_channel_with_reason` is the *member* RPC path but keeps the "for_channel" name; the new channel path took the `resolve_for_channel` name. Renaming the member path would touch `src/gateway/handlers/exec_approvals.rs` — out of this wave's scope. Surfaced for a follow-up.
- **Pre-existing quirk left in place:** `is_operator_approval` reports DEAD operator entries, so a callback whose id collides with a dead operator card is refused with the operator message before the live record is consulted. That is fail-closed (a false refusal, not a bypass), it predates this finding, and the early check is kept on purpose so an operator card is refused even for an unparsable action token.
- **No change** to the gateway RPC handlers, the audit hooks, the session-grant cascade, the operator RPC path's authorization, the policy file format, or the `ApprovalCallbackSink` trait.
- The `ApprovalResolveOutcome` enum is `pub(crate)`: it is not exposed to the gateway layer, which still sees `bool` from the RPC paths.
