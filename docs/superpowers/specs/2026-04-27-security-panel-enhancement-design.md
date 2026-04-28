# Security Panel Enhancement Design

> 将 Runtime Security Layer 配置添加到 Panel Settings

**Date**: 2026-04-27  
**Status**: Draft  
**Related**: [Runtime Security Enhancement Design](2026-04-27-runtime-security-enhancement-design.md)

---

## 1. Overview

### 1.1 Purpose
将前面实现的三组 Runtime Security Layer 配置（Shell Risk Patterns、PII Rules、Secret Protection）集成到 Web Panel 的安全设置页面，提供可视化配置界面。

### 1.2 Scope
- **In Scope**: 
  - Shell Security 配置（enable_custom_patterns + custom patterns）
  - 自定义 PII 规则管理（与现有 PII 设置合并）
  - Secret 保护配置（Virtual Keys + Custom Leak Patterns）
  - 前端正则验证
  - 配置保存/加载
- **Out of Scope**: 
  - 配置生效逻辑（已由后端实现）
  - 国际化完整覆盖（使用基础 key）
  - 复杂规则测试工具

### 1.3 Success Criteria
- [ ] 用户可以在安全设置页面查看和编辑三组新配置
- [ ] 正则表达式在保存前进行前端验证
- [ ] 配置保存后持久化到 TOML 文件
- [ ] 向后兼容：未配置的字段使用默认值

---

## 2. Architecture

### 2.1 Data Flow

```
┌─────────────────────────────────────────────────────────────┐
│                     Web Panel (Leptos/WASM)                 │
│  ┌─────────────────┐  ┌─────────────────┐  ┌──────────────┐ │
│  │ ShellSecurity   │  │ Custom PII      │  │ Secret       │ │
│  │ Section         │  │ Rules Section   │  │ Protection   │ │
│  │                 │  │                 │  │ Section      │ │
│  └────────┬────────┘  └────────┬────────┘  └──────┬───────┘ │
│           │                    │                   │         │
│           └────────────────────┼───────────────────┘         │
│                                │                             │
│                    ┌───────────▼────────┐                    │
│                    │ SecurityConfigApi  │                    │
│                    │ (get/update)       │                    │
│                    └───────────┬────────┘                    │
└────────────────────────────────┼─────────────────────────────┘
                                 │ JSON-RPC
                                 ▼
┌─────────────────────────────────────────────────────────────┐
│                    Gateway Handler                           │
│              security_config.get/update                      │
│  ┌─────────────┬──────────────┬────────────────────────────┐ │
│  │ Shell       │ Privacy      │ Secrets                    │ │
│  │ Security    │ Custom Rules │ Protection                 │ │
│  └──────┬──────┴──────┬───────┴────────────┬───────────────┘ │
│         │             │                    │                 │
│         ▼             ▼                    ▼                 │
│  ┌────────────┐ ┌────────────┐    ┌────────────┐             │
│  │[security.  │ │[privacy.   │    │[secrets_   │             │
│  │ shell]     │ │custom_rules│    │config]     │             │
│  └────────────┘ └────────────┘    └────────────┘             │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Component Hierarchy

```
SecurityView
├── GatewaySecuritySettings (existing)
├── NetworkAccessSection (existing)
├── OutboundSecuritySection (existing)
├── PairedDevices (existing)
├── PIISection (enhanced - merged)
│   ├── Existing PII Settings (existing)
│   └── CustomPiiRulesSubsection (NEW)
├── ShellSecuritySection (NEW)
│   ├── enable_custom_patterns toggle
│   ├── CustomRiskPatternList (blocked)
│   ├── CustomRiskPatternList (danger)
│   └── CustomRiskPatternList (safe)
└── SecretProtectionSection (NEW)
    ├── VirtualKeyMapList
    └── CustomLeakPatternList
```

---

## 3. Data Models

### 3.1 Extended SecurityConfig (Frontend)

```rust
// interfaces/webchat/src/api/security.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    // Existing fields
    pub require_auth: bool,
    pub enable_pairing: bool,
    pub allow_guest: bool,
    pub network_access: NetworkAccess,
    pub ssrf_enabled: bool,
    pub ssrf_allow_tool_private_network: bool,
    pub ssrf_allow_webhook_private_network: bool,
    pub ssrf_max_redirects: u8,
    pub ssrf_allowed_hosts: Vec<String>,
    pub ssrf_blocked_hosts: Vec<String>,
    
    // NEW: Shell Security
    #[serde(default)]
    pub shell_security: ShellSecurityConfig,
    
    // NEW: Custom PII Rules
    #[serde(default)]
    pub custom_pii_rules: Vec<CustomPiiRule>,
    
    // NEW: Secret Protection
    #[serde(default)]
    pub secrets_protection: SecretsProtectionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellSecurityConfig {
    pub enable_custom_patterns: bool,
    pub custom_blocked: Vec<CustomRiskPattern>,
    pub custom_danger: Vec<CustomRiskPattern>,
    pub custom_safe: Vec<CustomRiskPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRiskPattern {
    pub pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPiiRule {
    pub name: String,
    pub pattern: String,
    #[serde(default = "default_placeholder")]
    pub placeholder: String,
    #[serde(default)]
    pub severity: CustomPiiSeverity,
    #[serde(default)]
    pub action: PiiAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum CustomPiiSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum PiiAction {
    #[default]
    Block,
    Warn,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretsProtectionConfig {
    pub virtual_keys: Vec<VirtualKeyEntry>,
    pub custom_leak_patterns: Vec<CustomLeakPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualKeyEntry {
    pub alias: String,
    pub secret_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomLeakPattern {
    pub name: String,
    pub pattern: String,
}
```

### 3.2 TOML Storage Format

配置将分别存储到各自的 TOML 段，保持模块化：

```toml
# Shell Security
[security.shell]
enable_custom_patterns = true

[[security.shell.custom_blocked]]
pattern = "^dangerous_tool\\s+"
reason = "Custom blocked tool"

[[security.shell.custom_danger]]
pattern = "^custom_admin_cmd\\s+"
reason = "Requires approval"

[[security.shell.custom_safe]]
pattern = "^my_safe_script\\s+"
reason = "Auto-approved"

# Privacy - Custom Rules
[[privacy.custom_rules]]
name = "internal_token"
pattern = "IT-[A-Z0-9]{16}"
placeholder = "[INTERNAL_TOKEN]"
severity = "high"
action = "block"

[[privacy.custom_rules]]
name = "employee_id"
pattern = "EMP-[0-9]{6}"

# Secrets Protection
[secrets_config.virtual_keys]
"openai" = "OPENAI_API_KEY"
"anthropic" = "ANTHROPIC_API_KEY_PROD"

[[secrets_config.custom_leak_patterns]]
name = "Internal API Token"
pattern = "internal-[a-z0-9]{32}"
```

---

## 4. UI Design

### 4.1 ShellSecuritySection

```
┌─────────────────────────────────────────────────────────────┐
│ Shell Command Security                           [?]        │
├─────────────────────────────────────────────────────────────┤
│ [ ] Enable Custom Risk Patterns                             │
│     When enabled, custom patterns below supplement built-in │ │
│     security rules                                          │
│                                                             │
│ Blocked Patterns (execution denied)                         │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ Pattern: [^dangerous_tool\s+                   ] [🗑️]  │ │
│ │ Reason:  [Custom blocked tool                  ]       │ │
│ ├─────────────────────────────────────────────────────────┤ │
│ │ Pattern: [                                     ] [🗑️]  │ │
│ │ Reason:  [                                     ]       │ │
│ └─────────────────────────────────────────────────────────┘ │
│ [+ Add Blocked Pattern]                                     │
│                                                             │
│ Danger Patterns (require approval)                          │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ Pattern: [^custom_admin_cmd\s+                 ] [🗑️]  │ │
│ │ Reason:  [Requires approval                    ]       │ │
│ └─────────────────────────────────────────────────────────┘ │
│ [+ Add Danger Pattern]                                      │
│                                                             │
│ Safe Patterns (auto-approved)                               │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ Pattern: [^my_safe_script\s+                   ] [🗑️]  │ │
│ │ Reason:  [Auto-approved                        ]       │ │
│ └─────────────────────────────────────────────────────────┘ │
│ [+ Add Safe Pattern]                                        │
└─────────────────────────────────────────────────────────────┘
```

**Validation**: Pattern 字段实时验证正则有效性，无效时显示红色边框和错误提示。

### 4.2 CustomPiiRulesSection (Merged)

整合到现有 PIISection，分为两个子区域：

```
┌─────────────────────────────────────────────────────────────┐
│ PII Protection                                     [Save]   │
├─────────────────────────────────────────────────────────────┤
│ [ ] Enable PII Protection                                   │
│                                                             │
│ Standard PII Types (when enabled)                          │
│ [✓] Email      [✓] Phone      [✓] SSN      [✓] Credit Card │
│                                                             │
│ ────────────────────────────────────────────────────────────│
│ Custom PII Rules                                            │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ Name:        [Internal Token                    ] [🗑️] │ │
│ │ Pattern:     [IT-[A-Z0-9]{16}                     ]     │ │
│ │ Placeholder: [[INTERNAL_TOKEN]                  ]       │ │
│ │ Severity:    [Medium ▼]    Action: [Block ▼]            │ │
│ ├─────────────────────────────────────────────────────────┤ │
│ │ Name:        [Employee ID                       ] [🗑️] │ │
│ │ Pattern:     [EMP-[0-9]{6}                       ]     │ │
│ │ Placeholder: [[CUSTOM_PII]                      ]       │ │
│ │ Severity:    [Low ▼]       Action: [Warn ▼]             │ │
│ └─────────────────────────────────────────────────────────┘ │
│ [+ Add Custom Rule]                                         │
└─────────────────────────────────────────────────────────────┘
```

### 4.3 SecretProtectionSection

```
┌─────────────────────────────────────────────────────────────┐
│ Secret Protection                                [?]        │
├─────────────────────────────────────────────────────────────┤
│ Virtual Key Aliases                                         │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ Alias:     [openai                             ] [🗑️] │ │
│ │ Maps to:   [OPENAI_API_KEY                     ]       │ │
│ ├─────────────────────────────────────────────────────────┤ │
│ │ Alias:     [anthropic                          ] [🗑️] │ │
│ │ Maps to:   [ANTHROPIC_API_KEY_PROD             ]       │ │
│ └─────────────────────────────────────────────────────────┘ │
│ [+ Add Virtual Key]                                         │
│                                                             │
│ Custom Leak Detection Patterns                              │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ Name:    [Internal API Token                   ] [🗑️] │ │
│ │ Pattern: [internal-[a-z0-9]{32}                 ]       │ │
│ ├─────────────────────────────────────────────────────────┤ │
│ │ Name:    [Custom Service Token                 ] [🗑️] │ │
│ │ Pattern: [cst_[a-zA-Z0-9]{40}                    ]       │ │
│ └─────────────────────────────────────────────────────────┘ │
│ [+ Add Leak Pattern]                                        │
└─────────────────────────────────────────────────────────────┘
```

---

## 5. Backend Implementation

### 5.1 Handler Extensions

**File**: `src/gateway/handlers/security_config.rs`

扩展 `handle_get` 和 `handle_update`：

```rust
pub async fn handle_get(
    request: JsonRpcRequest,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    // Existing: read gateway.host, SSRF config
    // NEW: read shell_security, custom_pii_rules, secrets_protection
    let shell_security = read_shell_security_from_toml(&config_patcher);
    let custom_pii_rules = read_custom_pii_rules_from_toml(&config_patcher);
    let secrets_protection = read_secrets_protection_from_toml(&config_patcher);
    
    let security_config = SecurityConfig {
        // ... existing fields ...
        shell_security,
        custom_pii_rules,
        secrets_protection,
    };
    
    JsonRpcResponse::success(request.id, serde_json::to_value(&security_config).unwrap())
}

pub async fn handle_update(
    request: JsonRpcRequest,
    config_patcher: Arc<ConfigPatcher>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    // Parse extended SecurityConfig
    // Existing: write gateway.host, SSRF config
    // NEW: write shell_security, custom_pii_rules, secrets_protection to respective TOML sections
}
```

### 5.2 TOML Read/Write Helpers

为每组配置添加独立的读写函数（类似现有的 `read_ssrf_config_from_toml` / `write_ssrf_config_to_toml`）：

- `read_shell_security_from_toml` / `write_shell_security_to_toml`
- `read_custom_pii_rules_from_toml` / `write_custom_pii_rules_to_toml`
- `read_secrets_protection_from_toml` / `write_secrets_protection_to_toml`

---

## 6. Frontend Implementation

### 6.1 API Types Extension

**File**: `interfaces/webchat/src/api/security.rs`

添加第 3 节定义的新类型。

### 6.2 New Section Components

**File**: `interfaces/webchat/src/views/settings/security.rs`

添加三个新组件：

```rust
#[component]
fn ShellSecuritySection(
    config: RwSignal<Option<SecurityConfig>>,
    pattern_errors: RwSignal<Vec<(usize, String)>>, // (index, error_msg)
) -> impl IntoView { ... }

#[component]
fn CustomPiiRulesSubsection(
    rules: RwSignal<Vec<CustomPiiRule>>,
    pattern_errors: RwSignal<Vec<(usize, String)>>,
) -> impl IntoView { ... }

#[component]
fn SecretProtectionSection(
    config: RwSignal<Option<SecurityConfig>>,
    pattern_errors: RwSignal<Vec<(usize, String)>>,
) -> impl IntoView { ... }
```

### 6.3 Regex Validation

```rust
fn validate_regex(pattern: &str) -> Result<(), String> {
    // Use web_sys or js-sys to validate regex
    match js_sys::RegExp::new(pattern, "") {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("Invalid regex: {:?}", e)),
    }
}
```

### 6.4 State Management

在 `SecurityView` 中添加：

```rust
// Existing
let config = RwSignal::new(Option::<SecurityConfig>::None);
let loading = RwSignal::new(true);
let saving = RwSignal::new(false);

// NEW: Validation state
let shell_pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());
let pii_pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());
let leak_pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());
```

---

## 7. Validation & Error Handling

### 7.1 Frontend Validation

| Field | Validation | Error Display |
|-------|-----------|---------------|
| `pattern` (Shell) | Valid regex | Red border + tooltip |
| `pattern` (PII) | Valid regex | Red border + tooltip |
| `pattern` (Leak) | Valid regex | Red border + tooltip |
| `name` (PII) | Non-empty | Red border |
| `name` (Leak) | Non-empty | Red border |
| `alias` (Virtual Key) | Non-empty | Red border |
| `secret_name` (Virtual Key) | Non-empty | Red border |

### 7.2 Save Behavior

- 如果有验证错误，禁用 Save 按钮
- 保存时验证所有 pattern，失败不提交
- 后端返回错误时显示在页面顶部

---

## 8. Testing Strategy

### 8.1 Unit Tests

- Regex validation helper
- TOML serialization/deserialization
- Pattern list add/remove/update

### 8.2 Integration Tests

- End-to-end: add rule → save → reload → verify
- Error case: invalid regex → save blocked
- Empty list handling

---

## 9. Migration & Compatibility

### 9.1 Backward Compatibility

- 新字段使用 `#[serde(default)]`，未配置时不会报错
- 默认行为：所有新功能默认关闭（enable_custom_patterns = false, empty lists）
- 现有 TOML 无需修改即可继续工作

### 9.2 Migration Path

现有用户升级到包含新功能的版本后：
1. 打开安全设置页面
2. 看到新增的三个配置区域（均为空/关闭状态）
3. 可选择启用并配置，或保持现状

---

## 10. Open Questions

None - all design decisions confirmed with user.

---

## Appendix A: File List

### Backend
- `src/gateway/handlers/security_config.rs` - Extend handler
- `src/config/types/security.rs` - Already exists (ShellSecurityConfig)
- `src/config/types/privacy.rs` - Already exists (CustomPiiRule)
- `src/config/types/secrets.rs` - Already exists (VirtualKeyMap, CustomLeakPattern)

### Frontend
- `interfaces/webchat/src/api/security.rs` - Extend types
- `interfaces/webchat/src/views/settings/security.rs` - Add sections
- `interfaces/webchat/src/components/forms.rs` - Use existing components

### I18n (optional)
- `interfaces/webchat/locales/en.yaml` - Add translation keys
- `interfaces/webchat/locales/zh.yaml` - Add translation keys
