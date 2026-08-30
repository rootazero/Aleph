# Logic Review Report
**Module**: src/routing
**Scope**: 9 files (~3847 LOC), end-to-end audit
**Date**: 2026-08-28
**Mode**: strict

## Findings

### [Critical] `parse` cannot reconstruct `SessionKey::Main` — `[main_key]` arm is length-1 only, so the canonical `agent:<id>:main` roundtrips as `Task`
- **Location**: `src/routing/session_key.rs:640` (the `[main_key]` arm in `parse_rest`)
- **Trigger condition**: Any session key whose wire form is `agent:<agent_id>:main` (the canonical output of `SessionKey::Main { agent_id, main_key: "main", .. }.to_key_string()` and `SessionKey::dm(..., DmScope::Main).to_key_string()`), e.g. `agent:main:main`, `agent:work:main`, `agent:main:main:s3`.
- **Expected behavior**: `SessionKey::parse("agent:main:main").unwrap()` returns `SessionKey::Main { agent_id: "main", main_key: "main", epoch: 0 }`. `SessionKey::main("work").to_key_string() → parse → to_key_string` is the identity.
- **Actual behavior**: With `parts = ["agent", "main", "main"]`, `rest = &parts[2..] = ["main", "main"]`. `strip_epoch` returns `None` because the last element does not start with `s`. The `if let Some(...)` block is skipped, so execution falls through to `Self::parse_rest(&agent_id, rest, 0)`. Inside `parse_rest` the only `[main_key]` arm is **`[main_key]` (length 1 only)**; for a length-2 slice it doesn't match. The reserved-task-type guard `[task_type, task_id] if matches!(*task_type, "peer" | "dm" | "subagent" | "ephemeral") => None` doesn't fire either, because `"main"` is not in the reserved list. The catch-all `[task_type, task_id] => Some(Task)` therefore wins and returns `SessionKey::Task { task_type: "main", task_id: "main" }` (or `task_id = "s3"` for the `:s3` epoch form, after the `prefer_direct` logic at lines 539–542 forces a fall-through to the no-epoch parse). Effect:
  - `test_parse_main` (line 938), `test_parse_with_epoch` (line 1082), `test_roundtrip` (line 1044, includes `SessionKey::main("work")`), `test_dm_scope_main_collapses` (`src/routing/resolve.rs:718` — `assert_eq!(route.session_key.to_key_string(), "agent:main:main")` plus an implicit round-trip to `current_epoch`) and `is_interactive_true_for_human_variants` all rely on this round-trip and would fail.
  - `SessionKey::Main` is the only shape carrying `epoch`. If a real key is mis-parsed as `Task`, the orchestrator's `current_epoch` lookup (`gateway/inbound_router/agent_resolver.rs:215` via `base_key_pattern`) still resolves correctly because `base_key_pattern` is matched directly on the string — but the parsed struct used downstream is wrong, and any consumer that pattern-matches the returned `SessionKey` (e.g. `is_interactive()` for the strategic-planner gate, or `epoch()` for the `/new` epoch bump) silently gets a `Task` with `epoch() == 0` and `is_interactive() == false` — turning the agent's main session into a "non-interactive, no-epoch" handle, suppressing the planner on every Main turn and ignoring `/new`.
- **Suggested fix**:

  ```rust
  // Current (wrong) — single-segment Main arm only.
  // src/routing/session_key.rs:640
  [main_key] if !matches!(*main_key, "peer" | "dm" | "subagent" | "ephemeral") => {
      Some(Self::Main {
          agent_id: agent_id.to_string(),
          main_key: main_key.to_string(),
          epoch,
      })
  }

  // Proposed (fixed) — add a length-2 arm that maps any `agent:<id>:<main_key>`
  // to Main, and tighten the single-segment arm's name accordingly.
  // The new arm must (a) reject the structural markers it deliberately avoids
  // ("peer" / "dm" / "subagent" / "ephemeral" — handled by the length-1 arm
  // already) and (b) co-exist with the existing DM/Subagent/Ephemeral arms
  // (which all match specific tokens, not the bare `<x>:<y>` shape), so the
  // order-only change is to insert this arm BEFORE the
  // `[task_type, task_id]` catch-all (which currently wins and must lose for
  // any 2-segment rest).
  ["main", main_key]
      if !matches!(*main_key, "peer" | "dm" | "subagent" | "ephemeral") =>
  {
      // Restrict to the historical Main wire form. `agent:id:main` is the only
      // Main whose second segment is the literal token "main"; other 2-segment
      // rests (e.g. `agent:id:foo`) round-trip as Task { task_type: "id",
      // task_id: "foo" } and stay there. The dedicated arm below generalises
      // to any `<agent_id>:<main_key>` Main shape (`agent:id:room-7`,
      // `agent:id:p-7f3a9c`) by accepting any non-reserved second token.
      Some(Self::Main {
          agent_id: agent_id.to_string(),
          main_key: main_key.to_string(),
          epoch,
      })
  }
  [main_key] if !matches!(*main_key, "peer" | "dm" | "subagent" | "ephemeral") => {
      Some(Self::Main {
          agent_id: agent_id.to_string(),
          main_key: main_key.to_string(),
          epoch,
      })
  }
  ```

  Note: the existing tests at lines 938, 1044, 1082, 1078, 1090, 1103, 1113, 1125 in `session_key.rs` and lines 718 / 790 / 793 in `resolve.rs` already pin the desired behaviour; they will start passing once this arm is added. The new arm must precede the catch-all `[task_type, task_id]` arm at line 651.

---

### [Critical] Wiring gap: `identity_links` never takes effect on the zero-config fallback (`resolve_session_key_with_agent`)
- **Location**: `src/gateway/inbound_router/agent_resolver.rs:251–293` (`resolve_session_key_with_agent`); documented as a known gap in `src/routing/config.rs:25–38`.
- **Trigger condition**: A deployment configured with `[session] identity_links` (cross-channel ID → canonical mapping) and **no `[[bindings]]`** (or bindings that don't catch the incoming conversation). Every DM from such a deployment uses the raw `msg.sender_id` as the session-key peer segment, so the same person writing to the bot from `telegram:123` and `discord:456` gets two conversations — the documented "Sessions shared across platforms" promise is broken silently.
- **Expected behavior**: `resolve_session_key_with_agent` calls `resolve_linked_peer_id(identity_links, channel, sender_id)` and uses the canonical name, mirroring what `resolve_route` does on the bindings path (`src/routing/resolve.rs::build_session_key` at lines 215–220).
- **Actual behavior**: The function builds the session key with `SessionKey::dm(agent_id, channel, msg.sender_id.as_str(), dm_scope)` and never consults `self.config.identity_links`. The `SessionConfig` carries `identity_links` but it is only read by `build_session_key` inside `resolve_route`. The `config.rs` comment at lines 26–35 already names this as a wiring gap and proposes a fix; this audit confirms it is still open and identifies the exact consumer that needs the consultation.
- **Suggested fix**:

  ```rust
  // Current (wrong) — src/gateway/inbound_router/agent_resolver.rs:251
  pub(super) fn resolve_session_key_with_agent(
      &self,
      msg: &InboundMessage,
      agent_id: &str,
  ) -> SessionKey {
      let channel = msg.channel_id.as_str();
      if msg.is_group {
          SessionKey::group(
              agent_id, channel,
              crate::routing::session_key::PeerKind::Group,
              msg.conversation_id.as_str(),
          )
      } else {
          SessionKey::dm(
              agent_id, channel,
              msg.sender_id.as_str(),                       // ← raw sender id
              match self.config.dm_scope { ... },
          )
      }
  }

  // Proposed (fixed) — apply the same identity-link resolution that the
  // bindings path uses, so a deployment relying on `[session] identity_links`
  // does not silently fork a user's sessions across channels.
  pub(super) fn resolve_session_key_with_agent(
      &self,
      msg: &InboundMessage,
      agent_id: &str,
  ) -> SessionKey {
      let channel = msg.channel_id.as_str();
      if msg.is_group {
          SessionKey::group(
              agent_id, channel,
              crate::routing::session_key::PeerKind::Group,
              msg.conversation_id.as_str(),
          )
      } else {
          // Mirror build_session_key (resolve.rs:215) — the two routing paths
          // must agree on the canonical name, otherwise the same person on
          // telegram:123 and discord:456 gets two conversations.
          let canonical = crate::routing::identity_links::resolve_linked_peer_id(
              &self.config.identity_links, channel, &msg.sender_id,
          );
          let peer_id = canonical.as_deref().unwrap_or(msg.sender_id.as_str());
          SessionKey::dm(
              agent_id, channel, peer_id,
              match self.config.dm_scope {
                  DmScope::Main => crate::routing::session_key::DmScope::Main,
                  DmScope::PerPeer => crate::routing::session_key::DmScope::PerPeer,
                  DmScope::PerChannelPeer =>
                      crate::routing::session_key::DmScope::PerChannelPeer,
              },
          )
      }
  }
  ```

  Note: `resolve_linked_peer_id` is `pub(crate)`; `inbound_router` is in the same crate, so the visibility works. The `SessionConfig.identity_links` is already carried by `InboundRouter::config` (see line 91 doc), so the data is reachable. A regression test should cover: same DM sender from two channels with a configured `identity_links` entry → both `resolve_session_key_with_agent` calls produce the same key.

---

### [Warning] `OutcomeObserver` silently drops records under concurrency load — only a `warn` log, no backoff, no metric
- **Location**: `src/routing/observer.rs:12–16` (`MAX_IN_FLIGHT_ROUTING_RECORDS = 8`, the `OnceLock<Arc<Semaphore>>`), `src/routing/observer.rs:147–157` (`try_acquire_owned` failure path).
- **Trigger condition**: More than 8 `SessionCompleted` trace events arrive within the duration of one SQLite write (`record_routing_experience` + `prune_routing_experiences` on `spawn_blocking`). On a healthy box this is rare; on a slow disk or under a burst of concurrent subagent completions it can happen. Also reachable deterministically if the user spawns a fan-out team (>8 children all returning at once).
- **Expected behavior**: Either buffered (with bounded growth), retry-with-backoff, or an at-least-once promise surfaced as a counter so monitoring can detect sustained data loss.
- **Actual behavior**: `try_acquire_owned` returns `Err`, and the only observable side effect is a single `tracing::warn!("routing experience record dropped: concurrency limit reached")`. No counter, no per-run marker, no retry. The `task_emb` was already backfilled at run-start, so the run's actual `SessionCompleted` is still forwarded to the inner sink — only the routing-experience row is lost. This is a low-frequency, high-blast-radius data-loss mode that monitoring won't catch from the warn line alone.
- **Current impact**: medium
- **Suggestion**: Add a `static AtomicU64 ROUTING_RECORDS_DROPPED` incremented on the `Err` arm and expose it via `crate::diagnostics`. Or wrap `try_acquire_owned` in a `tokio::time::timeout` so the permit is queued and the record is best-effort scheduled. The simplest "do no harm" change is to keep the semaphore but log at `error` (not `warn`) and bump a counter; the operator already gets a single warn line on cold-start cold-cache anyway, and the two failure modes are indistinguishable in the current logs.

  ```rust
  // Current — src/routing/observer.rs:147
  Err(_) => {
      tracing::warn!(
          session_id = %self.attribution.session_id,
          "routing experience record dropped: concurrency limit reached"
      );
  }

  // Proposed — surface the loss so it can be alerted on
  static ROUTING_RECORDS_DROPPED: AtomicU64 = AtomicU64::new(0);
  Err(_) => {
      let n = ROUTING_RECORDS_DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
      tracing::error!(
          session_id = %self.attribution.session_id,
          dropped_total = n,
          "routing experience record dropped: concurrency limit reached"
      );
  }
  ```

---

### [Warning] `OutcomeObserver::on_trace` silently skips recording when `task_emb` was never backfilled
- **Location**: `src/routing/observer.rs:142–158`
- **Trigger condition**: A `SessionCompleted` trace event fires for a session whose `RoutingAttribution::task_emb` is still `None`. Two production paths can do this:
  - The `agent_resolver.rs` Tier-3 default path: if `routing_recall` is `None` (e.g. embedder not configured at boot, see `src/bin/aleph-server/commands/start/orchestrator_init.rs:351`), `runner_impl.rs:454` short-circuits with `None` and never touches `task_emb`. The observer sees `task_emb.get() == None` and drops the entire record silently.
  - A failure in `store.embed_task(...).await?` at `recall.rs:110` propagates as `Err`, the warn at `runner_impl.rs:457` is logged, but again `task_emb` is never set; the observer drops the record.
- **Expected behavior**: At minimum, the record should be made with a zero vector (and the embed skipped) so the per-model aggregate count (`aggregate_by_model`) stays accurate. Or the observer should be explicit about "no embed available" and not write the row, but log a count so the missing data is visible.
- **Actual behavior**: Silent skip; no metric, no log. The `task_emb.get()` arm wraps the whole record path, so the `try_acquire_owned` semaphore is never even acquired — this is independent of the [Warning] above and does not increment `ROUTING_RECORDS_DROPPED` if added.
- **Current impact**: low–medium (only triggered when the embedder is absent or failing, which is a degraded path anyway)
- **Suggestion**: Either insert a sentinel zero vector before the record path (and document that aggregates use it for `last_used_unix`), or emit a structured warn so the silent loss is observable:

  ```rust
  // Current — src/routing/observer.rs:142
  if let Some(task_emb) = self.attribution.task_emb.get().cloned() {
      match self.record_slots.clone().try_acquire_owned() { ... }
  }

  // Proposed — at minimum, observe the skip
  if let Some(task_emb) = self.attribution.task_emb.get().cloned() {
      match self.record_slots.clone().try_acquire_owned() { ... }
  } else {
      static ROUTING_RECORDS_SKIPPED_NO_EMBED: AtomicU64 = AtomicU64::new(0);
      let n = ROUTING_RECORDS_SKIPPED_NO_EMBED.fetch_add(1, Ordering::Relaxed) + 1;
      tracing::warn!(
          session_id = %self.attribution.session_id,
          skipped_total = n,
          "routing experience record skipped: no embed backfilled (recall disabled or embedder error)"
      );
  }
  ```

---

### [Warning] `outcome_from_session_completed` is `pub` but never used outside the crate
- **Location**: `src/routing/observer.rs:32` (function signature)
- **Trigger condition**: Static review / future contributor. The function is marked `pub fn` yet has zero callers outside `src/routing/observer.rs` itself (the only external reference is the doc comment in `experience_store.rs:75`).
- **Expected behavior**: Visibility should match the actual usage surface — `pub(crate)` so the function stays available to the test mod (which lives in the same crate) without inviting external callers to depend on a private construction detail.
- **Actual behavior**: The function is exported in the module's `pub` surface even though no other module imports it. This is a minor API smell, not a functional bug — but combined with the fact that `RoutingOutcome` itself has all fields `pub`, external callers can fabricate `RoutingOutcome` rows and write arbitrary model/provider IDs into `routing_experiences` if they acquire a `RoutingExperienceStore`. The intended write path is the observer, and `outcome_from_session_completed` should be the only allowed constructor.
- **Current impact**: low (style / API surface)
- **Suggestion**:

  ```rust
  // Current — src/routing/observer.rs:32
  pub fn outcome_from_session_completed(
      iterations: usize,
      ...
  ) -> RoutingOutcome { ... }

  // Proposed
  pub(crate) fn outcome_from_session_completed(
      iterations: usize,
      ...
  ) -> RoutingOutcome { ... }
  ```

  Tests inside the `#[cfg(test)] mod tests` block at line 247 still see `pub(crate)` because they live in the same crate. Add a regression test that exercises this constructor as the canonical `RoutingOutcome` builder (already present as `outcome_maps_raw_without_verdict`, line 247).

---

### [Warning] `to_key_string` for `DirectMessage { dm_scope: PerPeer, channel = "" }` always emits the legacy `peer:` form, but `parse` treats `dm:` and `peer:` as different shapes — silent round-trip asymmetry
- **Location**: `src/routing/session_key.rs:431` (`to_key_string` for DirectMessage) and `parse_rest` arms at lines 575–592 (DM shape matches).
- **Trigger condition**: `SessionKey::dm("main", "", "alice", DmScope::PerPeer).to_key_string()` returns `"agent:main:peer:alice"` (the `PerPeer` empty-channel branch in `format_dm_base` at lines 432–436). `parse("agent:main:peer:alice")` returns `DirectMessage { channel: "", dm_scope: PerPeer, peer_id: "alice" }`. That round-trips. BUT `SessionKey::dm("main", "telegram", "alice", DmScope::PerPeer).to_key_string()` returns `"agent:main:dm:alice"` (the non-empty-channel PerPeer branch at line 437), and parsing `"agent:main:dm:alice"` with the same function also returns `DirectMessage { channel: "", dm_scope: PerPeer, ... }` — silently dropping the `telegram` channel from the parsed form. The `test_to_key_string_dm_per_peer` (line 877) and the doc-comment at line 1166–1168 acknowledge this asymmetry ("a `dm:` key drops the channel for PerPeer scope, so the parsed form has an empty channel and canonically re-serializes via the legacy `peer:` spelling"). It is intentional but fragile:
  - Two semantically identical sessions (one minted from `SessionKey::dm("main", "telegram", "alice", PerPeer)` and one from `SessionKey::dm("main", "", "alice", PerPeer)`) serialize to **different** wire strings: `"agent:main:dm:alice"` vs `"agent:main:peer:alice"`. They both parse back to the same struct (channel=""), but the lookup-by-string that `session_manager` uses keys on the wire form, so the two coexist as separate rows in `session_events` and the per-channel-peer intent is lost on serialization.
  - If `dm_scope` is later changed from `PerChannelPeer` to `PerPeer` for an existing channel-scoped key, the wire form collapses but the database row keeps the old key — `current_epoch` lookup via `base_key_pattern` works, but the underlying row is now keyed on a string that no incoming message will ever produce.
- **Current impact**: low to medium (data model divergence; visible only on `sessions.list` cross-references)
- **Suggestion**: Document this asymmetry at `to_key_string` (the existing comment at lines 1166–1168 is buried in a test). Either pick one canonical serialization for `DirectMessage { PerPeer, channel != "" }` (and reject empty channel at construction), or add a regression test that proves two constructions with different `channel` inputs but same logical peer produce the same wire form:

  ```rust
  #[test]
  fn dm_perpeer_channel_drop_is_an_acknowledged_asymmetry() {
      // Documented asymmetry: `dm:` keys with a non-empty channel drop the
      // channel on parse, so two minting paths produce different wire forms.
      // Either tighten `dm` to refuse a non-empty channel under PerPeer, or
      // pin this divergence as a feature with a test that fails if either
      // path changes.
      let with_channel = SessionKey::dm("main", "telegram", "alice", DmScope::PerPeer);
      let without = SessionKey::dm("main", "", "alice", DmScope::PerPeer);
      // If `dm()` is tightened to refuse non-empty channel under PerPeer,
      // this assert becomes the contract.
      assert_ne!(
          with_channel.to_key_string(),
          without.to_key_string(),
          "DM serialization divergence — pin as a feature or tighten dm()"
      );
  }
  ```

---

### [Warning] `from_legacy` legacy-`peer` arm uses `rest.join(":")` without `sanitize_component` — bypasses the normaliser that `parse` applies
- **Location**: `src/routing/session_key.rs:678–685`
- **Trigger condition**: A legacy session-key string that goes through `from_key_string` and was rejected by `parse`, e.g. `agent:main:peer:user:1` (peer_id containing a colon). The legacy fallback joins the rest verbatim: `peer_id = "user:1"`. The peer_id is then used downstream as part of `to_key_string` (`format!("agent:{agent_id}:{channel}:dm:{peer_id}")`), producing strings that no `parse` can round-trip back.
- **Expected behaviour**: Either normalise the joined peer_id through `sanitize_component` (which would replace the colon with `-`, breaking any pre-existing legacy key but matching what `parse` does), or document the asymmetry.
- **Actual behaviour**: `peer_id` retains the colon; subsequent `parse(key.to_key_string())` returns `None` because `parse` only accepts a single token as `peer_id`. Net effect: a key constructed via the legacy fallback cannot be looked up again, and any downstream consumer that persists `to_key_string()` and then tries to parse it on a different request will silently fail to find the session.
- **Current impact**: low (legacy fallback only; modern callers always use `parse`)
- **Suggestion**: Sanitise the joined peer_id to match `parse` semantics:

  ```rust
  // Current — src/routing/session_key.rs:679
  Some(&["peer", ref rest @ ..]) if !rest.is_empty() => Some(Self::DirectMessage {
      agent_id,
      channel: String::new(),
      peer_id: rest.join(":"),                  // ← un-normalised
      dm_scope: DmScope::PerPeer,
      epoch: 0,
  }),

  // Proposed — apply sanitize_component so the resulting key round-trips
  Some(&["peer", ref rest @ ..]) if !rest.is_empty() => Some(Self::DirectMessage {
      agent_id,
      channel: String::new(),
      peer_id: sanitize_component(&rest.join(":")),   // ← match parse semantics
      dm_scope: DmScope::PerPeer,
      epoch: 0,
  }),
  ```

  Note: this is a behaviour change for any caller that depended on colon-bearing peer ids surviving the legacy fallback. A test that exercises `agent:main:peer:user:1` through both `parse` and `from_key_string` and asserts equal `to_key_string` would pin the desired contract.

---

### [Warning] `OverlaySource::as_str` does not have a wildcard arm — adding a new variant silently breaks `gateway_route` tool output
- **Location**: `src/routing/overlay.rs:55–66`
- **Trigger condition**: A future contributor adds a new `OverlaySource` variant. `as_str` is non-exhaustive (it is a `match` over the concrete variants), so the change compiles. The `gateway_route` tool serialises the result as `OverlaidRoute.source.as_str().to_string()` (see `src/builtin_tools/gateway_route.rs` around lines 230–235). Any Panel code reading `matched_by` as a string will see an "unknown" value because `as_str` doesn't fall through to a default — wait, it does return `&'static str` literals only; a new variant without an arm is a compile error, not a runtime unknown. So this is more of a forward-compat hazard than a current bug. Flagging it because the doc comment for `as_str` says "Wire-stable label" — which is true today, but the lack of a `_ => unreachable!()` or wildcard arm means future maintainers can accidentally change the labels.
- **Current impact**: low (forward-compat)
- **Suggestion**: Either add a `_ => "unknown"` arm (with a comment) so the wire form degrades gracefully, or add an explicit `_ => unreachable!("add an arm for the new variant")` so the next contributor is forced to decide whether the new variant has a wire form.

  ```rust
  // Current — src/routing/overlay.rs:55
  pub const fn as_str(self) -> &'static str {
      match self {
          Self::Binding(m) => match m {
              MatchedBy::Peer => "peer",
              ...
              MatchedBy::Default => "default",
          },
          Self::ChannelOverride => "channel_override",
          Self::BindingAgentMissing => "binding_agent_missing",
      }
  }

  // Proposed — pin the contract for future variants
  pub const fn as_str(self) -> &'static str {
      match self {
          Self::Binding(m) => match m { ... },
          Self::ChannelOverride => "channel_override",
          Self::BindingAgentMissing => "binding_agent_missing",
          // Wire-stable label: a new OverlaySource variant MUST either get an
          // arm here (and a wire value that the Panel knows) or this match
          // will fail to compile. Don't add `_ => "unknown"` — silent unknown
          // wire values have already cost us one Panel rendering bug.
      }
  }
  ```

---

### [Warning] `binding_problems_flags_unwired_account_id` does not assert the converse — it can be read as "an `account_id` is fine" even for `*` when no channel currently feeds it
- **Location**: `src/routing/config.rs:264–298`
- **Trigger condition**: Future contributor reads the test and assumes `account_id = "*"` is unconditionally accepted. In reality the test only iterates over `["default", "*", None]` after asserting the negative case — it does NOT assert what happens when `account_id` is a non-default-but-unsupported string like `"botA"` (the test only asserts ONE such case is flagged, not the exhaustive list). A new test for `"botA"` would be redundant because the existing assert checks `any(...)`, but a maintainer adding a new "supported account id" (e.g. `"primary"`) would not realise they also need to update `report_account_problem`.
- **Current impact**: low (test quality)
- **Suggestion**: Add a positive-control test asserting that an exhaustive list of NON-supported values (e.g. `["botA", "team-2", "prod-account"]`) all produce at least one flagged problem, so a regression in `report_account_problem`'s whitelist is caught:

  ```rust
  #[test]
  fn binding_problems_exhaustively() {
      let unsupported = ["botA", "team-2", "prod-account", " BotA ", "DEFAULT"];
      for acct in unsupported {
          let bindings = vec![RouteBinding {
              agent_id: "main".into(),
              match_rule: MatchRule {
                  channel: Some("telegram".into()),
                  account_id: Some(acct.into()),
                  ..Default::default()
              },
          }];
          assert!(
              binding_problems(&bindings).iter().any(|p| p.contains("account_id")),
              "unsupported account_id {acct:?} must be flagged"
          );
      }
  }
  ```

---

### [Warning] `is_interactive` returns `false` for `Subagent` — a subagent child whose parent is a `Main` key is then treated as automated by the planner gate, but a top-level run on `SessionKey::main("a")` is interactive. The asymmetry is intentional (R7) but undocumented at the call site
- **Location**: `src/routing/session_key.rs:354` (definition), with consumers in the harness planner gate.
- **Trigger condition**: Future contributor adds a planner-bypass flag for "interactive" sessions and forgets that `Subagent` is non-interactive. Since `SessionKey::subagent(parent, "x").agent_id() == parent.agent_id()`, a query like "is this run's agent the same as the top-level agent" can be answered either way depending on which method is used.
- **Current impact**: low (correct today, fragile tomorrow)
- **Suggestion**: Either add a `pub fn is_interactive_for_planner(&self) -> bool` that explicitly names the gate this is for, or add a doc-comment cross-reference at the call site so a future change to either side is greppable:

  ```rust
  // Current — src/routing/session_key.rs:351
  /// Used by the naked agent-loop strategic-planner gate so a cron job /
  /// group-chat member / subagent's first turn never trips the planner (R7:
  /// an origin fact, not a message-content heuristic). Fail-closed: any
  /// future internal variant defaults to non-interactive.

  // Proposed — name the gate so future greppable behaviour changes land
  /// alongside the gate's update
  /// Used by `harness::agent::naked_loop_planner_gate` (single call site) so
  /// a cron / group-chat member / subagent first turn never trips the planner.
  /// Do not add new "interactive" variants without updating the gate.
  ```

---

### [Suggested Test] `observer records nothing when `task_emb` is `None` AND `record` is unreachable — currently a silent skip
```rust
#[tokio::test]
async fn observer_skips_record_when_attribution_has_no_task_emb() {
    // Regression: when the embedder is absent at boot (no `routing_recall`),
    // task_emb is never backfilled. The observer MUST still forward the
    // SessionCompleted to the inner sink, and MUST record some observable
    // signal that no row was written.
    let (_scratch, backend_inner) = temp_backend();
    let backend = Arc::new(backend_inner);
    let embedder: Arc<dyn EmbeddingProvider> = Arc::new(StubEmbedder { vec: emb(1.0) });
    let store = Arc::new(RoutingExperienceStore::new(backend, embedder));
    let spy = Arc::new(SpySink::default());
    let attribution = Arc::new(RoutingAttribution::new("noembed".into()));
    // attribution.task_emb is NEVER set.
    let observer = OutcomeObserver::new(
        spy.clone() as Arc<dyn TraceSink>,
        store.clone(),
        attribution,
        "MODEL_X".into(),
        "PROV_Y".into(),
        "agentNoEmbed".into(),
    );
    observer.on_trace(&session_completed());
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    // Forwarding happened.
    assert_eq!(spy.session_completed.load(Ordering::SeqCst), 1);
    // No row was written (silent skip today).
    let got = store.recall("agentNoEmbed", &emb(1.0), 5).await.unwrap();
    assert!(
        got.is_empty(),
        "no task_emb ⇒ no record (pin the contract for the suggested warn)"
    );
}
```

---

### [Suggested Test] `resolve_session_key_with_agent` applies `identity_links` on the zero-config path
```rust
#[tokio::test]
async fn zero_config_fallback_applies_identity_links() {
    use crate::routing::identity_links::resolve_linked_peer_id;
    use std::collections::HashMap;

    // Wire-gating regression: a deployment with `[session] identity_links`
    // and NO `[[bindings]]` must still collapse the same person across
    // channels into one session, mirroring what `resolve_route` does.
    let mut links = HashMap::new();
    links.insert("john".to_string(), vec!["telegram:123".into(), "discord:456".into()]);
    let config = SessionConfig {
        dm_scope: DmScope::PerPeer,
        identity_links: links,
    };
    // (The test must live in inbound_router — this is the contract the fix
    //  must pin.)
    let tg_key = inbound_router.resolve_session_key_with_agent(&tg_msg, "main");
    let dc_key = inbound_router.resolve_session_key_with_agent(&dc_msg, "main");
    assert_eq!(tg_key.to_key_string(), "agent:main:dm:john");
    assert_eq!(dc_key.to_key_string(), "agent:main:dm:john");
}
```

---

### [Suggested Test] `parse` round-trip for the canonical `agent:<id>:main` form (currently broken by the Critical finding)
```rust
#[test]
fn parse_reconstructs_main_with_id_and_main_key() {
    // The wire form `agent:work:main` must parse back to Main { agent_id:
    // "work", main_key: "main", epoch: 0 }. The current parse_rest has
    // a single-segment `[main_key]` arm only and falls through to the
    // Task catch-all for two-segment rests.
    let k = SessionKey::parse("agent:work:main").expect("must parse Main");
    assert!(matches!(
        k,
        SessionKey::Main { ref agent_id, ref main_key, epoch }
            if agent_id == "work" && main_key == "main" && epoch == 0
    ));

    // And the epoch form:
    let k = SessionKey::parse("agent:work:main:s3").expect("must parse Main with epoch");
    assert!(matches!(
        k,
        SessionKey::Main { ref agent_id, ref main_key, epoch: 3 }
            if agent_id == "work" && main_key == "main"
    ));

    // And the round-trip from the constructor:
    let k = SessionKey::main("work");
    let s = k.to_key_string();
    let parsed = SessionKey::parse(&s).expect("round-trip must succeed");
    assert_eq!(parsed.to_key_string(), s, "round-trip failed for {s}");
}
```

---

### [Suggested Test] `dm_perpeer_channel_drop` symmetry test pinning the documented asymmetry
```rust
#[test]
fn dm_perpeer_channel_serialization_is_asymmetric_but_stable() {
    // Documented at session_key.rs:1166-1168: `dm:` keys drop the channel on
    // parse, so two minting paths produce different wire forms. Pin the
    // contract — if the asymmetry is intentional, this test guards against
    // accidental tightening of `dm()` that would break it.
    let a = SessionKey::dm("main", "telegram", "alice", DmScope::PerPeer);
    let b = SessionKey::dm("main", "", "alice", DmScope::PerPeer);
    assert_eq!(a.to_key_string(), "agent:main:dm:alice");
    assert_eq!(b.to_key_string(), "agent:main:peer:alice");
    // Both parse to the same struct shape (channel="").
    let pa = SessionKey::parse(&a.to_key_string()).unwrap();
    let pb = SessionKey::parse(&b.to_key_string()).unwrap();
    assert!(matches!(pa, SessionKey::DirectMessage { channel, peer_id, .. }
        if channel == "" && peer_id == "alice"));
    assert!(matches!(pb, SessionKey::DirectMessage { channel, peer_id, .. }
        if channel == "" && peer_id == "alice"));
}
```

---

## Wiring Gaps (this module → outside)

| Item | Type | Status | Should be used by |
|------|------|--------|------------------|
| `RoutingAttribution::task_emb` | `OnceLock<Vec<f32>>` | wired via `runner_impl.rs:348` (writer) and `runner_impl.rs:453` (reader, via `build_routing_experience_message`). Also `subagent_spawner/mod.rs:711–715` (writer). | OK: write-once, thread-safe. |
| `OutcomeObserver` | `TraceSink` impl | wired via `runner_impl.rs:691` (parent) and `subagent_spawner/mod.rs:716` (child). Also wrapped via `routing_recall` path. | OK. |
| `RoutingRecall::build_routing_experience_message` | async fn | **WIRED** in `runner_impl.rs:453`. | OK. |
| `provider_availability_from_config` | pub fn | **WIRED** in `bin/aleph-server/commands/start/orchestrator_init.rs:358`. | OK. |
| `binding_problems` | pub fn | **WIRED** in `src/bin/aleph-server/commands/start/builder/subsystems.rs:975`. | OK. |
| `validate_identity_links` | pub fn | used as serde-deserialize hook at `config.rs:17`. | OK. |
| `outcome_from_session_completed` | pub fn | only used inside `observer.rs` (the test mod at line 247). **OVER-VISIBLE** — should be `pub(crate)`. | none outside. |
| `overlay_route` | pub fn | **WIRED** in `src/gateway/inbound_router/agent_resolver.rs:124`. | OK. |
| `session_keys_for` | pub fn | **WIRED** in `src/builtin_tools/gateway_route.rs:214` and `resolve.rs:158` (internal). | OK. |
| `from_key_string` | pub fn | **WIRED** in 100+ call sites (hand-wave). | OK. |
| `from_legacy` | pub fn | **WIRED** as fallback of `from_key_string`. | OK. |
| `with_epoch` | pub fn | **WIRED** in 9 sites. | OK. |
| `with_next_epoch` | pub fn | **WIRED** in 14 sites. | OK. |
| `base_key_pattern` | pub fn | **WIRED** in 14 sites. | OK. |
| `project_room` | pub fn | **WIRED** in 13 sites (incl. `execution_engine/run_loop/tests.rs:1174/1242`). | OK. |
| `is_interactive` | pub fn | **WIRED** in 3 sites (naked-loop planner gate). | OK. |
| `MAX_IN_FLIGHT_ROUTING_RECORDS` constant | `const usize` | only used inside `observer.rs`. | OK. |
| `DEFAULT_RECALL_K` | `pub const usize` | only used inside `recall.rs`. | OK. |
| `DEFAULT_ROUTING_RETENTION_CAP` | `pub const usize` | only used inside `experience_store.rs`. | OK. |
| **CRITICAL GAP**: `identity_links` resolution on zero-config fallback | fn | **NOT WIRED** — `resolve_session_key_with_agent` (agent_resolver.rs:251) never calls `resolve_linked_peer_id`. | See Critical finding #2. |
| `OverlaySource` variants | enum | **FULLY COVERED** — `as_str` and `overlay_route` both handle all 3 variants. | OK. |
| `MatchedBy` variants | enum | **FULLY COVERED** — `as_str`, `overlay_route` (Specific check), `resolve_route` tier walk all handle all 6 variants. | OK. |
| `LoopTraceEvent::SessionCompleted` fields | struct fields | `outcome_from_session_completed` extracts 6 fields; `total_tokens` / `hit_limit` / `final_text` / `outcome` are ignored. | The comment at `observer.rs:29–31` documents the omission as intentional ("Pure: counts and discriminants only, zero interpretation; never reads judgment signals from `LoopTraceTurnMetrics` (not present on `SessionCompleted`)"). OK. |
| `RoutingOutcome` fields | struct | All 8 fields are written by the observer; only `context_tokens` / `context_window` are hardcoded `0` (documented at `experience_store.rs:73–82`). | OK, documented as deliberate. |

---

## Lock/Cross-Module Concerns

**Lock hierarchy compliance (Level 2 — `sync_primitives.rs:23–28`):**

- The routing module uses **`std::sync::Arc`** (allowed by `sync_primitives.rs:32–34`: "Arc is always `std::sync::Arc`"), **`std::sync::OnceLock`** (allowed by AGENTS.md exception list), and **`tokio::sync::Semaphore`** (Level 2 token).
- **No `Mutex` / `RwLock` from `crate::sync_primitives` are used**, which is correct: the module does not hold any cross-cutting locks. The `tokio::sync::Semaphore` for record slots is per-process and never held across an `.await` (the `permit` is moved into `tokio::spawn`, so the spawn itself owns the guard until the spawned future finishes — see `observer.rs:154–157`).
- `record` and `recall` use `tokio::task::spawn_blocking` to delegate SQLite work to the blocking pool. They clone the `Arc<SqliteMemoryBackend>` and the SQL connection lock (`self.conn.lock()` inside `routing_experience.rs:115`) is acquired **inside** the blocking task — never held across an `.await` in this module. This is the correct pattern (the doc comment at `experience_store.rs:81–86` calls it out explicitly).

**Cross-module concerns (runtimes, sandbox, search, secrets):**

- **`runtimes/`** — does NOT touch `routing`. No cross-module lock conflict.
- **`sandbox/`** — does NOT touch `routing`. No cross-module lock conflict.
- **`search/`** — `src/handlers/graph/*` reads `crate::routing::DEFAULT_AGENT_ID` (lines `manage.rs:38/131/261/337/374/411`, `search.rs:30/165`, `node_detail.rs:32/171/197`, `query.rs:28/243/271`). These are read-only string references; no lock interaction.
- **`secrets/`** — `recall.rs::provider_availability_from_config` (line 60) calls `resolve_vault_secret` on every recall. Each recall invokes the vault lookup **once per `available` check** (line 60: `crate::gateway::handlers::resolve_vault_secret(&format!("ai:{provider}"), tm).is_some()`). This is on the recall hot path. The function is async-safe (no global lock acquired), but a recall rendering N aggregates + K neighbors = N+K vault lookups per recall. If the vault path becomes slow under load (e.g. SQLite contention on `security.db`), recall latency compounds linearly. Worth flagging — the `configured_keys: HashMap<String, bool>` is a snapshot at boot; only the vault check goes live, and it's the one that may degrade. **Not a bug, but a performance concern for the secrets batch.** Recommend caching the per-provider availability result at boot or memoising inside the closure with a short TTL.
- **`session_service` / `SessionActor`** — the `subagent_spawner` calls `base.session.emit_event(&child_id, ...)` directly with the parsed `child_id: SessionKey`. There is no routing module dependency in the actor itself; routing is upstream of the session service.

**SPEC §6 divergence (documented):** `routing/mod.rs:30` flags `task_emb: std::sync::OnceLock<Vec<f32>>` against the spec's `OnceCell`. Per AGENTS.md, this is allowed. No fix needed.

**Past-review follow-up (from `docs/engineering-reports/review-results/routing.md`):**

| Past issue | Status today |
|---|---|
| Task keys with reserved `task_type` mis-parsed | **FIXED** — guard arm at `session_key.rs:633–638`, pinned by `parse_never_yields_a_task_whose_type_is_a_reserved_marker` test. |
| Subagent `rposition` collapses nested layers | **PARTIALLY FIXED** — `rposition` (now last marker) is correct, but the `[main_key]` arm's length-1 restriction causes the new Critical bug #1. |
| `from_legacy` doesn't normalise agent_id | **FIXED** — line 686 calls `normalize_agent_id(parts[1])`. |
| Regex-based task classification violates R8/R10 | **FIXED** — `src/routing/rules.rs` no longer exists. |

---

## Summary

| Level | Count |
|-------|-------|
| Critical | 2 |
| Warning | 7 |
| Suggested Test | 4 |

**Top 3 most impactful issues:**

1. **`session_key.rs:640` — `[main_key]` arm is length-1 only; `agent:<id>:main` roundtrips as `Task`, not `Main`.** This breaks the canonical session-key shape for the agent's main session, suppresses the strategic planner on every Main turn (because `is_interactive()` returns `false` for the mis-parsed `Task`), and silently zeroes `epoch()` (which is `Main | DirectMessage` only). The most common form of the bug is `agent:main:main` (the default agent's own default key) — touches every gateway panel render that does `from_key_string(s).unwrap()` and pattern-matches the result.

2. **`inbound_router/agent_resolver.rs:251` — `identity_links` is never applied on the zero-config fallback path.** Cross-channel identity linking silently fails for any deployment that hasn't configured `[[bindings]]`. The `config.rs` doc-comment at lines 25–35 already names this as a known gap; this audit confirms it is still open and identifies the exact call site that needs the consultation.

3. **`observer.rs:147` — `try_acquire_owned` failure path silently drops routing-experience rows under load.** No counter, no `error` log, no retry. On a slow disk or a fan-out team, routing data is lost without monitoring visibility. Either bump a counter and log at `error`, or wrap in a `timeout`/queue.

**Cross-module concerns affecting other modules in this batch:**

- `secrets/` — `recall.rs:60` calls `resolve_vault_secret` once per availability check (N+K lookups per recall rendering). This is a performance concern, not a bug, but should be on the secrets batch's radar. Recommend boot-time memoisation or a short TTL on the availability closure.
- `search/` and `runtimes/` / `sandbox/` — no routing interactions.

**Findings document written to:** `/home/zou/data/workspace/Aleph/.worktrees/audit-2026-08-28/review-results/routing-findings.md`