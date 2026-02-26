---
name: test
description: Unified testing workflow — TDD cycles, framework selection, test patterns across languages
scope: standalone
---

# Testing & TDD

## When to Use

Invoke this skill when writing tests, setting up test infrastructure, running TDD cycles, or analyzing test coverage. It provides a unified approach across frameworks.

## Framework Detection

Before writing tests, identify the project's framework:

| Config File | Framework | Run Command |
|-------------|-----------|-------------|
| `Cargo.toml` | Rust built-in | `cargo test` |
| `vitest.config.*` | Vitest | `npx vitest` |
| `jest.config.*` | Jest | `npx jest` |
| `pyproject.toml` (pytest) | pytest | `pytest` |
| `Package.swift` | XCTest | `swift test` |
| `playwright.config.*` | Playwright | `npx playwright test` |

If no test config exists, recommend the idiomatic default for the language.

## TDD Cycle (POE-Mapped)

### Red — Define the Contract (Principle)

Write a failing test that describes the desired behavior. This IS the success contract.

```
1. Name the test after the behavior, not the implementation
   Good: test_returns_empty_list_when_no_items_match
   Bad:  test_filter_function

2. Use Arrange-Act-Assert structure:
   Arrange: Set up inputs and expected outputs
   Act:     Call the function/method under test
   Assert:  Verify the result matches expectation

3. Run the test — it MUST fail
   If it passes, either the test is wrong or the feature already exists
```

### Green — Minimal Implementation (Operation)

Write the **minimum code** to make the test pass. No more.

```
- Hardcode the return value if that passes the test
- Resist adding error handling for cases not yet tested
- Resist making it "clean" — that's the next step
- Run the test — it MUST pass
```

### Refactor — Clean Up (Evaluation)

Improve the code while keeping all tests green.

```
- Remove duplication
- Improve naming
- Extract helper functions if repeated
- Run ALL tests after each change — they MUST stay green
```

### Cycle Complete -> Commit -> Next Test

## What to Test

**Always test:**
- Public API / exported functions
- Edge cases: empty input, null/None, boundary values, overflow
- Error paths: invalid input, network failures, missing resources
- Business logic: calculations, state transitions, conditional branches

**Don't test:**
- Private implementation details (they change without affecting behavior)
- Framework internals (someone else tests those)
- Trivial getters/setters with no logic
- Third-party library behavior

## Test Patterns

### Isolation

Each test must be independent. No test should depend on another test's state.

```
- Reset state in setup/beforeEach/setUp
- Use fresh fixtures per test
- Avoid shared mutable state between tests
- Tests must pass in any order
```

### Mocking

Mock external dependencies, not internal collaborators:

```
Mock:  HTTP clients, databases, file systems, clocks, random
Don't: Internal classes, helper functions, data transformations
```

Use the **narrowest mock possible** — mock the specific method, not the entire object.

### Test Naming

Format: `test_<behavior>_when_<condition>_expects_<outcome>`

```
Good: test_login_with_invalid_password_returns_401
Good: test_search_with_empty_query_returns_all_items
Good: test_checkout_with_expired_coupon_ignores_discount
```

## Coverage

Coverage measures lines/branches executed, not quality. Use it to find **untested paths**, not as a target.

- 80% line coverage is a reasonable floor
- 100% is usually not worth the effort
- Branch coverage matters more than line coverage
- A single well-designed test beats ten shallow ones

## Anti-Patterns

- **Testing implementation**: Assert on internal state instead of observable behavior
- **Flaky tests**: Tests that sometimes pass, sometimes fail — fix or delete immediately
- **Test interdependence**: Test B fails when Test A doesn't run first
- **Excessive mocking**: Mocking everything makes tests brittle and meaningless
- **No assertion**: A test that runs code but checks nothing
- **Copy-paste tests**: Duplicated test code — extract helpers instead
