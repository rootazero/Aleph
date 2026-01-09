# Tasks: Fix Typewriter Output Issues

## Implementation Tasks

### Phase 1: Diagnose and Fix Core Issue

- [ ] **Task 1.1**: Add diagnostic logging to typeCharacter and typeSpecialKey
  - Log each character being typed
  - Log success/failure status
  - Log timing information
  - **Validation**: Run typewriter in Notes, check logs for where failure occurs

- [ ] **Task 1.2**: Fix newline character handling
  - Change `\n` handling from `kVK_Return` to Unicode string input
  - Test newline insertion in Notes.app
  - **Validation**: Typewriter correctly inserts newlines without triggering special behaviors

- [ ] **Task 1.3**: Increase inter-event delays
  - Change 10ms delay to 20ms between key down/up
  - Add 30ms delay after each complete character
  - Make delays configurable via constants
  - **Validation**: No more beep sounds during output

### Phase 2: Add Reliability Improvements

- [ ] **Task 2.1**: Implement retry logic for failed key events
  - Add max retry count (3 attempts)
  - Add exponential backoff (20ms, 40ms, 80ms)
  - Log retry attempts
  - **Validation**: Transient failures are recovered automatically

- [ ] **Task 2.2**: Add fallback to clipboard for problematic characters
  - Detect when character fails to type after retries
  - Fall back to clipboard paste for that character
  - Restore original clipboard after
  - **Validation**: All characters eventually output even if CGEvent fails

### Phase 3: Testing and Validation

- [ ] **Task 3.1**: Test in Notes.app
  - Test with short text (< 50 chars)
  - Test with long text (> 500 chars)
  - Test with multiple paragraphs
  - Test with special characters (emoji, CJK)
  - **Validation**: All tests pass without beeps or interruption

- [ ] **Task 3.2**: Test in other applications
  - TextEdit
  - VSCode
  - Slack
  - WeChat
  - **Validation**: Typewriter works consistently across apps

- [ ] **Task 3.3**: Verify settings integration
  - Test different typing speeds (slow/medium/fast)
  - Test instant mode still works
  - Test ESC cancellation
  - **Validation**: All behavior settings work correctly

## Dependencies

- Task 1.2 depends on Task 1.1 (need diagnostics to verify fix)
- Task 2.1 depends on Task 1.3 (timing must be fixed first)
- Phase 3 depends on Phase 1 and Phase 2 completion

## Parallel Work

- Tasks 1.2 and 1.3 can be done in parallel
- Tasks 3.1 and 3.2 can be done in parallel after Phase 2
