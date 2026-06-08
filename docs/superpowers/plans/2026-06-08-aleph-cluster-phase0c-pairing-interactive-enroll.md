# Cluster Phase 0c-pairing — Interactive Enroll Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a tokenless `aleph-server node` dial the center, get a 6-digit pairing code the operator approves from the Panel, receive a node-role token, persist it, and run normally — reusing the browser-pairing machinery.

**Architecture:** Center-side reuses the cold-browser pairing flow (anonymous `start` → `PairingRequested` event → operator `pairing.approve` → single-use `pairing.poll`), adding a `PairingRequest::Node` variant that mints a `DeviceRole::Node` token instead of a chat-tier device token. Node-side adds a `run_pairing` phase before the existing 0c-core `run_session` reconnect loop, persists the credential to `~/.aleph/node/<name>.json`, and re-pairs on `AUTH_FAILED`.

**Tech Stack:** Rust, tokio, tokio-tungstenite (WS), axum gateway, rusqlite-backed `SecurityStore`, serde_json.

**Spec:** `docs/superpowers/specs/2026-06-08-aleph-cluster-phase0c-pairing-interactive-enroll.md`

---

## Setup (controller, before Task 1)

Create a worktree from `main` (which already contains 0a + 0b + 0c-core):

```bash
git -C /Volumes/TBU4/Workspace/Aleph worktree add /Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0c-pairing -b feat/cluster-phase0c-pairing main
```

All task commands run from `/Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0c-pairing`. A bare `cargo` from the parent dir resolves to the main workspace (wrong `target/`, lacks this branch) — always `cd` into the worktree first.

---

## Deviations from spec (correctness-driven, same intent)

Two refinements discovered while reading the code. Both preserve the spec's "reuse browser machinery" intent:

1. **One generalized poll, not a separate `poll_node_pairing`.** `pairing.poll` dispatches to a single shared handler (`handle_pairing_poll` → `poll_browser_pairing`). A second method would be unreachable. Instead, generalize the one method's Pending check from `pairing_type == "browser"` to `matches!(.., "browser" | "node")`. The approved/rejected side-tables are already keyed by globally-unique code and type-agnostic.

2. **Panel-only approval; CLI rejects node codes.** The `approved_browser_sessions` side-table is in-process (a `DashMap` in the running server). The separate `aleph pairing approve` CLI process cannot deposit into it — exactly why browser codes are CLI-rejected today. Node pairing inherits this: the node's printed message points to the **Panel notification card**, and the CLI `pairing.approve` rejects node codes with a clear message. The gateway `pairing.approve` RPC (Panel/authenticated client, same process) is the approval path.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/gateway/security/pairing.rs` | `PairingRequest::Node` variant, `From`/`code`/`expires_at` arms, `request_node_pairing`, generalized poll | 1 |
| `src/gateway/handlers/auth/pairing.rs` | approve handler `Node` arm (mint node token), `handle_pairing_start_node` | 1, 2 |
| `src/gateway/handlers/auth/mod.rs` | re-export `handle_pairing_start_node` | 2 |
| `src/bin/aleph-server/commands/pairing.rs` | CLI list display + CLI approve `Node` reject arms (compile-fix) | 1 |
| `src/wizard/flows/pairing.rs` | wizard `Node` error arm (compile-fix) | 1 |
| `src/gateway/server/handler.rs` | WS unauth allowlist adds `pairing.start_node` | 2 |
| `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` | `register_handler!` for `pairing.start_node` | 2 |
| `src/bin/aleph-server/cli.rs` | `Node.token` → `Option<String>` | 3 |
| `src/bin/aleph-server/main.rs` | node arm passes `Option` | 3 |
| `src/bin/aleph-server/commands/node.rs` | `NodeCredential` + persistence + `run_pairing` + three-state `handle_node` + `SessionOutcome` fallback | 3 |

---

## Task 1: Center-side pairing state + node approval

Adding the `Node` enum variant breaks every exhaustive `match` on `PairingRequest`, so this task lands the variant **and** every match-site update together (required to compile), including the substantive gateway approve arm.

**Files:**
- Modify: `src/gateway/security/pairing.rs` (enum, `From`, `code`, `expires_at`, `request_node_pairing`, generalized poll)
- Modify: `src/gateway/handlers/auth/pairing.rs:118` (approve handler `Node` arm)
- Modify: `src/bin/aleph-server/commands/pairing.rs:38,118` (CLI display + CLI approve reject)
- Modify: `src/wizard/flows/pairing.rs:95` (wizard error arm)

- [ ] **Step 1: Add the `Node` variant**

In `src/gateway/security/pairing.rs`, after the `Browser { .. }` variant (closes at line 76), add:

```rust
    /// Cluster-node pairing: a tokenless `aleph-server node` dials the center
    /// and requests a node-role credential the operator approves from the same
    /// notification card as a browser. Mirrors `Browser` but the approval mints
    /// a `DeviceRole::Node` token instead of a chat-tier device token.
    Node {
        request_id: String,
        code: String,
        node_name: String,
        created_at: i64,
        expires_at: i64,
    },
```

- [ ] **Step 2: Update `code()` and `expires_at()` match arms**

In `impl PairingRequest`, add a `Node` arm to each (after the `Browser` arm):

```rust
            PairingRequest::Node { code, .. } => code,
```
```rust
            PairingRequest::Node { expires_at, .. } => *expires_at,
```

- [ ] **Step 3: Write the failing `From` test**

In the `#[cfg(test)] mod tests` of `pairing.rs`, add:

```rust
    #[test]
    fn node_row_maps_to_node_variant() {
        let row = PairingRequestRow {
            request_id: "r1".into(),
            code: "123456".into(),
            pairing_type: "node".into(),
            device_name: Some("worker-1".into()),
            device_type: None,
            public_key: None,
            channel: None,
            sender_id: None,
            remote_addr: None,
            metadata: None,
            origin_label: None,
            user_agent: None,
            peer_ip: None,
            created_at: 0,
            expires_at: 9_999_999_999_999,
        };
        let req = PairingRequest::from(row);
        assert!(matches!(req, PairingRequest::Node { node_name, .. } if node_name == "worker-1"));
    }
```

`PairingRequestRow` is imported as `super::store::PairingRequestRow` (already in scope via `use super::store::{PairingRequestData, PairingRequestRow, SecurityStore};` at the top of `pairing.rs`).

- [ ] **Step 4: Run the test to verify it fails to compile**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0c-pairing
cargo test -p alephcore --lib node_row_maps_to_node_variant 2>&1 | tail -20
```
Expected: compile error — `From<PairingRequestRow>` does not yet handle `"node"` (it falls into the `_ => Channel` arm, so the `matches!` assertion fails).

- [ ] **Step 5: Add the `"node"` arm to `From`**

In `impl From<PairingRequestRow> for PairingRequest`, add before the `_ => PairingRequest::Channel { .. }` catch-all:

```rust
            "node" => PairingRequest::Node {
                request_id: row.request_id,
                code: row.code,
                node_name: row.device_name.unwrap_or_else(|| "aleph-node".into()),
                created_at: row.created_at,
                expires_at: row.expires_at,
            },
```

- [ ] **Step 6: Run the `From` test to verify it passes**

```bash
cargo test -p alephcore --lib node_row_maps_to_node_variant 2>&1 | tail -10
```
Expected: PASS (will still fail to compile until Steps 7–10 fix the other match sites; if so, complete Steps 7–10 first, then re-run).

- [ ] **Step 7: Add `request_node_pairing` and generalize the poll**

In `impl PairingManager` (in `pairing.rs`), after `create_browser_pairing` (ends ~line 406), add:

```rust
    /// Create a cluster-node pairing record and return `(code, expires_at_ms)`.
    /// Mirrors `create_browser_pairing` but tags the row `pairing_type = "node"`
    /// and stores the human node name in the `device_name` column. The code is
    /// the same 6-digit numeric namespace (globally unique across pairing types).
    pub fn request_node_pairing(&self, node_name: &str) -> Result<(String, i64), PairingError> {
        let pending_count = self
            .store
            .count_pairing_requests()
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        if pending_count >= self.max_pending {
            return Err(PairingError::TooManyPending(self.max_pending));
        }

        let request_id = Uuid::new_v4().to_string();
        let code = self.generate_unique_browser_code()?;
        let now = current_timestamp_ms();
        let expires_at = now + self.expiry_ms;

        self.store
            .insert_pairing_request(&PairingRequestData {
                request_id: &request_id,
                code: &code,
                pairing_type: "node",
                device_name: Some(node_name),
                expires_at,
                ..PairingRequestData::default()
            })
            .map_err(|e| PairingError::DatabaseError(e.to_string()))?;

        Ok((code, expires_at))
    }
```

Then generalize the Pending check in `poll_browser_pairing`. Change:

```rust
        match self.store.get_pairing_request(code) {
            Ok(Some(row)) if row.pairing_type == "browser" => PollState::Pending,
            _ => PollState::Expired,
        }
```

to:

```rust
        // Shared by browser and cluster-node pairing: the approved/rejected
        // side-tables are keyed by globally-unique code (type-agnostic), and
        // `pairing.poll` dispatches both through this one method.
        match self.store.get_pairing_request(code) {
            Ok(Some(row)) if matches!(row.pairing_type.as_str(), "browser" | "node") => {
                PollState::Pending
            }
            _ => PollState::Expired,
        }
    }
```

- [ ] **Step 8: Add the gateway approve `Node` arm**

In `src/gateway/handlers/auth/pairing.rs`, inside `handle_pairing_approve`'s `match &pairing_request` block (the one starting at line 52, returning `(device_name, device_type)`), add a `Node` arm before the closing `}` of the match (after the `Browser { .. } => { ... }` arm that ends at line 182):

```rust
        PairingRequest::Node { node_name, code, .. } => {
            // Mint a node-role credential (mirrors cluster.enroll) and stash the
            // combined "token:signature" bearer keyed by the pairing code for the
            // node's `pairing.poll` to drain single-use. Panel-only: the CLI
            // approve path rejects node codes (in-process side-table).
            let device_id = uuid::Uuid::new_v4().to_string();
            let fingerprint: String = device_id.chars().take(16).collect();
            if let Err(e) = ctx.security_store.upsert_device(&DeviceUpsertData {
                device_id: &device_id,
                device_name: node_name,
                device_type: None,
                public_key: &[0u8; 32],
                fingerprint: &fingerprint,
                role: DeviceRole::Node.as_str(),
                scopes: &["node".to_string()],
            }) {
                warn!(error = %e, "Failed to register node device");
                return JsonRpcResponse::error(
                    request.id,
                    -32603,
                    format!("Failed to register node: {}", e),
                );
            }
            let signed = match ctx.token_manager.issue_token(
                &device_id,
                DeviceRole::Node,
                vec!["node".to_string()],
            ) {
                Ok(t) => t,
                Err(e) => {
                    warn!(error = %e, "Failed to issue node token");
                    return JsonRpcResponse::error(
                        request.id,
                        -32603,
                        format!("Failed to issue node token: {}", e),
                    );
                }
            };
            let bearer = format!("{}:{}", signed.token, signed.signature);
            ctx.pairing_manager
                .record_browser_credential(code, &bearer, &device_id);
            info!(code = %code, node = %node_name, "Node pairing approved");
            return JsonRpcResponse::success(
                request.id,
                json!({
                    "code": code,
                    "kind": "node",
                    "device_id": device_id,
                    "approved": true,
                }),
            );
        }
```

`DeviceRole`, `DeviceUpsertData`, `JsonRpcResponse`, `json`, `warn`, `info` are already imported in this file (used by the Device/Browser arms).

- [ ] **Step 9: Add the CLI compile-fix arms**

In `src/bin/aleph-server/commands/pairing.rs`, the list-display match (`match &req` at line 38) — add after the `Browser` arm:

```rust
                PairingRequest::Node {
                    code, node_name, ..
                } => {
                    println!(
                        "{:<10} {:<8} {:<30} {}s",
                        "node", code, node_name, remaining
                    );
                }
```

The CLI approve match (`match &pairing_request` at line 118) — add after the `Browser` arm (which `std::process::exit(1)`s):

```rust
        PairingRequest::Node { code, .. } => {
            eprintln!(
                "Error: Node pairing code '{}' must be approved from the center \
                 Panel notification card, not the CLI.",
                code
            );
            std::process::exit(1);
        }
```

- [ ] **Step 10: Add the wizard compile-fix arm**

In `src/wizard/flows/pairing.rs`, the `match &confirmed` block at line 95 — add after the `Browser` arm:

```rust
            PairingRequest::Node { .. } => {
                return Err(WizardSessionError::FlowError(
                    "PairingFlow expects a device request, got a node request".to_string(),
                ));
            }
```

- [ ] **Step 11: Write the end-to-end approval test**

In the `#[cfg(test)] mod tests` block of `src/gateway/handlers/auth/pairing.rs` (the one using `super::super::tests::create_test_context`), add:

```rust
    #[tokio::test]
    async fn node_pairing_mints_node_role_token_single_use() {
        use crate::gateway::security::PollState;
        let ctx = super::super::tests::create_test_context();

        // Create a node pairing record directly (start_node handler lands in a
        // later task; this exercises approve + poll + token minting).
        let (code, _expires) = ctx
            .pairing_manager
            .request_node_pairing("worker-1")
            .expect("create node pairing");

        let approve = handle_pairing_approve(
            JsonRpcRequest::new(
                "pairing.approve",
                Some(json!({ "code": code.clone() })),
                Some(json!(2)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(approve.is_success(), "approve should succeed: {:?}", approve.error);
        assert_eq!(approve.result.unwrap().get("kind").unwrap(), "node");

        // Poll drains the credential single-use; token validates as Node role.
        match ctx.pairing_manager.poll_browser_pairing(&code) {
            PollState::Approved { token, .. } => {
                let (tok, sig) = token.split_once(':').expect("token:signature");
                let v = ctx.token_manager.validate_token(tok, sig).unwrap();
                assert_eq!(v.role, DeviceRole::Node);
            }
            other => panic!("expected Approved, got {other:?}"),
        }
        assert_eq!(
            ctx.pairing_manager.poll_browser_pairing(&code),
            PollState::Expired,
            "second poll is single-use Expired"
        );
    }
```

- [ ] **Step 12: Run the full center-side test set**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0c-pairing
cargo test -p alephcore --lib node_row_maps_to_node_variant node_pairing_mints_node_role_token_single_use 2>&1 | tail -20
```
Expected: both PASS.

- [ ] **Step 13: Format, lint, commit**

```bash
cargo fmt -p alephcore
cargo clippy -p alephcore --lib 2>&1 | tail -5
git add src/gateway/security/pairing.rs src/gateway/handlers/auth/pairing.rs \
        src/bin/aleph-server/commands/pairing.rs src/wizard/flows/pairing.rs
git commit -m "cluster: PairingRequest::Node variant + node-role approve arm"
```

---

## Task 2: Anonymous `pairing.start_node` RPC + WS wiring

**Files:**
- Modify: `src/gateway/handlers/auth/pairing.rs` (add `handle_pairing_start_node`)
- Modify: `src/gateway/handlers/auth/mod.rs:46-48` (re-export)
- Modify: `src/gateway/server/handler.rs:1537-1539` (unauth allowlist)
- Modify: `src/bin/aleph-server/commands/start/builder/handlers/auth.rs` (register)

- [ ] **Step 1: Write the failing allowlist test**

In `src/gateway/server/handler.rs`, find the existing test (around line 1657) asserting `allow_unauth_browser_pairing("pairing.start_browser")`. Add a new assertion in that same `#[test]` fn:

```rust
        assert!(allow_unauth_browser_pairing("pairing.start_node"));
```

- [ ] **Step 2: Run it to verify failure**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0c-pairing
cargo test -p alephcore --lib allow_unauth_browser_pairing 2>&1 | tail -15
```
Expected: FAIL — `pairing.start_node` is not yet in the `matches!`.

- [ ] **Step 3: Extend the allowlist**

In `src/gateway/server/handler.rs`, change `allow_unauth_browser_pairing` (line 1537-1539):

```rust
fn allow_unauth_browser_pairing(method: &str) -> bool {
    matches!(
        method,
        "pairing.start_browser" | "pairing.start_node" | "pairing.poll"
    )
}
```

Update its doc comment's "two anonymous methods" wording to "anonymous pairing methods (`pairing.start_browser`, `pairing.start_node`, `pairing.poll`)".

- [ ] **Step 4: Run the allowlist test to verify pass**

```bash
cargo test -p alephcore --lib allow_unauth_browser_pairing 2>&1 | tail -10
```
Expected: PASS.

- [ ] **Step 5: Write the failing `start_node` handler test**

In the `#[cfg(test)] mod tests` block of `src/gateway/handlers/auth/pairing.rs`, add:

```rust
    #[tokio::test]
    async fn start_node_returns_six_digit_code() {
        let ctx = super::super::tests::create_test_context();
        let resp = handle_pairing_start_node(
            JsonRpcRequest::new(
                "pairing.start_node",
                Some(json!({ "node_name": "worker-1" })),
                Some(json!(1)),
            ),
            ctx.clone(),
        )
        .await;
        assert!(resp.is_success(), "start_node should succeed: {:?}", resp.error);
        let code = resp.result.unwrap().get("code").unwrap().as_str().unwrap().to_string();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        // The pending row is a node row, pollable as Pending.
        assert_eq!(
            ctx.pairing_manager.poll_browser_pairing(&code),
            crate::gateway::security::PollState::Pending
        );
    }
```

- [ ] **Step 6: Run it to verify it fails to compile**

```bash
cargo test -p alephcore --lib start_node_returns_six_digit_code 2>&1 | tail -15
```
Expected: compile error — `handle_pairing_start_node` undefined.

- [ ] **Step 7: Implement `handle_pairing_start_node`**

In `src/gateway/handlers/auth/pairing.rs`, after `handle_pairing_start_browser` (ends ~line 502, before `handle_pairing_poll`), add:

```rust
/// Handle "pairing.start_node" — anonymous RPC from a tokenless
/// `aleph-server node`. Creates a `node` pairing record and emits
/// `PairingRequested` so the operator's Panel pops the approve card.
/// Reachable without a token (see `allow_unauth_browser_pairing`); the
/// security boundary is the operator's 1-click approve.
pub async fn handle_pairing_start_node(
    request: JsonRpcRequest,
    ctx: Arc<AuthContext>,
) -> JsonRpcResponse {
    #[derive(Debug, Deserialize)]
    struct StartNodeParams {
        node_name: String,
    }

    let params: StartNodeParams = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let (code, expires_at) = match ctx.pairing_manager.request_node_pairing(&params.node_name) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "Failed to create node pairing");
            return JsonRpcResponse::error(
                request.id,
                -32603,
                format!("Failed to create node pairing: {}", e),
            );
        }
    };

    if let Err(e) = ctx
        .event_bus
        .publish_frame(&GatewayEventFrame::PairingRequested {
            device_name: params.node_name.clone(),
        })
    {
        warn!(error = %e, "Failed to publish PairingRequested frame");
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let expires_in_secs = if expires_at > now_ms {
        ((expires_at - now_ms) / 1000) as u64
    } else {
        0
    };

    info!(code = %code, node = %params.node_name, "Node pairing started");
    JsonRpcResponse::success(
        request.id,
        json!({
            "code": code,
            "expires_at": expires_at,
            "expires_in_secs": expires_in_secs,
        }),
    )
}
```

`GatewayEventFrame`, `parse_params`, `warn`, `info`, `Deserialize`, `json` are already imported in this file.

- [ ] **Step 8: Re-export the handler**

In `src/gateway/handlers/auth/mod.rs`, the `pub use pairing::{ ... }` block (lines 46-48) — add `handle_pairing_start_node`:

```rust
pub use pairing::{
    handle_pairing_approve, handle_pairing_list, handle_pairing_poll, handle_pairing_reject,
    handle_pairing_start_browser, handle_pairing_start_node,
};
```

- [ ] **Step 9: Run the handler test to verify pass**

```bash
cargo test -p alephcore --lib start_node_returns_six_digit_code 2>&1 | tail -10
```
Expected: PASS.

- [ ] **Step 10: Register the handler in the WS dispatcher**

In `src/bin/aleph-server/commands/start/builder/handlers/auth.rs`, after the `pairing.start_browser` registration (ends ~line 44), add:

```rust
    register_handler!(
        server,
        "pairing.start_node",
        auth_handlers::handle_pairing_start_node,
        auth_ctx
    );
```

- [ ] **Step 11: Build the server binary to verify wiring**

```bash
cargo build -p alephcore --bin aleph-server 2>&1 | tail -5
```
Expected: `Finished` (the `register_handler!` resolves `auth_handlers::handle_pairing_start_node`).

- [ ] **Step 12: Format, lint, commit**

```bash
cargo fmt -p alephcore
cargo clippy -p alephcore --lib 2>&1 | tail -5
git add src/gateway/handlers/auth/pairing.rs src/gateway/handlers/auth/mod.rs \
        src/gateway/server/handler.rs \
        src/bin/aleph-server/commands/start/builder/handlers/auth.rs
git commit -m "cluster: anonymous pairing.start_node RPC + WS unauth allowlist"
```

---

## Task 3: Node-side interactive pairing + persistence + auth-failed fallback

This task changes `handle_node`'s signature (`token: Option<String>`), so `cli.rs`, `main.rs`, and `node.rs` change together to keep the binary compiling.

**Files:**
- Modify: `src/bin/aleph-server/cli.rs:210-212,769-779` (`token: Option<String>` + parse test)
- Modify: `src/bin/aleph-server/main.rs:250-255` (pass `Option`)
- Modify: `src/bin/aleph-server/commands/node.rs` (credential, helpers, `run_pairing`, three-state `handle_node`, `SessionOutcome`)

- [ ] **Step 1: Make `--token` optional in the CLI**

In `src/bin/aleph-server/cli.rs`, change the `Node.token` field (lines 210-212):

```rust
        /// Node auth token (minted via center `cluster.enroll`). Optional:
        /// omit to interactively pair on first start. A persisted credential
        /// from a prior pairing takes precedence over this flag.
        #[arg(long, value_name = "TOKEN", env = "ALEPH_NODE_TOKEN")]
        token: Option<String>,
```

In the CLI parse test (lines 769-779), update the assertion — `token` is now `Option`:

```rust
            Some(Command::Node {
                center,
                token,
                name,
            }) => {
                assert_eq!(center, "ws://127.0.0.1:18790");
                assert_eq!(token.as_deref(), Some("node-tok"));
                assert_eq!(name, "edge-1");
            }
```

- [ ] **Step 2: Pass the `Option` through `main.rs`**

In `src/bin/aleph-server/main.rs`, the node arm (lines 250-255) already destructures `token` and passes it to `handle_node`. No change needed to the call shape — `handle_node(center, token, name)` now receives `Option<String>`. Verify the arm reads:

```rust
        Some(Command::Node {
            center,
            token,
            name,
        }) => {
            return commands::node::handle_node(center, token, name).await;
        }
```

- [ ] **Step 3: Write the failing pure-helper tests**

In `src/bin/aleph-server/commands/node.rs`, inside the existing `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn node_credential_round_trips_through_disk() {
        let cred = NodeCredential {
            node_id: "n-1".into(),
            bearer: "tok:sig".into(),
            center: "ws://c".into(),
        };
        let path = std::env::temp_dir().join("aleph-node-cred-roundtrip-test.json");
        write_credential(&path, &cred).unwrap();
        let loaded = read_credential(&path).expect("reads back");
        assert_eq!(loaded, cred);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn parse_pairing_outcome_reads_each_status() {
        let approved = json!({"result":{"status":"approved","token":"t:s","device_id":"d"}});
        match parse_pairing_outcome(&approved) {
            PairingOutcome::Approved { bearer, node_id } => {
                assert_eq!(bearer, "t:s");
                assert_eq!(node_id, "d");
            }
            other => panic!("expected Approved, got {other:?}"),
        }
        assert!(matches!(
            parse_pairing_outcome(&json!({"result":{"status":"rejected"}})),
            PairingOutcome::Rejected
        ));
        assert!(matches!(
            parse_pairing_outcome(&json!({"result":{"status":"expired"}})),
            PairingOutcome::Expired
        ));
        assert!(matches!(
            parse_pairing_outcome(&json!({"result":{"status":"pending"}})),
            PairingOutcome::Pending
        ));
    }

    #[test]
    fn connect_rejected_auth_detects_auth_failed() {
        assert!(connect_rejected_auth(
            &json!({"error":{"code":-32001,"message":"x"}})
        ));
        assert!(!connect_rejected_auth(&json!({"result":{"ok":true}})));
        assert!(!connect_rejected_auth(
            &json!({"error":{"code":-32000,"message":"transient"}})
        ));
    }
```

Add `#[derive(Debug)]` to `PairingOutcome` (the test's `{other:?}` needs it).

- [ ] **Step 4: Run the tests to verify they fail to compile**

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0c-pairing
cargo test -p alephcore --bin aleph-server node_credential_round_trips_through_disk 2>&1 | tail -15
```
Expected: compile error — `NodeCredential`, `write_credential`, `parse_pairing_outcome`, etc. undefined.

- [ ] **Step 5: Add the credential type, persistence, and pure helpers**

In `src/bin/aleph-server/commands/node.rs`, update the imports at the top and add the new items. Replace the `use` block and constants (lines 6-23) with:

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use alephcore::cluster::{CommandDescriptor, CommandTable};
use alephcore::gateway::protocol::JsonRpcResponse;
use alephcore::routing::session_key::SessionKey;
use alephcore::sandbox::config::SandboxConfig;
use alephcore::sandbox::exec_approval::gate::ApprovalGate;
use alephcore::sandbox::exec_approval::types::ApprovalConfig;
use alephcore::sandbox::factory::build_sandbox;
use alephcore::sandbox::platforms::create_platform_driver_from_config;
use alephcore::sandbox::rate_limit::SandboxRateLimitConfig;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

const BACKOFF_INITIAL_MS: u64 = 2_000;
const BACKOFF_MAX_MS: u64 = 60_000;
const POLL_INTERVAL_MS: u64 = 2_000;
const AUTH_FAILED_CODE: i64 = -32001;
```

After the constants, add:

```rust
/// Persisted node credential. `bearer` is the combined "{token}:{signature}"
/// string the center's `pairing.poll` hands out — sent verbatim as the
/// `connect` `token` param.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct NodeCredential {
    node_id: String,
    bearer: String,
    center: String,
}

/// `~/.aleph/node/<name>.json`. `None` only if the home dir is unresolvable.
fn credential_path(name: &str) -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".aleph").join("node").join(format!("{name}.json")))
}

fn read_credential(path: &Path) -> Option<NodeCredential> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_credential(path: &Path, cred: &NodeCredential) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(cred).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

/// Outcome of parsing a `pairing.poll` reply. Pure.
#[derive(Debug)]
enum PairingOutcome {
    Pending,
    Approved { bearer: String, node_id: String },
    Rejected,
    Expired,
}

fn parse_pairing_outcome(resp: &Value) -> PairingOutcome {
    match resp.pointer("/result/status").and_then(|s| s.as_str()) {
        Some("approved") => PairingOutcome::Approved {
            bearer: resp
                .pointer("/result/token")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string(),
            node_id: resp
                .pointer("/result/device_id")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("rejected") => PairingOutcome::Rejected,
        Some("expired") => PairingOutcome::Expired,
        _ => PairingOutcome::Pending,
    }
}

/// Result of one `run_session` attempt.
enum SessionOutcome {
    /// Connected and the inbound loop ended (clean reconnect).
    Ended,
    /// Center rejected `connect` with AUTH_FAILED — credential is stale.
    AuthFailed,
}

/// True when a `connect` reply is an AUTH_FAILED (-32001) error. Pure.
fn connect_rejected_auth(connect_resp: &Value) -> bool {
    connect_resp
        .pointer("/error/code")
        .and_then(|c| c.as_i64())
        == Some(AUTH_FAILED_CODE)
}
```

- [ ] **Step 6: Run the pure-helper tests to verify pass**

```bash
cargo test -p alephcore --bin aleph-server \
  node_credential_round_trips_through_disk parse_pairing_outcome_reads_each_status \
  connect_rejected_auth_detects_auth_failed 2>&1 | tail -15
```
Expected: 3 PASS. (`dirs` is already a dependency used by the library.)

- [ ] **Step 7: Add `run_pairing`**

In `src/bin/aleph-server/commands/node.rs`, after `build_command_table` (ends ~line 67), add:

```rust
/// Interactive pairing: anonymous WS → `pairing.start_node` → print the code →
/// poll `pairing.poll` every 2s until the operator approves from the Panel.
/// Returns the minted node credential. No retry/backoff here — the caller's
/// reconnect loop owns that; pairing is linear and readable.
async fn run_pairing(
    url: &str,
    center: &str,
    name: &str,
    declared: &[CommandDescriptor],
) -> Result<NodeCredential, Box<dyn std::error::Error>> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;

    let start = json!({
        "jsonrpc": "2.0", "id": 1, "method": "pairing.start_node",
        "params": { "node_name": name, "commands": declared }
    });
    ws.send(Message::Text(start.to_string().into())).await?;
    let start_reply = ws
        .next()
        .await
        .ok_or("center closed before start_node reply")??;
    let Message::Text(start_text) = start_reply else {
        return Err("unexpected non-text start_node reply".into());
    };
    let start_val: Value = serde_json::from_str(start_text.as_str())?;
    let code = start_val
        .pointer("/result/code")
        .and_then(|c| c.as_str())
        .ok_or("center did not return a pairing code")?
        .to_string();

    println!("\n  Aleph 节点配对码: {code}");
    println!("  请在中心 Panel 通知卡批准此节点\n");

    let mut poll_id = 2;
    loop {
        let poll = json!({
            "jsonrpc": "2.0", "id": poll_id, "method": "pairing.poll",
            "params": { "code": code }
        });
        ws.send(Message::Text(poll.to_string().into())).await?;
        let poll_reply = ws.next().await.ok_or("center closed during poll")??;
        if let Message::Text(poll_text) = poll_reply {
            let poll_val: Value = serde_json::from_str(poll_text.as_str())?;
            match parse_pairing_outcome(&poll_val) {
                PairingOutcome::Approved { bearer, node_id } => {
                    tracing::info!("node '{name}' pairing approved");
                    return Ok(NodeCredential {
                        node_id,
                        bearer,
                        center: center.to_string(),
                    });
                }
                PairingOutcome::Rejected => return Err("pairing rejected by operator".into()),
                PairingOutcome::Expired => return Err("pairing code expired".into()),
                PairingOutcome::Pending => {}
            }
        }
        poll_id += 1;
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }
}
```

- [ ] **Step 8: Rewrite `handle_node` (three-state) and `run_session` (`SessionOutcome`)**

Replace `handle_node` (lines 25-46) with:

```rust
pub async fn handle_node(
    center: String,
    token: Option<String>,
    name: String,
) -> Result<(), Box<dyn std::error::Error>> {
    let table = Arc::new(build_command_table(&name));
    let declared = table.descriptors();
    let url = format!("{}/ws", center.trim_end_matches('/'));
    let cred_path = credential_path(&name);

    // Resolve the bearer: persisted credential > --token > interactive pairing.
    let mut bearer = match cred_path.as_deref().and_then(read_credential) {
        Some(cred) => {
            tracing::info!("node '{name}' using persisted credential");
            cred.bearer
        }
        None => match token {
            Some(t) => t,
            None => {
                let cred = run_pairing(&url, &center, &name, &declared).await?;
                persist_credential(cred_path.as_deref(), &cred);
                cred.bearer
            }
        },
    };

    let mut backoff = BACKOFF_INITIAL_MS;
    loop {
        match run_session(&url, &bearer, &name, &declared, &table).await {
            Ok(SessionOutcome::Ended) => {
                tracing::warn!("node session ended cleanly; reconnecting");
                backoff = BACKOFF_INITIAL_MS;
            }
            Ok(SessionOutcome::AuthFailed) => {
                tracing::warn!("node credential rejected by center; clearing and re-pairing");
                if let Some(p) = cred_path.as_deref() {
                    let _ = std::fs::remove_file(p);
                }
                let cred = run_pairing(&url, &center, &name, &declared).await?;
                persist_credential(cred_path.as_deref(), &cred);
                bearer = cred.bearer;
                backoff = BACKOFF_INITIAL_MS;
                continue;
            }
            Err(e) => tracing::error!("node session error: {e}; retrying in {backoff}ms"),
        }
        tokio::time::sleep(Duration::from_millis(backoff)).await;
        backoff = (backoff * 2).min(BACKOFF_MAX_MS);
    }
}

/// Best-effort persist; a write failure warns but does not abort the node
/// (it will re-pair on next restart).
fn persist_credential(path: Option<&Path>, cred: &NodeCredential) {
    if let Some(p) = path {
        if let Err(e) = write_credential(p, cred) {
            tracing::warn!("failed to persist node credential: {e}");
        }
    }
}
```

Replace `run_session` (lines 69-92) — change the return type and inspect the connect reply:

```rust
async fn run_session(
    url: &str,
    token: &str,
    name: &str,
    declared: &[CommandDescriptor],
    table: &CommandTable,
) -> Result<SessionOutcome, Box<dyn std::error::Error>> {
    let (mut ws, _resp) = tokio_tungstenite::connect_async(url).await?;
    let connect = json!({
        "jsonrpc": "2.0", "id": 1, "method": "connect",
        "params": { "token": token, "device_name": name, "commands": declared }
    });
    ws.send(Message::Text(connect.to_string().into())).await?;
    let reply = ws
        .next()
        .await
        .ok_or("center closed before connect reply")??;
    if let Message::Text(text) = &reply {
        if let Ok(v) = serde_json::from_str::<Value>(text.as_str()) {
            if connect_rejected_auth(&v) {
                tracing::warn!("node '{name}' rejected by center (auth failed)");
                return Ok(SessionOutcome::AuthFailed);
            }
        }
    }
    tracing::info!("node '{name}' connected to center");

    while let Some(msg) = ws.next().await {
        let Message::Text(text) = msg? else { continue };
        if let Some(reply) = handle_frame(table, text.as_str()).await {
            ws.send(Message::Text(reply.into())).await?;
        }
    }
    Ok(SessionOutcome::Ended)
}
```

- [ ] **Step 9: Build the binary and run all node tests**

```bash
cargo build -p alephcore --bin aleph-server 2>&1 | tail -5
cargo test -p alephcore --bin aleph-server 2>&1 | tail -20
```
Expected: build `Finished`; node tests (the 3 new + the 3 existing `handle_frame_*`) all PASS.

- [ ] **Step 10: Format, lint, commit**

```bash
cargo fmt -p alephcore
cargo clippy -p alephcore --bin aleph-server 2>&1 | tail -5
git add src/bin/aleph-server/cli.rs src/bin/aleph-server/main.rs \
        src/bin/aleph-server/commands/node.rs
git commit -m "cluster: node-side interactive pairing + credential persistence + auth-failed re-pair"
```

---

## Final verification (controller, after all tasks)

```bash
cd /Volumes/TBU4/Workspace/Aleph-wt-cluster-phase0c-pairing
cargo test -p alephcore --lib pairing 2>&1 | tail -15
cargo test -p alephcore --bin aleph-server 2>&1 | tail -15
cargo clippy -p alephcore 2>&1 | tail -8
cargo build -p alephcore --bin aleph-server 2>&1 | tail -3
```

Expected: all pairing lib tests pass, node bin tests pass, no new clippy warnings, binary builds.

---

## Testing strategy note

There is **no `tests/` WS integration test** for `pairing.start_node`. Pairing RPCs are dispatched through the **bin-registered** handler map (`register_auth_handlers`), which `GatewayServer::with_config` (the lib integration harness) does not populate — only `connect` is special-cased in the lib WS loop. The center-side flow is therefore covered end-to-end at the **handler level** (Task 1 Step 11: `request_node_pairing` → gateway `pairing.approve` → `poll` → Node-role token validation, single-use), and the WS gate bypass is unit-tested (Task 2 Step 1). Node-side I/O orchestration (`run_pairing`/`run_session`) is verified via its pure helpers (`parse_pairing_outcome`, `connect_rejected_auth`, credential round-trip) plus a clean binary build — mirroring 0c-core, where `run_session` itself is not unit-tested.
