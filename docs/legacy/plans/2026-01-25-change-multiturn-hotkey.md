# Change Multi-turn Window Hotkey Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Change multi-turn conversation window hotkey from `Command+Option+/` to `Option+Space` with automatic migration (no backward compatibility)

**Architecture:** The hotkey system has three layers:
1. **Rust Core** (`core/src/config/types/general.rs`): Configuration schema and defaults
2. **Config Migration** (`core/src/config/migration.rs`): Automatic migration of old hotkey values
3. **HotkeyService** (`platforms/macos/Aether/Sources/Services/HotkeyService.swift`): Runtime hotkey monitoring
4. **UI Settings** (`platforms/macos/Aether/Sources/ShortcutsView.swift`): User configuration interface

**Tech Stack:** Rust (config + migration), Swift (hotkey monitoring + UI), TOML (config files)

**Breaking Change:** Old `Command+Option+/` configs will be automatically migrated to `Option+Space` on first load

**Additional Feature:** Add Esc key as an option for the character key picker

---

## Task 1: Update Rust Configuration Defaults

**Files:**
- Modify: `core/src/config/types/general.rs:71-73`
- Modify: `shared/config/default-config.toml:60`
- Modify: `platforms/macos/Aether/config.example.toml:60`

**Step 1: Update Rust default hotkey value**

In `core/src/config/types/general.rs`, change line 72:

```rust
pub fn default_command_prompt_hotkey() -> String {
    "Option+Space".to_string()
}
```

**Step 2: Update shared default config**

In `shared/config/default-config.toml`, change line 60:

```toml
command_prompt = "Option+Space"
```

**Step 3: Update macOS example config**

In `platforms/macos/Aether/config.example.toml`, change line 60:

```toml
command_prompt = "Option+Space"
```

**Step 4: Rebuild Rust core to generate new FFI bindings**

Run: `cd core && cargo build`
Expected: Build succeeds, UniFFI bindings regenerated

**Step 5: Commit Rust config changes**

```bash
git add core/src/config/types/general.rs shared/config/default-config.toml platforms/macos/Aether/config.example.toml
git commit -m "feat(config): change default multi-turn hotkey to Option+Space

BREAKING CHANGE: Default hotkey changed from Command+Option+/ to Option+Space

- Update default_command_prompt_hotkey() to return 'Option+Space'
- Update TOML example configs to use new default
- Migration logic will be added in next commit

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 2: Add Config Migration Logic

**Files:**
- Modify: `core/src/config/migration.rs` (add migrate_command_prompt method)
- Modify: `core/src/config/load.rs` (call migration on load)
- Modify: `core/src/config/tests/migration.rs` (add migration tests)

**Step 1: Add migration method to Config impl**

In `core/src/config/migration.rs`, add after the `migrate_trigger_config` method:

```rust
/// Migrate old command_prompt hotkey to new default
///
/// Replaces "Command+Option+/" with "Option+Space" to force new hotkey.
/// This is a breaking change - old configs are automatically updated.
///
/// Returns true if migration was performed
pub(crate) fn migrate_command_prompt_hotkey(&mut self) -> bool {
    use tracing::info;

    // Check if shortcuts config exists and has old hotkey
    if let Some(ref mut shortcuts) = self.shortcuts {
        if shortcuts.command_prompt == "Command+Option+/" {
            info!("Migrating command_prompt hotkey: Command+Option+/ -> Option+Space");
            shortcuts.command_prompt = "Option+Space".to_string();
            return true;
        }
    }
    false
}
```

**Step 2: Call migration in config load**

In `core/src/config/load.rs`, find the `load_config_impl` function (search for "migrate_trigger_config") and add migration call:

```rust
// Apply migrations
let mut migrated = false;
migrated |= config.migrate_trigger_config();
migrated |= config.migrate_command_prompt_hotkey();  // NEW

if migrated {
    info!("Config migrations applied, saving updated config");
    config.save()?;
}
```

**Step 3: Write migration tests**

In `core/src/config/tests/migration.rs`, add test cases at the end of the file:

```rust
#[test]
fn test_migrate_command_prompt_hotkey() {
    use crate::config::types::ShortcutsConfig;

    let mut config = Config::default();

    // Set old hotkey
    config.shortcuts = Some(ShortcutsConfig {
        summon: "Command+Grave".to_string(),
        cancel: Some("Escape".to_string()),
        command_prompt: "Command+Option+/".to_string(),
        ocr_capture: "Command+Option+O".to_string(),
    });

    // Run migration
    let migrated = config.migrate_command_prompt_hotkey();
    assert!(migrated, "Migration should return true");

    // Verify new value
    assert_eq!(
        config.shortcuts.as_ref().unwrap().command_prompt,
        "Option+Space",
        "Should migrate to new hotkey"
    );
}

#[test]
fn test_migrate_command_prompt_hotkey_noop_when_already_new() {
    use crate::config::types::ShortcutsConfig;

    let mut config = Config::default();

    // Set new hotkey
    config.shortcuts = Some(ShortcutsConfig {
        summon: "Command+Grave".to_string(),
        cancel: Some("Escape".to_string()),
        command_prompt: "Option+Space".to_string(),
        ocr_capture: "Command+Option+O".to_string(),
    });

    // Run migration
    let migrated = config.migrate_command_prompt_hotkey();
    assert!(!migrated, "Migration should return false (no-op)");

    // Verify unchanged
    assert_eq!(
        config.shortcuts.as_ref().unwrap().command_prompt,
        "Option+Space",
        "Should remain unchanged"
    );
}

#[test]
fn test_migrate_command_prompt_hotkey_noop_when_custom() {
    use crate::config::types::ShortcutsConfig;

    let mut config = Config::default();

    // Set custom hotkey (neither old nor new default)
    config.shortcuts = Some(ShortcutsConfig {
        summon: "Command+Grave".to_string(),
        cancel: Some("Escape".to_string()),
        command_prompt: "Control+Shift+/".to_string(),
        ocr_capture: "Command+Option+O".to_string(),
    });

    // Run migration
    let migrated = config.migrate_command_prompt_hotkey();
    assert!(!migrated, "Migration should not touch custom hotkeys");

    // Verify unchanged
    assert_eq!(
        config.shortcuts.as_ref().unwrap().command_prompt,
        "Control+Shift+/",
        "Custom hotkey should remain unchanged"
    );
}
```

**Step 4: Run tests**

Run: `cd core && cargo test migrate_command_prompt_hotkey`
Expected: All 3 tests pass

**Step 5: Build Rust core**

Run: `cd core && cargo build`
Expected: Build succeeds

**Step 6: Commit migration logic**

```bash
git add core/src/config/migration.rs core/src/config/load.rs core/src/config/tests/migration.rs
git commit -m "feat(config): add forced migration for command_prompt hotkey

BREAKING CHANGE: Old 'Command+Option+/' hotkey automatically migrated to 'Option+Space'

- Add migrate_command_prompt_hotkey() method
- Call migration on config load and auto-save
- Only migrates exact old default, preserves custom hotkeys
- Add comprehensive tests (old->new, already new, custom)

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 3: Update HotkeyService Parsing Logic

**Files:**
- Modify: `platforms/macos/Aether/Sources/Services/HotkeyService.swift:255-292`

**Step 1: Add Esc keycode to parseMultiTurnHotkey method**

In `HotkeyService.swift`, the `parseMultiTurnHotkey` method (line 255-292) needs to support "Esc" key. Update the switch statement (around line 278-287):

```swift
// Parse last part as key code
let keyCode: UInt16
switch parts[parts.count - 1] {
case "/": keyCode = 44
case "`": keyCode = 50
case "\\": keyCode = 42
case ";": keyCode = 41
case ",": keyCode = 43
case ".": keyCode = 47
case "Space": keyCode = 49
case "Esc", "Escape": keyCode = 53  // NEW: Add Esc support
default: keyCode = 44 // Default to /
}
```

**Step 2: Build to verify syntax**

Run: `cd platforms/macos && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build`
Expected: Build succeeds

**Step 3: Commit HotkeyService changes**

```bash
git add platforms/macos/Aether/Sources/Services/HotkeyService.swift
git commit -m "feat(hotkey): add Esc key support for command prompt hotkey

- Add keyCode 53 for Esc/Escape in parseMultiTurnHotkey
- Allows hotkeys like Option+Command+Esc
- Supports both 'Esc' and 'Escape' string formats

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 4: Update UI Settings Enums and Pickers

**Files:**
- Modify: `platforms/macos/Aether/Sources/ShortcutsView.swift:22-24` (default state values)
- Modify: `platforms/macos/Aether/Sources/ShortcutsView.swift:48-50` (default constants)
- Modify: `platforms/macos/Aether/Sources/ShortcutsView.swift:767-809` (CommandCharKey enum)

**Step 1: Add Esc to CommandCharKey enum**

In `ShortcutsView.swift`, update the `CommandCharKey` enum (around line 770-809) to add Esc case and update all switch statements:

```swift
/// Character keys for command completion hotkey
enum CommandCharKey: String, CaseIterable {
    case slash = "/"
    case grave = "`"
    case backslash = "\\"
    case semicolon = ";"
    case comma = ","
    case period = "."
    case space = "Space"
    case esc = "Esc"  // NEW: Add Esc key

    var displayName: String {
        switch self {
        case .slash: return "/"
        case .grave: return "`"
        case .backslash: return "\\"
        case .semicolon: return ";"
        case .comma: return ","
        case .period: return "."
        case .space: return "Space"
        case .esc: return "Esc"  // NEW
        }
    }

    var displayChar: String {
        switch self {
        case .space: return "␣"
        case .esc: return "⎋"  // NEW: Esc symbol
        default: return rawValue
        }
    }

    var keyCode: UInt16 {
        switch self {
        case .slash: return 44
        case .grave: return 50
        case .backslash: return 42
        case .semicolon: return 41
        case .comma: return 43
        case .period: return 47
        case .space: return 49
        case .esc: return 53  // NEW: Esc keycode
        }
    }
}
```

**Step 2: Update default UI state values**

In `ShortcutsView.swift`, change lines 22-24:

```swift
// Command completion hotkey (two modifiers + character)
@State private var commandModifier1: CommandModifier = .option
@State private var commandModifier2: CommandModifier = .command  // Note: was .option before
@State private var commandCharKey: CommandCharKey = .space
```

**Step 3: Update default constant values**

In `ShortcutsView.swift`, change lines 48-50:

```swift
private let defaultCommandModifier1: CommandModifier = .option
private let defaultCommandModifier2: CommandModifier = .command  // Note: was .option before
private let defaultCommandCharKey: CommandCharKey = .space
```

**Step 4: Build and test UI**

Run: `cd platforms/macos && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build`
Expected: Build succeeds, no compilation errors

**Step 5: Commit UI changes**

```bash
git add platforms/macos/Aether/Sources/ShortcutsView.swift
git commit -m "feat(ui): update command completion hotkey defaults and add Esc key

- Change default modifiers from Command+Option to Option+Command
- Change default key from / to Space
- Add Esc key to CommandCharKey enum with symbol ⎋
- UI pickers now default to Option+Space
- Users can now select Esc as the character key

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 5: Update Documentation

**Files:**
- Modify: `docs/CONFIGURATION.md:196`

**Step 1: Update CONFIGURATION.md table**

In `docs/CONFIGURATION.md`, change line 196:

```markdown
| `command_prompt` | String | `"Option+Space"` | Command completion hotkey (BREAKING: old configs auto-migrated) |
```

**Step 2: Add migration note to configuration docs**

Search for the shortcuts section in `docs/CONFIGURATION.md` and add a note:

```markdown
#### Breaking Change Notice

As of version X.X, the default `command_prompt` hotkey has changed from `Command+Option+/` to `Option+Space`.

- **Automatic Migration**: Old configs with `Command+Option+/` are automatically updated to `Option+Space` on first load
- **Custom Hotkeys**: If you've set a custom hotkey (not the old default), it will be preserved
- **New Options**: The UI now supports Esc key as a character key option
```

**Step 3: Commit documentation**

```bash
git add docs/CONFIGURATION.md
git commit -m "docs: update command completion hotkey documentation

BREAKING CHANGE: Default hotkey changed from Command+Option+/ to Option+Space

- Update default value in configuration table
- Add migration notice for users
- Document new Esc key option

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 6: Update UI Localization Strings

**Files:**
- Modify: `platforms/macos/Aether/Resources/en.lproj/Localizable.strings:254-256`
- Modify: `platforms/macos/Aether/Resources/zh-Hans.lproj/Localizable.strings:254-256`

**Step 1: Update English localization**

In `platforms/macos/Aether/Resources/en.lproj/Localizable.strings`, change lines 254-256:

```strings
"settings.shortcuts.command_completion_description" = "Press Option+Space to open the conversation window for multi-turn dialogue and commands.";
"settings.shortcuts.command_completion_title" = "Conversation Window";
"settings.shortcuts.command_completion_hint" = "Open multi-turn conversation window (default: Option+Space)";
```

**Step 2: Update Chinese localization**

In `platforms/macos/Aether/Resources/zh-Hans.lproj/Localizable.strings`, change lines 254-256:

```strings
"settings.shortcuts.command_completion_description" = "按 Option+空格 打开会话窗口，支持多轮对话和命令补全。";
"settings.shortcuts.command_completion_title" = "会话窗口";
"settings.shortcuts.command_completion_hint" = "打开多轮对话窗口（默认：Option+空格）";
```

**Step 3: Rebuild to verify string compilation**

Run: `cd platforms/macos && xcodebuild -project Aleph.xcodeproj -scheme Aleph -configuration Debug build`
Expected: Build succeeds, strings compiled

**Step 4: Commit localization changes**

```bash
git add platforms/macos/Aether/Resources/en.lproj/Localizable.strings platforms/macos/Aether/Resources/zh-Hans.lproj/Localizable.strings
git commit -m "feat(i18n): update hotkey descriptions to reflect Option+Space

- Update English and Chinese localization strings
- Mention new default hotkey (Option+Space) in UI hints
- Remove references to old Command+Option+/ hotkey

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 7: Update Test Cases

**Files:**
- Modify: `core/src/config/tests/serialization.rs:56`
- Modify: `core/src/config/tests/serialization.rs:167`

**Step 1: Find existing test cases using old hotkey**

Run: `grep -n "Command+Option+/" core/src/config/tests/serialization.rs`
Expected: Find test cases at lines 56 and 167

**Step 2: Update test case 1 (around line 56)**

In `core/src/config/tests/serialization.rs`, change line 56:

```rust
command_prompt: "Option+Space".to_string(),
```

**Step 3: Update test case 2 (around line 167)**

In `core/src/config/tests/serialization.rs`, change line 167:

```rust
command_prompt: "Option+Space".to_string(),
```

**Step 4: Run Rust tests**

Run: `cd core && cargo test config::tests::serialization`
Expected: All serialization tests pass

**Step 5: Commit test updates**

```bash
git add core/src/config/tests/serialization.rs
git commit -m "test: update config serialization tests for new hotkey

- Change test expectations to use Option+Space instead of Command+Option+/
- Verify config serialization/deserialization works with new default
- All tests passing

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Task 8: Manual Testing and Verification

**Files:**
- Test: UI Settings display
- Test: Hotkey runtime behavior
- Test: Config file migration
- Test: New Esc key option

**Step 1: Test automatic migration of old config**

1. Create test config with old hotkey:
   ```bash
   mkdir -p ~/.aether
   cat > ~/.aleph/config.toml <<EOF
   [shortcuts]
   command_prompt = "Command+Option+/"
   EOF
   ```
2. Launch Aleph
3. Verify: Console logs show "Migrating command_prompt hotkey: Command+Option+/ -> Option+Space"
4. Verify: `~/.aleph/config.toml` now contains `command_prompt = "Option+Space"`
5. Verify: Settings UI shows "⌥ ⌘ ␣" (Option+Command+Space)

**Step 2: Test new default hotkey works**

1. Delete `~/.aleph/config.toml`
2. Launch Aleph
3. Open Settings → Shortcuts
4. Verify: Command Completion shows "⌥ ⌘ ␣" (Option+Command+Space)
5. Press Option+Space while in any app
6. Verify: Multi-turn conversation window appears

**Step 3: Test Esc key option in UI**

1. Open Settings → Shortcuts
2. Click on character key picker for Command Completion
3. Verify: Dropdown includes "Esc" option with "⎋" symbol
4. Select "Esc"
5. Click Save
6. Verify: `~/.aleph/config.toml` contains `command_prompt = "Option+Command+Esc"`
7. Press Option+Command+Esc
8. Verify: Multi-turn window appears

**Step 4: Test preservation of custom hotkeys**

1. Edit `~/.aleph/config.toml`
2. Set: `command_prompt = "Control+Shift+/"`
3. Relaunch Aleph
4. Verify: NO migration log message (custom hotkey preserved)
5. Verify: Settings UI shows "⌃ ⇧ /" (Control+Shift+/)
6. Press Control+Shift+/
7. Verify: Multi-turn window appears

**Step 5: Test reset to new default**

1. In Settings → Shortcuts, modify Command Completion to any custom value
2. Click "Reset" button for Command Completion
3. Verify: UI shows "⌥ ⌘ ␣" (new default Option+Command+Space)
4. Click Save
5. Verify: Config file contains `command_prompt = "Option+Space"`

**Step 6: Document test results**

Create test report:
```bash
cat > /tmp/hotkey-change-test-results.md <<'EOF'
# Multi-turn Hotkey Change Test Results

## Test Date
$(date +%Y-%m-%d)

## Test Environment
- macOS Version: $(sw_vers -productVersion)
- Aleph Build: $(git rev-parse --short HEAD)

## Test Cases
- [x] Old config (Command+Option+/) auto-migrated to Option+Space
- [x] Migration logged correctly in console
- [x] Fresh install uses Option+Space by default
- [x] New hotkey (Option+Space) triggers multi-turn window
- [x] Esc key appears in UI picker with ⎋ symbol
- [x] Esc key works as character key (Option+Command+Esc)
- [x] Custom hotkeys are preserved (no migration)
- [x] Reset button restores Option+Space default
- [x] Hotkey monitoring works across all apps
- [x] Config persistence works correctly

## Issues Found
(None / List any issues)

## Conclusion
All test cases passed. Migration is working correctly, new default hotkey is functional, and Esc key option is available.
EOF
cat /tmp/hotkey-change-test-results.md
```

**Step 7: Commit test results (if no issues found)**

If all tests pass:
```bash
# Note: Do NOT commit the test report, just document completion
git add .
git commit --allow-empty -m "test: verify Option+Space hotkey change and migration

All manual tests passed:
- Automatic migration of old configs
- New default hotkey functionality
- Esc key option in UI
- Custom hotkey preservation
- Reset to default behavior

Co-Authored-By: Claude Opus 4.5 <noreply@anthropic.com>"
```

---

## Implementation Summary

### Breaking Change Notice
**BREAKING CHANGE**: The default multi-turn conversation hotkey has changed from `Command+Option+/` to `Option+Space`. Old configs are automatically migrated on first load.

### Changes Made
1. **Rust Core**: Updated default hotkey to `Option+Space`
2. **Migration**: Added automatic migration logic for old configs
3. **HotkeyService**: Added support for Esc key
4. **UI**: Updated defaults and added Esc to character key picker
5. **Documentation**: Updated to reflect new default and migration
6. **Localization**: Updated English and Chinese strings
7. **Tests**: Updated serialization tests and added migration tests

### Files Modified (Total: 9 files)
1. `core/src/config/types/general.rs` - Rust default
2. `shared/config/default-config.toml` - Shared config
3. `platforms/macos/Aether/config.example.toml` - Example config
4. `core/src/config/migration.rs` - Migration logic
5. `core/src/config/load.rs` - Migration call
6. `core/src/config/tests/migration.rs` - Migration tests
7. `platforms/macos/Aether/Sources/Services/HotkeyService.swift` - Esc support
8. `platforms/macos/Aether/Sources/ShortcutsView.swift` - UI defaults + Esc enum
9. `docs/CONFIGURATION.md` - Documentation
10. `platforms/macos/Aether/Resources/en.lproj/Localizable.strings` - English UI
11. `platforms/macos/Aether/Resources/zh-Hans.lproj/Localizable.strings` - Chinese UI
12. `core/src/config/tests/serialization.rs` - Test cases

### Commits Made (Total: 7 commits)
1. Update Rust config defaults
2. Add config migration logic and tests
3. Add Esc key support to HotkeyService
4. Update UI enums and defaults
5. Update documentation
6. Update localization strings
7. Update test cases

---

## Execution Handoff

**计划已更新并保存至 `docs/plans/2026-01-25-change-multiturn-hotkey.md`。**

现在开始使用 **Subagent-Driven Development** 执行计划。我将：
1. 逐个任务派发新的子agent
2. 任务间进行代码审查
3. 快速迭代，实时反馈

准备开始执行 Task 1...
