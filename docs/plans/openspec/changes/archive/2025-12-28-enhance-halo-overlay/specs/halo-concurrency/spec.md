# Halo Concurrency Specification

## ADDED Requirements

### Requirement: Queue Pending Hotkey Operations
EventHandler SHALL queue up to 3 pending hotkey operations when Halo is already processing.

#### Scenario: Rapid hotkey presses enqueue operations

**Given** Halo is processing first operation (isProcessing = true)
**When** user presses Cmd+~ three more times rapidly (within 1 second)
**Then** operations 2, 3, 4 are added to pendingOperations queue
**And** queue depth is 3
**And** Halo displays queue badge "3 pending"
**And** operations remain queued until first completes

---

#### Scenario: Queue full error on 4th rapid press

**Given** pendingOperations queue has 3 items (max capacity)
**When** user presses Cmd+~ again
**Then** EventHandler rejects operation
**And** Halo briefly shows "Queue full" error overlay (2s duration)
**And** original processing continues uninterrupted
**And** no crash or hang occurs

---

### Requirement: Sequential FIFO Processing of Queued Operations
Queued operations SHALL process sequentially (FIFO) with 0.5s delay between operations.

#### Scenario: Sequential processing of queued operations

**Given** pendingOperations contains [Op2, Op3, Op4]
**When** first operation completes (success or error)
**Then** EventHandler sets isProcessing = false
**And** waits 0.5 seconds
**Then** dequeues Op2 (first in queue)
**And** processes Op2 (reads clipboard, sends to AI)
**And** sets isProcessing = true
**And** queue badge updates to "2 pending"

**When** Op2 completes
**Then** waits 0.5s, processes Op3
**And** queue badge updates to "1 pending"

**When** Op3 completes
**Then** waits 0.5s, processes Op4
**And** queue badge hides (queue empty)

---

### Requirement: Display Queue Badge for Pending Operations
Queue badge SHALL display pending operation count in top-right corner of Halo overlay.

#### Scenario: Queue badge appearance

**Given** pendingOperations has 2 items
**When** Halo is visible
**Then** QueueBadge renders in top-right corner (offset +40px x, +40px y)
**And** displays "2" in white text on red circular background
**And** badge size is 24x24 pixels
**And** badge fades in with 0.2s animation when queue grows
**And** badge updates count immediately on queue changes

---

#### Scenario: Queue badge hides when queue empty

**Given** queue badge is visible showing "1"
**When** last queued operation begins processing
**Then** pendingOperations becomes empty
**And** queue badge fades out (0.2s duration)
**And** badge completely removed from view hierarchy

---

### Requirement: Maintain isProcessing Flag for Concurrency Control
EventHandler SHALL maintain isProcessing flag to prevent concurrent hotkey handling.

#### Scenario: Guard against concurrent processing

**Given** isProcessing = true (operation in progress)
**When** onHotkeyDetected() is called
**Then** guard clause checks isProcessing
**And** skips immediate processing
**And** either enqueues operation (if queue not full)
**And** or shows "Queue full" error (if queue at capacity)
**And** function returns early (no concurrent execution)

---

### Requirement: Clear Queue on App Background or Quit
Queue SHALL clear automatically when app enters background or user quits.

#### Scenario: Queue clears on app quit

**Given** pendingOperations has 2 items
**When** user quits app via menu bar → Quit
**Then** applicationWillTerminate() is called
**And** EventHandler clears pendingOperations
**And** isProcessing resets to false
**And** no operations persist across app launches

---

## Cross-References

- **Related Specs**: `event-handler` (hotkey detection), `halo-theming` (badge overlay)
- **Depends On**: Phase 2 hotkey handling foundation
- **Blocks**: None
