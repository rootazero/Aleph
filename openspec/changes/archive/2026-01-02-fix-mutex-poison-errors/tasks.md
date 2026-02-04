# Implementation Tasks: Fix Mutex Poison Errors

**Change ID**: `fix-mutex-poison-errors`
**Status**: Ready for Implementation
**Priority**: P0 (Critical)

## Task Breakdown

### Phase 1: Fix Unsafe Mutex Operations (Critical) ⚠️

**Estimated Time**: 1-2 hours
**Priority**: P0 - Must complete immediately

#### Task 1.1: Fix is_typewriting Mutex (2 occurrences)

**File**: `Aleph/core/src/core.rs`

**Locations**:
- Line 433: `cancel_typewriter()` method
- Line 446: `is_typewriting()` method

**Changes**:
```rust
// Line 433 - BEFORE
let is_typing = *self.is_typewriting.lock().unwrap();

// Line 433 - AFTER
let is_typing = *self.is_typewriting.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in is_typewriting (cancel_typewriter), recovering");
    e.into_inner()
});

// Line 446 - BEFORE
*self.is_typewriting.lock().unwrap()

// Line 446 - AFTER
*self.is_typewriting.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in is_typewriting (is_typewriting), recovering");
    e.into_inner()
})
```

**Validation**:
- ✅ Compile succeeds
- ✅ Unit test: Verify typewriter state access after simulated panic
- ✅ Manual test: Press ESC during typewriter animation

---

#### Task 1.2: Fix last_request Mutex (3 occurrences)

**File**: `Aleph/core/src/core.rs`

**Locations**:
- Line 460: `retry_last_request()` method
- Line 517: `store_request_context()` method
- Line 527: `clear_request_context()` method

**Changes**:
```rust
// Line 460 - BEFORE
let mut last_request_lock = self.last_request.lock().unwrap();

// Line 460 - AFTER
let mut last_request_lock = self.last_request.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in last_request (retry_last_request), recovering");
    e.into_inner()
});

// Line 517 - BEFORE
let mut last_request = self.last_request.lock().unwrap();

// Line 517 - AFTER
let mut last_request = self.last_request.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in last_request (store_request_context), recovering");
    e.into_inner()
});

// Line 527 - BEFORE
let mut last_request = self.last_request.lock().unwrap();

// Line 527 - AFTER
let mut last_request = self.last_request.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in last_request (clear_request_context), recovering");
    e.into_inner()
});
```

**Validation**:
- ✅ Compile succeeds
- ✅ Unit test: Verify retry mechanism after simulated panic
- ✅ Manual test: Trigger error and retry from Halo UI

---

#### Task 1.3: Fix current_context Mutex (4 occurrences)

**File**: `Aleph/core/src/core.rs`

**Locations**:
- Line 635: `set_current_context()` method
- Line 684: `store_interaction_memory()` method
- Line 768: `retrieve_and_augment_prompt()` method
- Line 1444: Another occurrence (need to verify exact context)

**Changes**:
```rust
// Line 635 - BEFORE
let mut current_context = self.current_context.lock().unwrap();

// Line 635 - AFTER
let mut current_context = self.current_context.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in current_context (set_current_context), recovering");
    e.into_inner()
});

// Line 684 - BEFORE
let current_context = self.current_context.lock().unwrap();

// Line 684 - AFTER
let current_context = self.current_context.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in current_context (store_interaction_memory), recovering");
    e.into_inner()
});

// Line 768 - BEFORE
let current_context = self.current_context.lock().unwrap();

// Line 768 - AFTER
let current_context = self.current_context.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in current_context (retrieve_and_augment_prompt), recovering");
    e.into_inner()
});

// Line 1444 - BEFORE
let current_context = self.current_context.lock().unwrap();

// Line 1444 - AFTER
let current_context = self.current_context.lock().unwrap_or_else(|e| {
    warn!("Mutex poisoned in current_context, recovering");
    e.into_inner()
});
```

**Validation**:
- ✅ Compile succeeds
- ✅ Unit test: Verify context capture after simulated panic
- ✅ Manual test: Use hotkey in different apps to verify context switching

---

#### Task 1.4: Rebuild Rust Core

**Command**:
```bash
cd Aleph/core
cargo clean
cargo build --release
cargo run --bin uniffi-bindgen -- generate --library target/release/libaethecore.dylib --language swift --out-dir ../Sources/Generated/
cp target/release/libaethecore.dylib ../Frameworks/
```

**Validation**:
- ✅ Build succeeds without errors
- ✅ UniFFI bindings generated successfully
- ✅ dylib copied to Frameworks directory
- ✅ File size matches expected (~9-10 MB)

---

### Phase 2: Core Initialization Validation (High Priority)

**Estimated Time**: 2-3 hours
**Priority**: P1 - Complete after Phase 1

#### Task 2.1: Add isInitialized() Method to Rust Core

**File**: `Aleph/core/src/core.rs`

**Changes**:
```rust
impl AlephCore {
    /// Check if core is fully initialized and ready to process requests
    pub fn is_initialized(&self) -> bool {
        // Check all critical components are initialized
        self.router.is_some()
            && self.memory_db.is_some()
            && self.config.lock().is_ok()
    }
}
```

**File**: `Aleph/core/src/aleph.udl` (UniFFI interface)

**Changes**:
```idl
interface AlephCore {
    // ... existing methods ...

    // NEW: Initialization check
    boolean is_initialized();
};
```

**Validation**:
- ✅ Compile succeeds
- ✅ UniFFI generates Swift binding
- ✅ Method callable from Swift

---

#### Task 2.2: Add Initialization Check in Swift Hotkey Handler

**File**: `Aleph/Sources/AppDelegate.swift`

**Location**: Line ~747 (after showing Halo)

**Changes**:
```swift
// NEW: Check if core is fully initialized
guard let core = core, core.isInitialized() else {
    print("[AppDelegate] ❌ Core not fully initialized")
    DispatchQueue.main.async { [weak self] in
        self?.haloWindow?.updateState(.error(
            type: .unknown,
            message: NSLocalizedString("error.core_initializing", comment: ""),
            suggestion: NSLocalizedString("error.core_initializing.suggestion", comment: "")
        ))
    }
    return
}
```

**Validation**:
- ✅ Compile succeeds
- ✅ Manual test: Press hotkey immediately after app launch
- ✅ Verify error message appears if core not ready

---

### Phase 3: Error Handling Improvements (Medium Priority)

**Estimated Time**: 3-4 hours
**Priority**: P2 - Nice to have

#### Task 3.1: Add Try-Catch Around Core Method Calls

**File**: `Aleph/Sources/AppDelegate.swift`

**Location**: Line ~792 (`processInput()` call)

**Changes**:
```swift
do {
    let response = try core.processInput(
        userInput: userInput,
        context: capturedContext
    )
    // ... existing code ...
} catch let error as AlephException {
    // Handle typed Rust errors
    print("[AppDelegate] ❌ Core error: \(error)")
    DispatchQueue.main.async { [weak self] in
        self?.haloWindow?.updateState(.error(
            type: .apiError,  // Map from exception type
            message: error.message,
            suggestion: error.suggestion
        ))
    }
} catch {
    // Handle unexpected errors
    print("[AppDelegate] ❌ Unexpected error: \(error)")
    DispatchQueue.main.async { [weak self] in
        self?.haloWindow?.updateState(.error(
            type: .unknown,
            message: error.localizedDescription,
            suggestion: "Please try again or restart the app"
        ))
    }
}
```

**Validation**:
- ✅ Compile succeeds
- ✅ Simulate network error (disconnect WiFi)
- ✅ Verify Halo shows error with retry option

---

#### Task 3.2: Add Localized Error Messages

**File**: `Aleph/Sources/Localizable.xcstrings`

**New Keys**:
```json
{
  "error.core_initializing": {
    "en": "Aleph is still starting up...",
    "zh-Hans": "Aleph 正在启动中..."
  },
  "error.core_initializing.suggestion": {
    "en": "Please wait a moment and try again",
    "zh-Hans": "请稍等片刻后重试"
  },
  "error.mutex_poisoned": {
    "en": "Internal state error detected",
    "zh-Hans": "检测到内部状态错误"
  },
  "error.mutex_poisoned.suggestion": {
    "en": "Aleph recovered automatically. If this persists, restart the app.",
    "zh-Hans": "Aleph 已自动恢复。如果持续出现，请重启应用。"
  }
}
```

**Validation**:
- ✅ Strings appear in both English and Chinese
- ✅ Manual test: Trigger errors and verify translations

---

## Testing Checklist

### Unit Tests

- [ ] Test Mutex recovery in `is_typewriting`
- [ ] Test Mutex recovery in `last_request`
- [ ] Test Mutex recovery in `current_context`
- [ ] Test `isInitialized()` returns correct state
- [ ] Test error handling paths in Swift

### Integration Tests

- [ ] Simulate panic during config read
- [ ] Simulate panic during context capture
- [ ] Simulate panic during provider call
- [ ] Verify graceful recovery in all cases

### Manual Testing

**Scenario 1: Selected Text (Previously PoisonError)**
1. Open Notes.app
2. Type: "测试选中文本"
3. Select all (Cmd+A)
4. Press ` key
5. ✅ Verify: AI processes text without error
6. ✅ Verify: Response is typed back

**Scenario 2: Unselected Text (Previously Silent Failure)**
1. Open Notes.app
2. Type: "测试未选中文本"
3. Do NOT select (cursor in text)
4. Press ` key
5. ✅ Verify: Halo appears at cursor
6. ✅ Verify: No beep sound
7. ✅ Verify: AI processes and responds

**Scenario 3: Settings Menu (Previously Crash)**
1. Launch Aleph
2. Click menu bar icon
3. Click "Settings"
4. ✅ Verify: Settings window opens
5. ✅ Verify: No crash

**Scenario 4: Early Hotkey Press**
1. Launch Aleph
2. Immediately press ` key (before core fully initialized)
3. ✅ Verify: Error message appears
4. ✅ Verify: Suggested action displayed
5. Wait 2 seconds
6. Press ` key again
7. ✅ Verify: Works normally

---

## Rollout Plan

### Step 1: Phase 1 Implementation (Immediate)
- Implement all Mutex fixes
- Rebuild Rust core
- Test locally

### Step 2: User Testing
- Deploy to test user
- Monitor crash reports
- Collect feedback

### Step 3: Phase 2 Implementation (If Phase 1 Successful)
- Add initialization checks
- Test robustness

### Step 4: Phase 3 Implementation (Optional)
- Improve error messages
- Add localization

---

## Success Metrics

- **Crash Rate**: Should drop to 0% for Mutex-related crashes
- **Error Recovery**: 100% of poison errors should be recovered
- **User Feedback**: No reports of PoisonError or silent failures
- **Performance**: No measurable impact on latency

---

## Rollback Plan

If issues are discovered:
1. Revert to previous `libaethecore.dylib` (backup before deployment)
2. Restore previous Swift code via git
3. Investigate logs and crash reports
4. Create hotfix if specific issue identified

---

## Notes

- All changes are backward compatible
- No database migrations required
- No config file changes required
- No user action required after update
