---
name: refactor
description: Safe code refactoring — smell detection, technique selection, test-protected incremental changes
scope: standalone
---

# Code Refactoring

## When to Use

Invoke this skill when code needs restructuring without changing external behavior. Always refactor separately from bug fixes and feature additions.

## Prerequisites

Before refactoring, you MUST have:

1. **Passing tests** covering the code to refactor
2. If no tests exist, write **characterization tests** first (tests that capture current behavior, right or wrong)
3. A clear statement of **what smell you're removing** and **what the target structure looks like**

## Code Smell Catalog

| Smell | Symptom | Refactoring |
|-------|---------|-------------|
| **Long method** | Function > 30 lines, does multiple things | Extract Method |
| **God class** | Class with 10+ responsibilities | Extract Class, move methods to collaborators |
| **Feature envy** | Method uses another class's data more than its own | Move Method to the data's class |
| **Shotgun surgery** | One change requires edits in 10+ files | Move related logic into one module |
| **Primitive obsession** | Using strings/ints where a value object fits | Introduce Value Object / Newtype |
| **Long parameter list** | Function takes 5+ parameters | Introduce Parameter Object or Builder |
| **Duplicate code** | Same logic in 3+ places | Extract into shared function |
| **Dead code** | Unreachable code, unused variables/functions | Delete it. Don't comment it out. |
| **Deep nesting** | 4+ levels of if/for/match | Early return, extract method, flatten |
| **Magic numbers** | Unexplained literals in code | Extract to named constant |

## Refactoring Techniques

### Extract Method
Take a code block and turn it into a named function. The name should describe **what** it does, not **how**.

### Extract Class
Split a large class into smaller, focused classes. Each class should have one reason to change.

### Inline
The opposite of extract — when indirection adds complexity without value, collapse it.

### Rename
The cheapest refactoring with the highest impact. Good names eliminate comments.

### Replace Conditional with Polymorphism
When a switch/match dispatches behavior by type, use trait/interface implementations instead.

### Introduce Parameter Object
Group related parameters into a struct/class. Reduces parameter count and creates a named concept.

## Safety Protocol

**Every refactoring step follows this cycle:**

```
1. Run tests -> all green
2. Make ONE structural change (extract, move, rename, inline)
3. Run tests -> all green
4. Commit
5. Repeat
```

If tests break: revert the last change (`git checkout -- .`) and try a smaller step.

**Never** combine multiple refactoring techniques in one commit. One commit = one transformation.

## Aleph Conventions

Aleph's CODE_ORGANIZATION.md defines these rules:
- Files > 500 lines should be split
- One public type per file (with exceptions for small related types)
- Module structure mirrors domain concepts

When refactoring Aleph code, respect these boundaries.

## Anti-Patterns

- **Refactoring without tests**: You're changing structure blind — bugs will hide in the gaps
- **Refactoring during debugging**: Fix the bug first, refactor later — never both at once
- **Big bang refactoring**: Rewriting an entire module at once. Small steps, always.
- **Refactoring for aesthetics**: "This could be cleaner" is not a reason unless it causes real problems
- **Premature abstraction**: Don't extract a pattern until you've seen it 3 times
