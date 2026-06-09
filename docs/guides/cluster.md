# Aleph Cluster Guide

## Concept

A cluster extends *execution* across machines:
- **Center**: the brain — runs the DB, LLM, memory, and the agent loop.
- **Node**: a pure execution arm — runs bash / tools in a local sandbox.
  No DB, no LLM. It dials out to the center and serves reverse-RPC
  `tool.call`.

This differs from "一核多端" (multi-channel): channels are I/O surfaces;
nodes are remote *hands* the center can run commands on.

## Enroll a node (mint a token)

On the center, mint a node-role token:
- Tool / RPC: `cluster.enroll` (operator only) → returns `{node_id, token}`.
- Or Panel: Settings → 服务与集群 → Aleph 集群 → **+ Enroll** → name the node
  → copy the token.

## Connect the node (dial out)

On the node machine:

```bash
aleph-server node \
  --center ws://<center-host>:18790 \
  --token <token-from-enroll> \
  --name <node-name>
```

- Omit `--token` to pair interactively on first start: the node prints a
  6-digit code; an operator approves it in the Panel.
- The credential persists to `~/.aleph/node/<name>.json` (0600); a stored
  credential takes precedence over `--token`.
- The node auto-reconnects with backoff if the center drops.

## Use a node (from the LLM)

Once registered (visible in `environments.list` and the Panel cluster list):
- `node_invoke` — run a command (e.g. bash) on a named node.
- `node_file` — push / pull files between center and node.
- When a node's sandbox hits a capability that needs approval, it sends a
  reverse approval request; an operator decides in the Panel approval card.

## Caveats

- Cluster management (enroll, list, invoke) requires **operator** privilege.
- Treat node tokens like secrets — they grant execution on the center's behalf.
- If a node disconnects, in-flight calls fail fast (no hang).
- The allowlist of runnable commands is authoritative on the node side.

## See also

- Engineering reference (architecture, wire protocol, redlines, tests):
  `docs/reference/CLUSTER.md`
- `multi_channel` guide — how this differs from 一核多端 (I/O ends vs execution arms).
