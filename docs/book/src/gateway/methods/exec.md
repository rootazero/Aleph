# exec.* Methods

Command execution approval system with three-tier security.

## Security Levels

| Level | Description |
|-------|-------------|
| `deny` | Block all commands |
| `allowlist` | Only approved patterns |
| `full` | Allow everything |

## Methods

### exec.approval.request

Request approval for a command execution.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "exec.approval.request",
  "params": {
    "command": "rm -rf /tmp/cache",
    "cwd": "/home/user",
    "agent_id": "main",
    "session_key": "agent:main:main",
    "timeout_ms": 120000
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "id": "approval-uuid",
    "approved": true,
    "decision": "allow_once",
    "timeout": false
  }
}
```

### exec.approval.resolve

Resolve a pending approval request.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "exec.approval.resolve",
  "params": {
    "id": "approval-uuid",
    "decision": "allow_once",
    "resolved_by": "user@terminal"
  }
}
```

**Decision Types:**

| Decision | Description |
|----------|-------------|
| `allow_once` | Allow this execution only |
| `allow_session` | Allow for current session |
| `allowlist` | Add to permanent allowlist |
| `deny` | Deny execution |

### exec.approvals.get

Get the current approval configuration.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "exec.approvals.get"
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "result": {
    "config": {
      "version": 1,
      "security": "allowlist",
      "allowlist": [
        "git *",
        "npm install",
        "cargo build"
      ],
      "denylist": [
        "rm -rf /"
      ]
    },
    "hash": "sha256:abc123..."
  }
}
```

### exec.approvals.set

Update approval configuration with optimistic locking.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "exec.approvals.set",
  "params": {
    "config": {
      "version": 1,
      "security": "allowlist",
      "allowlist": [
        "git *",
        "npm *",
        "cargo *"
      ]
    },
    "base_hash": "sha256:abc123..."
  }
}
```

### exec.approvals.pending

List pending approval requests.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "exec.approvals.pending"
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "result": {
    "pending": [
      {
        "id": "uuid-1",
        "command": "sudo apt update",
        "cwd": "/home/user",
        "agent_id": "main",
        "session_key": "agent:main:main",
        "created_at": "2024-01-15T10:30:00Z"
      }
    ]
  }
}
```

## Allowlist Patterns

Patterns support glob-style matching:

| Pattern | Matches |
|---------|---------|
| `git *` | Any git command |
| `npm install` | Exact match |
| `cargo build --*` | cargo build with any flags |
| `ls -la /tmp/*` | ls in /tmp subdirectories |

## IPC Integration

For CLI integration, Aleph supports Unix socket IPC:

```
/tmp/aether-exec-{user}.sock
```

The IPC protocol uses HMAC-SHA256 for authentication:

1. Client sends challenge request
2. Server responds with nonce
3. Client signs with shared secret
4. Server verifies and proceeds
