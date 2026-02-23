# IronClaw Phase 2 & Phase 3 Design

> Agent Secret Management: Host-Boundary Secret Controls + EVM Signing
> Codename: IronClaw P1/P2 (internal Phase 2 + Phase 3)

## Background

Phase 1 (P0) established the encrypted SecretVault foundation:
- AES-256-GCM vault at `~/.aleph/secrets.vault`
- `secret_name` field in ProviderConfig and GenerationProviderConfig
- CLI commands (init/set/list/delete/verify)
- Config migration from plaintext api_key
- Gateway handler support for both provider types

**Remaining Phase 1 gaps:**
- Server startup doesn't call vault migration or `resolve_provider_secrets()`
- CLI commands need validation helpers
- Generation provider validation needs secret_name parity

**Phase 2 (P1)** adds the IronClaw host-boundary pipeline: placeholder injection, runtime-only secret resolution, and bidirectional leak detection.

**Phase 3 (P2)** adds approval workflows for sensitive secret usage and EVM-compatible Web3 signing that never exposes private keys to the Agent/LLM.

## Scope

### Phase 2: Host-Boundary Secret Controls

- `{{secret:NAME}}` placeholder parsing and validation
- Host-side injection pipeline (resolve at send time, never persist)
- Bidirectional leak detection (outbound request scan + inbound response scan)
- Integration with existing SecretMasker and PiiEngine

### Phase 3: Approval Workflow + EVM Signing

- `secret.use` permission namespace with approval workflow
- RPC handlers for approval request/resolve/pending
- EVM-compatible signing (secp256k1): PersonalSign, EIP-712, Transaction
- Signed results never contain private key material

## Architecture

### Phase 2: Injection + Detection Pipeline

```
Tool/Provider Request
    |
    v
extract_secret_refs(input)          # placeholder.rs (existing)
    |  finds {{secret:NAME}} patterns
    v
render_with_secrets(input, resolver) # injection.rs (new)
    |  vault.get(NAME) -> replace placeholder
    |  record injected values for leak scan
    v
leak_detector.scan_outbound(request) # leak_detector.rs (new)
    |  pattern rules + exact value detection
    |
    v  [if Allow]
Send HTTP Request
    |
    v
Receive Response
    |
    v
leak_detector.scan_inbound(response) # leak_detector.rs
    |  detect echoed secrets
    |
    v  [if Allow]
Return response to Agent
```

### Phase 3: Approval + Signing Flow

```
Agent requests secret usage
    |
    v
PermissionManager.evaluate("secret.use", context)
    |
    +-- Allow    -> inject secret / sign directly
    +-- Ask      -> send approval request to Client
    |               |
    |               +-- User approves -> proceed
    |               +-- User denies   -> return error
    +-- Deny     -> reject immediately
```

```
Agent requests EVM signing
    |
    v
Approval workflow (above)
    |  [approved]
    v
EvmSigner.sign(secret_name, intent)
    |  1. vault.get(secret_name) -> DecryptedSecret
    |  2. parse private key bytes
    |  3. sign with secp256k1
    |  4. zeroize private key immediately
    v
SignedResult { signature, signer_address }
    |  NO private key in result
    v
Return to Agent
```

## Core Types

### Phase 2

```rust
// core/src/secrets/placeholder.rs (existing)
pub struct SecretRef { pub name: String, pub raw: String }
pub fn extract_secret_refs(input: &str) -> Result<Vec<SecretRef>, SecretError>;

// core/src/secrets/injection.rs (new)
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, name: &str) -> Result<DecryptedSecret, SecretError>;
}

pub fn render_with_secrets(
    input: &str,
    resolver: &dyn SecretResolver,
) -> Result<(String, Vec<InjectedSecret>), SecretError>;

pub struct InjectedSecret {
    pub name: String,
    pub value_hash: u64,  // SipHash for fast comparison
    pub value_len: usize,
}

// core/src/secrets/leak_detector.rs (new)
pub struct LeakDetector {
    pattern_rules: Vec<Regex>,           // known secret formats
    injected_hashes: HashSet<u64>,       // SipHash of injected values
    injected_substrings: Vec<String>,    // first 8 chars for prefix scan
}

pub enum LeakDecision {
    Allow,
    Block { reason: String, redacted_content: String },
}

impl LeakDetector {
    pub fn scan_outbound(&self, content: &str) -> LeakDecision;
    pub fn scan_inbound(&self, content: &str) -> LeakDecision;
    pub fn register_injected(&mut self, secrets: &[InjectedSecret]);
}
```

### Phase 3

```rust
// core/src/secrets/web3_signer.rs (new)
pub struct EvmSigner<'a> {
    vault: &'a SecretVault,
}

pub enum SignIntent {
    /// EIP-191 personal_sign
    PersonalSign { message: Vec<u8> },
    /// EIP-712 typed data signing
    TypedData {
        domain_hash: [u8; 32],
        struct_hash: [u8; 32],
    },
    /// Raw transaction signing
    Transaction {
        chain_id: u64,
        to: [u8; 20],
        value: [u8; 32],  // U256 as bytes
        data: Vec<u8>,
        nonce: u64,
        gas_limit: u64,
        max_fee_per_gas: u64,
        max_priority_fee_per_gas: u64,
    },
}

pub struct SignedResult {
    pub signature: Vec<u8>,         // 65 bytes: r(32) + s(32) + v(1)
    pub signer_address: [u8; 20],   // derived from public key
}

impl fmt::Debug for SignedResult {
    // Shows signature hex and address, never private key
}

impl<'a> EvmSigner<'a> {
    pub fn sign(&self, secret_name: &str, intent: &SignIntent) -> Result<SignedResult, SecretError>;
    pub fn get_address(&self, secret_name: &str) -> Result<[u8; 20], SecretError>;
}

// core/src/gateway/handlers/secret_approvals.rs (new)
// RPC methods:
//   secret.approval.request  - Agent requests secret usage
//   secret.approval.resolve  - Client approves/denies
//   secret.approvals.pending - List pending approvals
```

## New Dependencies

```toml
# Phase 3: EVM signing
k256 = { version = "0.13", features = ["ecdsa", "keccak256"] }
```

## Integration Points

### HttpProvider (Phase 2)

Modify `execute` and `execute_stream` in `core/src/providers/http_provider.rs`:
1. Before request build: `render_with_secrets()` on tool parameters
2. Before request send: `leak_detector.scan_outbound()`
3. After response receive: `leak_detector.scan_inbound()`

### Permission Manager (Phase 3)

Add `secret.use` namespace to `core/src/permission/rule.rs`:
- Default: `Allow` for provider API keys (backward compatible)
- Default: `Ask` for signing operations
- Configurable per secret via metadata

### Gateway Handlers (Phase 3)

New handler file `secret_approvals.rs` following the pattern of `exec_approvals.rs`:
- Reuse existing approval storage and timeout infrastructure
- Add event bus notifications for approval state changes

## Module Organization

```
core/src/secrets/
├── mod.rs              # existing - add new pub mods
├── crypto.rs           # existing - AES-256-GCM + HKDF
├── types.rs            # existing - DecryptedSecret, errors
├── vault.rs            # existing - SecretVault CRUD
├── migration.rs        # existing - config migration
├── placeholder.rs      # existing - {{secret:NAME}} parser
├── injection.rs        # NEW - render_with_secrets()
├── leak_detector.rs    # NEW - bidirectional leak scan
└── web3_signer.rs      # NEW - EVM signing (secp256k1)
```

## Testing Strategy

| Type | Coverage |
|------|----------|
| Unit | Placeholder parsing, injection rendering, leak detection patterns |
| Unit | EVM signing roundtrip, address derivation, signature verification |
| Integration | Full pipeline: placeholder → inject → send → scan response |
| Integration | Approval: request → pending → resolve → proceed/deny |
| Security | Injected secret echoed in response → blocked |
| Security | Wrong master key → signing fails |
| Security | SignedResult never contains private key bytes |

## Rollout Strategy

1. **Task 1-2** (Phase 1 closure): Deploy behind default-safe behavior
2. **Task 3-5** (Phase 2 boundary): Logging-first mode for 48h, then enforce block
3. **Task 6-7** (Phase 3 approval/signing): Opt-in mode per workspace profile
4. **Task 8** (E2E verification): All gates pass before full enforcement

## Exit Criteria

- No plaintext provider secret persisted to `~/.aleph/config.toml`
- `secret_name` supported for both chat and generation providers
- `{{secret:NAME}}` resolves only at host boundary
- Outbound and inbound leak detection blocks known/observed leakage
- High-risk secret usage and signing require explicit approval
- Web3 signing path never returns private key material to Agent/LLM
- All integration tests pass
