# WhatsApp Channel Architecture (Current State)

> Status: Current · Last updated: 2026-04-22
> Supersedes: `WHATSAPP_ARCHITECTURE_DESIGN-2026-04-06.md` (archived in git history — the original "Baileys migration" doc described a Go bridge → Baileys migration that never happened; the actual stack is `whatsapp-rust` direct)
> Implementation history: see `docs/superpowers/specs/2026-04-22-whatsapp-arch-r1-design.md` + `docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md`

## TL;DR

WhatsApp channel uses `whatsapp-rust` crate (wacore) directly — **no Go bridge, no Baileys, no IPC**. The architecture is:

- `WhatsAppChannel` (mod.rs) — `Channel` trait impl; owns a `WaRuntime`
- `WaRuntime` (wa_runtime/) — wraps `whatsapp_rust::Bot`; event loop drives a 9-variant `PairingState` FSM
- `PairingState` (pairing.rs) — **single source of truth** for status; `to_channel_status()` is the only path to `Channel::status()`
- `WaAuthManager` (wa_auth/) — persists creds in `SecretVault` (production) or injected vault (tests, via `with_vault_and_crypto`)
- `WhatsAppConfig` (config.rs) — flat config with sub-structs (`AccessConfig`, `DeliveryConfig`, `ReactionConfig`)
- `InboundPolicy` (wa_inbound/) — DM/Group allow + pairing gate
- `OutboundAdapter` (wa_outbound/) — text chunking + media + reactions + typing/read
- `WhatsAppConfig::reactions` (config.rs) — consumed in event loop via `ReactionHandler`
- `WhatsAppConfig::history` (config.rs) — `GroupHistoryBuffer` filled by mapper (downstream context injection: R2)
- `FakeWaRuntime` (wa_runtime/fake.rs) — test double for `trait WaRuntime`; 4 end-to-end scenarios

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                Aleph Gateway (alephcore)                     │
│                                                              │
│  ChannelRegistry ──▶ WhatsAppChannel ──▶ Arc<dyn WaRuntime>  │
│                         │                  │                 │
│                         │                  ├── RealWaRuntime │
│                         │                  │    (wacore)     │
│                         │                  │                 │
│                         │                  └── FakeWaRuntime │
│                         │                       (test only)  │
│                         ▼                                    │
│                 WhatsAppConfig (reactions/history/access)    │
│                                                              │
│  InboundRouter ◀── channel_state.receiver() ◀── event_loop   │
│                                                              │
│  OutboundRouter ──▶ channel.send() ──▶ runtime.send_message()│
└─────────────────────────────────────────────────────────────┘
```

## Key Design Decisions (R1)

| Decision | Rationale | Source |
|----------|-----------|--------|
| `PairingState` single source of truth; `ConnectionState` removed | Avoided CLAUDE.md §0 (orphan structure) + §8 (fail-closed) — `Channel::status()` now reads `PairingState::to_channel_status()` | spec §3.1 D1 |
| `WhatsAppAccountRegistry` CUT (deleted) | No product need for multi-account; CLAUDE.md §0/§19 favor simplification; full implementation is 5–7 PR | spec §3.3 D3 |
| `config.reactions` + `config.history` fields now consumed | Were parsed-then-dropped (CLAUDE.md §11 no-op) | spec §3.2 D2 |
| `trait WaRuntime` extracted | Testability — `FakeWaRuntime` for end-to-end scenarios without wacore connection | spec §3.4 D4 |
| `MediaProcessor` conservative (size + MIME only) | Avoid `image` crate per R3; real-encode deferred to R2 | spec §3.5 D5 |
| 4 WhatsApp events drive `PairingState` (`PairingQrCode`, `PairSuccess`, `Disconnected`, `PairError`) | `Scanned` not in wacore 0.5.0; deferred to R2 | plan Task 2 deviations |

## Module Map

| File | Role |
|------|------|
| `mod.rs` | `WhatsAppChannel`, event loop, `status()`, factory, plugin registration |
| `config.rs` | `WhatsAppConfig` + sub-configs |
| `pairing.rs` | 9-variant FSM (`Idle/Initializing/WaitingQr/QrExpired/Scanned/Syncing/Connected/Disconnected/Failed`) + `to_channel_status()` |
| `wa_runtime/traits.rs` | `trait WaRuntime` |
| `wa_runtime/client.rs` | `RealWaRuntime` (wacore-backed) |
| `wa_runtime/fake.rs` | `FakeWaRuntime` (test-only, 4 scenarios) |
| `wa_runtime/event_loop.rs` | Event dispatch + `PairingState` driver |
| `wa_runtime/state.rs` | **removed in R1** (was `ConnectionState`) |
| `wa_auth/` | `WaAuthManager` + `vault_store` (TempDir-isolated tests) |
| `wa_inbound/` | `mapper` + `InboundPolicy` |
| `wa_outbound/` | `sender` + `media` (conservative R1) |
| `wa_policy/` | DM/Group allow + pairing logic |
| `reactions.rs` | `ReactionHandler` + `trait ReactionSender` |
| `history_buffer.rs` | `GroupHistoryBuffer` |
| `media.rs` | (legacy simple module — retained for capability reporting) |
| `account.rs`, `account_registry.rs` | **deleted in R1 (CUT path)** |
| `message.rs`, `types.rs` | Common types |
| `tests/` | Contract + fake-runtime end-to-end scenarios |

## Open / Future Work (R2+)

- Multi-account routing (`chat_id` → `account_id`) — see spec §6 design draft
- `Event::QrScannedWithoutMultidevice` → `PairingState::Scanned` adapter (currently skipped; wacore 0.5.0 limitation)
- Downstream `GroupHistoryBuffer::get_context` injection into LLM prompt (currently only `add` is wired)
- `image` crate for actual JPEG/PNG re-encoding (currently conservative size-check only)
- `should_agent_react` wiring (currently `Off`/`Ack`/`Minimal`/`Extensive` level field is parsed but agent-initiated reactions are not triggered)
- e2e QA via `qa/channels/run.sh` (whatsapp not yet in script)

## References

- Spec: `docs/superpowers/specs/2026-04-22-whatsapp-arch-r1-design.md`
- Plan: `docs/superpowers/plans/2026-04-22-whatsapp-arch-r1.md`
- WhatsApp runtime: <https://github.com/jlucasoares/whatsapp-rust> (wacore 0.5.0)
- CLAUDE.md: §0 (orphan structure), §8 (fail-closed), §11 (no-op), §19 (widening)
- FEATURE_LOCATOR: §11 (WhatsApp channel) — see §11 entry for current state