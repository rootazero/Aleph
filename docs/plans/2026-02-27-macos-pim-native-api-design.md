# macOS PIM Native API Integration Design

> Date: 2026-02-27
> Status: Approved

## Overview

Extend Aleph's macOS Desktop Bridge to access macOS system applications — Calendar, Reminders, Notes, and Contacts — through their native frameworks (EventKit, Contacts.framework, AppleScript), exposing them as a unified `pim` tool to the Agent/LLM layer.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Technology | EventKit + Contacts.framework + AppleScript (Notes) | Native performance, full API coverage. Notes has no Swift API. |
| Integration | Extend existing Desktop Bridge (`pim.*` namespace) | Reuse mature UDS JSON-RPC channel, single communication path |
| Tool design | Single `pim` tool with `action` field | Fewer tools for LLM, consistent with existing `desktop` tool pattern |
| Approval | Read-write separation | Reads auto-execute, writes require user confirmation |

## Bridge Protocol

### Method Namespace

All PIM methods use `pim.` prefix, grouped by application:

| Method | Type | Description |
|--------|------|-------------|
| **Calendar** | | |
| `pim.calendar.list` | Read | Query events in date range |
| `pim.calendar.get` | Read | Get single event details |
| `pim.calendar.create` | Write | Create new event |
| `pim.calendar.update` | Write | Modify existing event |
| `pim.calendar.delete` | Write | Delete event |
| `pim.calendar.calendars` | Read | List all calendars |
| **Reminders** | | |
| `pim.reminders.list` | Read | Query reminders in a list |
| `pim.reminders.get` | Read | Get single reminder details |
| `pim.reminders.create` | Write | Create new reminder |
| `pim.reminders.complete` | Write | Mark reminder complete/incomplete |
| `pim.reminders.delete` | Write | Delete reminder |
| `pim.reminders.lists` | Read | List all reminder lists |
| **Notes** | | |
| `pim.notes.list` | Read | List notes (with folder filter) |
| `pim.notes.get` | Read | Get single note content |
| `pim.notes.create` | Write | Create new note |
| `pim.notes.update` | Write | Modify note content |
| `pim.notes.delete` | Write | Delete note |
| `pim.notes.folders` | Read | List all folders |
| **Contacts** | | |
| `pim.contacts.search` | Read | Search contacts |
| `pim.contacts.get` | Read | Get contact details |
| `pim.contacts.create` | Write | Create new contact |
| `pim.contacts.update` | Write | Modify contact |
| `pim.contacts.delete` | Write | Delete contact |
| `pim.contacts.groups` | Read | List contact groups |

24 methods total, 6 per application (CRUD + list containers).

### Request/Response Examples

```json
// List calendar events
{"jsonrpc":"2.0", "id":"uuid", "method":"pim.calendar.list",
 "params":{"from":"2026-02-27T00:00:00+08:00", "to":"2026-03-06T00:00:00+08:00", "calendar_id": null}}

// Response
{"jsonrpc":"2.0", "id":"uuid", "result":{
  "events":[
    {"id":"EK-xxx", "title":"Weekly Meeting", "start":"2026-02-28T10:00:00+08:00",
     "end":"2026-02-28T11:00:00+08:00", "calendar":"Work", "location":"Room A",
     "notes":"Discuss Q1 plans", "all_day":false, "recurring":false}
  ]}}

// Create reminder
{"jsonrpc":"2.0", "id":"uuid", "method":"pim.reminders.create",
 "params":{"title":"Buy milk", "list":"Shopping", "due_date":"2026-02-28T18:00:00+08:00", "priority": 1, "notes": null}}

// Search contacts
{"jsonrpc":"2.0", "id":"uuid", "method":"pim.contacts.search",
 "params":{"query":"John"}}
```

## Swift Architecture

### File Structure

```
apps/macos-native/Aleph/
├── Bridge/
│   ├── PIMHandlers.swift          ← NEW: Register all pim.* methods, route to Services
│   └── DesktopHandlers.swift      ← EXISTING: Unchanged
├── PIM/                           ← NEW directory
│   ├── CalendarService.swift      ← EventKit calendar operations
│   ├── RemindersService.swift     ← EventKit reminders operations
│   ├── ContactsService.swift      ← Contacts.framework operations
│   └── NotesService.swift         ← AppleScript (osascript) operations
```

### PIMHandlers.swift — Routing Layer

```swift
final class PIMHandlers {
    private let calendar = CalendarService()
    private let reminders = RemindersService()
    private let contacts = ContactsService()
    private let notes = NotesService()

    func register(on server: BridgeServer) {
        // Calendar
        server.registerHandler("pim.calendar.list") { [self] params in
            try await calendar.listEvents(params: params)
        }
        server.registerHandler("pim.calendar.get") { [self] params in
            try await calendar.getEvent(params: params)
        }
        // ... remaining 22 methods follow same pattern
    }
}
```

PIMHandlers does routing only, zero business logic. Consistent with `DesktopHandlers.swift` pattern.

### CalendarService.swift — EventKit Calendar

- Uses `EKEventStore` for all calendar operations
- `ensureAccess()` requests `requestFullAccessToEvents()` on first call
- `predicateForEvents(withStart:end:calendars:)` for date range queries
- Returns serialized dictionaries matching JSON-RPC response format

### RemindersService.swift — EventKit Reminders

- Shares `EKEventStore` pattern with CalendarService
- `requestFullAccessToReminders()` for permission
- `fetchReminders(matching:)` wrapped as async via `withCheckedThrowingContinuation`
- Supports completion toggling and list management

### ContactsService.swift — Contacts Framework

- Uses `CNContactStore` for all contact operations
- `requestAccess(for: .contacts)` for permission
- Predicate-based search (`predicateForContacts(matchingName:)`)
- Fetches standard keys: name, phone, email

### NotesService.swift — AppleScript

- No native Swift API for Notes.app; uses `/usr/bin/osascript`
- AppleScript commands via `Process` execution
- Parses AppleScript text output to structured JSON
- Notes.app auto-launches when invoked (system behavior)

### Info.plist Permission Declarations

```xml
<key>NSCalendarsFullAccessUsageDescription</key>
<string>Aleph needs calendar access to help you manage events.</string>
<key>NSRemindersFullAccessUsageDescription</key>
<string>Aleph needs reminders access to help you manage tasks.</string>
<key>NSContactsUsageDescription</key>
<string>Aleph needs contacts access to help you find people.</string>
<key>NSAppleEventsUsageDescription</key>
<string>Aleph needs automation access to interact with Notes.</string>
```

## Rust Core Architecture

### File Changes

```
core/src/
├── desktop/
│   └── types.rs                ← MODIFY: Extend DesktopRequest enum with PIM variants
├── builtin_tools/
│   ├── pim.rs                  ← NEW: PimTool (AlephTool trait implementation)
│   └── mod.rs                  ← MODIFY: Register PimTool
├── executor/
│   └── builtin_registry/       ← MODIFY: Add pim_tool field to BuiltinToolRegistry
```

### DesktopRequest Extension

24 new variants in `DesktopRequest` enum, one per PIM method. Each variant serializes to its corresponding `pim.*` method name via `method_name()`.

### PimTool

- `const NAME: &str = "pim"`
- `type Args = PimArgs` — flat struct with `action: PimAction` enum + optional fields per action
- `PimAction` enum: 24 variants (`CalendarList`, `CalendarGet`, ..., `ContactsGroups`)
- `PimAction::is_write()` — classifies 12 write actions for approval gating
- Transparent JSON pass-through from Bridge response

### Approval Policy

- Read actions (list, get, search, calendars, lists, folders, groups): auto-execute
- Write actions (create, update, delete, complete): require user confirmation via `ApprovalPolicy`
- Approval description includes action context (e.g., "Create calendar event: Weekly Meeting")

## Error Handling

### Error Codes

| Code | Meaning | Example |
|------|---------|---------|
| `-32001` | Permission denied | "Calendar access denied. Grant in System Settings > Privacy > Calendars." |
| `-32002` | Resource not found | "Event with id 'EK-xxx' not found" |
| `-32003` | Validation failure | "Missing required field 'title' for calendar_create" |
| `-32004` | AppleScript error | "Notes automation error: ..." (Notes only) |
| `-32602` | Invalid params (standard) | "Invalid date format, expected ISO 8601" |

### Graceful Degradation

- Bridge unavailable → "Desktop bridge not connected. PIM features require the macOS app."
- Permission denied → Specific System Settings guidance
- Individual capability unavailable → Other capabilities work normally
- Handshake capability check → "macOS app version too old, please update"

### Handshake Extension

4 new capabilities added to handshake response:
`pim_calendar`, `pim_reminders`, `pim_notes`, `pim_contacts`

## Testing Strategy

### Swift Unit Tests

- Mock `EKEventStore` / `CNContactStore` for CRUD logic
- Mock osascript output for Notes parsing
- Permission denial returns correct error codes
- ISO 8601 date parsing variants
- Empty result sets return `[]`

### Rust Unit Tests

- `PimAction::is_write()` classification correctness
- `PimArgs` → `DesktopRequest` conversion
- ApprovalPolicy integration (reads skip, writes intercept)
- Bridge unavailable graceful degradation

### Integration Tests (Manual)

- All 24 methods on real macOS
- Permission dialog triggering
- Write operation approval flow

## Architecture Diagram

```
                    ┌─────────────────────────────────┐
                    │         Agent / LLM             │
                    │   {"action":"calendar_list",...} │
                    └──────────────┬──────────────────┘
                                   │
                    ┌──────────────▼──────────────────┐
                    │         PimTool (Rust)           │
                    │  • action → DesktopRequest       │
                    │  • is_write() → ApprovalPolicy   │
                    │  • bridge.send(request)          │
                    └──────────────┬──────────────────┘
                                   │ UDS JSON-RPC
                    ┌──────────────▼──────────────────┐
                    │      PIMHandlers (Swift)         │
                    │  • pim.calendar.* → Calendar     │
                    │  • pim.reminders.* → Reminders   │
                    │  • pim.notes.* → Notes           │
                    │  • pim.contacts.* → Contacts     │
                    └──────────────┬──────────────────┘
                                   │
                    ┌──────────────▼──────────────────┐
                    │     macOS Native Frameworks      │
                    │  EventKit │ Contacts │ osascript │
                    └─────────────────────────────────┘
```

## Architectural Compliance

- **R1 (Brain-Limb Separation)**: Core defines capability contract only; all native API calls in Swift layer
- **R2 (Single Source of UI Truth)**: No UI changes; pure data API
- **R4 (I/O-Only Interfaces)**: PIMHandlers is pure I/O routing
- **R7 (One Core, Many Shells)**: PimTool is platform-agnostic; macOS implementation via Bridge
