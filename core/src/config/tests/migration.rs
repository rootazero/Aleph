//! Configuration migration tests

use super::super::*;

// Migration tests are currently minimal as most migrations are no-ops
// Future migrations can add tests here

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

    // Set new hotkey (already Option+Space)
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
