# Cluster Federation (Single-Center Asymmetric Node Federation)

> One center `aleph-server` orchestrates many machines (nodes) as execution
> arms — the "one core, many bodies" extension of R6, while the cluster keeps
> exactly one mind. Reverse RPC over the node's dial-out WS; `node_invoke` +
> `environments.list` as the only two LLM-facing cluster tools.

**Location**: `src/cluster/` · `src/builtin_tools/node_invoke.rs` ·
`src/builtin_tools/node_file.rs` · `src/approval/node_requester.rs` ·
`src/bin/aleph-server/commands/node.rs` · `src/gateway/handlers/cluster.rs`

**Design spec**: `docs/superpowers/specs/2026-06-08-aleph-cluster-design.md`
(canonical architecture). Phase specs/plans live alongside it under
`docs/superpowers/{specs,plans}/2026-06-0*-aleph-cluster-*`.

---

## 1. Overview

R6 ("one core, many channels") today covers *many channels* — multiple I/O
terminals attached to one core. The cluster pushes it to **"one core, many
channels + one core, many bodies"**: a single **center** Aleph core orchestrates
several **nodes** (whole machines) that act as pure execution arms — while
strictly preserving "the cluster has exactly one brain."

This is **orthogonal** to shell-core separation (see
[DESKTOP_BRIDGE.md](./DESKTOP_BRIDGE.md) and the Spec A remote-gateway design):

- **Axis 1 · shell-core separation** — a thin I/O shell attaches to *one* core
  (local or remote, mutually exclusive). Unchanged by the cluster.
- **Axis 2 · cluster federation** — one center core commands *many* nodes. This
  document.

The two axes meet at exactly one point: operating the cluster from afar = using
Axis 1 to remotely connect a channel to the **center**. The Panel only *renders*
the center's environment view (a thin R4 contract); it never aggregates or
routes.

> **Rejected design (Model A):** a Panel connecting to local + multiple remote
> cores and aggregating them is abandoned — it makes a limb hold many brains
> (violates R4/R6). "Unified multi-machine view" is provided entirely by this
> cluster through one connection to the center plus `environments`. The shell
> always holds exactly one brain.

---

## 2. Topology: single-center, asymmetric

```
        [Human / Aleph Channel]              ← attaches to the single front door
              │ JSON-RPC over WS               (center), local or remote, exclusive
              ▼
   ╔══════════════════════════╗
   ║   Center Aleph Core       ║   LLM sees exactly TWO cluster tools:
   ║   (the only brain)        ║     environments.list      (read)
   ║   Think → Act loop        ║     node_invoke(node,cmd,params)  (write)
   ║                           ║
   ║  ┌────────────────────┐   ║   NodeRegistry: nodes_by_id / nodes_by_conn
   ║  │ src/cluster/        │   ║   PendingInvokes: reverse-RPC id correlation
   ║  │  NodeRegistry        │   ║
   ║  │  env aggregation +   │   ║
   ║  │  routing             │   ║
   ║  └─────────┬──────────┘   ║
   ╚════════════│═════════════╝
   center→node  │ node.invoke         node→center ▲ events
   (reverse RPC)│ (over node's        (push back)  │
                │  always-open WS)
        ┌───────┴───────┬───────────────────┐
        ▼               ▼                   ▼
   ┌─────────┐     ┌─────────┐        ┌──────────┐
   │ Node B   │    │ Node C   │        │  local   │
   │NodeClient│    │NodeClient│        │ (host is │
   │ dials →  │    │ dials →  │        │  also an │
   │  center  │    │  center  │        │  env)    │
   │ declares │    │ declares │        └──────────┘
   │ commands │    │ commands │
   │ executes │    │ executes │
   │ locally  │    │ locally  │
   └─────────┘     └─────────┘
   node = pure execution arm: receives node.invoke → runs a local
   tool/agent → returns result + events
```

| Dimension | Decision |
|-----------|----------|
| Topology | Single-center asymmetric: 1 center (only brain, holds `NodeRegistry` + orchestration) + N nodes (pure execution arms that dial in). Symmetric mesh / multi-master / consensus are **rejected**. |
| Front door | The center is the **only** entry. Connecting to a node ≠ using the cluster — a node can neither see nor orchestrate the cluster. |
| Dial direction | Nodes **dial out** to the center (NAT-friendly), not the reverse. |
| Identity | identity = (the core you connect to) × (the tier that core grants your device). Reuses existing pairing + operator/guest + chat/config. No new identity system. |
| Capability payload | Four capability classes collapse into one bidirectional transport + one command catalog: ① tool execution ② capability access ③ sub-agent delegation ④ event reporting. |
| Memory | center memory = cluster memory; **no distributed shared memory**. Node sub-agents use node-local memory; results flow back to center. |
| Dual role | A machine is exactly one of standalone / center / node. |

> **Not a high-availability cluster.** The single center is a single point — fine
> for a personal assistant spanning a few machines. No failover, no multi-center,
> no consensus.

---

## 3. Components and physical placement (P2 high cohesion / R10 no harness pollution)

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `NodeRegistry` / `NodeSession` | `src/cluster/registry.rs` | node registry (`nodes_by_id` / `nodes_by_conn` dual maps), environment aggregation, `node.invoke` reverse-RPC + pending correlation, timeout cleanup, `node_identity_by_conn` (identity stamped from the authenticated connection) |
| Reverse RPC channel | `src/cluster/reverse_rpc.rs` | `PendingInvokes` (`DashMap<(conn,req_id), oneshot::Sender>`), `ReverseRpcChannel`, `cancel_all()` (drain on disconnect → callers get `Cancelled` instead of waiting out the timeout) |
| `NodeClient` (dial-out arm) | `src/bin/aleph-server/commands/node.rs` | node role: dials out to center, authenticates with the node token, declares its command catalog, receives invokes → dispatches to **local** tools/agents, pushes results + events back, reconnects with backoff |
| Node command runtime | `src/cluster/node_runtime.rs` | `NodeCommand` trait + `CommandTable` (its keys *are* the allowlist — the node is authoritative), `BashNodeCommand`, dispatch |
| Node file commands | `src/cluster/node_file_cmd.rs` | `file.read` / `file.write` host-fs commands, jailed inside the node session workspace (`canonical_root` containment) |
| Node-side approval requester | `src/approval/node_requester.rs` | `CenterApprovalRequester` — routes a node's sandbox approval requests up to the center over the reverse-RPC channel |
| Center approval routing | `src/cluster/node_approval.rs` | `run_node_approval` — feeds node approval requests into the shared `ExecApprovalManager` + the existing `ApprovalRequested` frame |
| `role:node` handshake | `src/gateway/handlers/auth/connect.rs` + `src/gateway/security/device.rs` (`DeviceRole::Node`) | connect handshake recognizes the node role and registers it into the `NodeRegistry` |
| Reverse-RPC orchestration | `src/gateway/server/handler.rs` + `src/gateway/server/mod.rs` | server→client id-correlated request/response wiring, connection cleanup, `node.connected` / `node.disconnected` events |
| `environments.list` / `cluster.*` / `pairing.start_node` | `src/gateway/handlers/cluster.rs` + `src/gateway/handlers/auth/pairing.rs` | environment enumeration; cluster management; anonymous node pairing entry |
| `node_invoke` meta-tool | `src/builtin_tools/node_invoke.rs` | plain `AlephTool`: `Args{node_id, command, params}`, schema auto-generated; `call()` routes through `NodeRegistry` to the target node |
| `node_file` meta-tool | `src/builtin_tools/node_file.rs` | `Args{node, direction: push\|pull, local_path, remote_path, overwrite?}`; moves bytes process-to-process, never into the LLM context |
| environments context injection | `src/harness/agent/prompt.rs` (data segment only) | injects the online environment + command catalog as **data** into the prompt (R9) |

**R10 held:** `src/harness/` gains no logic — `node_invoke`/`node_file` are
tools (`builtin_tools`), `NodeRegistry`/reverse RPC are subsystems
(`cluster` + `gateway`). The harness still only schedules Think→Act and does not
even know a tool is remote. `prompt.rs` injects data only, no decision logic.

---

## 4. The LLM sees two tools, not N

The center LLM never sees a growing tool surface as nodes join:

- **`environments.list`** → self-describing: each online node's `id` / `status` /
  command catalog (with JSON Schema) / capability tags. The host itself is an
  environment with `id: "local"`.
- **`node_invoke(node_id, command, params)`** → universal execution entry.

The model reads `environments` and assembles the invoke itself — intent
understanding, machine choice, param assembly are all prompt-level reasoning
(R7/R9). The four capability classes are expressed uniformly:

| Capability | Expressed as |
|------------|--------------|
| ① tool execution | `node_invoke("node:B", "bash", { ... })` |
| ② capability access | `node_invoke("node:B", "desktop.screenshot", { ... })` |
| ③ sub-agent delegation | `node_invoke("node:B", "agent.run", { task })` — B runs Think→Act locally, streams back |
| ④ event reporting | node → center event channel (reverse); center subscribes + LLM reacts |

`node_file` is the dedicated byte-transport channel (push/pull, SHA-256 verified
on both ends, single frame capped at 8 MB). It reuses the `node_invoke`-injected
`node_registry` OnceCell — no extra `agent_init` wiring.

---

## 5. Node lifecycle (all reuse existing pairing / token / tool machinery)

1. **Enroll (interactive pairing).** On node B the operator says, in natural
   language, "join this machine to `<center URL>`." B's `aleph-server node` arm
   dials out and runs the **interactive pairing** flow: it calls the anonymous
   `pairing.start_node` RPC, receives a 6-digit code, prints it to stdout, then
   polls `pairing.poll`. The center operator approves from the **Panel** (same
   surface as cold-browser pairing), which mints a `DeviceRole::Node` token. B
   persists it at `~/.aleph/node/<name>.json` (mode `0600`). `AUTH_FAILED`
   (`-32001`) auto-clears the credential and re-pairs.
2. **Declare.** Once connected, B reports the command catalog it is willing to
   expose (names + JSON Schema), sourced from B's local tools/skills. **Default
   deny, explicit allowlist** — the `CommandTable` keys are the boundary.
3. **Invoke.** Center LLM calls `node_invoke("node:B","bash",{...})` → the tool →
   `NodeRegistry` finds B's session → delivered over the always-open WS as a
   reverse-RPC `tool.call` frame → B's `NodeClient` dispatches to a local tool
   (on its own task — never blocking the read loop) → result returns by id →
   pending table wakes → control returns to the LLM. Long tasks stream over the
   event channel.
4. **Perceive.** B's daemon events / sub-agent progress push back over the same
   WS; the center routes by topic to subscribers (Panel rendering + LLM
   reaction).

---

## 6. Reverse RPC (the core new mechanism)

A plain Gateway connection is client→server request/response plus server→client
one-way notifications (the event bus). There was **no** server→client
id-correlated request/response and **no** pending table. (`ToolCallParams` /
`ToolCallResult` in `protocol.rs` existed as scaffolding but were unwired.)

Added in `src/cluster/reverse_rpc.rs` + `src/gateway/server/`:

```
center                                   node B (NodeClient)
  │  node_invoke("node:B","bash",{…})        │
  ├─ pending.insert((conn,id), tx) ──────────┤
  │  tool.call { method, params, id } ──────▶ │  dispatch to local tool
  │                                          │  (tokio::spawn, never blocks
  │                                          │   the read loop)
  │  ◀────────── { result, id } ─────────────┤
  ├─ pending.remove((conn,id)) → tx.send ────┤
  │  result returns to the LLM               │
```

1. **Pending table:** `DashMap<(conn_id, req_id), oneshot::Sender<JsonRpcResponse>>`.
2. **Server dispatch:** sends `{jsonrpc, method:"tool.call", params, id}` to the
   target node connection.
3. **Client route-back:** the node's read loop intercepts id-correlated requests,
   spawns the dispatch, and replies `{jsonrpc, result, id}`; the server wakes the
   pending entry by id.
4. **Timeout / cancel:** unanswered requests time out; on disconnect
   `PendingInvokes::cancel_all()` drains all waiters so every in-flight `call()`
   returns `Cancelled` immediately rather than waiting out the timeout
   (≤130s otherwise).

**Liveness:** the center emits `node.connected` (at the register seam, after the
handshake short-circuits success) and `node.disconnected` (at the deregister
seam, identity resolved *before* deregister to defend against stale-reconnect
double-emit). These mirror `presence.joined` / `presence.left` byte-for-byte.
Application-level heartbeats are deliberately omitted (YAGNI) — the transport's
WS-ping + idle-watchdog already tears down half-open sockets, which triggers
cleanup → fail-fast.

---

## 7. Approval routing (nodes can ask the center)

A node runs **headless** — no operator sits at it (its `ApprovalGate` originally
had `requester = None`, denying everything). When a node's sandbox hits a command
needing a capability upgrade, it routes the approval **back up to the center**:

```
node B sandbox hits a privileged command
  │  node.approval.request   (reverse, over B's WS, via CenterApprovalRequester)
  ▼
center → run_node_approval → ExecApprovalManager → Panel approval card
              (the SAME ApprovalRequested frame as local exec approval;
               node context encoded into the command field:
               "node '<name>': <tool> — <reason>")
  │  operator approve / deny / approve-for-session  (exec.approval.resolve)
  ▼
decision returns down to B as the JSON-RPC response → B maps it to an
outcome (approved / approved_session / denied / timeout) — fail-closed
on every path
```

Key decisions:

- **Reuse the existing Panel card, not a dedicated frame.** The Panel approval
  card subscribes to topic `approval.**` and renders `record.command`. A
  dedicated `NodeApprovalRequested` frame would make the card *not appear* and
  require new event scope / RPC / WASM. Encoding node context into the `command`
  field of the existing `ApprovalRequested` = zero WASM, zero new event scope.
- **Identity from the connection, not params.** The node's identity is stamped
  from the authenticated connection (`NodeRegistry::node_identity_by_conn`), so a
  node cannot forge another's identity or approve itself; resolve is
  operator-only.
- **Node read-loop concurrency.** Because a `tool.call` (e.g. bash) may block
  awaiting an approval response that must arrive over the *same* read loop, the
  node uses split read/write halves + an outbound mpsc + a writer task, and
  dispatches `tool.call` on `tokio::spawn` so the read loop never stalls on the
  command waiting for its own approval.
- **Fail-closed wire protocol.** node→center `{tool, reason}`; center→node
  `{outcome: approved|approved_session|denied|timeout}`. `NODE_TIMEOUT` (130s) >
  center approval timeout (120s).

---

## 8. Security model (highest priority)

`node_invoke` is, by design, a remote-code-execution channel. The boundaries are
non-negotiable:

- **The node-side allowlist is the only security boundary.** B exposes only
  explicitly declared commands; the center can call **only** approved ones.
  Default deny. Even a compromised center can do only what B permits.
- **Bidirectional trust.** B dialing in + holding a center-issued node token = B
  trusts that center. The center holding B's allowlist = the center can only
  drive B within the boundary.
- **Tier gating on the human.** Whoever triggers `node_invoke` is bound by the
  center's tiering — operators may orchestrate; chat/guest are read-only
  (`environments.list`) and cannot invoke.
- **Sensitive node operations route to approval** (see §7), reusing the Phase-2b
  operator-approval infra.
- **Credential isolation.** The node token is never the host token, mirroring the
  "host token never leaks to a remote" discipline of shell-core separation.
- **Transport.** Same explicit trade-off as remote shells: plaintext over
  LAN/Tailscale is acceptable; run over a private network.
- **File transfer.** Both ends verify SHA-256; single frame ≤ 8 MB; the node
  jails every path inside its session workspace via an explicit `canonical_root`
  containment check (`check_and_resolve_path` does *not* enforce containment on
  its own — the jail is load-bearing).

---

## 9. Redline accounting

| Redline | How it holds |
|---------|--------------|
| R1 — brain/limb separation | Federation is core↔node, all Rust. A node's platform capabilities still go through its local `DesktopCapability` trait + bridge; `src` never touches platform APIs. |
| R3 — core minimalism | Zero new heavy dependencies; reuses the existing WS / JSON-RPC / DashMap stack. |
| R4 — interface pure I/O | The Panel only *renders* `environments` (a thin contract) — no aggregation, routing, or persistence. |
| R6 — one core, many channels | The single center is the only brain; the cluster is its "one core, many bodies" extension. |
| R7 — LLM sovereignty | Machine choice / param assembly / retry on failure are all left to the model. No deterministic intent classification or routing engine. |
| R8 — everything is a tool | Cluster management (enroll / approve / expose / list) + node capabilities are all tools. |
| R9 — intelligence in the prompt | `environments` injected as data; one inference pass covers the placement decision. |
| R10 — thin harness | `src/harness/` gains no logic; `NodeRegistry`, reverse RPC, `node_invoke` live in `cluster` / `gateway` / `builtin_tools`. |

---

## 10. Implementation status (shipped)

| Phase | Scope | Status |
|-------|-------|--------|
| 0a · reverse RPC | server→client id-correlated request/response + `PendingInvokes` + `ReverseRpcChannel` | ✅ merged |
| 0b · node registry | `NodeRegistry`, dual maps, `node_identity_by_conn`, node-token issuance | ✅ merged |
| 0c · core node runtime | `aleph-server node` dial-out + `node_invoke` tool + node-side bash + `CommandTable` allowlist | ✅ merged |
| 0c · interactive pairing | `pairing.start_node` + Panel-approved `DeviceRole::Node` enroll + credential persistence | ✅ merged |
| file transfer | `node_file` push/pull, SHA-256, 8 MB cap, path jail | ✅ merged |
| node approval routing | node sandbox → `CenterApprovalRequester` → center Panel card → decision down | ✅ merged |
| node liveness | `node.connected` / `node.disconnected` events + `cancel_all` on disconnect | ✅ merged |

**Deferred (YAGNI):** symmetric mesh / multi-master / consensus; distributed
shared memory; HA / center failover / multi-center; dual identity (a machine that
is both a center and someone else's node); Model A (Panel aggregating multiple
cores).

**Follow-up phases:** Phase 1 (more node commands: desktop / files / mcp =
capability ②) · Phase 2 (cross-machine sub-agents via `agent.run`, reusing
`subagent_spawner` = capability ③) · Phase 3 (cross-machine perception: node
daemon events bridged to center + proactive reaction = capability ④, the
cross-machine version of R5).

---

## 11. Testing strategy

- `NodeRegistry`: register/deregister, dual-map consistency, timeout cleanup.
- Reverse RPC: id round-trip, timeout, node-offline error surface,
  `cancel_all` determinism (in-flight `call()` spins until the waiter is
  registered, then cancels → `Cancelled`).
- `node_invoke`: schema generation, routing to the correct session, LLM-readable
  offline errors.
- Security: non-allowlisted command rejected; chat/guest tier cannot invoke
  (only read `environments`); node token ≠ host token (connect-frame assertion).
- Enroll/pairing: node-role pairing round-trip, `DeviceRole::Node` issuance,
  schema migration including the `node` role.
- Approval routing: approve / approve-session / deny / timeout round-trips;
  timeout uses `#[tokio::test(start_paused)]` to advance virtual time 120s.

---

## See also

- [ARCHITECTURE.md](./ARCHITECTURE.md) — the five-layer pipeline
- [DESKTOP_BRIDGE.md](./DESKTOP_BRIDGE.md) — shell-core separation (the orthogonal axis)
- [GATEWAY.md](./GATEWAY.md) — the WebSocket control plane the reverse-RPC channel extends
- [SECURITY.md](./SECURITY.md) — auth, pairing, and the approval trust model
- [HARNESS_PHILOSOPHY.md](./HARNESS_PHILOSOPHY.md) — why the harness stays thin
