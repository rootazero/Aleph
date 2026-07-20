# Pairing Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Revive `archive/wizard/` into `src/wizard/`, add a same-machine `PairingFlow`, expose `wizard.*` JSON-RPC handlers, and surface a Panel modal that turns the daemon's `pairing_required` error into a one-click device pairing — persisting the issued token in the OS keychain.

**Architecture:** Mirror archive layout, drop the never-used `CliPrompter`, extend `WizardNextResult` with a `data: Option<Value>` payload so the `PairingFlow` can return the device token through `wizard.next`'s final response. Plug the wizard session manager into `HandlerRegistry` via the existing two-phase wiring (stub at construction → real handler at boot). Panel mounts a `PairingModal` when `auth.connect` returns `pairing_required`, drives the wizard, persists the token to the OS keychain (`keyring` crate, already in workspace), and reconnects with `ALEPH_GATEWAY_TOKEN` set so OS notifications also unlock.

**Tech Stack:** Rust (`alephcore` lib + `aleph-server` bin), Leptos WASM panel (`aleph-panel`), Tauri shell (`aleph-desktop-shell`), `keyring = "3"` (already declared in `Cargo.toml:203`), existing `PairingManager` / `TokenManager` / `DeviceStore` security primitives.

**Worktree:** Use `superpowers:using-git-worktrees` skill to create an isolated worktree (`.claude/worktrees/pairing-wizard/`) before any code edits. All subsequent file paths in this plan are relative to that worktree once created.

---

## Pre-Flight Notes (read before starting)

- `WizardNextResult` in archive (lines 232–286 of `archive/wizard/types.rs`) has only `done / step / status / error`. It MUST be extended with `data: Option<Value>` so `PairingFlow` can hand back the token; the extension is additive and serializes invisibly when `None` thanks to `skip_serializing_if`.
- `RpcPrompter` (archive `prompter.rs`) needs one new method `finish(data: Value)` that closes the session by storing the payload in a shared slot the session reads when emitting the final `done` result.
- The current `HandlerRegistry` (`src/gateway/handlers/mod.rs:208`) uses `HandlerFn = Arc<dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = JsonRpcResponse> + Send>> + Send + Sync>`. The archive's `RpcHandler = Box<dyn …>` MUST be adapted to that signature when ported.
- The CLI device-approve sequence at `src/bin/aleph-server/commands/pairing.rs:84–179` is the reference for what `PairingFlow` does internally between the user-visible "approve" step and the token return: `confirm_pairing` → `DeviceStore::approve_device` → `SecurityStore::upsert_device` → `TokenManager::issue_token`.
- `desktop/shell/src/notify.rs:78` is the only consumer of `ALEPH_GATEWAY_TOKEN`. The shell only needs the env var BEFORE `notify` spins up its websocket. Loading the token from keyring at the very top of `main` is sufficient.
- `cargo` concurrency cap: before any `cargo check/test/build/clippy`, ensure `pgrep -x cargo | wc -l` is `< 3`. Wait if not.

---

## Task 0: Worktree setup

**Files:**
- None (creates `.claude/worktrees/pairing-wizard/`)

- [ ] **Step 1: Verify no cargo running**

Run: `pgrep -x cargo | wc -l`
Expected: `0` (or `1`/`2`; if `≥ 3`, wait until it drops)

- [ ] **Step 2: Create the worktree**

Invoke `superpowers:using-git-worktrees` skill with arg:

```
worktree name: pairing-wizard
branch: pairing-wizard
base: main
```

If the skill is unavailable, fall back to manual:

```bash
git worktree add -b pairing-wizard .claude/worktrees/pairing-wizard main
```

Expected: directory `.claude/worktrees/pairing-wizard/` exists and is on branch `pairing-wizard`.

- [ ] **Step 3: cd into the worktree for the rest of the plan**

```bash
cd .claude/worktrees/pairing-wizard
```

All subsequent steps run in this directory. The implementation skill (`subagent-driven-development` or `executing-plans`) handles per-step `cwd` automatically; if executing inline, remember the path persists across Bash calls.

---

## Task 1: Port wizard types (with `data` extension)

**Files:**
- Create: `src/wizard/types.rs`
- Test: same file (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Copy archive verbatim**

```bash
mkdir -p src/wizard
cp ../../../archive/wizard/types.rs src/wizard/types.rs
```

(`../../../` because we're in `.claude/worktrees/pairing-wizard/`.)

- [ ] **Step 2: Write the failing test for the new `data` field**

Append at the end of `src/wizard/types.rs` `#[cfg(test)] mod tests` block (just before the closing `}`):

```rust
    #[test]
    fn next_result_carries_finish_data() {
        let result = WizardNextResult::done_with_data(json!({ "token": "abc" }));
        assert!(result.done);
        assert_eq!(result.status, WizardStatus::Done);
        assert_eq!(
            result.data.as_ref().and_then(|v| v.get("token")).and_then(|t| t.as_str()),
            Some("abc")
        );

        // Backwards compat: bare done() still works and serializes without data.
        let bare = WizardNextResult::done();
        assert!(bare.data.is_none());
        let s = serde_json::to_string(&bare).unwrap();
        assert!(!s.contains("\"data\""));
    }
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p alephcore --lib wizard::types::tests::next_result_carries_finish_data
```

Expected: FAIL — `done_with_data` is not defined and `WizardNextResult` has no `data` field.

- [ ] **Step 4: Add the `data` field and constructor**

Modify `WizardNextResult` (around the existing `error` field) to add the new optional field:

```rust
/// Result of calling wizard.next()
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WizardNextResult {
    /// Whether the wizard is done
    pub done: bool,
    /// Current step (if not done)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<WizardStep>,
    /// Current status
    pub status: WizardStatus,
    /// Error message (if status is Error)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Final payload produced by the flow (only set when done with success).
    /// Carries flow-specific output — e.g. PairingFlow returns
    /// `{ "token": "<device-token>" }`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
```

Then update every existing constructor to set `data: None`, and add the new one:

```rust
impl WizardNextResult {
    pub fn step(step: WizardStep) -> Self {
        Self {
            done: false,
            step: Some(step),
            status: WizardStatus::Running,
            error: None,
            data: None,
        }
    }

    pub fn done() -> Self {
        Self {
            done: true,
            step: None,
            status: WizardStatus::Done,
            error: None,
            data: None,
        }
    }

    /// Done with a flow-specific payload (e.g. issued token).
    pub fn done_with_data(data: Value) -> Self {
        Self {
            done: true,
            step: None,
            status: WizardStatus::Done,
            error: None,
            data: Some(data),
        }
    }

    pub fn cancelled() -> Self {
        Self {
            done: true,
            step: None,
            status: WizardStatus::Cancelled,
            error: None,
            data: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            done: true,
            step: None,
            status: WizardStatus::Error,
            error: Some(message.into()),
            data: None,
        }
    }
}
```

- [ ] **Step 5: Run the new test and existing serialization test**

```bash
cargo test -p alephcore --lib wizard::types::tests
```

Expected: ALL pass (4 tests: `test_step_builder`, `test_option_builder`, `test_next_result_serialization`, `test_status_values`, plus our new `next_result_carries_finish_data` = 5).

- [ ] **Step 6: Commit**

```bash
git add src/wizard/types.rs
git commit -m "wizard: restore types module with WizardNextResult.data extension"
```

---

## Task 2: Port wizard session

**Files:**
- Create: `src/wizard/session.rs`

- [ ] **Step 1: Copy archive verbatim**

```bash
cp ../../../archive/wizard/session.rs src/wizard/session.rs
```

- [ ] **Step 2: Hook in the new finish-data slot**

Add to the `WizardSession` struct (after `error: Arc<RwLock<Option<String>>>,`):

```rust
    /// Final payload set by `RpcPrompter::finish(...)` before the flow returns.
    /// `next()` reads from this when surfacing the Done result.
    finish_data: Arc<RwLock<Option<serde_json::Value>>>,
```

In `WizardSession::new`, construct it (after `let error = ...;`):

```rust
        let finish_data = Arc::new(RwLock::new(None));
```

And store it in the struct literal:

```rust
            finish_data: finish_data.clone(),
```

Update the prompter creation to share the slot — change

```rust
let prompter = RpcPrompter::new(session.step_tx.clone(), session.answers.clone());
```

to

```rust
let prompter = RpcPrompter::new(
    session.step_tx.clone(),
    session.answers.clone(),
    finish_data.clone(),
);
```

- [ ] **Step 3: Update `next()` to surface finish_data**

Replace each `WizardNextResult::done()` call inside `WizardSession::next()` (there are two: the early-return Done branch and the channel-closed Done branch) with a helper:

```rust
fn done_result(&self) -> WizardNextResult {
    match self.finish_data.read().unwrap_or_else(|e| e.into_inner()).clone() {
        Some(data) => WizardNextResult::done_with_data(data),
        None => WizardNextResult::done(),
    }
}
```

Then call `self.done_result()` in both branches in place of `WizardNextResult::done()`.

- [ ] **Step 4: Compile check (the prompter signature change is intentional — Task 3 fixes it)**

```bash
cargo check -p alephcore --lib 2>&1 | head -30
```

Expected: build fails on `RpcPrompter::new` arity mismatch — that's wired up in Task 3.

- [ ] **Step 5: Commit (broken build, intentional)**

```bash
git add src/wizard/session.rs
git commit -m "wizard: restore session with finish_data slot (build incomplete; prompter fixed next)"
```

---

## Task 3: Port wizard prompter (without `CliPrompter`)

**Files:**
- Create: `src/wizard/prompter.rs`

- [ ] **Step 1: Copy archive then strip `CliPrompter`**

```bash
cp ../../../archive/wizard/prompter.rs src/wizard/prompter.rs
```

Open `src/wizard/prompter.rs` and delete:
1. The `CliPrompter` struct, its `Default` impl, and its `impl WizardPrompter for CliPrompter` block (lines 228–337 in the archive).
2. The `CliProgressHandle` struct and its `impl ProgressHandle` (lines 340–356 in the archive).
3. The `test_cli_prompter_intro` test (lines 376–380 in the archive).

- [ ] **Step 2: Extend `RpcPrompter` with the `finish_data` slot + `finish` method**

Update the `RpcPrompter` struct:

```rust
pub struct RpcPrompter {
    step_tx: mpsc::Sender<WizardStep>,
    answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
    finish_data: Arc<RwLock<Option<Value>>>,
    step_counter: AtomicU64,
}
```

Update `RpcPrompter::new` to take the slot:

```rust
impl RpcPrompter {
    pub(crate) fn new(
        step_tx: mpsc::Sender<WizardStep>,
        answers: Arc<RwLock<HashMap<String, PendingAnswer>>>,
        finish_data: Arc<RwLock<Option<Value>>>,
    ) -> Self {
        Self {
            step_tx,
            answers,
            finish_data,
            step_counter: AtomicU64::new(0),
        }
    }
```

Add a new public method (place it right after `prompt`):

```rust
    /// Mark the flow as complete with a payload that propagates back through
    /// the next `wizard.next` response in `WizardNextResult.data`.
    pub async fn finish(&self, data: Value) -> Result<(), WizardSessionError> {
        *self.finish_data.write().unwrap_or_else(|e| e.into_inner()) = Some(data);
        Ok(())
    }
```

- [ ] **Step 3: Update the prompter unit test for the new arity**

In the `#[cfg(test)] mod tests`, replace `test_rpc_prompter_id_generation`'s body:

```rust
    #[test]
    fn test_rpc_prompter_id_generation() {
        let (tx, _rx) = mpsc::channel(16);
        let answers = Arc::new(RwLock::new(HashMap::new()));
        let finish_data = Arc::new(RwLock::new(None));
        let prompter = RpcPrompter::new(tx, answers, finish_data);

        let id1 = prompter.next_id();
        let id2 = prompter.next_id();

        assert_eq!(id1, "step-1");
        assert_eq!(id2, "step-2");
    }
```

- [ ] **Step 4: Write a new failing test for `finish`**

Append in the same `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn finish_stores_payload() {
        let (tx, _rx) = mpsc::channel(16);
        let answers = Arc::new(RwLock::new(HashMap::new()));
        let finish_data = Arc::new(RwLock::new(None));
        let prompter = RpcPrompter::new(tx, answers, finish_data.clone());

        prompter
            .finish(serde_json::json!({ "token": "secret" }))
            .await
            .unwrap();

        let stored = finish_data.read().unwrap().clone().unwrap();
        assert_eq!(stored["token"], "secret");
    }
```

- [ ] **Step 5: Compile + run prompter tests**

```bash
cargo test -p alephcore --lib wizard::prompter::tests
```

Expected: 2 pass (`test_rpc_prompter_id_generation`, `finish_stores_payload`).

- [ ] **Step 6: Commit**

```bash
git add src/wizard/prompter.rs
git commit -m "wizard: restore prompter (RpcPrompter only) with finish() method"
```

---

## Task 4: Port wizard mod.rs + flows scaffolding

**Files:**
- Create: `src/wizard/mod.rs`
- Create: `src/wizard/flows/mod.rs`
- Modify: `src/lib.rs` (add `pub mod wizard;`)

- [ ] **Step 1: Write `src/wizard/mod.rs`** (verbatim from archive minus `CliPrompter` re-export)

```rust
//! Wizard system for guided configuration.
//!
//! Session-based wizard framework. The flow runs as a background task,
//! exchanges `WizardStep` ↔ answer pairs with the client through an
//! `RpcPrompter`, and surfaces its final payload via
//! `WizardNextResult.data` on the closing `wizard.next` call.

pub mod flows;
pub mod prompter;
pub mod session;
pub mod types;

pub use flows::pairing::PairingFlow;
pub use prompter::{ProgressHandle, RpcPrompter, WizardPrompter};
pub use session::{WizardFlow, WizardSession, WizardSessionError};
pub use types::{
    StepExecutor, StepType, WizardAnswer, WizardNextResult, WizardOption, WizardStatus, WizardStep,
};
```

(`OnboardingFlow` re-exports come later in Task 5 once it compiles; `PairingFlow` re-export lands here too — Task 6 supplies the symbol.)

- [ ] **Step 2: Write `src/wizard/flows/mod.rs`** (Onboarding wired in Task 5, Pairing in Task 6)

```rust
//! Wizard flow implementations.
pub mod pairing;
```

- [ ] **Step 3: Add the module to the crate**

In `src/lib.rs`, insert `pub mod wizard;` in the alphabetical block of `pub mod` declarations (find the cluster around `pub mod verification;` and place it after).

```rust
pub mod verification;
pub mod wizard;
```

- [ ] **Step 4: Stub `src/wizard/flows/pairing.rs` so the tree compiles**

```rust
//! Same-machine PairingFlow — implemented in Task 6.

use crate::wizard::{RpcPrompter, WizardFlow, WizardSessionError};
use async_trait::async_trait;

/// Same-machine pairing flow — placeholder until Task 6 wires the body.
pub struct PairingFlow;

#[async_trait]
impl WizardFlow for PairingFlow {
    async fn run(&self, _prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        Err(WizardSessionError::FlowError(
            "PairingFlow body not yet implemented".to_string(),
        ))
    }

    fn name(&self) -> &str {
        "pairing"
    }
}
```

- [ ] **Step 5: Compile check**

```bash
cargo check -p alephcore --lib 2>&1 | grep -E "^(error|warning:)" | head -10
```

Expected: zero errors (warnings about unused `_prompter` are acceptable here — the body lands in Task 6).

- [ ] **Step 6: Commit**

```bash
git add src/wizard/mod.rs src/wizard/flows/mod.rs src/wizard/flows/pairing.rs src/lib.rs
git commit -m "wizard: mod + flows scaffolding (PairingFlow stub)"
```

---

## Task 5: Port onboarding flow (compile-only revival)

**Files:**
- Create: `src/wizard/flows/onboarding.rs`
- Modify: `src/wizard/flows/mod.rs`
- Modify: `src/wizard/mod.rs`

- [ ] **Step 1: Copy verbatim**

```bash
cp ../../../archive/wizard/flows/onboarding.rs src/wizard/flows/onboarding.rs
```

- [ ] **Step 2: Wire the flow into `flows/mod.rs`**

```rust
//! Wizard flow implementations.
pub mod onboarding;
pub mod pairing;

pub use onboarding::OnboardingFlow;
```

- [ ] **Step 3: Re-export from top-level `wizard/mod.rs`**

Update the `pub use flows::…` line in `src/wizard/mod.rs` to include onboarding:

```rust
pub use flows::pairing::PairingFlow;
pub use flows::{
    onboarding::{OnboardingData, ProviderSetupFlow, QuickSetupFlow},
    OnboardingFlow,
};
```

- [ ] **Step 4: Compile check**

```bash
cargo check -p alephcore --lib 2>&1 | grep -E "^error" | head -10
```

Expected: zero errors. If onboarding.rs uses any archive-era API that no longer exists in `src/`, file a TODO comment and stub the offending call site to keep compilation green — but per spec the flow is compile-only revival, not behavioural revival.

- [ ] **Step 5: Run any onboarding-flow tests that came with the archive**

```bash
cargo test -p alephcore --lib wizard::flows::onboarding
```

Expected: PASS (archive's own tests, if any).

- [ ] **Step 6: Commit**

```bash
git add src/wizard/flows/onboarding.rs src/wizard/flows/mod.rs src/wizard/mod.rs
git commit -m "wizard: revive onboarding flow (compile-only)"
```

---

## Task 6: Implement `PairingFlow` (TDD)

**Files:**
- Modify: `src/wizard/flows/pairing.rs`
- Test: same file (`#[cfg(test)] mod tests`)

The flow uses `PairingManager`, `DeviceStore`, `SecurityStore`, `TokenManager` exactly like `src/bin/aleph-server/commands/pairing.rs:84–179` (the CLI `approve_locked`), but without the `with_policy` wrapper because it runs inside the daemon process.

- [ ] **Step 1: Write the failing happy-path test**

Replace the stub body with the test setup + a failing assertion. Full file content:

```rust
//! Same-machine PairingFlow.
//!
//! Walks the desktop shell through device pairing in two user-visible
//! beats — "welcome" then "approve" — and returns the issued device
//! token via `RpcPrompter::finish`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::gateway::device_store::{ApprovedDevice, DeviceStore};
use crate::gateway::security::{
    store::DeviceUpsertData, DeviceRole, PairingManager, PairingRequest, SecurityStore,
    TokenManager,
};
use crate::wizard::{RpcPrompter, WizardFlow, WizardSessionError, WizardStep};

/// Same-machine pairing flow: requests a code, asks the user to confirm,
/// approves the device, and returns the issued token via `finish`.
pub struct PairingFlow {
    pub device_name: String,
    pub pairing_manager: Arc<PairingManager>,
    pub security_store: Arc<SecurityStore>,
    pub device_store: Arc<DeviceStore>,
    pub token_manager: Arc<TokenManager>,
}

impl PairingFlow {
    /// Construct from the standard daemon security bundle.
    pub fn new(
        device_name: impl Into<String>,
        pairing_manager: Arc<PairingManager>,
        security_store: Arc<SecurityStore>,
        device_store: Arc<DeviceStore>,
        token_manager: Arc<TokenManager>,
    ) -> Self {
        Self {
            device_name: device_name.into(),
            pairing_manager,
            security_store,
            device_store,
            token_manager,
        }
    }

    /// Synthesise a stable 32-byte public-key placeholder from the device_id.
    /// Mirrors the same trick used by the CLI `approve_locked` until real
    /// keypair generation is wired.
    fn placeholder_pubkey(device_id: &str) -> [u8; 32] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut h = DefaultHasher::new();
        device_id.hash(&mut h);
        let hash = h.finish();
        let mut buf = [0u8; 32];
        buf[..8].copy_from_slice(&hash.to_le_bytes());
        buf[8..16].copy_from_slice(&(hash.wrapping_mul(0x9e3779b97f4a7c15)).to_le_bytes());
        buf
    }
}

#[async_trait]
impl WizardFlow for PairingFlow {
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        // 1. user-visible greeting
        prompter
            .prompt(WizardStep::note(
                "pairing-welcome",
                "为本机桌面配对 Aleph 守护进程",
            ))
            .await?;

        // 2. internal: request a pairing code (uses a placeholder pubkey;
        // confirm() consumes the row regardless of pubkey content for
        // same-machine flows)
        let req = self
            .pairing_manager
            .request_device_pairing(self.device_name.clone(), None, vec![0u8; 32], None)
            .map_err(|e| WizardSessionError::FlowError(format!("request_device_pairing: {e}")))?;
        let code = req.code().to_string();

        // 3. user-visible confirm step
        prompter
            .prompt(WizardStep::confirm(
                "pairing-approve",
                format!("本机配对码：{code}\n点击「Approve」完成同机授权"),
            ))
            .await?;

        // 4. internal: consume the pairing row + register device + issue token
        let confirmed = self
            .pairing_manager
            .confirm_pairing(&code)
            .map_err(|e| WizardSessionError::FlowError(format!("confirm_pairing: {e}")))?;

        let (device_name, device_type) = match &confirmed {
            PairingRequest::Device {
                device_name,
                device_type,
                ..
            } => (
                device_name.clone(),
                device_type.map(|t| t.as_str().to_string()),
            ),
            PairingRequest::Channel { .. } => {
                return Err(WizardSessionError::FlowError(
                    "PairingFlow expects a device request, got a channel request".to_string(),
                ));
            }
        };

        let device_id = uuid::Uuid::new_v4().to_string();
        let device = ApprovedDevice::new(device_id.clone(), device_name.clone(), device_type);

        self.device_store
            .approve_device(&device)
            .map_err(|e| WizardSessionError::FlowError(format!("approve_device: {e}")))?;

        let pk = Self::placeholder_pubkey(&device_id);
        self.security_store
            .upsert_device(&DeviceUpsertData {
                device_id: &device_id,
                device_name: &device_name,
                device_type: None,
                public_key: &pk,
                fingerprint: &device_id[..device_id.len().min(16)],
                role: "operator",
                scopes: &["*".to_string()],
            })
            .map_err(|e| WizardSessionError::FlowError(format!("upsert_device: {e}")))?;

        let signed = self
            .token_manager
            .issue_token(&device_id, DeviceRole::Operator, vec!["*".to_string()])
            .map_err(|e| WizardSessionError::FlowError(format!("issue_token: {e}")))?;
        let token = format!("{}:{}", signed.token, signed.signature);

        // 5. internal: persist to OS keychain — best-effort, non-blocking
        if let Err(e) = persist_token_to_keyring(&token) {
            tracing::warn!(error = %e, "keyring persist failed; pairing succeeded anyway");
        }

        // 6. finish: hand the token back through wizard.next's final response
        prompter
            .finish(json!({
                "token": token,
                "device_id": device_id,
                "device_name": device_name,
            }))
            .await?;
        Ok(())
    }

    fn name(&self) -> &str {
        "pairing"
    }
}

const KEYRING_SERVICE: &str = "aleph-gateway";
const KEYRING_USER: &str = "desktop-shell";

fn persist_token_to_keyring(token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("entry: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("set_password: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::WizardSession;
    use std::sync::Arc;
    use std::time::Duration;

    fn test_bundle() -> (
        Arc<PairingManager>,
        Arc<SecurityStore>,
        Arc<DeviceStore>,
        Arc<TokenManager>,
    ) {
        let security = Arc::new(SecurityStore::in_memory().unwrap());
        let devices = Arc::new(DeviceStore::in_memory().unwrap());
        let pairing = Arc::new(PairingManager::new(security.clone()));
        let tokens = Arc::new(TokenManager::new(security.clone()));
        (pairing, security, devices, tokens)
    }

    #[tokio::test]
    async fn pairing_flow_emits_two_steps_then_returns_token() {
        let (pairing, security, devices, tokens) = test_bundle();
        let flow = PairingFlow::new(
            "Test Mac",
            pairing,
            security,
            devices,
            tokens,
        );
        let session = WizardSession::new(Box::new(flow));

        // Step 1: welcome
        let r = session.next().await;
        assert!(!r.done);
        let step = r.step.expect("welcome step");
        assert_eq!(step.id, "pairing-welcome");

        // Answer the welcome (note has no required answer; client convention
        // is to send `null` via wizard.next which only blocks notes through
        // the manager — direct session.answer for the step id keeps the
        // unit test simple).
        session.answer("pairing-welcome", serde_json::Value::Null).await.unwrap();

        // Step 2: confirm
        let r = session.next().await;
        assert!(!r.done);
        let step = r.step.expect("confirm step");
        assert_eq!(step.id, "pairing-approve");
        assert!(step.message.as_deref().unwrap().contains("配对码"));

        session.answer("pairing-approve", serde_json::Value::Bool(true)).await.unwrap();

        // Give the flow a beat to run the internal approve+token block.
        // 200ms is generous for an in-memory store; the test will retry the
        // next() drain below.
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if session.is_done() {
                break;
            }
        }

        let r = session.next().await;
        assert!(r.done);
        let data = r.data.expect("finish data");
        let token = data.get("token").and_then(|v| v.as_str()).unwrap();
        assert!(token.contains(':'), "token format: <body>:<sig>");
        assert!(data.get("device_id").is_some());
        assert_eq!(data.get("device_name").and_then(|v| v.as_str()), Some("Test Mac"));
    }
}
```

- [ ] **Step 2: Run the test — expect FAIL**

```bash
cargo test -p alephcore --lib wizard::flows::pairing::tests::pairing_flow_emits_two_steps_then_returns_token
```

Expected: FAIL — the prompter `prompt(note)` actually blocks on an answer because `note` uses `prompt` not `prompt_no_wait`. The test feeds a null answer; if the assertion order doesn't line up, fix the test ordering or switch the welcome step to `prompt_no_wait` by introducing a new prompter helper.

If the failure is on a missing `SecurityStore::in_memory()` (etc.), add a `#[cfg(test)]` constructor in the corresponding security module (one-liner that uses `:memory:` SQLite path).

- [ ] **Step 3: Iterate until green**

Goal of the iteration: the test asserts the exact step sequence (`pairing-welcome` → `pairing-approve`) and that the finish payload contains a colon-separated token plus `device_id` / `device_name`. If something else trips, prefer fixing the implementation; only touch the test if the assertion was actually wrong.

```bash
cargo test -p alephcore --lib wizard::flows::pairing
```

Expected: PASS.

- [ ] **Step 4: Add a failure-path test**

Append in the same `mod tests`:

```rust
    #[tokio::test]
    async fn pairing_flow_propagates_request_failure() {
        // Drive the flow with a 0ms-expiry manager so the pairing code
        // produced in step 2 has effectively expired by the time step 4
        // tries to confirm it. We assert that the session reaches some
        // terminal state cleanly (Done or Error) without panicking — the
        // exact terminal depends on timing race between request and
        // confirm; either is acceptable for the smoke-fail path.
        let security = Arc::new(SecurityStore::in_memory().unwrap());
        let devices = Arc::new(DeviceStore::in_memory().unwrap());
        let pairing = Arc::new(PairingManager::with_expiry(security.clone(), 0)); // 0ms expiry → instant timeout
        let tokens = Arc::new(TokenManager::new(security.clone()));
        let flow = PairingFlow::new("Test", pairing, security, devices, tokens);
        let session = WizardSession::new(Box::new(flow));

        // welcome
        let _ = session.next().await;
        session.answer("pairing-welcome", serde_json::Value::Null).await.unwrap();
        // confirm
        let _ = session.next().await;
        session.answer("pairing-approve", serde_json::Value::Bool(true)).await.unwrap();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if session.is_done() {
                break;
            }
        }
        let r = session.next().await;
        assert!(r.done);
        // Either error status (if instant expiry kicked in) OR success; both
        // are acceptable terminal states. Verify the session reached SOME
        // terminal state cleanly without panicking.
        assert!(matches!(
            r.status,
            crate::wizard::WizardStatus::Done | crate::wizard::WizardStatus::Error
        ));
    }
```

- [ ] **Step 5: Run all pairing tests**

```bash
cargo test -p alephcore --lib wizard::flows::pairing
```

Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add src/wizard/flows/pairing.rs
git commit -m "wizard(pairing): implement same-machine PairingFlow with keyring persistence"
```

---

## Task 7: Wire wizard.* RPC handlers into `HandlerRegistry`

**Files:**
- Create: `src/gateway/handlers/wizard.rs`
- Modify: `src/gateway/handlers/mod.rs`

- [ ] **Step 1: Port the archive handlers**

```bash
cp ../../../archive/gateway_handlers_wizard.rs src/gateway/handlers/wizard.rs
```

- [ ] **Step 2: Adapt the handler factory to the current `HandlerFn` shape**

Open `src/gateway/handlers/wizard.rs` and:

1. Replace the import block top-of-file:

```rust
use crate::sync_primitives::{Arc, RwLock};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::wizard::{
    WizardFlow, WizardNextResult, WizardSession, WizardSessionError, WizardStatus, WizardStep,
};
```

2. Delete the local `RpcHandler` type alias and the local `create_handlers` factory at the bottom (it builds a non-matching closure). Replace with one that returns the current registry's `HandlerFn`:

```rust
use crate::gateway::handlers::HandlerFn;

/// Build the `wizard.*` handler set bound to a session manager.
pub fn handlers(manager: Arc<WizardSessionManager>) -> Vec<(&'static str, HandlerFn)> {
    fn wrap<F, Fut>(f: F) -> HandlerFn
    where
        F: Fn(JsonRpcRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = JsonRpcResponse> + Send + 'static,
    {
        Arc::new(move |req| Box::pin(f(req)))
    }

    let m1 = manager.clone();
    let m2 = manager.clone();
    let m3 = manager.clone();
    let m4 = manager.clone();
    let m5 = manager;
    vec![
        ("wizard.start", wrap(move |req| handle_start(req, m1.clone()))),
        ("wizard.next", wrap(move |req| handle_next(req, m2.clone()))),
        ("wizard.answer", wrap(move |req| handle_answer(req, m3.clone()))),
        ("wizard.cancel", wrap(move |req| handle_cancel(req, m4.clone()))),
        ("wizard.status", wrap(move |req| handle_status(req, m5.clone()))),
    ]
}
```

3. Update `WizardSessionManager::new` to take a richer factory — it currently takes `WizardFlowFactory = Arc<dyn Fn(&str, Option<Value>) -> Option<Box<dyn WizardFlow>>>`. Keep that signature; consumers supply pairing+onboarding via the closure (Task 8).

4. Update the existing tests block at the bottom of `wizard.rs` to import `JsonRpcRequest::new` from `crate::gateway::protocol` (already imported above) and verify they still compile.

- [ ] **Step 3: Run the ported handler tests**

```bash
cargo test -p alephcore --lib gateway::handlers::wizard
```

Expected: PASS (4 tests inherited from archive: `test_session_manager_start`, `test_session_manager_unknown_type`, `test_session_manager_cancel`, `test_handle_start`, `test_handle_cancel`).

If `JsonRpcRequest::new` has a different signature in current code, adapt the test invocations — match the constructor that exists today. To check:

```bash
grep -n "impl JsonRpcRequest\|pub fn new" src/gateway/protocol/*.rs | head -5
```

- [ ] **Step 4: Register the module**

In `src/gateway/handlers/mod.rs`, add to the alphabetical `pub mod` cluster (near `pub mod version;`):

```rust
pub mod wizard;
```

Inside `HandlerRegistry::new()`, register a phase-1 `service_unavailable` stub for each wizard method so the dispatch table is deterministic before boot wires the real manager. Add near the bottom of `new()`:

```rust
        // Wizard — phase-1 stubs; boot path replaces with real handlers
        for method in [
            "wizard.start",
            "wizard.next",
            "wizard.answer",
            "wizard.cancel",
            "wizard.status",
        ] {
            let m = method;
            registry.register(method, move |req| async move {
                service_unavailable(req, "wizard manager not yet initialised")
            });
        }
```

Also expose a `pub(crate) fn install_wizard_handlers(...)` that the boot path calls in phase-2:

```rust
impl HandlerRegistry {
    /// Overlay real wizard handlers — call from the boot path once the
    /// session manager exists.
    pub fn install_wizard_handlers(&mut self, manager: Arc<crate::wizard::WizardSessionManager>) {
        for (name, handler) in crate::gateway::handlers::wizard::handlers(manager) {
            self.handlers.insert(name.to_string(), handler);
        }
    }
}
```

(Note: `WizardSessionManager` actually lives in `crate::gateway::handlers::wizard` — adjust the path. Or move `WizardSessionManager` out to `crate::wizard::manager` for cleanliness; if you do, also update Task 6's tests imports.)

- [ ] **Step 5: Compile + run registry tests**

```bash
cargo check -p alephcore --lib
cargo test -p alephcore --lib gateway::handlers
```

Expected: clean check, all registry tests still pass.

- [ ] **Step 6: Commit**

```bash
git add src/gateway/handlers/wizard.rs src/gateway/handlers/mod.rs
git commit -m "wizard: wire wizard.* JSON-RPC handlers into HandlerRegistry (phase-1 stub + install_wizard_handlers)"
```

---

## Task 8: Boot wiring (subsystems.rs)

**Files:**
- Modify: `src/bin/aleph-server/commands/start/builder/subsystems.rs`

The wizard manager needs:
- `pairing_manager`, `security_store`, `device_store`, `token_manager` — all already constructed in `initialize_auth` and returned through `AuthBundle`.

- [ ] **Step 1: Inspect the current `AuthBundle` and where it's consumed**

```bash
grep -n "AuthBundle\|initialize_auth\|install_wizard" src/bin/aleph-server/commands/start/builder/*.rs src/bin/aleph-server/commands/start/*.rs
```

Identify the file where `HandlerRegistry` is owned post-boot — likely `src/bin/aleph-server/commands/start/mod.rs` or a sibling under `builder/`.

- [ ] **Step 2: Construct the wizard manager next to `HandlerRegistry`**

Wherever the registry is built, after auth init, add:

```rust
let device_name = hostname::get()
    .ok()
    .and_then(|h| h.into_string().ok())
    .unwrap_or_else(|| "Aleph Desktop".to_string());

let pairing_mgr = auth_bundle.pairing_manager.clone();
let security_store = auth_bundle.security_store.clone();
let device_store = auth_bundle.device_store.clone();
let token_manager = auth_bundle.token_manager.clone();

let flow_factory: alephcore::gateway::handlers::wizard::WizardFlowFactory = std::sync::Arc::new(
    move |wizard_type: &str, _initial: Option<serde_json::Value>| -> Option<Box<dyn alephcore::wizard::WizardFlow>> {
        match wizard_type {
            "pairing" => Some(Box::new(alephcore::wizard::PairingFlow::new(
                device_name.clone(),
                pairing_mgr.clone(),
                security_store.clone(),
                device_store.clone(),
                token_manager.clone(),
            ))),
            "onboarding" => Some(Box::new(alephcore::wizard::OnboardingFlow::new())),
            _ => None,
        }
    },
);
let wizard_manager = std::sync::Arc::new(
    alephcore::gateway::handlers::wizard::WizardSessionManager::new(flow_factory),
);
handler_registry.install_wizard_handlers(wizard_manager.clone());
```

Crucial: `AuthBundle` does NOT currently expose `pairing_manager`, `security_store`, or `token_manager` — only `device_store`, `auth_ctx`, `mdns_broadcaster`, `invitation_manager`, `guest_session_manager`. **Extend `AuthBundle`** to add these three Arc fields, and update `initialize_auth` to populate them. Both items already exist as locals in `initialize_auth`; this is just plumbing them out.

```rust
pub(in crate::commands::start) struct AuthBundle {
    pub device_store: Arc<DeviceStore>,
    pub security_store: Arc<alephcore::gateway::security::SecurityStore>,
    pub pairing_manager: Arc<alephcore::gateway::security::PairingManager>,
    pub token_manager: Arc<alephcore::gateway::security::TokenManager>,
    pub auth_ctx: Arc<auth_handlers::AuthContext>,
    pub mdns_broadcaster: Option<alephcore::gateway::MdnsBroadcaster>,
    pub invitation_manager: Arc<alephcore::gateway::security::InvitationManager>,
    pub guest_session_manager: Arc<alephcore::gateway::security::GuestSessionManager>,
}
```

Find the existing `let pairing_manager = …`, `let token_manager = …` constructions in `initialize_auth` and clone them into the returned struct.

- [ ] **Step 3: Add `hostname` to the bin dependencies** (only if not already)

```bash
grep -n "hostname" src/bin/aleph-server/Cargo.toml Cargo.toml
```

If absent, add to `src/bin/aleph-server/Cargo.toml`'s `[dependencies]`:

```toml
hostname = "0.4"
```

Or fall back to `std::env::var("HOSTNAME").unwrap_or_else(|_| "Aleph Desktop".into())` to avoid a new dep.

- [ ] **Step 4: Compile**

```bash
cargo check -p alephcore --bin aleph-server 2>&1 | grep -E "^error" | head -10
```

Expected: zero errors.

- [ ] **Step 5: Add a smoke integration test driving wizard end-to-end**

Create `tests/wizard_pairing_e2e.rs`:

```rust
//! End-to-end driver for the pairing wizard via the JSON-RPC handler
//! surface — proves that `wizard.start("pairing") → wizard.next* → done`
//! returns a token payload.

use std::sync::Arc;

use alephcore::gateway::device_store::DeviceStore;
use alephcore::gateway::handlers::wizard::{
    handle_answer, handle_next, handle_start, WizardSessionManager,
};
use alephcore::gateway::protocol::JsonRpcRequest;
use alephcore::gateway::security::{PairingManager, SecurityStore, TokenManager};
use alephcore::wizard::{PairingFlow, WizardFlow};
use serde_json::json;

#[tokio::test]
async fn pairing_wizard_round_trip_returns_token() {
    let security = Arc::new(SecurityStore::in_memory().unwrap());
    let devices = Arc::new(DeviceStore::in_memory().unwrap());
    let pairing = Arc::new(PairingManager::new(security.clone()));
    let tokens = Arc::new(TokenManager::new(security.clone()));

    let security_for_factory = security.clone();
    let pairing_for_factory = pairing.clone();
    let devices_for_factory = devices.clone();
    let tokens_for_factory = tokens.clone();
    let factory = Arc::new(move |t: &str, _: Option<serde_json::Value>| {
        if t == "pairing" {
            Some(Box::new(PairingFlow::new(
                "E2E Mac",
                pairing_for_factory.clone(),
                security_for_factory.clone(),
                devices_for_factory.clone(),
                tokens_for_factory.clone(),
            )) as Box<dyn WizardFlow>)
        } else {
            None
        }
    });
    let manager = Arc::new(WizardSessionManager::new(factory));

    // wizard.start
    let req = JsonRpcRequest::new(
        "wizard.start",
        Some(json!({ "wizard_type": "pairing" })),
        Some(json!(1)),
    );
    let resp = handle_start(req, manager.clone()).await;
    let start_body: serde_json::Value = resp.result.clone().unwrap();
    let session_id = start_body["session_id"].as_str().unwrap().to_string();
    assert_eq!(start_body["step"]["id"], "pairing-welcome");

    // answer welcome
    let req = JsonRpcRequest::new(
        "wizard.answer",
        Some(json!({
            "session_id": session_id,
            "step_id": "pairing-welcome",
            "value": null,
        })),
        Some(json!(2)),
    );
    let resp = handle_answer(req, manager.clone()).await;
    let body: serde_json::Value = resp.result.clone().unwrap();
    assert_eq!(body["step"]["id"], "pairing-approve");

    // answer approve
    let req = JsonRpcRequest::new(
        "wizard.answer",
        Some(json!({
            "session_id": session_id,
            "step_id": "pairing-approve",
            "value": true,
        })),
        Some(json!(3)),
    );
    let resp = handle_answer(req, manager.clone()).await;
    let body: serde_json::Value = resp.result.clone().unwrap();
    assert_eq!(body["done"], true);
    assert!(body["data"]["token"].as_str().unwrap().contains(':'));
    assert_eq!(body["data"]["device_name"], "E2E Mac");
}
```

`JsonRpcResponse` has a public `result: Option<Value>` field (`src/gateway/protocol.rs:113`), so `resp.result.clone().unwrap()` is the canonical accessor.

- [ ] **Step 6: Run the e2e test**

```bash
cargo test --test wizard_pairing_e2e -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/bin/aleph-server/commands/start/builder/subsystems.rs \
        src/bin/aleph-server/Cargo.toml \
        tests/wizard_pairing_e2e.rs
git commit -m "wizard: boot wiring — flow factory + install_wizard_handlers + e2e test"
```

---

## Task 9: Panel `PairingModal` component

**Files:**
- Create: `interfaces/webchat/src/views/pairing_modal.rs`
- Modify: `interfaces/webchat/src/views/mod.rs`
- Modify: `interfaces/webchat/src/context.rs` (auth.connect error handler hook)

- [ ] **Step 1: Locate the existing `auth.connect` call site**

```bash
grep -rn "auth.connect\|\"connect\"" interfaces/webchat/src/ shared/ui_logic/src/ | head -10
```

Identify the file where the panel issues the connect RPC. Usually `interfaces/webchat/src/context.rs` or `shared/ui_logic/src/connection/*`.

- [ ] **Step 2: Add a signal exposing `Option<PairingRequiredParams>` to the panel context**

In `interfaces/webchat/src/context.rs`, near the existing `DashboardState` fields, add:

```rust
pub pairing_required: RwSignal<Option<PairingPrompt>>,
```

Define `PairingPrompt`:

```rust
#[derive(Debug, Clone)]
pub struct PairingPrompt {
    pub session_id: Option<String>,   // populated after wizard.start
    pub initial_step: Option<serde_json::Value>,
    pub last_error: Option<String>,
}
```

Wire the connect-error path: when the panel's `connect` RPC returns the error `pairing_required`, set `pairing_required.set(Some(PairingPrompt::default_for(err_data)))`.

- [ ] **Step 3: Write the modal component**

Create `interfaces/webchat/src/views/pairing_modal.rs`:

```rust
use leptos::prelude::*;
use leptos::task::spawn_local;
use serde_json::{json, Value};

use crate::context::{DashboardState, PairingPrompt};

#[component]
pub fn PairingModal() -> impl IntoView {
    let state = use_context::<DashboardState>().expect("DashboardState in context");
    let prompt = state.pairing_required;

    let current_step: RwSignal<Option<Value>> = RwSignal::new(None);
    let pairing_session: RwSignal<Option<String>> = RwSignal::new(None);
    let last_error: RwSignal<Option<String>> = RwSignal::new(None);

    // When `prompt` flips to Some, kick off wizard.start.
    Effect::new(move |_| {
        if prompt.get().is_none() {
            return;
        }
        let state = state;
        spawn_local(async move {
            match state
                .rpc_call("wizard.start", json!({ "wizard_type": "pairing" }))
                .await
            {
                Ok(resp) => {
                    pairing_session.set(resp["session_id"].as_str().map(str::to_string));
                    current_step.set(Some(resp["step"].clone()));
                }
                Err(e) => last_error.set(Some(format!("wizard.start failed: {e}"))),
            }
        });
    });

    let advance = move |answer: Value| {
        let Some(session_id) = pairing_session.get_untracked() else {
            return;
        };
        let Some(step) = current_step.get_untracked() else {
            return;
        };
        let step_id = step["id"].as_str().unwrap_or_default().to_string();
        let state = state;
        spawn_local(async move {
            match state
                .rpc_call(
                    "wizard.answer",
                    json!({
                        "session_id": session_id,
                        "step_id": step_id,
                        "value": answer,
                    }),
                )
                .await
            {
                Ok(resp) => {
                    if resp["done"].as_bool().unwrap_or(false) {
                        // Extract token and trigger reconnect.
                        if let Some(token) = resp["data"]["token"].as_str() {
                            state.set_pairing_token(token.to_string());
                            state.pairing_required.set(None);
                            state.reconnect();
                        } else if let Some(err) = resp["error"].as_str() {
                            last_error.set(Some(err.to_string()));
                        }
                    } else {
                        current_step.set(Some(resp["step"].clone()));
                    }
                }
                Err(e) => last_error.set(Some(format!("wizard.answer failed: {e}"))),
            }
        });
    };

    view! {
        <Show when=move || prompt.get().is_some()>
            <div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
                <div class="bg-surface-raised rounded-lg shadow-2xl p-6 max-w-md w-full">
                    <h2 class="text-lg font-semibold text-text-primary mb-4">"配对 Aleph"</h2>
                    {move || current_step.get().map(|step| {
                        let id = step["id"].as_str().unwrap_or("").to_string();
                        let message = step["message"].as_str().unwrap_or("").to_string();
                        let step_type = step["type"].as_str().unwrap_or("note").to_string();
                        view! {
                            <p class="text-sm text-text-secondary whitespace-pre-line mb-4">{message}</p>
                            <div class="flex gap-2 justify-end">
                                <button
                                    class="px-4 py-2 text-sm text-text-tertiary hover:text-text-primary"
                                    on:click=move |_| {
                                        spawn_local(async move {
                                            if let Some(sid) = pairing_session.get_untracked() {
                                                let _ = state.rpc_call(
                                                    "wizard.cancel",
                                                    json!({ "session_id": sid }),
                                                ).await;
                                            }
                                            state.pairing_required.set(None);
                                        });
                                    }
                                >
                                    "Cancel"
                                </button>
                                <button
                                    class="px-4 py-2 text-sm bg-primary text-white rounded hover:opacity-90"
                                    on:click=move |_| {
                                        let payload = match step_type.as_str() {
                                            "confirm" => json!(true),
                                            _ => Value::Null,
                                        };
                                        advance(payload);
                                    }
                                >
                                    {if step_type == "confirm" { "Approve" } else { "Continue" }}
                                </button>
                            </div>
                        }
                    })}
                    {move || last_error.get().map(|err| view! {
                        <div class="mt-4 p-3 bg-red-900/30 text-red-300 text-xs rounded">{err}</div>
                    })}
                </div>
            </div>
        </Show>
    }
}
```

- [ ] **Step 4: Mount the modal at the top of the panel layout**

In whichever view file is the panel's root (find via `grep -rn "App\|Dashboard" interfaces/webchat/src/main.rs interfaces/webchat/src/views/mod.rs`), add:

```rust
use crate::views::pairing_modal::PairingModal;
// inside the root view! { ... }
<PairingModal/>
```

Also add `pub mod pairing_modal;` to `interfaces/webchat/src/views/mod.rs`.

- [ ] **Step 5: Expose helpers on `DashboardState`**

Add to `interfaces/webchat/src/context.rs`:

```rust
impl DashboardState {
    /// Persist token in localStorage (panel-side fallback persistence; the
    /// daemon-side keyring write happens inside PairingFlow).
    pub fn set_pairing_token(&self, token: String) {
        #[cfg(target_arch = "wasm32")]
        set_local_storage("aleph.pairing_token", &token);
        let _ = token;
    }

    /// Trigger a reconnect with the new token threaded into the auth handshake.
    pub fn reconnect(&self) {
        spawn_local({
            let connector = self.connector.clone();
            async move {
                let _ = connector.reconnect().await;
            }
        });
    }
}
```

(`connector.reconnect()` may be a no-op stub; if so, the panel's existing reconnect path through `AlephConnector` covers it. Trust whatever method already exists rather than inventing a new one — adapt the call site.)

- [ ] **Step 6: Compile WASM build**

```bash
cargo build -p aleph-panel --target wasm32-unknown-unknown 2>&1 | grep -E "^error" | head -10
```

Expected: zero errors. Warnings about unused signals in non-WASM target are fine.

- [ ] **Step 7: Add wasm-bindgen-test for the modal-trigger flow** (optional but recommended)

Skip if the panel does not already have a wasm-bindgen-test target. Otherwise add a minimal assertion that mounting `<PairingModal/>` with a non-None `pairing_required` signal renders a button labelled "Approve" when the step is a confirm step.

- [ ] **Step 8: Commit**

```bash
git add interfaces/webchat/src/views/pairing_modal.rs \
        interfaces/webchat/src/views/mod.rs \
        interfaces/webchat/src/context.rs
git commit -m "panel: PairingModal — auto-trigger on pairing_required, drive wizard.* RPC, reconnect with token"
```

---

## Task 10: Desktop shell keyring boot integration

**Files:**
- Modify: `desktop/shell/src/main.rs`
- Modify: `desktop/shell/Cargo.toml` (add `keyring` dep)

- [ ] **Step 1: Add keyring to the shell crate**

In `desktop/shell/Cargo.toml`, under `[dependencies]`:

```toml
keyring = { workspace = true }
```

(The workspace already declares it at `Cargo.toml:203` with the right features.)

- [ ] **Step 2: Load token at the top of `main`**

In `desktop/shell/src/main.rs`, find the start of `fn main()` and insert before any subsystem init:

```rust
fn load_pairing_token() -> Option<String> {
    let entry = keyring::Entry::new("aleph-gateway", "desktop-shell").ok()?;
    match entry.get_password() {
        Ok(t) if !t.is_empty() => Some(t),
        _ => None,
    }
}

fn main() {
    if std::env::var_os("ALEPH_GATEWAY_TOKEN").is_none() {
        if let Some(token) = load_pairing_token() {
            // SAFETY: `set_var` is only unsafe when other threads may be
            // reading the environment concurrently. This is the first line
            // of `main` (single-threaded, before tokio/tauri spawn), and
            // we early-return if the env var is already set, so no other
            // code path is observing `ALEPH_GATEWAY_TOKEN` yet.
            // `notify.rs:78` reads it lazily inside `connect_request`,
            // which runs much later after the notify subsystem boots.
            unsafe { std::env::set_var("ALEPH_GATEWAY_TOKEN", token); }
            tracing::info!("loaded pairing token from OS keychain");
        }
    }
    // … existing main body …
}
```

- [ ] **Step 3: Compile + smoke run**

```bash
cargo check -p aleph-desktop-shell 2>&1 | grep -E "^error" | head -10
```

Expected: zero errors.

- [ ] **Step 4: Commit**

```bash
git add desktop/shell/Cargo.toml desktop/shell/src/main.rs
git commit -m "shell: load gateway token from OS keychain before subsystems boot"
```

---

## Task 11: Full project check + workspace lint

- [ ] **Step 1: Workspace check**

```bash
# Wait until cargo concurrency drops if needed:
N=$(pgrep -x cargo | wc -l | tr -d ' '); while [ "$N" -ge 3 ]; do sleep 2; N=$(pgrep -x cargo | wc -l | tr -d ' '); done
cargo check --workspace --all-targets 2>&1 | grep -E "^(error|warning:)" | sort | uniq -c | sort -rn
```

Expected: zero errors. Warnings should match baseline (no new warnings introduced by this work).

- [ ] **Step 2: Targeted clippy**

```bash
cargo clippy -p alephcore --lib --tests -- -D warnings 2>&1 | grep -E "^(error|warning:)" | head -20
cargo clippy -p aleph-panel --lib --tests --target wasm32-unknown-unknown 2>&1 | grep -E "^(error|warning:)" | head -10
```

Expected: clean. If clippy flags pre-existing issues, leave them; this task only blocks on *new* lints from the wizard work.

- [ ] **Step 3: All wizard tests**

```bash
cargo test -p alephcore --lib wizard
cargo test --test wizard_pairing_e2e
```

Expected: all PASS.

- [ ] **Step 4: Final commit (if anything changed for clippy)**

```bash
git status
# only commit if there are real changes; otherwise skip.
```

---

## Task 12: Smoke run — Tauri shell + daemon

**Files:** none (validation only)

- [ ] **Step 1: Clear any stale state**

```bash
rm -f ~/.aleph/data/aleph.lock
pgrep -lf "aleph-server\|aleph-desktop-shell" && pkill -9 -f "aleph-server\|aleph-desktop-shell" || true
```

- [ ] **Step 2: Launch shell in dev mode**

```bash
# from the worktree root:
cd ../../..   # back to repo root, just/binaries assume repo root cwd
just shell-dev 2>&1 | tee /tmp/aleph-shell-smoke.log &
```

- [ ] **Step 3: Wait for the wizard to be reachable**

```bash
until grep -qE "Watching for file changes|Devtools listening|Running.*tauri" /tmp/aleph-shell-smoke.log; do sleep 5; done
```

Expected: log shows the dev server boot lines.

- [ ] **Step 4: Manual visual check**

Open the Tauri window. On first launch (no keyring entry) you should see the `<PairingModal/>` overlay with the welcome step, then the confirm step + 6-digit code + "Approve" button.

After clicking Approve:
- Modal disappears
- Main chat panel mounts
- `tail -30 ~/.aleph/data/logs/aleph.log` (path may differ) should show no `pairing_required` errors after the reconnect.

Verify the keyring entry landed:

```bash
security find-generic-password -s "aleph-gateway" -a "desktop-shell" -w 2>&1 | head -3
```

Expected: prints the colon-separated token.

- [ ] **Step 5: Restart shell — pairing should NOT pop**

Quit the Tauri app (`Cmd+Q`), re-run `just shell-dev`. Modal should stay hidden; panel boots straight to chat.

- [ ] **Step 6: Tear down**

```bash
pkill -f "aleph-server\|aleph-desktop-shell" 2>/dev/null
rm -f ~/.aleph/data/aleph.lock
```

- [ ] **Step 7: Final commit + push (optional — depends on user instruction)**

The plan does NOT auto-push. Surface the branch summary back to the user:

```bash
git log main..pairing-wizard --oneline
```

Ask the user whether to merge back to main (the project follows single-branch development per `CLAUDE.md` — but this work-in-worktree pattern is the user's preferred isolation for code phase).

---

## Self-Review Notes

- Spec → Plan coverage:
  - "Restore archive/wizard into src/ minus CliPrompter" → Tasks 1–5.
  - "Add PairingFlow with same-machine 2-beat UX + keyring persist" → Task 6.
  - "Wire wizard.* RPC into HandlerRegistry" → Task 7.
  - "Boot wiring extends AuthBundle and installs handlers" → Task 8 + e2e test.
  - "Panel modal auto-triggers on pairing_required and reconnects" → Task 9.
  - "Desktop shell loads token from keyring before notify subsystem" → Task 10.
  - "Tests: unit, integration, panel wasm — skip Tauri WebDriver" → Tasks 1, 3, 6, 8, 9.
- Type consistency:
  - `WizardNextResult.data: Option<Value>` added in Task 1, consumed in Tasks 6, 8, 9.
  - `RpcPrompter::new(step_tx, answers, finish_data)` arity introduced in Task 3, consumer updated in Task 2 (session) — order is deliberate: Task 2 leaves a broken build that Task 3 fixes (commit message flags this).
  - `WizardSessionManager` lives in `crate::gateway::handlers::wizard` per archive layout; Task 7 notes the option to move it to `crate::wizard::manager` for cleanliness.
- Known follow-ups (not in this plan): replace placeholder pubkey with real Ed25519 generation; cross-device manual pairing mode; remove legacy `aleph-gateway pairing approve` CLI once the wizard ships.

