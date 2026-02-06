# Client Architecture Refactoring - Progress Report

**Date**: 2026-02-06
**Worktree**: `.worktrees/client-refactoring`
**Branch**: `client-architecture-refactoring`

## ✅ Completed: Phase 1 - Directory Reorganization

**Status**: Complete (Commit: `35c26c2c`)

### Achievements

- ✅ Moved `platforms/macos` → `clients/macos`
- ✅ Moved `platforms/tauri` → `clients/desktop`
- ✅ Updated workspace `Cargo.toml`
- ✅ Fixed all build script paths
- ✅ Updated documentation (CLAUDE.md)

### Verification Results

| Client | Compilation | Notes |
|--------|------------|-------|
| CLI | ✅ Success | 6 warnings (acceptable) |
| macOS | ✅ N/A | Swift project, no Rust compilation |
| Desktop (Tauri) | ⚠️ Fails | Pre-existing issue (same errors on main branch) |

**Conclusion**: Phase 1 完成，无功能回退。Tauri 编译问题是原有的，不是重构引入的。

---

## 🚧 In Progress: Phase 2 - Build Aleph Client SDK

**Status**: SDK skeleton created (Commit: `4d5e5dc0`)

### Completed Tasks

#### Task 2.1: Create `clients/shared` crate ✅

**Created Files**:
```
clients/shared/
├── Cargo.toml          # Feature flags configured
└── src/
    ├── lib.rs          # Public API
    ├── error.rs        # ClientError types
    ├── transport.rs    # WebSocket layer (stub)
    ├── rpc.rs          # JSON-RPC client (stub)
    ├── auth.rs         # ConfigStore trait
    ├── client.rs       # GatewayClient (stub)
    └── executor.rs     # LocalExecutor trait
```

**Feature Flags**:
- `transport`: WebSocket connection management
- `rpc`: JSON-RPC protocol handling
- `client`: Complete client instance (default)
- `local-executor`: Local tool execution
- `native-tls`: Native TLS (default)
- `rustls`: Pure Rust TLS
- `tracing`: Optional logging

**Status**: ✅ SDK compiles successfully with all features

### Next Tasks (Task 2.2-2.8)

#### Task 2.2: Extract WebSocket logic from CLI → `transport.rs`

**Source**: `clients/cli/src/client.rs:66-135`

**Key Components to Extract**:
- WebSocket connection handling (`connect_async`)
- Read/Write split pattern
- Connection state management
- Auto-reconnection logic (TODO)
- Heartbeat/Ping-Pong handling

#### Task 2.3: Extract JSON-RPC logic → `rpc.rs`

**Source**: `clients/cli/src/client.rs:29-32, 137-213`

**Key Components**:
- `PendingRequest` tracking (HashMap)
- Request ID generation
- Request/Response matching
- Timeout handling
- Message parsing (JsonRpcRequest/JsonRpcResponse)

#### Task 2.4: Implement Managed Auth → `auth.rs`

**Source**: `clients/cli/src/config.rs`, `clients/cli/src/client.rs:384-433`

**Key Components**:
- `ConfigStore` trait implementation example (file-based)
- Authentication flow (`connect` RPC)
- Token storage/retrieval
- Pairing flow (optional, for future)

#### Task 2.5: Assemble `GatewayClient` → `client.rs`

**Integration**: Combine transport + RPC + auth

**Public API**:
```rust
impl GatewayClient {
    pub fn new(url: &str) -> Self;
    pub async fn connect(&self) -> Result<EventStream>;
    pub async fn authenticate(&self, config: &impl ConfigStore) -> Result<AuthToken>;
    pub async fn call(&self, method: &str, params: Option<Value>) -> Result<Value>;
    pub fn is_connected(&self) -> bool;
    pub async fn close(&self) -> Result<()>;
}
```

#### Task 2.6: Define `LocalExecutor` trait → `executor.rs`

**Source**: `clients/cli/src/executor.rs`

**Current CLI Implementation**:
- `shell:exec` / `shell_exec` / `exec` tool
- Command execution with tokio::process

#### Task 2.7: Refactor CLI to use SDK

**Changes Required**:
1. Update `clients/cli/Cargo.toml` to depend on `aleph-client-sdk`
2. Replace `clients/cli/src/client.rs` with SDK usage
3. Implement `ConfigStore` for CLI (file-based)
4. Keep CLI-specific UI code (ratatui, commands, etc.)

#### Task 2.8: Integration Testing

**Test Scenarios**:
1. Gateway connection and authentication
2. RPC call/response
3. Tool execution (reverse RPC)
4. Stream events handling
5. Reconnection on disconnect
6. Long-running connection stability

---

## 📊 Current Directory Structure

```
aleph/
├── clients/
│   ├── cli/              # ✅ Compiles, needs SDK refactor
│   ├── macos/            # ✅ Functional (Swift, Thin Client)
│   ├── desktop/          # ⚠️ Has compilation issues
│   └── shared/           # 🚧 SDK skeleton created
├── core/                 # ✅ Server-side logic
└── shared/protocol/      # ✅ Protocol definitions
```

---

## 🎯 Next Session Plan

1. **Continue Phase 2**:
   - Extract WebSocket logic from CLI → SDK transport.rs
   - Extract RPC logic → SDK rpc.rs
   - Implement authentication → SDK auth.rs
   - Refactor CLI to use SDK
   - Run integration tests

2. **Phase 3 (After Phase 2)**:
   - Apply SDK to Tauri Desktop client
   - Remove alephcore dependency from Tauri
   - Implement Command Proxy pattern
   - Verify frontend transparency

---

## 🔗 References

- Design Document: `docs/plans/2026-02-06-client-architecture-refactoring.md`
- Server-Client Architecture: `docs/plans/2026-02-06-server-client-architecture-design.md`
- Protocol Definition: `shared/protocol/`

---

## 💡 Notes

1. **Tauri Pre-existing Issue**: Desktop client has FFI-related compilation errors that exist on main branch. These will be addressed in Phase 3.

2. **SDK Design Decision**: Using feature flags for modularity. CLI uses full features, Tauri can be more selective.

3. **Authentication Strategy**: ConfigStore trait allows platform-specific storage (file for CLI, Tauri store for desktop, Keychain for macOS).

4. **Token Conservation**: Due to token limits, detailed extraction will continue in next session.
