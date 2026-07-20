# Pairing Wizard Design

Date: 2026-05-23
Status: Draft → awaiting user review

## Problem

The desktop shell now spawns / connects to `aleph-server` cleanly, but a
first-time user has no path to obtain an `ALEPH_GATEWAY_TOKEN`. Today the
Gateway returns `pairing_required` (code in `src/gateway/handlers/auth/connect.rs`)
and the shell only logs a `WARN`:

```
Gateway rejected the desktop shell connection (pairing_required).
Set ALEPH_GATEWAY_TOKEN to enable OS notifications.
```

The user has to manually run `aleph-gateway pairing approve <code>` (which
requires stopping the server) — an unacceptable first-run UX.

A previously-shipped wizard framework lives in `archive/wizard/` and
`archive/gateway_handlers_wizard.rs` (~2050 lines), originally built for
provider/onboarding setup. It is the natural foundation for a guided
pairing flow.

## Goal

Revive `archive/wizard/` into `src/`, prune the unused `CliPrompter` path,
add a same-machine `PairingFlow`, and make the desktop shell auto-trigger
the wizard when it sees `pairing_required`. Persist the resulting token
into the OS keychain so subsequent launches connect silently.

## Non-Goals

- Cross-device pairing (phone ↔ Mac). Out of scope for v1; the design
  leaves a `mode` seam so it can be added later.
- Reviving the `CliPrompter` path. No current consumer.
- Tauri WebDriver E2E for the modal. wasm-bindgen-test on the panel covers
  the RPC handshake; visual smoke is manual.

## Architecture

Architecture B — mirror archive + drop CliPrompter.

### Module layout

```
src/wizard/
├── mod.rs               # Top-level re-exports
├── types.rs             # WizardStep / WizardStatus / StepType / WizardOption
├── session.rs           # WizardSession + WizardFlow trait + state machine
├── prompter.rs          # RpcPrompter + WizardPrompter trait only
└── flows/
    ├── mod.rs
    ├── onboarding.rs    # OnboardingFlow / QuickSetupFlow / ProviderSetupFlow
    └── pairing.rs       # NEW: same-machine desktop-shell ↔ daemon flow

src/bin/aleph-server/commands/start/builder/handlers/wizard.rs
                       # JSON-RPC handlers: wizard.start / wizard.next /
                       # wizard.cancel. Registered alongside auth.rs in
                       # subsystems.rs.

interfaces/webchat/src/views/pairing_modal.rs
                       # NEW: full-screen Leptos modal that drives
                       # wizard.start("pairing") → renders steps →
                       # reconnects with the returned token.

desktop/shell/src/daemon.rs        # tiny patch: load token from keyring
                                    # at startup and inject into
                                    # ALEPH_GATEWAY_TOKEN env.
```

### Dependencies

New: `keyring = "3"` in `Cargo.toml` (workspace dep). Cross-platform
binding to macOS Keychain, Windows Credential Manager, Linux Secret
Service. Used only by `src/wizard/flows/pairing.rs` and
`desktop/shell/src/daemon.rs`.

## PairingFlow steps (~130 LOC, mock-friendly)

```rust
// src/wizard/flows/pairing.rs
impl WizardFlow for PairingFlow {
    async fn run(&self, p: &RpcPrompter) -> Result<(), WizardSessionError> {
        // 1. greeting (user-visible)
        p.prompt(WizardStep::note(
            "welcome",
            "为本机桌面配对 Aleph 守护进程",
        )).await?;

        // 2. internal: request a pairing code
        let req = self
            .pairing_manager
            .request_device_pairing(self.device_name.clone(), None, vec![0u8; 32], None)
            .map_err(|e| WizardSessionError::FlowError(format!("request_device_pairing: {e}")))?;

        // 3. confirm (user-visible)
        p.prompt(WizardStep::confirm(
            "approve",
            format!("本机配对码：{}\n点击「Approve」完成同机授权", req.code()),
        )).await?;

        // 4. internal: approve + harvest token
        let token = self
            .pairing_manager
            .approve_device(req.code())
            .map_err(|e| WizardSessionError::FlowError(format!("approve_device: {e}")))?;

        // 5. internal: persist to OS keychain (non-blocking on failure)
        if let Err(e) = keyring::Entry::new("aleph-gateway", "desktop-shell")
            .and_then(|e| e.set_password(&token))
        {
            tracing::warn!(?e, "keyring persist failed; token returned but not saved");
        }

        // 6. finish step delivers `{ "token": "<…>" }` payload back to panel
        p.finish(json!({ "token": token })).await?;
        Ok(())
    }
}
```

Step 2/4/5 are pure Rust between `prompt()` calls — they're not separate
`WizardStep`s. The Wizard already exposes a `data` field in the
done-status response (see `archive/wizard/types.rs::WizardNextResult`)
that we extend with `Option<Value>` (or reuse a field if one exists).

## Data flow (same-machine pairing)

```
shell.daemon.rs::launch_daemon()
  → load_token_from_keyring("aleph-gateway", "desktop-shell")  → Option<String>
  → spawn aleph-server with ALEPH_GATEWAY_TOKEN env (if Some)
  ↓
panel boots, opens IPC, calls auth.connect(token?)
  ├─ ok → enter main chat UI
  └─ err pairing_required(code, …) → <PairingModal/> mounted
            ↓
       wizard.start({wizard_type: "pairing"})
            ↓ { session_id, step1: note "welcome", status: running }
       modal renders → user [Continue]
            ↓ wizard.next(session_id, null)
       { step3: confirm "approve" + code, status: running }
       user [Approve]
            ↓ wizard.next(session_id, null)
       { status: done, data: { token } }
       panel.persistInMemory(token)  // RPC client re-arms
       panel.reconnect()
            ↓
       auth.connect → ok → main chat UI
```

### Trigger / re-trigger

- **First launch**: no keyring entry → empty env → daemon refuses →
  `pairing_required` → modal pops.
- **Stale token** (revoked from CLI): same path, naturally re-pairs.
- **User closes modal mid-flow**: `wizard.cancel` is sent on unmount; on
  next reconnect attempt the modal re-pops. No infinite-loop risk: same
  reactive code path, not a polling loop.

### Concurrency

Per `archive/gateway_handlers_wizard.rs`, `WizardManager` is
`Arc<RwLock<HashMap<session_id, WizardSession>>>`. Multiple panel tabs
each take their own session — fine for our flow because each session
ends with `approve_device` which is idempotent at the
`PairingManager` level (already enforced today by code expiry +
remove-on-approve).

## Error handling

| Failure                                      | Flow handling                                    | UX                                                            |
| -------------------------------------------- | ------------------------------------------------ | ------------------------------------------------------------- |
| `wizard.start` → no PairingManager           | RPC `-32603` internal_error                      | Modal: "无法启动配对，请检查 daemon 日志" + Retry             |
| `request_device_pairing` (store IO)          | `FlowError`, session → Error                     | Modal red banner + Retry                                      |
| `approve_device(code)` (expired/replay)      | `FlowError`, session → Error                     | Modal shows server message + Retry                            |
| `keyring.set` write fail                     | log warn, **do not block**, still return token   | Modal yellow banner: "本次成功但未能保存，下次启动需要重新配对"|
| User clicks "Cancel" or closes modal         | `wizard.cancel`, session → Cancelled             | Modal closes; next reconnect re-pops                          |
| Panel reload mid-flow                        | Session TTL ≥ PairingManager code lifetime (60s) | Next mount calls `wizard.start` again                         |

## Testing

### Unit (`src/wizard/`)
- Inherited tests from archive (`session.rs`: empty flow, status
  transitions, cancellation) — keep verbatim.
- **NEW** `flows/pairing.rs` unit tests:
  - `PairingManager` (struct at `src/gateway/security/pairing.rs:115`) is
    constructed against an in-memory `SecurityStore` for the test;
    `RpcPrompter` is driven from the test
  - assert: step sequence emitted (note → confirm), token round-tripped
    in finish payload
  - assert: failure path (`request_device_pairing` errors → session
    Error + propagated message)

### Integration (`tests/wizard_pairing_e2e.rs`, new)
- Spin up an in-process `WizardManager` + mock `PairingManager` +
  registered `wizard.*` JSON-RPC handlers.
- Drive `wizard.start("pairing") → wizard.next … → done` over the real
  handler dispatch, assert returned token in `data` payload.
- keyring write swapped via a fake `KeyringSink` trait so the test does
  not touch real keychains (CI safety).

### Panel (`interfaces/webchat`)
- wasm-bindgen-test: mock RPC client; assert receiving `pairing_required`
  from `auth.connect` triggers `PairingModal` mount; assert receiving
  `done + token` triggers `reconnect()` with the new token.
- No Tauri WebDriver E2E (cost > benefit at this stage).

### Skipped
- keyring crate behavior (system-dependent, mocked above).
- Tauri shell GUI (manual smoke).

## Migration / rollout

1. Add `keyring` to workspace `Cargo.toml`.
2. Copy `archive/wizard/{types,session,prompter,flows/onboarding,mod,flows/mod}.rs`
   to `src/wizard/`, dropping `CliPrompter` from `prompter.rs`.
3. Copy `archive/gateway_handlers_wizard.rs` to
   `src/bin/aleph-server/commands/start/builder/handlers/wizard.rs`.
4. Register handlers in `subsystems.rs` alongside `auth.rs`.
5. Write `src/wizard/flows/pairing.rs` + tests.
6. Write `interfaces/webchat/src/views/pairing_modal.rs` + wasm tests.
7. Patch `desktop/shell/src/daemon.rs` to load token from keyring.
8. `just check` + `just clippy` clean; run new test suites.

## Out of scope (future work)

- Cross-device manual pairing path (`PairingFlow::with_mode(Manual)`).
- Replacing onboarding flow with the panel — current onboarding still
  uses CLI-era assumptions; revival ≠ adoption.
- Removing the legacy `aleph-gateway pairing approve` CLI subcommand;
  it still serves cross-process recovery scenarios.

## Open questions

None at design freeze. Implementation may surface follow-ups; those
become plan items.
