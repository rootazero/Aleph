# Sandbox Subsystem

OS-native sandboxing for AI-generated skills with fine-grained permission control.

## Architecture

```
SandboxManager
    ↓
SandboxAdapter (trait)
    ↓
Platform Implementation (macOS, Linux, Windows)
    ↓
OS-native sandbox (sandbox-exec, seccomp, AppContainer)
```

## Usage

```rust
use alephcore::exec::sandbox::*;
use std::sync::Arc;

// Create platform-specific adapter
#[cfg(target_os = "macos")]
let adapter = Arc::new(platforms::MacOSSandbox::new());

// Create manager
let manager = SandboxManager::new(adapter);

// Define capabilities
let capabilities = Capabilities {
    filesystem: vec![FileSystemCapability::TempWorkspace],
    network: NetworkCapability::Deny,
    process: ProcessCapability {
        no_fork: true,
        max_execution_time: 300,
        max_memory_mb: Some(512),
    },
    environment: EnvironmentCapability::Restricted,
};

// Execute command
let command = SandboxCommand {
    program: "python3".to_string(),
    args: vec!["script.py".to_string()],
    working_dir: None,
};

let (result, audit_log) = manager
    .execute_sandboxed("skill-id", command, capabilities)
    .await?;
```

## Capabilities

### FileSystemCapability

- `ReadOnly { path }`: Read-only access to specific path
- `ReadWrite { path }`: Read-write access to specific path
- `TempWorkspace`: Temporary directory (auto-created and cleaned)

### NetworkCapability

- `Deny`: No network access
- `AllowDomains(vec)`: Whitelist specific domains
- `AllowAll`: Full network access

### ProcessCapability

- `no_fork`: Prohibit child processes
- `max_execution_time`: Timeout in seconds
- `max_memory_mb`: Memory limit

### EnvironmentCapability

- `None`: No environment variables
- `Restricted`: Safe variables only (PATH, HOME)
- `Full`: All environment variables

## Platform Support

| Platform | Implementation | Status |
|----------|----------------|--------|
| macOS | sandbox-exec | ✅ Implemented |
| Linux | seccomp | 🚧 Planned |
| Windows | AppContainer | 🚧 Planned |

## Security

- **Default Deny**: All permissions denied by default
- **Fail-Safe**: Sandbox failure → execution denied
- **Audit Logging**: All executions logged
- **Resource Limits**: CPU time and memory bounded

## Testing

```bash
# Run all sandbox tests
cargo test sandbox --lib

# Run platform-specific tests
cargo test sandbox::platforms::macos --lib

# Run integration tests
cargo test sandbox::tests::integration_tests --lib
```
