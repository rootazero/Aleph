# Tasks: Unify Intent Detection with Conversation

## Phase 1: Simplify Configuration

- [x] **1.1** Remove `ai_first` field from `IntentDetectionConfig` in `config/mod.rs`
- [x] **1.2** Update config loading to not require `ai_first` field
- [ ] **1.3** Update default config in documentation

## Phase 2: Remove Legacy Intent Detection

- [x] **2.1** Delete `intent/patterns.rs` (weather-specific patterns)
- [x] **2.2** Delete `intent/smart_trigger.rs` (regex-based triggers)
- [x] **2.3** Simplify `intent/mod.rs` - keep only `AiIntentDetector` exports
- [x] **2.4** Remove `IntentDetector` struct and `IntentType::Weather`
- [x] **2.5** Remove `detect_intent_and_complete_params` from `core.rs` (legacy flow)
- [x] **2.6** Update `core.rs` to remove references to deleted modules
- [x] **2.7** Remove `process_with_ai_internal` method (unused after refactoring)

## Phase 3: Ensure AI-First is Always Used

- [x] **3.1** Verify `start_conversation()` always calls `process_with_ai_first()` (done)
- [x] **3.2** Verify `continue_conversation()` always calls `process_with_ai_first()` (done)
- [x] **3.3** Remove conditional logic checking `ai_first` config flag
- [x] **3.4** Update `process_with_ai()` to directly call `process_with_ai_first()`

## Phase 4: Focus Management (Swift)

- [x] **4.1** Ensure target app is activated before paste in `AppDelegate` (already implemented)
- [x] **4.2** Add auto-focus to continuation input field in `HaloWindow` (already implemented via IMETextField)
- [x] **4.3** Ensure ESC key closes continuation input properly (already implemented)
- [ ] **4.4** Test focus behavior with different applications (manual testing needed)

## Phase 5: Cleanup and Validation

- [x] **5.1** Run `cargo build` - ensure no compile errors
- [x] **5.2** Run `cargo test --lib` - 529 tests pass
- [ ] **5.3** Run `cargo clippy` - fix any warnings
- [ ] **5.4** Manual test: "今天天气怎么样" triggers clarification
- [ ] **5.5** Manual test: Provide city → search executes → response returned
- [ ] **5.6** Manual test: Multi-turn conversation works with intent detection

## Dependencies

```
Phase 1 ──► Phase 2 ──► Phase 3 ──► Phase 5
                              │
Phase 4 ──────────────────────┘
```

- Phase 2 depends on Phase 1 (config changes first)
- Phase 3 depends on Phase 2 (remove legacy before unifying)
- Phase 4 can run in parallel
- Phase 5 depends on all others

## Verification Criteria

1. **Build Success**: `cargo build --release` completes without errors
2. **Tests Pass**: `cargo test` passes (after removing legacy tests)
3. **Manual Verification**:
   - Input "今天天气怎么样" → AI asks for location → User selects → Search runs
   - Input "summarize this video" → AI asks for URL → User provides → Video capability runs
   - Multi-turn conversation maintains context
   - Cursor focuses in Halo input, returns to target app after paste

## Summary of Changes

### Deleted Files
- `intent/patterns.rs` - Weather-specific regex patterns
- `intent/smart_trigger.rs` - Legacy trigger detection

### Modified Files
- `config/mod.rs` - Removed `ai_first` field from IntentDetectionConfig
- `core.rs` - Simplified processing flow to always use AI-first mode
- `intent/mod.rs` - Simplified to only export AiIntentDetector
- `lib.rs` - Updated exports

### Lines Removed
- ~700 lines of legacy intent detection code
- ~500 lines of unused `process_with_ai_internal` method
