# Desktop Native Phase 3: macOS PIM & System Capabilities

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement real macOS native capabilities — Automation (osascript/Shortcuts), System (apps/notifications/clipboard/sysinfo), and PIM (Notes/Calendar/Reminders/Contacts via Swift CLI) — so LLM tools get live data instead of stubs.

**Architecture:** AutomationCapability and SystemCapability are implemented in Rust with `tokio::process::Command` (calling osascript, open, etc.). PimCapability delegates to the `aleph-bridge` Swift CLI via `SwiftBridge`. The Swift CLI calls Apple frameworks (AppleScript for Notes, EventKit for Calendar/Reminders, Contacts.framework for Contacts). The existing `PimTool` is rewired to prefer `DesktopPlatform.pim()` over the legacy `DesktopBridgeClient`.

**Tech Stack:** Rust (async-trait, tokio), Swift (EventKit, Contacts, ArgumentParser), AppleScript (Notes.app)

**Spec:** `docs/superpowers/specs/2026-03-21-desktop-native-capabilities-design.md` — Phase 3

---

## File Map

### New Files

| File | Responsibility |
|------|---------------|
| `crates/desktop-macos/src/automation.rs` | `MacOSAutomation`: impl AutomationCapability (osascript + shortcuts CLI) |
| `crates/desktop-macos/src/system.rs` | `MacOSSystem`: impl SystemCapability (open, osascript, clipboard, sysinfo) |
| `crates/desktop-macos/src/pim.rs` | `MacOSPim`: impl PimCapability (delegates to SwiftBridge) |
| `apps/macos-bridge/Sources/AlephBridge/NotesCommands.swift` | Real Notes.app operations via AppleScript |
| `apps/macos-bridge/Sources/AlephBridge/CalendarCommands.swift` | Real Calendar operations via EventKit |
| `apps/macos-bridge/Sources/AlephBridge/RemindersCommands.swift` | Real Reminders operations via EventKit |
| `apps/macos-bridge/Sources/AlephBridge/ContactsCommands.swift` | Real Contacts operations via Contacts.framework |
| `apps/macos-bridge/Sources/AlephBridge/SystemCommands.swift` | Enhanced System operations |

### Modified Files

| File | Change |
|------|--------|
| `crates/desktop-macos/src/lib.rs` | Store and return MacOSAutomation, MacOSSystem, MacOSPim |
| `crates/desktop-macos/Cargo.toml` | Add `dirs` dependency (for SwiftBridge) |
| `apps/macos-bridge/Sources/AlephBridge/main.swift` | Extract commands to separate files, keep only root + helpers |
| `src/builtin_tools/pim/mod.rs` | Add `platform` field, prefer platform.pim() over bridge client |
| `src/executor/builtin_registry/builder.rs` | Pass platform to PimTool |

---

## Task 1: macOS AutomationCapability

**Files:**
- Create: `crates/desktop-macos/src/automation.rs`
- Modify: `crates/desktop-macos/src/lib.rs`

Implement AutomationCapability in pure Rust using `tokio::process::Command` to call `osascript` and `shortcuts` CLI. No SwiftBridge needed.

- [ ] **Step 1: Create `crates/desktop-macos/src/automation.rs`**

```rust
//! macOS automation capability — AppleScript, JXA, and Shortcuts.

use async_trait::async_trait;
use tokio::process::Command;

use aleph_desktop::automation_types::{ScriptLanguage, ShortcutInfo};
use aleph_desktop::traits::AutomationCapability;
use aleph_desktop::{DesktopError, Result};

pub struct MacOSAutomation {
    _private: (),
}

impl MacOSAutomation {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

#[async_trait]
impl AutomationCapability for MacOSAutomation {
    async fn run_script(&self, language: ScriptLanguage, source: &str) -> Result<String> {
        let (program, args): (&str, Vec<String>) = match language {
            ScriptLanguage::AppleScript => ("osascript", vec!["-e".into(), source.into()]),
            ScriptLanguage::Jxa => (
                "osascript",
                vec!["-l".into(), "JavaScript".into(), "-e".into(), source.into()],
            ),
            ScriptLanguage::Shell => ("bash", vec!["-c".into(), source.into()]),
            ScriptLanguage::PowerShell => {
                return Err(DesktopError::NotImplemented(
                    "PowerShell is not available on macOS".into(),
                ));
            }
        };

        let output = Command::new(program)
            .args(&args)
            .output()
            .await
            .map_err(|e| DesktopError::NotAvailable(format!("failed to run {program}: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "script failed (exit {}): {stderr}",
                output.status.code().unwrap_or(-1)
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(stdout)
    }

    async fn list_shortcuts(&self) -> Result<Vec<ShortcutInfo>> {
        let output = Command::new("shortcuts")
            .arg("list")
            .output()
            .await
            .map_err(|e| DesktopError::NotAvailable(format!("shortcuts CLI not found: {e}")))?;

        if !output.status.success() {
            return Ok(vec![]);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let shortcuts = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| ShortcutInfo {
                name: line.trim().to_string(),
                id: None,
                description: None,
            })
            .collect();

        Ok(shortcuts)
    }

    async fn run_shortcut(&self, name: &str, input: Option<&str>) -> Result<String> {
        let mut cmd = Command::new("shortcuts");
        cmd.arg("run").arg(name);

        if let Some(input_data) = input {
            cmd.arg("--input-type").arg("text").arg("--input").arg(input_data);
        }

        let output = cmd
            .output()
            .await
            .map_err(|e| DesktopError::NotAvailable(format!("shortcuts CLI not found: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(DesktopError::InputFailed(format!(
                "shortcut '{name}' failed: {stderr}"
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(stdout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_applescript() {
        let auto = MacOSAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::AppleScript, "return 2 + 2")
            .await;
        assert_eq!(result.unwrap(), "4");
    }

    #[tokio::test]
    async fn test_run_shell() {
        let auto = MacOSAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::Shell, "echo hello")
            .await;
        assert_eq!(result.unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_list_shortcuts() {
        let auto = MacOSAutomation::new();
        // Should not error even if no shortcuts exist
        let result = auto.list_shortcuts().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_powershell_not_available() {
        let auto = MacOSAutomation::new();
        let result = auto
            .run_script(ScriptLanguage::PowerShell, "Write-Host hello")
            .await;
        assert!(matches!(result, Err(DesktopError::NotImplemented(_))));
    }
}
```

- [ ] **Step 2: Wire into MacOSPlatform**

Update `crates/desktop-macos/src/lib.rs`:
- Add `mod automation;`
- Add `automation: automation::MacOSAutomation` field to `MacOSPlatform`
- Return `Some(&self.automation)` from `automation()` method
- Initialize in `new()`

- [ ] **Step 3: Verify and test**

Run: `cargo test -p aleph-desktop-macos --lib`
Expected: automation tests pass (AppleScript "return 2+2" = "4", shell echo, list_shortcuts)

- [ ] **Step 4: Commit**

```bash
git add crates/desktop-macos/
git commit -m "desktop-macos: implement AutomationCapability (osascript + Shortcuts CLI)"
```

---

## Task 2: macOS SystemCapability

**Files:**
- Create: `crates/desktop-macos/src/system.rs`
- Modify: `crates/desktop-macos/src/lib.rs`

Implement SystemCapability in Rust using `tokio::process::Command` for app management and osascript for notifications/clipboard.

- [ ] **Step 1: Create `crates/desktop-macos/src/system.rs`**

```rust
//! macOS system capability — app management, notifications, clipboard, system info.

use async_trait::async_trait;
use tokio::process::Command;

use aleph_desktop::system_types::{AppInfo, ClipboardContent, SystemInfo};
use aleph_desktop::traits::SystemCapability;
use aleph_desktop::{DesktopError, Result};

pub struct MacOSSystem {
    _private: (),
}

impl MacOSSystem {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

#[async_trait]
impl SystemCapability for MacOSSystem {
    async fn launch_app(&self, app_name: &str) -> Result<()> {
        // Try bundle ID first, then app name
        let status = Command::new("open")
            .arg("-a")
            .arg(app_name)
            .status()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to launch: {e}")))?;

        if !status.success() {
            return Err(DesktopError::InputFailed(format!(
                "failed to launch '{app_name}'"
            )));
        }
        Ok(())
    }

    async fn quit_app(&self, app_name: &str) -> Result<()> {
        let script = format!(
            r#"tell application "{}" to quit"#,
            app_name.replace('"', "\\\"")
        );
        let status = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to quit app: {e}")))?;

        if !status.success() {
            return Err(DesktopError::InputFailed(format!(
                "failed to quit '{app_name}'"
            )));
        }
        Ok(())
    }

    async fn list_running_apps(&self) -> Result<Vec<AppInfo>> {
        let script = r#"
            set output to ""
            tell application "System Events"
                set appList to every process whose background only is false
                repeat with proc in appList
                    set appName to name of proc
                    set appPID to unix id of proc
                    set appBundle to bundle identifier of proc
                    set isFront to (proc is equal to first process whose frontmost is true)
                    set output to output & appName & "\t" & appPID & "\t" & appBundle & "\t" & isFront & linefeed
                end repeat
            end tell
            return output
        "#;

        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("failed to list apps: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let apps = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| {
                let parts: Vec<&str> = line.split('\t').collect();
                if parts.len() >= 4 {
                    Some(AppInfo {
                        name: parts[0].to_string(),
                        bundle_id: if parts[2].is_empty() { None } else { Some(parts[2].to_string()) },
                        pid: parts[1].parse().ok(),
                        is_active: parts[3] == "true",
                    })
                } else {
                    None
                }
            })
            .collect();

        Ok(apps)
    }

    async fn send_notification(&self, title: &str, body: &str) -> Result<()> {
        let script = format!(
            r#"display notification "{}" with title "{}""#,
            body.replace('"', "\\\""),
            title.replace('"', "\\\"")
        );
        Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .status()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("notification failed: {e}")))?;
        Ok(())
    }

    async fn clipboard_read(&self) -> Result<ClipboardContent> {
        let output = Command::new("pbpaste")
            .output()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("pbpaste failed: {e}")))?;

        let text = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(ClipboardContent {
            text: if text.is_empty() { None } else { Some(text) },
            has_image: false,
            image_base64: None,
        })
    }

    async fn clipboard_write(&self, text: &str) -> Result<()> {
        use std::process::Stdio;
        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| DesktopError::InputFailed(format!("pbcopy failed: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(text.as_bytes())
                .await
                .map_err(|e| DesktopError::InputFailed(format!("write to pbcopy failed: {e}")))?;
        }

        child
            .wait()
            .await
            .map_err(|e| DesktopError::InputFailed(format!("pbcopy wait failed: {e}")))?;
        Ok(())
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".into());

        let username = std::env::var("USER").unwrap_or_else(|_| "unknown".into());

        // Get macOS version via sw_vers
        let version_output = Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .await
            .ok();

        let os_version = version_output
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".into());

        Ok(SystemInfo {
            os_name: "macOS".into(),
            os_version,
            hostname,
            arch: std::env::consts::ARCH.into(),
            username,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_system_info() {
        let sys = MacOSSystem::new();
        let info = sys.system_info().await.unwrap();
        assert_eq!(info.os_name, "macOS");
        assert!(!info.hostname.is_empty());
        assert!(!info.username.is_empty());
    }

    #[tokio::test]
    async fn test_clipboard_roundtrip() {
        let sys = MacOSSystem::new();
        let test_text = "aleph-test-clipboard-12345";
        sys.clipboard_write(test_text).await.unwrap();
        let content = sys.clipboard_read().await.unwrap();
        assert_eq!(content.text.as_deref(), Some(test_text));
    }
}
```

- [ ] **Step 2: Add `hostname` crate to `Cargo.toml`**

Add to `crates/desktop-macos/Cargo.toml`:
```toml
hostname = "0.4"
```

- [ ] **Step 3: Wire into MacOSPlatform**

Update `crates/desktop-macos/src/lib.rs`:
- Add `mod system;`
- Add `system: system::MacOSSystem` field
- Return `Some(&self.system)` from `system()` method

- [ ] **Step 4: Verify and test**

Run: `cargo test -p aleph-desktop-macos --lib`
Expected: system_info and clipboard tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/desktop-macos/
git commit -m "desktop-macos: implement SystemCapability (apps, notifications, clipboard, sysinfo)"
```

---

## Task 3: Implement Swift CLI Real PIM Commands

**Files:**
- Create: `apps/macos-bridge/Sources/AlephBridge/NotesCommands.swift`
- Create: `apps/macos-bridge/Sources/AlephBridge/CalendarCommands.swift`
- Create: `apps/macos-bridge/Sources/AlephBridge/RemindersCommands.swift`
- Create: `apps/macos-bridge/Sources/AlephBridge/ContactsCommands.swift`
- Create: `apps/macos-bridge/Sources/AlephBridge/SystemCommands.swift`
- Modify: `apps/macos-bridge/Sources/AlephBridge/main.swift`

Split the monolithic main.swift into per-domain files with real Apple API implementations. The main.swift keeps only the root `AlephBridge` command, `printJSON()` helper, and `printError()` helper.

**Implementation approach per domain:**
- **Notes**: AppleScript via `NSAppleScript` (Notes.app has no Swift API)
- **Calendar**: `EventKit` (EKEventStore)
- **Reminders**: `EventKit` (EKReminderStore)
- **Contacts**: `Contacts.framework` (CNContactStore)

- [ ] **Step 1: Refactor main.swift**

Slim down to root command + helpers only. Move all command structs to separate files.

```swift
// main.swift — keep only root command and helpers
import ArgumentParser
import Foundation

@main
struct AlephBridge: ParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "aleph-bridge",
        abstract: "Aleph Desktop Bridge — macOS native API access via CLI",
        version: "0.1.0",
        subcommands: [
            Notes.self,
            Calendar.self,
            Reminders.self,
            Contacts.self,
            System.self,
        ]
    )
}

// MARK: - Helpers

func printJSON(_ value: Any) {
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]),
       let string = String(data: data, encoding: .utf8) {
        print(string)
    } else {
        fputs("Failed to serialize JSON\n", stderr)
        Foundation.exit(1)
    }
}

func printError(_ message: String) -> Never {
    fputs(message + "\n", stderr)
    Foundation.exit(1)
}
```

- [ ] **Step 2: Create `NotesCommands.swift`**

Notes.app only supports AppleScript — use `NSAppleScript` for all operations.

```swift
import ArgumentParser
import Foundation

struct Notes: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Notes.app operations",
        subcommands: [List.self, Get.self, Create.self, Update.self, Delete.self, Folders.self]
    )

    struct List: ParsableCommand {
        @Option(help: "Filter by folder name")
        var folder: String?

        func run() throws {
            var script = """
                set noteList to {}
                tell application "Notes"
            """
            if let folder = folder {
                script += """
                    set targetFolder to folder "\(folder)"
                    set allNotes to notes of targetFolder
                """
            } else {
                script += "set allNotes to every note\n"
            }
            script += """
                    repeat with n in allNotes
                        set noteId to id of n
                        set noteTitle to name of n
                        set noteMod to modification date of n as «class isot» as string
                        set noteBody to plaintext of n
                        set noteSnippet to text 1 thru (min of {100, length of noteBody}) of noteBody
                        set end of noteList to {noteId, noteTitle, noteMod, noteSnippet}
                    end repeat
                end tell
                return noteList
            """
            let result = runAppleScript(script)
            // Parse AppleScript list result into JSON
            // AppleScript returns comma-separated items
            printJSON(["notes": parseNotesList(result), "count": 0])
        }
    }

    // ... (remaining Note commands follow same pattern)
    // Implementation creates AppleScript strings, runs them, parses output

    struct Get: ParsableCommand {
        @Argument(help: "Note ID") var id: String
        func run() throws {
            let script = """
                tell application "Notes"
                    set n to first note whose id is "\(id)"
                    set noteTitle to name of n
                    set noteBody to body of n
                    set noteFolder to name of container of n
                    set noteCreated to creation date of n as «class isot» as string
                    set noteMod to modification date of n as «class isot» as string
                    return {noteTitle, noteBody, noteFolder, noteCreated, noteMod}
                end tell
            """
            let result = runAppleScript(script)
            printJSON(["id": id, "title": "", "body": result, "folder": ""])
        }
    }

    struct Create: ParsableCommand {
        @Option var title: String
        @Option var body: String?
        @Option var folder: String?
        func run() throws {
            var script = "tell application \"Notes\"\n"
            if let folder = folder {
                script += "    set targetFolder to folder \"\(folder)\"\n"
                script += "    make new note at targetFolder with properties {name:\"\(title)\""
            } else {
                script += "    make new note with properties {name:\"\(title)\""
            }
            if let body = body {
                script += ", body:\"<html><body>\(body.replacingOccurrences(of: "\"", with: "\\\""))</body></html>\""
            }
            script += "}\n    set newId to id of result\n    return newId\nend tell"
            let noteId = runAppleScript(script)
            printJSON(["id": noteId.trimmingCharacters(in: .whitespacesAndNewlines), "title": title])
        }
    }

    struct Update: ParsableCommand {
        @Argument var id: String
        @Option var title: String?
        @Option var body: String?
        func run() throws {
            var script = "tell application \"Notes\"\n    set n to first note whose id is \"\(id)\"\n"
            if let title = title { script += "    set name of n to \"\(title)\"\n" }
            if let body = body { script += "    set body of n to \"<html><body>\(body)</body></html>\"\n" }
            script += "end tell"
            runAppleScript(script)
            printJSON(["updated": true, "id": id])
        }
    }

    struct Delete: ParsableCommand {
        @Argument var id: String
        func run() throws {
            let script = """
                tell application "Notes"
                    delete (first note whose id is "\(id)")
                end tell
            """
            runAppleScript(script)
            printJSON(["deleted": true, "id": id])
        }
    }

    struct Folders: ParsableCommand {
        func run() throws {
            let script = """
                tell application "Notes"
                    set folderNames to name of every folder
                    return folderNames
                end tell
            """
            let result = runAppleScript(script)
            let folders = result.components(separatedBy: ", ").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            printJSON(["folders": folders])
        }
    }
}

// MARK: - AppleScript Helpers

@discardableResult
func runAppleScript(_ source: String) -> String {
    var error: NSDictionary?
    let script = NSAppleScript(source: source)
    let result = script?.executeAndReturnError(&error)
    if let error = error {
        let message = error[NSAppleScript.errorMessage] as? String ?? "Unknown AppleScript error"
        printError("AppleScript error: \(message)")
    }
    return result?.stringValue ?? ""
}

func parseNotesList(_ raw: String) -> [[String: Any]] {
    // AppleScript list parsing — simplified for stub
    return []
}
```

**Note to implementer:** The exact AppleScript syntax will need adjustment based on testing with real Notes.app. The structure above is the right pattern — implement each command, run it against real Notes.app, fix syntax issues.

- [ ] **Step 3: Create `CalendarCommands.swift`**

Use EventKit framework. Requires requesting calendar access.

```swift
import ArgumentParser
import EventKit
import Foundation

struct Calendar: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Calendar operations (EventKit)",
        subcommands: [Events.self, Get.self, Create.self, Update.self, Delete.self, Calendars.self]
    )

    struct Events: ParsableCommand {
        @Option var from: String
        @Option var to: String
        @Option var calendarId: String?

        func run() throws {
            let store = EKEventStore()
            requestCalendarAccess(store)

            let df = ISO8601DateFormatter()
            guard let startDate = df.date(from: from) else { printError("Invalid --from date") }
            guard let endDate = df.date(from: to) else { printError("Invalid --to date") }

            let predicate: NSPredicate
            if let calId = calendarId,
               let cal = store.calendars(for: .event).first(where: { $0.calendarIdentifier == calId }) {
                predicate = store.predicateForEvents(withStart: startDate, end: endDate, calendars: [cal])
            } else {
                predicate = store.predicateForEvents(withStart: startDate, end: endDate, calendars: nil)
            }

            let events = store.events(matching: predicate).map { eventToDict($0) }
            printJSON(["events": events, "count": events.count])
        }
    }

    struct Get: ParsableCommand {
        @Argument var id: String
        func run() throws {
            let store = EKEventStore()
            requestCalendarAccess(store)
            guard let event = store.event(withIdentifier: id) else {
                printError("Event not found: \(id)")
            }
            printJSON(eventToDict(event))
        }
    }

    struct Create: ParsableCommand {
        @Option var title: String
        @Option var start: String
        @Option var end: String
        @Option var calendarId: String?
        @Option var location: String?
        @Option var notes: String?
        @Flag var allDay: Bool = false

        func run() throws {
            let store = EKEventStore()
            requestCalendarAccess(store)

            let df = ISO8601DateFormatter()
            let event = EKEvent(eventStore: store)
            event.title = title
            event.startDate = df.date(from: start)
            event.endDate = df.date(from: end)
            event.isAllDay = allDay
            event.location = location
            event.notes = notes

            if let calId = calendarId {
                event.calendar = store.calendars(for: .event).first { $0.calendarIdentifier == calId }
            } else {
                event.calendar = store.defaultCalendarForNewEvents
            }

            try store.save(event, span: .thisEvent)
            printJSON(["id": event.eventIdentifier ?? "", "title": title])
        }
    }

    struct Update: ParsableCommand {
        @Argument var id: String
        @Option var title: String?
        @Option var start: String?
        @Option var end: String?
        @Option var location: String?
        @Option var notes: String?

        func run() throws {
            let store = EKEventStore()
            requestCalendarAccess(store)
            guard let event = store.event(withIdentifier: id) else { printError("Event not found") }
            let df = ISO8601DateFormatter()
            if let t = title { event.title = t }
            if let s = start { event.startDate = df.date(from: s) }
            if let e = end { event.endDate = df.date(from: e) }
            if let l = location { event.location = l }
            if let n = notes { event.notes = n }
            try store.save(event, span: .thisEvent)
            printJSON(["updated": true, "id": id])
        }
    }

    struct Delete: ParsableCommand {
        @Argument var id: String
        func run() throws {
            let store = EKEventStore()
            requestCalendarAccess(store)
            guard let event = store.event(withIdentifier: id) else { printError("Event not found") }
            try store.remove(event, span: .thisEvent)
            printJSON(["deleted": true, "id": id])
        }
    }

    struct Calendars: ParsableCommand {
        func run() throws {
            let store = EKEventStore()
            requestCalendarAccess(store)
            let cals = store.calendars(for: .event).map { cal -> [String: Any] in
                ["id": cal.calendarIdentifier, "title": cal.title, "color": cal.cgColor?.description ?? ""]
            }
            printJSON(["calendars": cals])
        }
    }
}

func requestCalendarAccess(_ store: EKEventStore) {
    let semaphore = DispatchSemaphore(value: 0)
    var granted = false
    if #available(macOS 14.0, *) {
        store.requestFullAccessToEvents { g, _ in granted = g; semaphore.signal() }
    } else {
        store.requestAccess(to: .event) { g, _ in granted = g; semaphore.signal() }
    }
    semaphore.wait()
    if !granted { printError("Calendar access denied") }
}

func eventToDict(_ event: EKEvent) -> [String: Any] {
    let df = ISO8601DateFormatter()
    var dict: [String: Any] = [
        "id": event.eventIdentifier ?? "",
        "title": event.title ?? "",
        "start": df.string(from: event.startDate),
        "end": df.string(from: event.endDate),
        "all_day": event.isAllDay,
        "calendar_id": event.calendar?.calendarIdentifier ?? "",
    ]
    if let loc = event.location { dict["location"] = loc }
    if let notes = event.notes { dict["notes"] = notes }
    return dict
}
```

- [ ] **Step 4: Create `RemindersCommands.swift`**

Same EventKit pattern but for reminders.

- [ ] **Step 5: Create `ContactsCommands.swift`**

Use Contacts.framework (CNContactStore).

- [ ] **Step 6: Create `SystemCommands.swift`**

Extract and enhance the existing System.Info command.

- [ ] **Step 7: Update main.swift to reference new files**

Remove all command structs from main.swift, keep only root `AlephBridge` + helpers. Swift Package Manager auto-discovers all .swift files in the target directory, so just delete the old inline commands.

- [ ] **Step 8: Build and test**

Run: `cd apps/macos-bridge && swift build 2>&1 | tail -5`
Expected: builds successfully

Test each domain:
```bash
swift run AlephBridge notes folders
swift run AlephBridge calendar calendars
swift run AlephBridge reminders lists
swift run AlephBridge contacts groups
swift run AlephBridge system info
```

- [ ] **Step 9: Commit**

```bash
git add apps/macos-bridge/
git commit -m "apps: implement real macOS API calls in Swift CLI bridge"
```

---

## Task 4: macOS PimCapability via SwiftBridge

**Files:**
- Create: `crates/desktop-macos/src/pim.rs`
- Modify: `crates/desktop-macos/src/lib.rs`
- Modify: `crates/desktop-macos/Cargo.toml`

Implement `PimCapability` by delegating each method to `SwiftBridge::call()`.

- [ ] **Step 1: Create `crates/desktop-macos/src/pim.rs`**

```rust
//! macOS PIM capability — delegates to aleph-bridge Swift CLI via SwiftBridge.

use async_trait::async_trait;

use aleph_desktop::bridge::SwiftBridge;
use aleph_desktop::pim_types::*;
use aleph_desktop::traits::PimCapability;
use aleph_desktop::Result;

pub struct MacOSPim {
    bridge: SwiftBridge,
}

impl MacOSPim {
    pub fn new() -> Self {
        Self {
            bridge: SwiftBridge::default(),
        }
    }

    pub fn with_bridge(bridge: SwiftBridge) -> Self {
        Self { bridge }
    }
}

#[async_trait]
impl PimCapability for MacOSPim {
    // Notes
    async fn notes_list(&self, folder: Option<&str>) -> Result<Vec<NoteInfo>> {
        #[derive(serde::Deserialize)]
        struct Response { notes: Vec<NoteInfo> }
        let mut args = vec![];
        if let Some(f) = folder { args.push(("folder", f)); }
        let resp: Response = self.bridge.call("notes", "list", &args).await?;
        Ok(resp.notes)
    }

    async fn notes_read(&self, note_id: &str) -> Result<NoteContent> {
        self.bridge.call("notes", "get", &[("id", note_id)]).await
    }

    async fn notes_create(&self, folder: &str, title: &str, body: &str) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct Response { id: String }
        let args = vec![("title", title), ("body", body), ("folder", folder)];
        let resp: Response = self.bridge.call("notes", "create", &args).await?;
        Ok(resp.id)
    }

    async fn notes_update(&self, note_id: &str, title: Option<&str>, body: Option<&str>) -> Result<()> {
        let mut args: Vec<(&str, &str)> = vec![("id", note_id)];
        if let Some(t) = title { args.push(("title", t)); }
        if let Some(b) = body { args.push(("body", b)); }
        let _: serde_json::Value = self.bridge.call("notes", "update", &args).await?;
        Ok(())
    }

    async fn notes_delete(&self, note_id: &str) -> Result<()> {
        let _: serde_json::Value = self.bridge.call("notes", "delete", &[("id", note_id)]).await?;
        Ok(())
    }

    async fn notes_folders(&self) -> Result<Vec<String>> {
        #[derive(serde::Deserialize)]
        struct Response { folders: Vec<String> }
        let resp: Response = self.bridge.call("notes", "folders", &[]).await?;
        Ok(resp.folders)
    }

    // Calendar
    async fn calendar_list_events(&self, from: &str, to: &str, calendar_id: Option<&str>) -> Result<Vec<CalendarEvent>> {
        #[derive(serde::Deserialize)]
        struct Response { events: Vec<CalendarEvent> }
        let mut args = vec![("from", from), ("to", to)];
        if let Some(cal) = calendar_id { args.push(("calendar-id", cal)); }
        let resp: Response = self.bridge.call("calendar", "events", &args).await?;
        Ok(resp.events)
    }

    async fn calendar_get_event(&self, event_id: &str) -> Result<CalendarEvent> {
        self.bridge.call("calendar", "get", &[("id", event_id)]).await
    }

    async fn calendar_create_event(&self, event: NewCalendarEvent) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct Response { id: String }
        let mut args = vec![
            ("title".to_string(), event.title),
            ("start".to_string(), event.start),
            ("end".to_string(), event.end),
        ];
        if let Some(cal) = event.calendar_id { args.push(("calendar-id".into(), cal)); }
        if let Some(loc) = event.location { args.push(("location".into(), loc)); }
        if let Some(notes) = event.notes { args.push(("notes".into(), notes)); }
        if event.all_day.unwrap_or(false) { args.push(("all-day".into(), "true".into())); }
        let string_args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let resp: Response = self.bridge.call("calendar", "create", &string_args).await?;
        Ok(resp.id)
    }

    async fn calendar_update_event(&self, event_id: &str, event: NewCalendarEvent) -> Result<()> {
        let mut args = vec![("id".to_string(), event_id.to_string())];
        args.push(("title".into(), event.title));
        args.push(("start".into(), event.start));
        args.push(("end".into(), event.end));
        if let Some(loc) = event.location { args.push(("location".into(), loc)); }
        if let Some(notes) = event.notes { args.push(("notes".into(), notes)); }
        let string_args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let _: serde_json::Value = self.bridge.call("calendar", "update", &string_args).await?;
        Ok(())
    }

    async fn calendar_delete_event(&self, event_id: &str) -> Result<()> {
        let _: serde_json::Value = self.bridge.call("calendar", "delete", &[("id", event_id)]).await?;
        Ok(())
    }

    async fn calendar_calendars(&self) -> Result<Vec<CalendarInfo>> {
        #[derive(serde::Deserialize)]
        struct Response { calendars: Vec<CalendarInfo> }
        let resp: Response = self.bridge.call("calendar", "calendars", &[]).await?;
        Ok(resp.calendars)
    }

    // Reminders — same pattern as Calendar
    async fn reminders_list(&self, list_id: Option<&str>, include_completed: bool) -> Result<Vec<Reminder>> {
        #[derive(serde::Deserialize)]
        struct Response { reminders: Vec<Reminder> }
        let mut args: Vec<(&str, &str)> = vec![];
        if let Some(id) = list_id { args.push(("list-id", id)); }
        let completed_str;
        if include_completed { completed_str = "true".to_string(); args.push(("include-completed", &completed_str)); }
        let resp: Response = self.bridge.call("reminders", "list", &args).await?;
        Ok(resp.reminders)
    }

    async fn reminders_get(&self, reminder_id: &str) -> Result<Reminder> {
        self.bridge.call("reminders", "get", &[("id", reminder_id)]).await
    }

    async fn reminders_create(&self, reminder: NewReminder) -> Result<String> {
        #[derive(serde::Deserialize)]
        struct Response { id: String }
        let mut args = vec![("title".to_string(), reminder.title)];
        if let Some(list) = reminder.list_id { args.push(("list-id".into(), list)); }
        if let Some(due) = reminder.due_date { args.push(("due-date".into(), due)); }
        if let Some(pri) = reminder.priority { args.push(("priority".into(), pri.to_string())); }
        if let Some(notes) = reminder.notes { args.push(("notes".into(), notes)); }
        let string_args: Vec<(&str, &str)> = args.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let resp: Response = self.bridge.call("reminders", "create", &string_args).await?;
        Ok(resp.id)
    }

    async fn reminders_complete(&self, reminder_id: &str) -> Result<()> {
        let _: serde_json::Value = self.bridge.call("reminders", "complete", &[("id", reminder_id)]).await?;
        Ok(())
    }

    async fn reminders_delete(&self, reminder_id: &str) -> Result<()> {
        let _: serde_json::Value = self.bridge.call("reminders", "delete", &[("id", reminder_id)]).await?;
        Ok(())
    }

    async fn reminders_lists(&self) -> Result<Vec<ReminderList>> {
        #[derive(serde::Deserialize)]
        struct Response { lists: Vec<ReminderList> }
        let resp: Response = self.bridge.call("reminders", "lists", &[]).await?;
        Ok(resp.lists)
    }

    // Contacts
    async fn contacts_search(&self, query: &str) -> Result<Vec<Contact>> {
        #[derive(serde::Deserialize)]
        struct Response { contacts: Vec<Contact> }
        let resp: Response = self.bridge.call("contacts", "search", &[("query", query)]).await?;
        Ok(resp.contacts)
    }

    async fn contacts_get(&self, contact_id: &str) -> Result<ContactDetail> {
        self.bridge.call("contacts", "get", &[("id", contact_id)]).await
    }

    async fn contacts_groups(&self) -> Result<Vec<ContactGroup>> {
        #[derive(serde::Deserialize)]
        struct Response { groups: Vec<ContactGroup> }
        let resp: Response = self.bridge.call("contacts", "groups", &[]).await?;
        Ok(resp.groups)
    }
}
```

- [ ] **Step 2: Wire into MacOSPlatform**

Update `crates/desktop-macos/src/lib.rs`:
- Add `mod pim;`
- Add `pim: pim::MacOSPim` field
- Return `Some(&self.pim)` from `pim()` method

- [ ] **Step 3: Verify compilation**

Run: `cargo check -p aleph-desktop-macos`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
git add crates/desktop-macos/
git commit -m "desktop-macos: implement PimCapability via SwiftBridge"
```

---

## Task 5: Rewire PimTool to Use DesktopPlatform

**Files:**
- Modify: `src/builtin_tools/pim/mod.rs`
- Modify: `src/executor/builtin_registry/builder.rs`

Follow the same pattern used for DesktopTool in Phase 2: add `platform` field, prefer `platform.pim()` over the legacy `DesktopBridgeClient`.

- [ ] **Step 1: Add platform field to PimTool**

In `src/builtin_tools/pim/mod.rs`, modify the struct:

```rust
pub struct PimTool {
    client: DesktopBridgeClient,
    approval_policy: Option<Arc<dyn ApprovalPolicy>>,
    platform: Option<Arc<dyn aleph_desktop::DesktopPlatform>>,  // NEW
}
```

Update `new()` to initialize `platform: None`.

Add builder:
```rust
pub fn with_platform(mut self, platform: Arc<dyn aleph_desktop::DesktopPlatform>) -> Self {
    self.platform = Some(platform);
    self
}
```

- [ ] **Step 2: Add `call_via_platform()` method**

Add a method that dispatches PIM operations through `platform.pim()`:

```rust
async fn call_via_platform(&self, args: &PimArgs) -> Option<PimOutput> {
    let platform = self.platform.as_ref()?;
    let pim = platform.pim()?;

    let result: std::result::Result<serde_json::Value, String> = match args.action.as_str() {
        "notes_list" => { /* delegate to pim.notes_list() */ }
        "notes_get" => { /* delegate to pim.notes_read() */ }
        "notes_create" => { /* delegate to pim.notes_create() */ }
        // ... all 23+ actions
        _ => return None, // unknown action, fall through to bridge
    };

    Some(match result {
        Ok(data) => PimOutput { success: true, data: Some(data), message: None },
        Err(msg) => PimOutput { success: false, data: None, message: Some(msg) },
    })
}
```

- [ ] **Step 3: Update `call()` to prefer platform**

In the `call()` method, before the bridge availability check, add:

```rust
// Prefer DesktopPlatform.pim() over legacy bridge
if let Some(output) = self.call_via_platform(&args).await {
    return Ok(output);
}
```

- [ ] **Step 4: Update builder to pass platform**

In `src/executor/builtin_registry/builder.rs`, change:
```rust
let pim_tool = PimTool::new();
```
to:
```rust
let pim_tool = PimTool::new()
    .with_platform(Arc::clone(&desktop_platform));
```

- [ ] **Step 5: Verify and test**

Run: `cargo check -p alephcore && cargo test -p alephcore --lib pim`
Expected: compiles, existing PIM tests pass

- [ ] **Step 6: Commit**

```bash
git add src/builtin_tools/pim/mod.rs src/executor/builtin_registry/builder.rs
git commit -m "core: rewire PimTool to dispatch via DesktopPlatform.pim()"
```

---

## Task 6: Build Integration & Verification

**Files:**
- Modify: `justfile`

- [ ] **Step 1: Add swift build to justfile**

Add a recipe to build the Swift CLI:

```just
# Build Swift bridge (macOS only)
swift-bridge:
    cd apps/macos-bridge && swift build -c release
    @echo "✓ Swift bridge: apps/macos-bridge/.build/release/AlephBridge"
```

Update the `build` recipe to include swift-bridge on macOS:

```just
# Full build: WASM → Swift Bridge → Server (release)
build: wasm swift-bridge
    cargo build -p alephcore --bin {{server_bin}} --release
    @echo "✓ Server: {{release_dir}}/{{server_bin}}"
```

- [ ] **Step 2: Full compile check**

Run: `cargo check -p alephcore`
Expected: no errors

- [ ] **Step 3: Run all core tests**

Run: `cargo test -p alephcore --lib`
Expected: 8303+ tests, 0 failures

- [ ] **Step 4: Run platform crate tests**

Run: `cargo test -p aleph-desktop-macos --lib`
Expected: all tests pass (automation, system, platform creation)

- [ ] **Step 5: Verify Swift CLI builds**

Run: `cd apps/macos-bridge && swift build`
Expected: builds successfully

- [ ] **Step 6: Commit if fixes needed**

```bash
git add justfile
git commit -m "build: add swift-bridge recipe to justfile for macOS native APIs"
```
