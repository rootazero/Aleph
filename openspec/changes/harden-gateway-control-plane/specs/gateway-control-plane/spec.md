# gateway-control-plane Specification

## ADDED Requirements

### Requirement: Authenticated Connection Handshake
When `require_auth` is enabled, the Gateway SHALL require a `connect` handshake before processing other RPC methods.

#### Scenario: First request is not connect
- **WHEN** a client sends any RPC method other than `connect` as its first request
- **AND** `require_auth` is enabled
- **THEN** the Gateway returns `AUTH_REQUIRED`
- **AND** the connection is closed after the response

#### Scenario: Successful connect
- **WHEN** the client sends a valid `connect` request
- **THEN** the Gateway marks the connection as authenticated
- **AND** subsequent RPC requests are accepted
- **AND** the response includes issued permissions and expiry information

### Requirement: Connection‑Level Authorization Gate
The Gateway SHALL reject unauthenticated requests with a consistent error when authorization is required.

#### Scenario: Unauthorized request
- **WHEN** an unauthenticated connection calls any method other than `connect`
- **THEN** the Gateway returns `AUTH_REQUIRED`
- **AND** the request is not dispatched to handlers

### Requirement: Registered Auth and Pairing RPC Methods
The Gateway SHALL register RPC methods for authentication and device/channel pairing management.

#### Scenario: Auth RPC availability
- **WHEN** a client requests `connect`
- **THEN** the Gateway handles the request and returns either a token or a pairing requirement

#### Scenario: Pairing and device management RPCs
- **WHEN** a client requests `pairing.list`, `pairing.approve`, or `pairing.reject`
- **THEN** the Gateway responds with the current pairing state
- **AND** `devices.list` / `devices.revoke` are available for approved device management

### Requirement: Event Subscription Filtering
The Gateway SHALL support `events.subscribe`, `events.unsubscribe`, and `events.list` to filter outbound events per connection.

#### Scenario: Subscribe to stream events only
- **WHEN** a client subscribes to `stream.*`
- **THEN** only events whose method or topic matches `stream.*` are delivered

#### Scenario: No subscription configured
- **WHEN** a connection has no explicit subscription
- **THEN** it receives all events by default

### Requirement: Inbound Router Startup and Channel Auto‑Start
The Gateway SHALL start the inbound message router on startup and auto‑start channels when configured.

#### Scenario: Router startup
- **WHEN** the Gateway starts
- **THEN** it creates and starts `InboundMessageRouter`
- **AND** inbound messages are routed to execution

#### Scenario: Auto‑start channels
- **WHEN** `auto_start_channels` is enabled
- **THEN** all registered channels are started automatically

### Requirement: Unified Bindings for WS and Channel Inbound
Inbound channel messages SHALL resolve agent selection using the same bindings and default agent as WS `agent.run`.

#### Scenario: Binding applies to inbound message
- **WHEN** a channel inbound message arrives with channel identifier `imessage:*
- **AND** bindings include `imessage:* -> work`
- **THEN** the message is routed to agent `work`
- **AND** the session key is created using configured DM scope rules

### Requirement: Agent Run Control RPCs
The Gateway SHALL expose `agent.status` and `agent.cancel` for run control.

#### Scenario: Query run status
- **WHEN** a client calls `agent.status` with a valid `run_id`
- **THEN** the Gateway returns the run state and timing metadata

#### Scenario: Cancel a run
- **WHEN** a client calls `agent.cancel` with an active `run_id`
- **THEN** the Gateway cancels the run and returns a cancellation result
