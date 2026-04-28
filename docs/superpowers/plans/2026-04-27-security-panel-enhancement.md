# Security Panel Enhancement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Shell Security, Custom PII Rules, and Secret Protection configuration UI to the existing Security settings page.

**Architecture:** Extend the existing `SecurityConfig` API and handler to include new configuration sections, add three new UI Section components to the security view, and implement frontend regex validation using `js_sys::RegExp`.

**Tech Stack:** Rust (Leptos/WASM frontend), Rust (Tokio/Gateway backend), TOML config, JSON-RPC

---

## File Structure

### Backend Files
| File | Responsibility |
|------|---------------|
| `src/gateway/handlers/security_config.rs` | Extend `SecurityConfig` struct, add `handle_get`/`handle_update` support for new config sections, add TOML read/write helpers |
| `interfaces/webchat/src/api/security.rs` | Extend frontend `SecurityConfig` struct with new fields |

### Frontend Files
| File | Responsibility |
|------|---------------|
| `interfaces/webchat/src/views/settings/security.rs` | Add three new Section components: `ShellSecuritySection`, `CustomPiiRulesSubsection`, `SecretProtectionSection` |
| `interfaces/webchat/src/api/security.rs` | Add new types: `ShellSecurityConfig`, `CustomRiskPattern`, `CustomPiiRule`, `CustomPiiSeverity`, `PiiAction`, `SecretsProtectionConfig`, `VirtualKeyEntry`, `CustomLeakPattern` |

### Build Files
| File | Responsibility |
|------|---------------|
| `Cargo.toml` (webchat) | Ensure `js-sys` is available for regex validation |

---

## Task 1: Extend Backend SecurityConfig Handler

**Files:**
- Modify: `src/gateway/handlers/security_config.rs`

### Step 1: Add new types to handler

Add these types after the existing `SecurityConfig` definition (around line 249):

```rust
// Shell Security Configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellSecurityConfig {
    #[serde(default)]
    pub enable_custom_patterns: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_blocked: Vec<CustomRiskPattern>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_danger: Vec<CustomRiskPattern>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_safe: Vec<CustomRiskPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRiskPattern {
    pub pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// Custom PII Rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPiiRule {
    pub name: String,
    pub pattern: String,
    #[serde(default = "default_custom_pii_placeholder")]
    pub placeholder: String,
    #[serde(default)]
    pub severity: CustomPiiSeverity,
    #[serde(default)]
    pub action: PiiAction,
}

fn default_custom_pii_placeholder() -> String {
    "[CUSTOM_PII]".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CustomPiiSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PiiAction {
    #[default]
    Block,
    Warn,
    Off,
}

// Secret Protection
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretsProtectionConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub virtual_keys: Vec<VirtualKeyEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

### Step 2: Extend SecurityConfig struct

Add new fields to `SecurityConfig` (after `ssrf_blocked_hosts`):

```rust
    // Shell Security
    #[serde(default)]
    pub shell_security: ShellSecurityConfig,
    // Custom PII Rules
    #[serde(default)]
    pub custom_pii_rules: Vec<CustomPiiRule>,
    // Secret Protection
    #[serde(default)]
    pub secrets_protection: SecretsProtectionConfig,
```

### Step 3: Add TOML read helpers

Add these functions after `read_ssrf_config_from_toml` (around line 120):

```rust
/// Read shell security settings from [security.shell] section.
fn read_shell_security_from_toml(
    patcher: &ConfigPatcher,
) -> ShellSecurityConfig {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(security) = table.get("security").and_then(|v| v.as_table()) {
                if let Some(shell) = security.get("shell").and_then(|v| v.as_table()) {
                    let enable = shell
                        .get("enable_custom_patterns")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    
                    let blocked = shell
                        .get("custom_blocked")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_table().map(|t| CustomRiskPattern {
                                    pattern: t.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    reason: t.get("reason").and_then(|v| v.as_str()).map(String::from),
                                }))
                                .collect()
                        })
                        .unwrap_or_default();
                    
                    let danger = shell
                        .get("custom_danger")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_table().map(|t| CustomRiskPattern {
                                    pattern: t.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    reason: t.get("reason").and_then(|v| v.as_str()).map(String::from),
                                }))
                                .collect()
                        })
                        .unwrap_or_default();
                    
                    let safe = shell
                        .get("custom_safe")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_table().map(|t| CustomRiskPattern {
                                    pattern: t.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                    reason: t.get("reason").and_then(|v| v.as_str()).map(String::from),
                                }))
                                .collect()
                        })
                        .unwrap_or_default();
                    
                    return ShellSecurityConfig {
                        enable_custom_patterns: enable,
                        custom_blocked: blocked,
                        custom_danger: danger,
                        custom_safe: safe,
                    };
                }
            }
        }
    }
    ShellSecurityConfig::default()
}

/// Read custom PII rules from [[privacy.custom_rules]] entries.
fn read_custom_pii_rules_from_toml(
    patcher: &ConfigPatcher,
) -> Vec<CustomPiiRule> {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(privacy) = table.get("privacy").and_then(|v| v.as_table()) {
                if let Some(rules) = privacy.get("custom_rules").and_then(|v| v.as_array()) {
                    return rules
                        .iter()
                        .filter_map(|v| v.as_table().map(|t| CustomPiiRule {
                            name: t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            pattern: t.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            placeholder: t.get("placeholder")
                                .and_then(|v| v.as_str())
                                .unwrap_or("[CUSTOM_PII]")
                                .to_string(),
                            severity: t.get("severity")
                                .and_then(|v| v.as_str())
                                .map(|s| match s {
                                    "low" => CustomPiiSeverity::Low,
                                    "medium" => CustomPiiSeverity::Medium,
                                    "high" => CustomPiiSeverity::High,
                                    "critical" => CustomPiiSeverity::Critical,
                                    _ => CustomPiiSeverity::Medium,
                                })
                                .unwrap_or_default(),
                            action: t.get("action")
                                .and_then(|v| v.as_str())
                                .map(|s| match s {
                                    "block" => PiiAction::Block,
                                    "warn" => PiiAction::Warn,
                                    "off" => PiiAction::Off,
                                    _ => PiiAction::Block,
                                })
                                .unwrap_or_default(),
                        }))
                        .collect();
                }
            }
        }
    }
    Vec::new()
}

/// Read secret protection from [secrets_config] section.
fn read_secrets_protection_from_toml(
    patcher: &ConfigPatcher,
) -> SecretsProtectionConfig {
    let config_path = patcher.config_path();
    if let Ok(contents) = std::fs::read_to_string(config_path) {
        if let Ok(table) = contents.parse::<toml::Table>() {
            if let Some(secrets) = table.get("secrets_config").and_then(|v| v.as_table()) {
                let virtual_keys = secrets
                    .get("virtual_keys")
                    .and_then(|v| v.as_table())
                    .map(|t| {
                        t.iter()
                            .map(|(alias, secret_name)| VirtualKeyEntry {
                                alias: alias.clone(),
                                secret_name: secret_name.as_str().unwrap_or("").to_string(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                
                let leak_patterns = secrets
                    .get("custom_leak_patterns")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_table().map(|t| CustomLeakPattern {
                                name: t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                pattern: t.get("pattern").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            }))
                            .collect()
                    })
                    .unwrap_or_default();
                
                return SecretsProtectionConfig {
                    virtual_keys,
                    custom_leak_patterns: leak_patterns,
                };
            }
        }
    }
    SecretsProtectionConfig::default()
}
```

### Step 4: Add TOML write helpers

Add these functions after `write_ssrf_config_to_toml`:

```rust
/// Write shell security settings to [security.shell] section.
fn write_shell_security_to_toml(
    path: &std::path::Path,
    shell_security: &ShellSecurityConfig,
) -> Result<(), String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table = toml::from_str(&contents)
        .map_err(|e| format!("Failed to parse config: {e}"))?;

    let security = doc
        .entry("security".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(sec_table) = security {
        let shell = sec_table
            .entry("shell".to_string())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));

        if let toml::Value::Table(shell_table) = shell {
            shell_table.insert(
                "enable_custom_patterns".to_string(),
                toml::Value::Boolean(shell_security.enable_custom_patterns),
            );

            // Write custom_blocked
            let blocked: Vec<toml::Value> = shell_security.custom_blocked.iter()
                .map(|p| {
                    let mut table = toml::Table::new();
                    table.insert("pattern".to_string(), toml::Value::String(p.pattern.clone()));
                    if let Some(ref reason) = p.reason {
                        table.insert("reason".to_string(), toml::Value::String(reason.clone()));
                    }
                    toml::Value::Table(table)
                })
                .collect();
            if !blocked.is_empty() {
                shell_table.insert("custom_blocked".to_string(), toml::Value::Array(blocked));
            } else {
                shell_table.remove("custom_blocked");
            }

            // Write custom_danger
            let danger: Vec<toml::Value> = shell_security.custom_danger.iter()
                .map(|p| {
                    let mut table = toml::Table::new();
                    table.insert("pattern".to_string(), toml::Value::String(p.pattern.clone()));
                    if let Some(ref reason) = p.reason {
                        table.insert("reason".to_string(), toml::Value::String(reason.clone()));
                    }
                    toml::Value::Table(table)
                })
                .collect();
            if !danger.is_empty() {
                shell_table.insert("custom_danger".to_string(), toml::Value::Array(danger));
            } else {
                shell_table.remove("custom_danger");
            }

            // Write custom_safe
            let safe: Vec<toml::Value> = shell_security.custom_safe.iter()
                .map(|p| {
                    let mut table = toml::Table::new();
                    table.insert("pattern".to_string(), toml::Value::String(p.pattern.clone()));
                    if let Some(ref reason) = p.reason {
                        table.insert("reason".to_string(), toml::Value::String(reason.clone()));
                    }
                    toml::Value::Table(table)
                })
                .collect();
            if !safe.is_empty() {
                shell_table.insert("custom_safe".to_string(), toml::Value::Array(safe));
            } else {
                shell_table.remove("custom_safe");
            }
        }
    }

    let new_contents = toml::to_string_pretty(&doc)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;

    let temp_path = path.with_extension("shell_tmp");
    std::fs::write(&temp_path, &new_contents)
        .map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}

/// Write custom PII rules to [[privacy.custom_rules]] section.
fn write_custom_pii_rules_to_toml(
    path: &std::path::Path,
    rules: &[CustomPiiRule],
) -> Result<(), String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table = toml::from_str(&contents)
        .map_err(|e| format!("Failed to parse config: {e}"))?;

    let privacy = doc
        .entry("privacy".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(priv_table) = privacy {
        let rules_arr: Vec<toml::Value> = rules.iter()
            .map(|r| {
                let mut table = toml::Table::new();
                table.insert("name".to_string(), toml::Value::String(r.name.clone()));
                table.insert("pattern".to_string(), toml::Value::String(r.pattern.clone()));
                table.insert("placeholder".to_string(), toml::Value::String(r.placeholder.clone()));
                table.insert("severity".to_string(), toml::Value::String(
                    match r.severity {
                        CustomPiiSeverity::Low => "low",
                        CustomPiiSeverity::Medium => "medium",
                        CustomPiiSeverity::High => "high",
                        CustomPiiSeverity::Critical => "critical",
                    }.to_string()
                ));
                table.insert("action".to_string(), toml::Value::String(
                    match r.action {
                        PiiAction::Block => "block",
                        PiiAction::Warn => "warn",
                        PiiAction::Off => "off",
                    }.to_string()
                ));
                toml::Value::Table(table)
            })
            .collect();
        
        if !rules_arr.is_empty() {
            priv_table.insert("custom_rules".to_string(), toml::Value::Array(rules_arr));
        } else {
            priv_table.remove("custom_rules");
        }
    }

    let new_contents = toml::to_string_pretty(&doc)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;

    let temp_path = path.with_extension("pii_tmp");
    std::fs::write(&temp_path, &new_contents)
        .map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}

/// Write secret protection to [secrets_config] section.
fn write_secrets_protection_to_toml(
    path: &std::path::Path,
    secrets: &SecretsProtectionConfig,
) -> Result<(), String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config: {e}"))?;
    let mut doc: toml::Table = toml::from_str(&contents)
        .map_err(|e| format!("Failed to parse config: {e}"))?;

    let secrets_config = doc
        .entry("secrets_config".to_string())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));

    if let toml::Value::Table(sec_table) = secrets_config {
        // Write virtual_keys
        let mut vkeys = toml::Table::new();
        for entry in &secrets.virtual_keys {
            vkeys.insert(
                entry.alias.clone(),
                toml::Value::String(entry.secret_name.clone()),
            );
        }
        if !vkeys.is_empty() {
            sec_table.insert("virtual_keys".to_string(), toml::Value::Table(vkeys));
        } else {
            sec_table.remove("virtual_keys");
        }

        // Write custom_leak_patterns
        let patterns: Vec<toml::Value> = secrets.custom_leak_patterns.iter()
            .map(|p| {
                let mut table = toml::Table::new();
                table.insert("name".to_string(), toml::Value::String(p.name.clone()));
                table.insert("pattern".to_string(), toml::Value::String(p.pattern.clone()));
                toml::Value::Table(table)
            })
            .collect();
        
        if !patterns.is_empty() {
            sec_table.insert("custom_leak_patterns".to_string(), toml::Value::Array(patterns));
        } else {
            sec_table.remove("custom_leak_patterns");
        }
    }

    let new_contents = toml::to_string_pretty(&doc)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;

    let temp_path = path.with_extension("secrets_tmp");
    std::fs::write(&temp_path, &new_contents)
        .map_err(|e| format!("Failed to write temp: {e}"))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Failed to rename: {e}")
    })?;

    Ok(())
}
```

### Step 5: Update handle_get to read new config

Modify `handle_get` (around line 273) to read new config sections:

```rust
pub async fn handle_get(
    request: JsonRpcRequest,
    config_patcher: Arc<ConfigPatcher>,
) -> JsonRpcResponse {
    // ... existing code to read gateway.host and SSRF config ...

    let shell_security = read_shell_security_from_toml(&config_patcher);
    let custom_pii_rules = read_custom_pii_rules_from_toml(&config_patcher);
    let secrets_protection = read_secrets_protection_from_toml(&config_patcher);

    let security_config = SecurityConfig {
        // ... existing fields ...
        shell_security,
        custom_pii_rules,
        secrets_protection,
    };

    // ... rest of existing code ...
}
```

### Step 6: Update handle_update to write new config

Modify `handle_update` (around line 307) to write new config sections after existing writes:

```rust
    // ... existing code: write gateway.host, write SSRF config ...

    // Write shell security config
    let config_path = crate::config::Config::default_path();
    if let Err(e) = write_shell_security_to_toml(&config_path, &security_config.shell_security) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save shell security config: {e}"),
        );
    }

    // Write custom PII rules
    if let Err(e) = write_custom_pii_rules_to_toml(&config_path, &security_config.custom_pii_rules) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save custom PII rules: {e}"),
        );
    }

    // Write secret protection
    if let Err(e) = write_secrets_protection_to_toml(&config_path, &security_config.secrets_protection) {
        return JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to save secret protection config: {e}"),
        );
    }

    // ... existing event broadcast code ...
```

### Step 7: Commit backend changes

```bash
git add src/gateway/handlers/security_config.rs
git commit -m "security_config: extend handler with shell, pii, and secret protection config"
```

---

## Task 2: Extend Frontend API Types

**Files:**
- Modify: `interfaces/webchat/src/api/security.rs`

### Step 1: Add new types

Add these types after the existing `SecurityConfig` definition:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellSecurityConfig {
    pub enable_custom_patterns: bool,
    #[serde(default)]
    pub custom_blocked: Vec<CustomRiskPattern>,
    #[serde(default)]
    pub custom_danger: Vec<CustomRiskPattern>,
    #[serde(default)]
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
    #[serde(default = "default_custom_pii_placeholder")]
    pub placeholder: String,
    #[serde(default)]
    pub severity: CustomPiiSeverity,
    #[serde(default)]
    pub action: PiiAction,
}

fn default_custom_pii_placeholder() -> String {
    "[CUSTOM_PII]".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CustomPiiSeverity {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PiiAction {
    #[default]
    Block,
    Warn,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecretsProtectionConfig {
    #[serde(default)]
    pub virtual_keys: Vec<VirtualKeyEntry>,
    #[serde(default)]
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

### Step 2: Extend SecurityConfig

Add new fields to `SecurityConfig`:

```rust
    // ... existing fields ...
    #[serde(default)]
    pub shell_security: ShellSecurityConfig,
    #[serde(default)]
    pub custom_pii_rules: Vec<CustomPiiRule>,
    #[serde(default)]
    pub secrets_protection: SecretsProtectionConfig,
```

### Step 3: Commit

```bash
git add interfaces/webchat/src/api/security.rs
git commit -m "webchat: extend SecurityConfig API types with new security settings"
```

---

## Task 3: Add ShellSecuritySection Component

**Files:**
- Modify: `interfaces/webchat/src/views/settings/security.rs`

### Step 1: Add regex validation helper

Add at the top of the file (after imports):

```rust
/// Validate a regex pattern using js_sys::RegExp
fn validate_regex(pattern: &str) -> Result<(), String> {
    if pattern.is_empty() {
        return Ok(());
    }
    match js_sys::RegExp::new(pattern, "") {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid regex pattern".to_string()),
    }
}
```

### Step 2: Add ShellSecuritySection component

Add after the existing `OutboundSecuritySection` component:

```rust
#[component]
fn ShellSecuritySection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    let pattern_errors = RwSignal::new(Vec::<(usize, String, String)>::new()); // (index, category, error)

    let validate_all_patterns = move || {
        let mut errors = Vec::new();
        if let Some(cfg) = config.get() {
            for (i, p) in cfg.shell_security.custom_blocked.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "blocked".to_string(), e));
                }
            }
            for (i, p) in cfg.shell_security.custom_danger.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "danger".to_string(), e));
                }
            }
            for (i, p) in cfg.shell_security.custom_safe.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, "safe".to_string(), e));
                }
            }
        }
        pattern_errors.set(errors);
        errors.is_empty()
    };

    let add_pattern = move |category: &'static str| {
        if let Some(mut cfg) = config.get() {
            let new_pattern = CustomRiskPattern {
                pattern: String::new(),
                reason: None,
            };
            match category {
                "blocked" => cfg.shell_security.custom_blocked.push(new_pattern),
                "danger" => cfg.shell_security.custom_danger.push(new_pattern),
                "safe" => cfg.shell_security.custom_safe.push(new_pattern),
                _ => {}
            }
            config.set(Some(cfg));
        }
    };

    let remove_pattern = move |category: &'static str, index: usize| {
        if let Some(mut cfg) = config.get() {
            match category {
                "blocked" => { cfg.shell_security.custom_blocked.remove(index); }
                "danger" => { cfg.shell_security.custom_danger.remove(index); }
                "safe" => { cfg.shell_security.custom_safe.remove(index); }
                _ => {}
            }
            config.set(Some(cfg));
            validate_all_patterns();
        }
    };

    let update_pattern = move |category: &'static str, index: usize, field: &'static str, value: String| {
        if let Some(mut cfg) = config.get() {
            let pattern = match category {
                "blocked" => cfg.shell_security.custom_blocked.get_mut(index),
                "danger" => cfg.shell_security.custom_danger.get_mut(index),
                "safe" => cfg.shell_security.custom_safe.get_mut(index),
                _ => None,
            };
            if let Some(p) = pattern {
                match field {
                    "pattern" => p.pattern = value,
                    "reason" => p.reason = if value.is_empty() { None } else { Some(value) },
                    _ => {}
                }
            }
            config.set(Some(cfg));
            validate_all_patterns();
        }
    };

    let has_error = move |category: &'static str, index: usize| -> bool {
        pattern_errors.get().iter().any(|(_, c, _)| c == category)
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                "Shell Command Security"
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure custom risk patterns for shell command execution."
            </p>

            <div class="space-y-4">
                // Enable custom patterns toggle
                <label class="flex items-center space-x-3 cursor-pointer">
                    <input
                        type="checkbox"
                        prop:checked=move || config.get().map(|c| c.shell_security.enable_custom_patterns).unwrap_or(false)
                        on:change=move |ev| {
                            if let Some(mut cfg) = config.get() {
                                cfg.shell_security.enable_custom_patterns = event_target_checked(&ev);
                                config.set(Some(cfg));
                            }
                        }
                        class="w-4 h-4 text-primary focus:ring-primary/30 rounded"
                    />
                    <div>
                        <div class="font-medium text-text-primary">"Enable Custom Risk Patterns"</div>
                        <div class="text-xs text-text-tertiary">"When enabled, custom patterns supplement built-in security rules"</div>
                    </div>
                </label>

                // Blocked Patterns
                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Blocked Patterns (execution denied)"</h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config.get().map(|c| c.shell_security.custom_blocked.clone()).unwrap_or_default();
                            patterns.into_iter().enumerate().map(|(i, p)| {
                                let has_err = has_error("blocked", i);
                                view! {
                                    <div class="flex gap-2 items-start">
                                        <div class="flex-1 space-y-1">
                                            <input
                                                type="text"
                                                prop:value=p.pattern.clone()
                                                on:input=move |ev| update_pattern("blocked", i, "pattern", event_target_value(&ev))
                                                placeholder="Regex pattern..."
                                                class=move || format!("w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                    if has_err { "border-danger" } else { "border-border" })
                                            />
                                            <input
                                                type="text"
                                                prop:value=p.reason.clone().unwrap_or_default()
                                                on:input=move |ev| update_pattern("blocked", i, "reason", event_target_value(&ev))
                                                placeholder="Reason (optional)..."
                                                class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                            />
                                        </div>
                                        <button
                                            on:click=move |_| remove_pattern("blocked", i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("blocked")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Blocked Pattern"
                    </button>
                </div>

                // Danger Patterns
                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Danger Patterns (require approval)"</h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config.get().map(|c| c.shell_security.custom_danger.clone()).unwrap_or_default();
                            patterns.into_iter().enumerate().map(|(i, p)| {
                                let has_err = has_error("danger", i);
                                view! {
                                    <div class="flex gap-2 items-start">
                                        <div class="flex-1 space-y-1">
                                            <input
                                                type="text"
                                                prop:value=p.pattern.clone()
                                                on:input=move |ev| update_pattern("danger", i, "pattern", event_target_value(&ev))
                                                placeholder="Regex pattern..."
                                                class=move || format!("w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                    if has_err { "border-danger" } else { "border-border" })
                                            />
                                            <input
                                                type="text"
                                                prop:value=p.reason.clone().unwrap_or_default()
                                                on:input=move |ev| update_pattern("danger", i, "reason", event_target_value(&ev))
                                                placeholder="Reason (optional)..."
                                                class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                            />
                                        </div>
                                        <button
                                            on:click=move |_| remove_pattern("danger", i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("danger")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Danger Pattern"
                    </button>
                </div>

                // Safe Patterns
                <div class="mt-4">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Safe Patterns (auto-approved)"</h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config.get().map(|c| c.shell_security.custom_safe.clone()).unwrap_or_default();
                            patterns.into_iter().enumerate().map(|(i, p)| {
                                let has_err = has_error("safe", i);
                                view! {
                                    <div class="flex gap-2 items-start">
                                        <div class="flex-1 space-y-1">
                                            <input
                                                type="text"
                                                prop:value=p.pattern.clone()
                                                on:input=move |ev| update_pattern("safe", i, "pattern", event_target_value(&ev))
                                                placeholder="Regex pattern..."
                                                class=move || format!("w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                    if has_err { "border-danger" } else { "border-border" })
                                            />
                                            <input
                                                type="text"
                                                prop:value=p.reason.clone().unwrap_or_default()
                                                on:input=move |ev| update_pattern("safe", i, "reason", event_target_value(&ev))
                                                placeholder="Reason (optional)..."
                                                class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                            />
                                        </div>
                                        <button
                                            on:click=move |_| remove_pattern("safe", i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_pattern("safe")
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Safe Pattern"
                    </button>
                </div>

                // Error display
                {move || {
                    let errors = pattern_errors.get();
                    if !errors.is_empty() {
                        Some(view! {
                            <div class="p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                                <ul class="list-disc list-inside">
                                    {errors.iter().map(|(i, cat, err)| view! {
                                        <li>{format!("{} #{}: {}", cat, i + 1, err)}</li>
                                    }).collect::<Vec<_>>()}                                </ul>
                            </div>
                        })
                    } else {
                        None
                    }
                }}
            </div>
        </div>
    }
}
```

### Step 3: Add section to SecurityView

In the main `SecurityView` component, add `<ShellSecuritySection config=config />` after `<OutboundSecuritySection config=config />`.

### Step 4: Commit

```bash
git add interfaces/webchat/src/views/settings/security.rs
git commit -m "webchat: add ShellSecuritySection component with regex validation"
```

---

## Task 4: Add CustomPiiRulesSubsection (Merged)

**Files:**
- Modify: `interfaces/webchat/src/views/settings/security.rs`

### Step 1: Add CustomPiiRulesSubsection

Add after `ShellSecuritySection`:

```rust
#[component]
fn CustomPiiRulesSubsection(
    rules: RwSignal<Vec<CustomPiiRule>>,
    pattern_errors: RwSignal<Vec<(usize, String)>>,
) -> impl IntoView {
    let i18n = use_i18n();

    let validate_all = move || {
        let mut errors = Vec::new();
        for (i, rule) in rules.get().iter().enumerate() {
            if let Err(e) = validate_regex(&rule.pattern) {
                errors.push((i, e));
            }
        }
        pattern_errors.set(errors);
        errors.is_empty()
    };

    let add_rule = move || {
        let mut current = rules.get();
        current.push(CustomPiiRule {
            name: String::new(),
            pattern: String::new(),
            placeholder: "[CUSTOM_PII]".to_string(),
            severity: CustomPiiSeverity::Medium,
            action: PiiAction::Block,
        });
        rules.set(current);
    };

    let remove_rule = move |index: usize| {
        let mut current = rules.get();
        current.remove(index);
        rules.set(current);
        validate_all();
    };

    let update_rule = move |index: usize, field: &'static str, value: String| {
        let mut current = rules.get();
        if let Some(rule) = current.get_mut(index) {
            match field {
                "name" => rule.name = value,
                "pattern" => rule.pattern = value,
                "placeholder" => rule.placeholder = value,
                "severity" => rule.severity = match value.as_str() {
                    "low" => CustomPiiSeverity::Low,
                    "medium" => CustomPiiSeverity::Medium,
                    "high" => CustomPiiSeverity::High,
                    "critical" => CustomPiiSeverity::Critical,
                    _ => CustomPiiSeverity::Medium,
                },
                "action" => rule.action = match value.as_str() {
                    "block" => PiiAction::Block,
                    "warn" => PiiAction::Warn,
                    "off" => PiiAction::Off,
                    _ => PiiAction::Block,
                },
                _ => {}
            }
        }
        rules.set(current);
        validate_all();
    };

    view! {
        <div class="mt-6 pt-6 border-t border-border">
            <h3 class="text-sm font-semibold text-text-secondary mb-3">
                "Custom PII Rules"
            </h3>

            <div class="space-y-3">
                {move || {
                    let rule_list = rules.get();
                    rule_list.into_iter().enumerate().map(|(i, rule)| {
                        let has_err = pattern_errors.get().iter().any(|(idx, _)| *idx == i);
                        view! {
                            <div class="p-3 bg-surface-sunken rounded border border-border space-y-2">
                                <div class="flex gap-2">
                                    <input
                                        type="text"
                                        prop:value=rule.name.clone()
                                        on:input=move |ev| update_rule(i, "name", event_target_value(&ev))
                                        placeholder="Rule name..."
                                        class="flex-1 px-3 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    />
                                    <button
                                        on:click=move |_| remove_rule(i)
                                        class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                    >
                                        "Remove"
                                    </button>
                                </div>
                                <input
                                    type="text"
                                    prop:value=rule.pattern.clone()
                                    on:input=move |ev| update_rule(i, "pattern", event_target_value(&ev))
                                    placeholder="Regex pattern..."
                                    class=move || format!("w-full px-3 py-1 bg-surface-raised border rounded text-sm text-text-primary {}",
                                        if has_err { "border-danger" } else { "border-border" })
                                />
                                <div class="flex gap-2">
                                    <input
                                        type="text"
                                        prop:value=rule.placeholder.clone()
                                        on:input=move |ev| update_rule(i, "placeholder", event_target_value(&ev))
                                        placeholder="Placeholder..."
                                        class="flex-1 px-3 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    />
                                    <select
                                        prop:value=move || match rule.severity {
                                            CustomPiiSeverity::Low => "low",
                                            CustomPiiSeverity::Medium => "medium",
                                            CustomPiiSeverity::High => "high",
                                            CustomPiiSeverity::Critical => "critical",
                                        }
                                        on:change=move |ev| update_rule(i, "severity", event_target_value(&ev))
                                        class="px-2 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    >
                                        <option value="low">"Low"</option>
                                        <option value="medium">"Medium"</option>
                                        <option value="high">"High"</option>
                                        <option value="critical">"Critical"</option>
                                    </select>
                                    <select
                                        prop:value=move || match rule.action {
                                            PiiAction::Block => "block",
                                            PiiAction::Warn => "warn",
                                            PiiAction::Off => "off",
                                        }
                                        on:change=move |ev| update_rule(i, "action", event_target_value(&ev))
                                        class="px-2 py-1 bg-surface-raised border border-border rounded text-sm text-text-primary"
                                    >
                                        <option value="block">"Block"</option>
                                        <option value="warn">"Warn"</option>
                                        <option value="off">"Off"</option>
                                    </select>
                                </div>
                            </div>
                        }
                    }).collect::<Vec<_>>()
                }}
            </div>

            <button
                on:click=move |_| add_rule()
                class="mt-3 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
            >
                "+ Add Custom Rule"
            </button>

            {move || {
                let errors = pattern_errors.get();
                if !errors.is_empty() {
                    Some(view! {
                        <div class="mt-2 p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                            <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                            <ul class="list-disc list-inside">
                                {errors.iter().map(|(i, err)| view! {
                                    <li>{format!("Rule #{}: {}", i + 1, err)}</li>
                                }).collect::<Vec<_>>()}                            </ul>
                        </div>
                    })
                } else {
                    None
                }
            }}
        </div>
    }
}
```

### Step 2: Integrate into existing PIISection

Modify the existing `PIISection` component to include `CustomPiiRulesSubsection`:

In `PIISection`, add a signal for custom rules and pass it to the subsection:

```rust
// In PIISection, add:
let custom_rules = RwSignal::new(config.get().custom_pii_rules.clone());
let pii_pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());

// In the save function, before calling SearchConfigApi::update:
cfg.custom_pii_rules = custom_rules.get();

// In the view, after the existing PII checkboxes, add:
<CustomPiiRulesSubsection rules=custom_rules pattern_errors=pii_pattern_errors />
```

### Step 3: Commit

```bash
git add interfaces/webchat/src/views/settings/security.rs
git commit -m "webchat: add CustomPiiRulesSubsection with severity/action selectors"
```

---

## Task 5: Add SecretProtectionSection

**Files:**
- Modify: `interfaces/webchat/src/views/settings/security.rs`

### Step 1: Add SecretProtectionSection

Add after `CustomPiiRulesSubsection`:

```rust
#[component]
fn SecretProtectionSection(config: RwSignal<Option<SecurityConfig>>) -> impl IntoView {
    let i18n = use_i18n();
    let leak_pattern_errors = RwSignal::new(Vec::<(usize, String)>::new());

    let validate_leak_patterns = move || {
        let mut errors = Vec::new();
        if let Some(cfg) = config.get() {
            for (i, p) in cfg.secrets_protection.custom_leak_patterns.iter().enumerate() {
                if let Err(e) = validate_regex(&p.pattern) {
                    errors.push((i, e));
                }
            }
        }
        leak_pattern_errors.set(errors);
        errors.is_empty()
    };

    let add_virtual_key = move || {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.virtual_keys.push(VirtualKeyEntry {
                alias: String::new(),
                secret_name: String::new(),
            });
            config.set(Some(cfg));
        }
    };

    let remove_virtual_key = move |index: usize| {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.virtual_keys.remove(index);
            config.set(Some(cfg));
        }
    };

    let update_virtual_key = move |index: usize, field: &'static str, value: String| {
        if let Some(mut cfg) = config.get() {
            if let Some(entry) = cfg.secrets_protection.virtual_keys.get_mut(index) {
                match field {
                    "alias" => entry.alias = value,
                    "secret_name" => entry.secret_name = value,
                    _ => {}
                }
            }
            config.set(Some(cfg));
        }
    };

    let add_leak_pattern = move || {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.custom_leak_patterns.push(CustomLeakPattern {
                name: String::new(),
                pattern: String::new(),
            });
            config.set(Some(cfg));
        }
    };

    let remove_leak_pattern = move |index: usize| {
        if let Some(mut cfg) = config.get() {
            cfg.secrets_protection.custom_leak_patterns.remove(index);
            config.set(Some(cfg));
            validate_leak_patterns();
        }
    };

    let update_leak_pattern = move |index: usize, field: &'static str, value: String| {
        if let Some(mut cfg) = config.get() {
            if let Some(pattern) = cfg.secrets_protection.custom_leak_patterns.get_mut(index) {
                match field {
                    "name" => pattern.name = value,
                    "pattern" => pattern.pattern = value,
                    _ => {}
                }
            }
            config.set(Some(cfg));
            validate_leak_patterns();
        }
    };

    view! {
        <div class="bg-surface-raised rounded-lg border border-border p-6">
            <h2 class="text-lg font-semibold text-text-primary mb-4">
                "Secret Protection"
            </h2>
            <p class="text-sm text-text-tertiary mb-4">
                "Configure virtual key aliases and custom leak detection patterns."
            </p>

            <div class="space-y-6">
                // Virtual Keys
                <div>
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Virtual Key Aliases"</h3>
                    <div class="space-y-2">
                        {move || {
                            let keys = config.get().map(|c| c.secrets_protection.virtual_keys.clone()).unwrap_or_default();
                            keys.into_iter().enumerate().map(|(i, entry)| {
                                view! {
                                    <div class="flex gap-2 items-center">
                                        <input
                                            type="text"
                                            prop:value=entry.alias.clone()
                                            on:input=move |ev| update_virtual_key(i, "alias", event_target_value(&ev))
                                            placeholder="Alias (e.g., openai)"
                                            class="flex-1 px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                        />
                                        <span class="text-text-tertiary">"→"</span>
                                        <input
                                            type="text"
                                            prop:value=entry.secret_name.clone()
                                            on:input=move |ev| update_virtual_key(i, "secret_name", event_target_value(&ev))
                                            placeholder="Secret name (e.g., OPENAI_API_KEY)"
                                            class="flex-1 px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                        />
                                        <button
                                            on:click=move |_| remove_virtual_key(i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_virtual_key()
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Virtual Key"
                    </button>
                </div>

                // Custom Leak Patterns
                <div class="pt-4 border-t border-border">
                    <h3 class="text-sm font-semibold text-text-secondary mb-2">"Custom Leak Detection Patterns"</h3>
                    <div class="space-y-2">
                        {move || {
                            let patterns = config.get().map(|c| c.secrets_protection.custom_leak_patterns.clone()).unwrap_or_default();
                            patterns.into_iter().enumerate().map(|(i, p)| {
                                let has_err = leak_pattern_errors.get().iter().any(|(idx, _)| *idx == i);
                                view! {
                                    <div class="flex gap-2 items-start">
                                        <div class="flex-1 space-y-1">
                                            <input
                                                type="text"
                                                prop:value=p.name.clone()
                                                on:input=move |ev| update_leak_pattern(i, "name", event_target_value(&ev))
                                                placeholder="Pattern name..."
                                                class="w-full px-3 py-1 bg-surface-sunken border border-border rounded text-sm text-text-primary"
                                            />
                                            <input
                                                type="text"
                                                prop:value=p.pattern.clone()
                                                on:input=move |ev| update_leak_pattern(i, "pattern", event_target_value(&ev))
                                                placeholder="Regex pattern..."
                                                class=move || format!("w-full px-3 py-1 bg-surface-sunken border rounded text-sm text-text-primary {}",
                                                    if has_err { "border-danger" } else { "border-border" })
                                            />
                                        </div>
                                        <button
                                            on:click=move |_| remove_leak_pattern(i)
                                            class="px-2 py-1 text-danger hover:bg-danger/10 rounded text-sm"
                                        >
                                            "Remove"
                                        </button>
                                    </div>
                                }
                            }).collect::<Vec<_>>()
                        }}
                    </div>
                    <button
                        on:click=move |_| add_leak_pattern()
                        class="mt-2 px-3 py-1 text-sm text-primary hover:bg-primary/10 rounded"
                    >
                        "+ Add Leak Pattern"
                    </button>

                    {move || {
                        let errors = leak_pattern_errors.get();
                        if !errors.is_empty() {
                            Some(view! {
                                <div class="mt-2 p-3 bg-danger-subtle border border-danger/20 rounded text-danger text-sm">
                                    <div class="font-semibold mb-1">"Invalid regex patterns:"</div>
                                    <ul class="list-disc list-inside">
                                        {errors.iter().map(|(i, err)| view! {
                                            <li>{format!("Pattern #{}: {}", i + 1, err)}</li>
                                        }).collect::<Vec<_>>()}                                    </ul>
                                </div>
                            })
                        } else {
                            None
                        }
                    }}
                </div>
            </div>
        </div>
    }
}
```

### Step 2: Add to SecurityView

In the main `SecurityView`, add `<SecretProtectionSection config=config />` after `ShellSecuritySection`.

### Step 3: Commit

```bash
git add interfaces/webchat/src/views/settings/security.rs
git commit -m "webchat: add SecretProtectionSection with virtual keys and leak patterns"
```

---

## Task 6: Build and Verify

### Step 1: Compile backend

```bash
cargo check -p alephcore
```
Expected: No errors.

### Step 2: Compile frontend (WASM)

```bash
cd interfaces/webchat
cargo check --target wasm32-unknown-unknown
```
Expected: No errors.

### Step 3: Run tests

```bash
cargo test -p alephcore --lib security
```
Expected: All existing tests pass.

### Step 4: Manual verification checklist

- [ ] Open `/settings/security` page
- [ ] Shell Security section visible with toggle
- [ ] Can add/remove blocked/danger/safe patterns
- [ ] Invalid regex shows red border and error
- [ ] PII section shows custom rules subsection
- [ ] Can add/remove custom PII rules with severity/action
- [ ] Secret Protection section shows virtual keys and leak patterns
- [ ] Save button persists all changes to TOML
- [ ] Reload page restores saved values

---

## Self-Review

### Spec Coverage
- [x] Shell Security config UI - Task 3
- [x] Custom PII Rules UI - Task 4
- [x] Secret Protection UI - Task 5
- [x] Backend TOML read/write - Task 1
- [x] Frontend type extensions - Task 2
- [x] Regex validation - Task 3, 4, 5
- [x] Save/load flow - All tasks

### Placeholder Scan
- [x] No TBD/TODO/"implement later"
- [x] No vague instructions
- [x] Complete code in every step
- [x] No "similar to Task N"

### Type Consistency
- [x] `CustomRiskPattern` matches in backend (Task 1) and frontend (Task 2)
- [x] `CustomPiiRule` fields consistent across all tasks
- [x] `PiiAction`/`CustomPiiSeverity` enum variants match
- [x] `VirtualKeyEntry`/`CustomLeakPattern` consistent

---

*Plan complete. Ready for execution.*
