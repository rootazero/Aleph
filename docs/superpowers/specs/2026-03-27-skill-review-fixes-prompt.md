# Skill System Review Fixes — 4 Important Issues

以下 4 个问题来自 Skill System Unification 的 code review，均为 Important 级别。请逐一修复，每个修复后运行 `cargo check -p alephcore && cargo test -p alephcore --lib skill` 验证。

---

## Issue 1: Shell 注入加固

**文件:** `src/skill/installer.rs` — `is_safe_shell_arg()` 函数（约第 12-16 行）

**问题:** 当前只拦截了 `;|&\`$(` 等，但未拦截空格和 `$VAR`（不带括号的环境变量展开）。`Download` 变体的 `curl -o` 路径也没有验证。

**修复:**

1. 在 `is_safe_shell_arg` 中增加检查：
```rust
fn is_safe_shell_arg(s: &str) -> bool {
    !s.is_empty()
        && !s.contains(';') && !s.contains('|') && !s.contains('&')
        && !s.contains('`') && !s.contains("$(")
        && !s.contains('$')    // 新增：拦截 $VAR 展开
        && !s.contains(' ')    // 新增：拦截空格导致参数分裂
        && !s.contains('\n') && !s.contains('\r')
}
```

2. `Download` 变体增加路径验证 — 拒绝包含 `..` 的路径：
```rust
InstallKind::Download => {
    spec.url.as_ref().and_then(|url| {
        if !is_safe_shell_arg(url) || spec.package.contains("..") {
            return None;
        }
        Some(format!("curl -fsSL -o {} {}", spec.package, url))
    })
}
```

3. 补充测试：
```rust
#[test]
fn rejects_space_in_package() {
    let spec = InstallSpec { kind: InstallKind::Brew, package: "foo bar".into(), ..default_spec() };
    assert!(build_install_command(&spec).is_none());
}

#[test]
fn rejects_dollar_in_package() {
    let spec = InstallSpec { kind: InstallKind::Brew, package: "$HOME".into(), ..default_spec() };
    assert!(build_install_command(&spec).is_none());
}

#[test]
fn rejects_path_traversal_in_download() {
    let spec = InstallSpec {
        kind: InstallKind::Download,
        package: "../../etc/passwd".into(),
        url: Some("https://example.com/file".into()),
        ..default_spec()
    };
    assert!(build_install_command(&spec).is_none());
}
```

---

## Issue 2: 双重 rebuild_snapshot

**文件:** `src/gateway/handlers/skills.rs` — `handle_update()` 和 `src/skill/mod.rs` — `update_config()`

**问题:** 当 `skills.update` 同时设置 `enabled` 和 `scope` 时，`update_config` 被调用两次，每次都触发 `rebuild_snapshot()`，造成不必要的双重重建。

**修复:**

1. 在 `src/skill/config.rs` 中，将 `SkillConfigUpdate` 改为支持批量更新：
```rust
#[derive(Debug, Default)]
pub struct SkillConfigUpdate {
    pub enabled: Option<bool>,
    pub scope: Option<PromptScope>,
}
```

2. 更新 `apply_update` 方法：
```rust
pub fn apply_update(&mut self, id: &SkillId, update: SkillConfigUpdate) {
    let entry = self.entries.entry(id.as_str().to_string()).or_default();
    if let Some(enabled) = update.enabled {
        entry.enabled = Some(enabled);
    }
    if let Some(scope) = update.scope {
        entry.scope_override = Some(scope);
    }
}
```

3. 更新 `handle_update` RPC handler，构建一个 `SkillConfigUpdate` 一次性传入：
```rust
let mut update = SkillConfigUpdate::default();
if let Some(enabled) = params.enabled {
    update.enabled = Some(enabled);
}
if let Some(scope_str) = &params.scope {
    update.scope = Some(parse_scope(scope_str)?);
}
system.update_config(&skill_id, update).await?;
```

4. 更新所有其他 `SkillConfigUpdate` 的调用方（LLM Tools 中的 `skill_manage.rs`、`status.rs` build 等）适配新的 struct 形式。

---

## Issue 3: `required_config` 检查未实现

**文件:** `src/skill/eligibility.rs` — `evaluate_spec()` 方法（约第 117-123 行）

**问题:** `required_config` 声明的配置键从未被检查，skills 总是被标记为 eligible，即使需要的配置不存在。`MissingRequirements.config` 始终为空。

**修复:**

由于完整的配置系统检查需要传入 config store，最小修复是：将所有 `required_config` 键标记为 missing，让 UI 正确显示"Needs Setup"：

```rust
// In evaluate_spec(), replace the debug skip with:
for key in &spec.required_config {
    // Config system not yet wired — conservatively mark as missing
    // so UI surfaces them as "Needs Setup" rather than silently passing
    reasons.push(IneligibilityReason::MissingConfig(key.clone()));
}
```

这样做是保守的（宁可误报 missing 也不漏报），符合 P7 防御性设计。等配置系统真正 wire 进来后再改为实际检查。

补充测试：
```rust
#[test]
fn required_config_reported_as_missing() {
    let spec = EligibilitySpec {
        required_config: vec!["api.endpoint".to_string()],
        ..Default::default()
    };
    let service = EligibilityService::new();
    let manifest = make_manifest_with_spec(spec);
    let result = service.evaluate(&manifest);
    assert!(!result.is_eligible());
}
```

---

## Issue 4: `api_key_set` 硬编码 false

**文件:** `src/skill/mod.rs` — `full_status()` 方法（约第 268-271 行）

**问题:** `api_key_set` 始终为 `false`，导致有 `primary_env` 的技能永远显示"API key missing"。

**修复:**

1. 给 `SkillSystem` 的 `Inner` 添加一个可选的 Vault 引用：
```rust
struct Inner {
    // ... existing fields ...
    vault: Option<Arc<crate::gateway::security::SharedTokenManager>>,
}
```

2. 添加 `with_vault()` builder 方法：
```rust
pub fn with_vault(&self, vault: Arc<SharedTokenManager>) {
    // Store vault reference — needs a separate field or use OnceLock
}
```

或者更简单的方式 — 给 `full_status` 添加一个 vault 参数：
```rust
pub async fn full_status_with_vault(
    &self,
    vault: Option<&SharedTokenManager>,
) -> Vec<SkillStatusEntry> {
    // ... same logic but check vault for api_key_set:
    let api_key_set = vault
        .and_then(|v| manifest.primary_env())
        .map(|env| {
            matches!(v.get_secret(&format!("skill:{}", manifest.id().as_str())), Ok(Some(_)))
        })
        .unwrap_or(false);
}
```

3. 更新 RPC handler `handle_status` 传入 vault：
```rust
// In shared_system() 或 handle_status 中获取 vault
// 如果 shared_system 模式不方便传 vault，可以在 handle_status 中单独创建 SharedTokenManager
```

注意：`SharedTokenManager` 的获取方式取决于 Gateway 的 wiring。最简单的做法是保留 `full_status()` 不变（无 vault），新增 `full_status_with_vault()` 供 RPC handler 使用。不传 vault 时 `api_key_set` 默认 false（CLI 等场景可接受）。

---

## 验证

每个 issue 修完后运行：
```bash
cargo check -p alephcore
cargo test -p alephcore --lib skill
cargo test -p alephcore --lib installer  # Issue 1
cargo test -p alephcore --lib eligibility  # Issue 3
```

全部修完后运行：
```bash
cargo test -p alephcore --lib
cargo check  # 包含 Panel
```
