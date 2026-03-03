# Gateway Protocol

The Aleph Gateway uses JSON-RPC 2.0 over WebSocket for all communication.

## Connection

```
ws://127.0.0.1:18790/ws
```

When `require_auth: true`, the first message must be a `connect` request:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "connect",
  "params": {
    "minProtocol": 1,
    "maxProtocol": 1,
    "client": {
      "id": "cli",
      "version": "0.1.0",
      "platform": "macos"
    },
    "role": "operator",
    "auth": {
      "token": "bearer_token"
    }
  }
}
```

## Message Types

### Request (Client → Gateway)

```json
{
  "jsonrpc": "2.0",
  "id": "unique-id",
  "method": "method.name",
  "params": { ... }
}
```

### Response (Gateway → Client)

Success:
```json
{
  "jsonrpc": "2.0",
  "id": "unique-id",
  "result": { ... }
}
```

Error:
```json
{
  "jsonrpc": "2.0",
  "id": "unique-id",
  "error": {
    "code": -32600,
    "message": "Invalid Request"
  }
}
```

### Event (Gateway → Client)

```json
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "topic": "stream.token",
    "data": { ... }
  }
}
```

## Method Categories

| Prefix | Description |
|--------|-------------|
| `agent.*` | Agent execution and control |
| `session.*` | Session management |
| `config.*` | Configuration operations |
| `exec.*` | Execution approval |
| `wizard.*` | Configuration wizard |
| `cron.*` | Scheduled jobs |
| `events.*` | Event subscription |
| `browser.*` | CDP browser control |

## Streaming

For methods that support streaming (like `agent.run`), events are sent as separate messages:

```json
// Start of stream
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "topic": "stream.start",
    "data": { "run_id": "..." }
  }
}

// Token stream
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "topic": "stream.token",
    "data": { "text": "Hello" }
  }
}

// End of stream
{
  "jsonrpc": "2.0",
  "method": "event",
  "params": {
    "topic": "stream.end",
    "data": { "run_id": "..." }
  }
}
```

## Error Codes

| Code | Meaning |
|------|---------|
| -32700 | Parse error |
| -32600 | Invalid Request |
| -32601 | Method not found |
| -32602 | Invalid params |
| -32603 | Internal error |
| -32000 | Auth required |
| -32001 | Auth failed |
| -32002 | Permission denied |

## Event Subscription

Subscribe to events:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "events.subscribe",
  "params": {
    "patterns": ["stream.*", "agent.*"]
  }
}
```

Unsubscribe:

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "events.unsubscribe",
  "params": {
    "patterns": ["stream.*"]
  }
}
```
