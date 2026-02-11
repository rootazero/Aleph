# System State Bus Phase 6: Critical Enhancements

> **Status**: Planning
> **Timeline**: 3-4 weeks
> **Priority**: High
> **Dependencies**: Phase 1-5 Complete

## Executive Summary

Phase 6 addresses critical technical debt and implements high-value enhancements to make SSB fully operational in production. This phase focuses on three key areas:

1. **macOS Accessibility API Integration** - Replace stubs with real AX API calls
2. **Proactive Triggers** - Enable condition-based automation
3. **Browser Plugin Connector** - Extend SSB to web applications

## Goals

### Primary Goals

- ✅ Complete AX API integration for native macOS apps
- ✅ Implement proactive trigger system for automation
- ✅ Add Chrome/Firefox plugin connector for web apps
- ✅ Achieve 95%+ ID stability in real-world usage
- ✅ Support 50+ concurrent subscriptions

### Secondary Goals

- Improve OCR accuracy to 80%+
- Add Control Plane UI for SSB monitoring
- Implement basic CV algorithms for element detection
- Create 5+ example skills using SSB

## Architecture Overview

### Phase 6 Components

```
┌─────────────────────────────────────────────────────────────────┐
│                    Enhanced SSB Architecture                     │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              Connector Layer (Enhanced)                   │  │
│  ├──────────────────────────────────────────────────────────┤  │
│  │  AxConnector     │  PluginConnector  │  VisionConnector  │  │
│  │  (REAL AX API)   │  (NEW: Browser)   │  (Enhanced OCR)   │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              Trigger Engine (NEW)                         │  │
│  ├──────────────────────────────────────────────────────────┤  │
│  │  Condition Evaluator  │  Action Dispatcher  │  Scheduler │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐  │
│  │              State Bus Core                               │  │
│  ├──────────────────────────────────────────────────────────┤  │
│  │  Event Bus  │  State Cache  │  History  │  Privacy       │  │
│  └──────────────────────────────────────────────────────────┘  │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

## Task Breakdown

### Task 1: macOS Accessibility API Integration (Week 1-2)

**Goal**: Replace stub implementation with real AX API calls.

#### Subtasks

**1.1 AX Observer Implementation**
- Implement CFRunLoop-based event observer
- Handle AX notifications (value changed, focus changed, window events)
- Thread isolation for AX API calls
- Error handling and recovery

**1.2 AX Tree Traversal**
- Implement recursive tree walking
- Depth and node count limits (max_depth: 12, max_nodes: 1500)
- Element attribute extraction (role, label, value, rect)
- Coordinate system conversion (screen coordinates)

**1.3 Element ID Generation**
- Implement 3-level ID fallback:
  - Level 1: AXIdentifier (if available)
  - Level 2: Semantic hash (role + label + position)
  - Level 3: Path hash (element hierarchy)
- ID stability testing across UI changes

**1.4 Integration Testing**
- Test with real macOS apps (Mail, Safari, Finder)
- Verify event delivery latency (< 10ms target)
- Stress test with 10+ concurrent subscriptions
- Memory leak detection

**Files to Create/Modify**:
```
core/src/perception/state_bus/ax_observer.rs (replace stub)
core/src/perception/connectors/ax_connector.rs (new)
core/tests/integration_ax_api.rs (new)
```

**Success Criteria**:
- ✅ Real-time event delivery from macOS apps
- ✅ < 10ms latency from AX event to WebSocket
- ✅ 95%+ ID stability across UI changes
- ✅ Zero crashes from AX API calls
- ✅ < 2% CPU overhead with 10 subscriptions

---

### Task 2: Proactive Trigger System (Week 2-3)

**Goal**: Enable condition-based automation ("when X happens, do Y").

#### Architecture

```rust
pub struct Trigger {
    pub id: String,
    pub name: String,
    pub condition: TriggerCondition,
    pub actions: Vec<TriggerAction>,
    pub enabled: bool,
    pub cooldown_ms: u64,
}

pub enum TriggerCondition {
    /// Value comparison: /app_context/unread_count > 10
    ValueCompare {
        path: String,
        operator: CompareOp,
        value: Value,
    },
    /// Element state: element "btn_send" becomes enabled
    ElementState {
        element_id: String,
        state_key: String,
        value: bool,
    },
    /// Pattern match: element value matches regex
    PatternMatch {
        element_id: String,
        pattern: String,
    },
    /// Composite: AND/OR of multiple conditions
    Composite {
        operator: LogicOp,
        conditions: Vec<TriggerCondition>,
    },
}

pub enum TriggerAction {
    /// Execute UI action
    ExecuteAction(ActionRequest),
    /// Send notification
    Notify { title: String, body: String },
    /// Call webhook
    Webhook { url: String, payload: Value },
    /// Run skill
    RunSkill { skill_name: String, args: Value },
}
```

#### Subtasks

**2.1 Trigger Engine Core**
- Implement condition evaluator
- Action dispatcher with retry logic
- Cooldown mechanism (prevent trigger spam)
- Trigger state persistence

**2.2 RPC Methods**
- `system.trigger.create` - Create new trigger
- `system.trigger.update` - Update trigger
- `system.trigger.delete` - Delete trigger
- `system.trigger.list` - List all triggers
- `system.trigger.enable` - Enable/disable trigger

**2.3 Event Integration**
- Subscribe to SSB events
- Evaluate conditions on state changes
- Execute actions when conditions met
- Emit trigger events (fired, failed, cooldown)

**2.4 Example Triggers**
```rust
// Example 1: Email notification
Trigger {
    name: "Urgent Email Alert",
    condition: ValueCompare {
        path: "/app_context/unread_count",
        operator: GreaterThan,
        value: 10,
    },
    actions: vec![
        Notify {
            title: "Urgent Emails",
            body: "You have 10+ unread emails",
        }
    ],
}

// Example 2: Auto-save
Trigger {
    name: "Auto-save on idle",
    condition: Composite {
        operator: And,
        conditions: vec![
            ElementState {
                element_id: "document_editor",
                state_key: "focused",
                value: false,
            },
            ValueCompare {
                path: "/app_context/unsaved_changes",
                operator: Equals,
                value: true,
            },
        ],
    },
    actions: vec![
        ExecuteAction(ActionRequest {
            target_id: "btn_save",
            method: Click,
            ..
        })
    ],
}
```

**Files to Create**:
```
core/src/perception/trigger_engine/mod.rs
core/src/perception/trigger_engine/types.rs
core/src/perception/trigger_engine/evaluator.rs
core/src/perception/trigger_engine/executor.rs
core/src/gateway/handlers/triggers.rs
core/tests/integration_triggers.rs
```

**Success Criteria**:
- ✅ Support 100+ concurrent triggers
- ✅ < 5ms condition evaluation latency
- ✅ Reliable action execution with retry
- ✅ Zero false positives/negatives
- ✅ Trigger state persists across restarts

---

### Task 3: Browser Plugin Connector (Week 3-4)

**Goal**: Extend SSB to Chrome/Firefox web applications.

#### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Browser Extension                         │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  Content Script (injected into web pages)            │  │
│  │  - DOM observer (MutationObserver)                   │  │
│  │  - Element tree extraction                           │  │
│  │  - Action execution (click, type, etc.)              │  │
│  └──────────────────────────────────────────────────────┘  │
│                          ↕                                   │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  Background Script (extension service worker)        │  │
│  │  - WebSocket connection to Aleph Gateway             │  │
│  │  - Message routing                                   │  │
│  │  - State caching                                     │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
                          ↕ WebSocket
┌─────────────────────────────────────────────────────────────┐
│                    Aleph Gateway                             │
│  ┌──────────────────────────────────────────────────────┐  │
│  │  PluginConnector                                      │  │
│  │  - Manages browser connections                        │  │
│  │  - Routes state updates to SSB                        │  │
│  │  - Dispatches actions to browser                      │  │
│  └──────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────┘
```

#### Subtasks

**3.1 Browser Extension (Chrome)**
- Manifest V3 extension setup
- Content script for DOM observation
- Background script for WebSocket connection
- Action execution (click, type, scroll)
- Element selector generation (CSS selectors)

**3.2 PluginConnector Implementation**
- WebSocket server for browser connections
- State message protocol
- Action dispatch to browser
- Connection lifecycle management

**3.3 Deep Integration Examples**
- Gmail: Unread count, compose state, send button
- Notion: Page title, block content, selection
- Slack: Channel list, message input, unread badges
- GitHub: PR status, review comments, CI status

**3.4 Extension Distribution**
- Chrome Web Store package
- Firefox Add-ons package
- Self-hosted installation guide
- Auto-update mechanism

**Files to Create**:
```
extensions/chrome/manifest.json
extensions/chrome/content.js
extensions/chrome/background.js
extensions/chrome/aleph-connector.js
core/src/perception/connectors/plugin_connector.rs
core/tests/integration_browser_plugin.rs
```

**Success Criteria**:
- ✅ Support Chrome and Firefox
- ✅ < 20ms latency for DOM changes
- ✅ Reliable action execution
- ✅ Works with Gmail, Notion, Slack
- ✅ Automatic reconnection on disconnect

---

## Implementation Timeline

### Week 1: AX API Integration (Part 1)
- Day 1-2: AX Observer implementation
- Day 3-4: AX Tree traversal
- Day 5: Element ID generation

### Week 2: AX API Integration (Part 2) + Triggers (Part 1)
- Day 1-2: Integration testing and bug fixes
- Day 3-4: Trigger engine core
- Day 5: Trigger RPC methods

### Week 3: Triggers (Part 2) + Browser Plugin (Part 1)
- Day 1-2: Trigger event integration and examples
- Day 3-4: Chrome extension development
- Day 5: PluginConnector implementation

### Week 4: Browser Plugin (Part 2) + Polish
- Day 1-2: Deep integration examples (Gmail, Notion)
- Day 3: Extension distribution
- Day 4-5: Documentation, testing, bug fixes

## Testing Strategy

### Unit Tests
- AX API: Element extraction, ID generation
- Triggers: Condition evaluation, action execution
- Browser Plugin: Message protocol, state parsing

### Integration Tests
- AX API: Real macOS apps (Mail, Safari, Finder)
- Triggers: End-to-end trigger firing
- Browser Plugin: Real web apps (Gmail, Notion)

### Performance Tests
- AX API: Latency, CPU usage, memory leaks
- Triggers: 100+ concurrent triggers
- Browser Plugin: DOM change latency

### Stress Tests
- 50+ concurrent SSB subscriptions
- 1000+ trigger evaluations per second
- 10+ browser tabs with active plugins

## Risk Mitigation

### Technical Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| AX API instability | High | Version detection, fallback to Vision |
| CFRunLoop conflicts | High | Dedicated thread isolation |
| Browser extension approval | Medium | Self-hosted option, clear privacy policy |
| Trigger performance | Medium | Condition indexing, lazy evaluation |

### Operational Risks

| Risk | Impact | Mitigation |
|------|--------|------------|
| High CPU usage | Medium | Adaptive polling, subscription limits |
| Memory leaks | High | Automated leak detection, strict limits |
| Privacy concerns | High | Clear documentation, opt-in for sensitive apps |

## Success Metrics

### Performance Metrics
- **AX API Latency**: < 10ms (event → WebSocket)
- **Trigger Evaluation**: < 5ms per condition
- **Browser Plugin Latency**: < 20ms (DOM change → SSB)
- **CPU Usage**: < 3% with 50 subscriptions
- **Memory Usage**: < 100MB with 50 subscriptions

### Quality Metrics
- **ID Stability**: > 95% across UI changes
- **Trigger Accuracy**: 100% (no false positives/negatives)
- **Browser Compatibility**: Chrome 90+, Firefox 88+
- **Uptime**: > 99.9% (no crashes)

### Adoption Metrics
- **Active Triggers**: > 20 in production
- **Browser Plugin Users**: > 10 beta testers
- **Skills Using SSB**: > 10 skills
- **Developer Satisfaction**: > 4.5/5

## Deliverables

### Code
- [ ] AX API integration (500+ lines)
- [ ] Trigger engine (800+ lines)
- [ ] Browser plugin connector (600+ lines)
- [ ] Chrome extension (400+ lines)
- [ ] Integration tests (500+ lines)

### Documentation
- [ ] AX API integration guide
- [ ] Trigger system documentation
- [ ] Browser plugin development guide
- [ ] Example triggers (10+ examples)
- [ ] Troubleshooting guide

### Examples
- [ ] 5+ example triggers
- [ ] 3+ browser plugin integrations
- [ ] Updated email auto-responder skill

## Future Enhancements (Phase 7+)

### Short-term (Next 3 months)
1. **Shadow Object Model (SOM)** - Logical models for common apps
2. **Cross-app Entity Linking** - Recognize entities across apps
3. **OCR Engine Integration** - tesseract/paddleocr
4. **CV Algorithms** - Template matching, edge detection

### Long-term (6-12 months)
1. **Multi-device SSB** - macOS, iOS, Linux sync
2. **Collaborative State** - Multi-agent shared state
3. **Predictive Caching** - Pre-fetch likely states
4. **Semantic Compression** - LLM-based state summarization

## Dependencies

### External Dependencies
- macOS Accessibility API (system)
- Chrome Extension APIs (Manifest V3)
- Firefox WebExtensions APIs

### Internal Dependencies
- Phase 1-5 complete ✅
- Gateway EventBus ✅
- Action Dispatcher ✅
- Privacy Filter ✅

## Approval

- [ ] Architecture Review
- [ ] Security Review
- [ ] Performance Review
- [ ] User Acceptance Testing

---

**Document Status**: Planning Complete
**Next Steps**: Begin Week 1 implementation
**Review Date**: 2026-02-18
**Owner**: Aleph Core Team
