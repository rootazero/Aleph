## MODIFIED Requirements
### Requirement: Context Anchor Creation
The system SHALL package captured context as a structured data type and send to Rust core, with an optional perception snapshot reference.
ZH: 系统必须将捕获的上下文封装为结构化数据，并附带可选的感知快照引用发送到 Rust Core。

#### Scenario: Create context anchor without perception
- **GIVEN** bundle_id = "com.apple.Notes"
- **AND** window_title = "Project Plan.txt"
- **WHEN** context is captured and no perception snapshot is available
- **THEN** creates `CapturedContext { bundle_id, window_title, perception: None }`
- **AND** sends to Rust via `core.setCurrentContext(context)`
- **AND** Rust stores in `Arc<Mutex<Option<CapturedContext>>>`

#### Scenario: Attach perception snapshot reference
- **GIVEN** bundle_id = "com.apple.Notes"
- **AND** window_title = "Project Plan.txt"
- **AND** perception_snapshot_id = "ps_1234"
- **WHEN** SnapshotTool completes within its time budget
- **THEN** creates `CapturedContext { bundle_id, window_title, perception: Some(PerceptionRef { snapshot_id: "ps_1234", focus_hint }) }`
- **AND** sends to Rust via `core.setCurrentContext(context)`
- **AND** Rust stores in `Arc<Mutex<Option<CapturedContext>>>`
