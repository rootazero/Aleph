# Change: Harden Dispatcher Data Processing and Security

## Status
- **Stage**: Proposal
- **Created**: 2026-01-10
- **Priority**: High

## Why

A comprehensive analysis of the intelligent intent detection, tool dispatch center, and AI auto-tool invocation system revealed several critical issues that need to be addressed:

### Critical Issues (High Priority)
1. **JSON Parsing Vulnerability**: The `extract_json_from_response()` function uses greedy matching (`rfind('}')`), which can incorrectly parse responses containing multiple JSON objects
2. **Prompt Injection Risk**: User input is directly concatenated into L3 routing prompts without sanitization, allowing potential manipulation of AI behavior
3. **Inconsistent JSON Logic**: Two separate JSON extraction functions exist with different behaviors, leading to maintenance burden and inconsistent results

### Medium Priority Issues
4. **Confidence Threshold Confusion**: Three different confidence thresholds (`l3_min_confidence`, `confirmation_threshold`, `auto_execute_threshold`) with unclear relationships
5. **No Timeout Graceful Degradation**: L3 router returns errors on timeout instead of gracefully falling back to chat mode
6. **Tool List Filtering Performance**: No caching for tool list filtering, causing redundant computation on every request

### Low Priority Issues
7. **Incomplete PII Coverage**: Missing patterns for Chinese phone numbers, Chinese ID cards, and bank card numbers
8. **No Token Counting**: Prompt construction lacks token estimation, risking context window overflow
9. **Incomplete Parameter Schema**: Tool parameters only show names, missing type constraints and requirements

## What Changes

### 1. JSON Parsing Fix (Critical)
- Replace greedy `rfind('}')` with proper brace-matching algorithm
- Consolidate duplicate JSON extraction functions into single robust implementation
- Add validation that extracted JSON is syntactically valid

### 2. Prompt Injection Protection (Critical)
- Add `sanitize_for_prompt()` function to neutralize injection markers
- Apply sanitization before constructing L3 routing prompts
- Escape or remove control sequences like `[TASK]`, `[SYSTEM]`, markdown code blocks

### 3. Unified Confidence Configuration (Medium)
- Define clear semantic hierarchy for confidence thresholds
- Add configuration validation to ensure logical ordering
- Document the relationship between thresholds

### 4. Timeout Graceful Degradation (Medium)
- Change timeout behavior from error to `Ok(None)` (fallback to chat)
- Add logging for degradation events
- Make degradation behavior configurable

### 5. PII Pattern Extension (Low)
- Add Chinese mobile phone pattern: `1[3-9]\d{9}`
- Add Chinese ID card pattern: `\d{17}[\dXx]`
- Add bank card number pattern: `\d{16,19}`

### 6. Tool List Caching (Low)
- Add LRU cache for filtered tool lists
- Invalidate cache on tool registry changes
- Configure cache size and TTL

## Impact

### Affected Specs
- `ai-routing` - MODIFIED: Add prompt sanitization requirement
- New spec: `data-security` - PII scrubbing and prompt injection protection
- New spec: `dispatcher-resilience` - Graceful degradation and timeout handling

### Affected Code
- `core/src/dispatcher/prompt_builder.rs` - JSON parsing, prompt sanitization
- `core/src/dispatcher/l3_router.rs` - Timeout handling, confidence checks
- `core/src/utils/pii.rs` - Extended PII patterns
- `core/src/dispatcher/integration.rs` - Unified confidence configuration

### Breaking Changes
- **None** - All changes are backward compatible
- Existing behavior preserved as default
- Security hardening is transparent to users

### Risk Assessment
| Change | Risk | Mitigation |
|--------|------|------------|
| JSON parsing fix | Low | Comprehensive test coverage |
| Prompt sanitization | Low | Sanitization is additive, preserves content |
| Timeout degradation | Low | Configurable, can revert to error behavior |
| PII extension | Low | New patterns only add coverage |

## Design Decisions

### Why brace-matching instead of regex for JSON?
- Regex cannot reliably match nested JSON structures
- Brace-matching handles arbitrary nesting depth
- Simpler to understand and maintain

### Why sanitize instead of structured prompts?
- Structured prompts require provider-specific formatting
- Sanitization works with current string-based approach
- Incremental improvement without architecture change

### Why fallback to chat instead of retry?
- Retries add latency and cost
- Chat mode provides immediate user feedback
- User can explicitly retry if needed

## Success Criteria
1. JSON parsing correctly handles multiple JSON objects in response
2. Prompt injection attempts are neutralized (test with known attack patterns)
3. Timeout events result in graceful degradation, not user-visible errors
4. Chinese PII patterns are correctly detected and scrubbed
5. All existing tests continue to pass
