# Link ACL Probe Tests Design

**Date**: 2026-03-14
**Status**: Approved

## Goal

Production-grade probe tests for the agent link access control system. Verify all enforcement points, configuration lifecycle, multi-agent matrix, and edge cases through a complete RouterTestHarness.

## Architecture

Three-layer probe test infrastructure following the cron probe pattern:
- **Layer 1 (Mocks)**: MockChannelRegistry (captures outbound replies), MockAgentRegistry (configurable allowed_links)
- **Layer 2 (Harness)**: RouterTestHarness wrapping InboundMessageRouter with all mocked dependencies
- **Layer 3 (Scenarios)**: 43 end-to-end scenarios across 7 subsystems (P1-P7)

## File Structure

```
tests/
├── link_acl_probe.rs                 # Entry point with mod declarations
└── link_acl_probe/
    ├── harness.rs                    # RouterTestHarness
    ├── mock_channel.rs               # MockChannelRegistry (capture outbound)
    ├── mock_agent.rs                 # MockAgentRegistry (configure allowed_links)
    ├── access_control.rs             # P1: check_link_access pure logic (6 scenarios)
    ├── message_routing.rs            # P2: handle_message enforcement (8 scenarios)
    ├── switch_command.rs             # P3: /switch command enforcement (6 scenarios)
    ├── intent_switch.rs              # P4: intent-based switch enforcement (5 scenarios)
    ├── config_lifecycle.rs           # P5: config hot-update + persistence (5 scenarios)
    ├── multi_agent_matrix.rs         # P6: multi-link x multi-agent matrix (6 scenarios)
    └── edge_cases.rs                 # P7: boundaries + system interaction (7 scenarios)
```

## Harness Design

### RouterTestHarness API

```rust
pub struct RouterTestHarness {
    pub router: Arc<InboundMessageRouter>,
    pub agent_registry: Arc<AgentRegistry>,
    pub channel_registry: Arc<ChannelRegistry>,
    pub workspace_manager: Arc<WorkspaceManager>,
    pub outbound_rx: mpsc::Receiver<CapturedReply>,
}

pub struct CapturedReply {
    pub channel_id: String,
    pub conversation_id: String,
    pub text: String,
}

impl RouterTestHarness {
    // Construction
    fn new() -> Self;

    // Agent configuration
    fn register_agent(&self, id: &str, allowed_links: Option<Vec<String>>);
    fn set_default_agent(&self, id: &str);
    fn update_allowed_links(&self, agent_id: &str, links: Option<Vec<String>>);

    // Message simulation
    fn send_message(&self, link_id: &str, text: &str) -> Result<(), RoutingError>;
    fn send_switch(&self, link_id: &str, target_agent: &str) -> Result<(), RoutingError>;

    // Assertions
    fn assert_reply_contains(&self, substring: &str);
    fn assert_no_reply(&self);
    fn assert_routed_to(&self, agent_id: &str);
    fn assert_denied(&self);
}
```

## Test Scenarios (43 total)

### P1: Access Control Pure Logic (6 scenarios)

| # | Scenario | allowed_links | link_id | Expected |
|---|----------|--------------|---------|----------|
| 1 | None = all allowed | `None` | `"telegram-bot"` | Ok |
| 2 | Empty list = all allowed | `Some([])` | `"telegram-bot"` | Ok |
| 3 | Whitelist hit | `Some(["telegram-bot"])` | `"telegram-bot"` | Ok |
| 4 | Whitelist miss | `Some(["telegram-bot"])` | `"discord-bot"` | Err(LinkNotAllowed) |
| 5 | Multi-link whitelist | `Some(["tg-1", "tg-2"])` | `"tg-2"` | Ok |
| 6 | Single-link whitelist rejects others | `Some(["tg-1"])` | `"dc-1"` | Err |

### P2: Message Routing Enforcement (8 scenarios)

| # | Scenario | Expected |
|---|----------|----------|
| 1 | Unrestricted agent, any link sends message | Normal routing |
| 2 | Restricted agent, allowed link sends message | Normal routing |
| 3 | Restricted agent, denied link sends message | Deny + reply error |
| 4 | Agent not in registry | Skip check, fallback logic |
| 5 | Denied message not executed | ExecutionEngine not called |
| 6 | Denial reply contains link_id and agent_id | Verify error format |
| 7 | Same link sends twice, both denied | No cache/bypass |
| 8 | Group message (is_group=true) also ACL controlled | Groups not exempt |

### P3: /switch Command Enforcement (6 scenarios)

| # | Scenario | Expected |
|---|----------|----------|
| 1 | /switch to allowed agent | Switch success |
| 2 | /switch to denied agent | Deny message, no switch |
| 3 | /switch to non-existent agent | "not found" (existing logic) |
| 4 | /switch from denied agent back to allowed | Switch success |
| 5 | Agent allows link-A, denies link-B | A succeeds, B denied |
| 6 | /switch denied, current agent unchanged | workspace_manager not called |

### P4: Intent Switch Enforcement (5 scenarios)

| # | Scenario | Expected |
|---|----------|----------|
| 1 | Intent switch to allowed agent | Switch success |
| 2 | Intent switch to denied agent | Deny + error reply |
| 3 | Dynamic agent creation + denied link | Created but switch denied |
| 4 | Intent with task ("use X to write report"), X denied | Deny, task not executed |
| 5 | Non-switch intent, no ACL check | Normal processing |

### P5: Config Lifecycle (5 scenarios)

| # | Scenario | Expected |
|---|----------|----------|
| 1 | Runtime update: None → restricted list | Next message denied |
| 2 | Runtime update: restricted → None | Next message allowed |
| 3 | Runtime update: list A → list B | Previously allowed link denied |
| 4 | Newly created agent default allowed_links=None | All links can access |
| 5 | Delete + recreate same ID agent | New config takes effect |

### P6: Multi-Link x Multi-Agent Matrix (6 scenarios)

| # | Scenario | Expected |
|---|----------|----------|
| 1 | 3 agents x 3 links, each agent allows different links | Verify all 9 combinations |
| 2 | Default agent restricted, fallback routing denied | Deny, no fallback to other agent |
| 3 | Agent-A allows link-1, Agent-B allows link-2 | Independent, no interference |
| 4 | Same link has different permissions per agent | Each route/switch checks independently |
| 5 | All agents deny a link | Link cannot access any agent |
| 6 | One agent allows all, another restricts all | Parallel verification |

### P7: Edge Cases + System Interaction (7 scenarios)

| # | Scenario | Expected |
|---|----------|----------|
| 1 | allowed_links contains deleted/nonexistent link ID | No effect on other links |
| 2 | link_id is empty string | Safe deny |
| 3 | allowed_links has duplicate entries | Works correctly, no crash |
| 4 | agent_id is empty string | Safe handling |
| 5 | Pairing + ACL: paired but ACL denied | ACL takes precedence |
| 6 | Routing rules specify agent + ACL denied | ACL still blocks |
| 7 | Concurrent: same link sends multiple messages to restricted agent | All denied, no race condition |

## Key Design Decisions

1. **Full RouterTestHarness** — mocks all dependencies, simulates complete message flow
2. **CapturedReply channel** — outbound messages captured via mpsc for assertion
3. **Same directory pattern as cron probe** — `tests/link_acl_probe/`
4. **43 scenarios across 7 subsystems** — comprehensive production-grade coverage
5. **Deterministic** — no real time, no real network, all in-process
