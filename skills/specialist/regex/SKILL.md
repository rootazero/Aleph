---
name: regex
description: Practical regex cookbook — validation, parsing, replacement patterns across languages
scope: standalone
---

# Regular Expressions

## When to Use

Invoke this skill when writing, debugging, or optimizing regular expressions. It provides ready-to-use patterns organized by use case.

## Quick Reference

### Metacharacters
| Pattern | Matches |
|---------|---------|
| `.` | Any character (except newline) |
| `\d` | Digit [0-9] |
| `\w` | Word character [a-zA-Z0-9_] |
| `\s` | Whitespace |
| `\b` | Word boundary |
| `^` | Start of string/line |
| `$` | End of string/line |
| `[abc]` | Character class |
| `[^abc]` | Negated class |

### Quantifiers
| Pattern | Meaning |
|---------|---------|
| `*` | 0 or more (greedy) |
| `+` | 1 or more (greedy) |
| `?` | 0 or 1 (optional) |
| `{n}` | Exactly n |
| `{n,m}` | Between n and m |
| `*?` `+?` | Lazy (non-greedy) versions |

### Groups
| Pattern | Meaning |
|---------|---------|
| `(abc)` | Capturing group |
| `(?:abc)` | Non-capturing group |
| `(?P<name>abc)` | Named group (Python) |
| `(?<name>abc)` | Named group (JS/Rust) |
| `a\|b` | Alternation (a or b) |

### Lookaround
| Pattern | Meaning |
|---------|---------|
| `(?=abc)` | Lookahead (followed by abc) |
| `(?!abc)` | Negative lookahead |
| `(?<=abc)` | Lookbehind (preceded by abc) |
| `(?<!abc)` | Negative lookbehind |

## Validation Patterns

```
Email (basic):    ^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$
URL:              ^https?://[^\s/$.?#].[^\s]*$
IPv4:             ^(\d{1,3}\.){3}\d{1,3}$
UUID:             ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$
Semver:           ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(-[\w.]+)?(\+[\w.]+)?$
ISO Date:         ^\d{4}-\d{2}-\d{2}$
Phone (intl):     ^\+?[1-9]\d{1,14}$
Hex Color:        ^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$
```

## Parsing Patterns

```
Log line:         ^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})\s+(\w+)\s+(.+)$
                  → timestamp, level, message

Key=Value:        (\w+)=("[^"]*"|\S+)
                  → key, value (handles quoted values)

CSV field:        (?:^|,)("(?:[^"]*"")*[^"]*"|[^,]*)
                  → handles quoted fields with escaped quotes

Markdown heading: ^(#{1,6})\s+(.+)$
                  → level, title

Import statement: ^import\s+(?:\{([^}]+)\}|(\w+))\s+from\s+['"]([^'"]+)['"]
                  → named imports, default import, module path
```

## Replacement Patterns

```
# CamelCase to snake_case
Find:    ([a-z])([A-Z])
Replace: $1_\L$2           (or use lowercase flag)

# Remove trailing whitespace
Find:    \s+$
Replace: (empty)

# Wrap words in quotes
Find:    (\b\w+\b)
Replace: "$1"

# Swap first/last name
Find:    (\w+)\s+(\w+)
Replace: $2, $1
```

## Language-Specific Usage

### JavaScript
```javascript
const re = /pattern/gi;          // literal
const re = new RegExp('pattern', 'gi');  // constructor
str.match(re)                    // all matches
str.replace(re, 'new')          // replace
re.test(str)                     // boolean test
```

### Python
```python
import re
re.search(r'pattern', string)    # first match
re.findall(r'pattern', string)   # all matches
re.sub(r'pattern', 'new', string) # replace
re.compile(r'pattern')           # compile for reuse
```

### Rust
```rust
use regex::Regex;
let re = Regex::new(r"pattern").unwrap();
re.is_match(&text)              // boolean
re.find(&text)                  // first match
re.captures(&text)              // capture groups
re.replace_all(&text, "new")   // replace
```

## Common Gotchas

| Gotcha | Problem | Fix |
|--------|---------|-----|
| Greedy by default | `.*` matches too much | Use `.*?` (lazy) |
| Backtracking | `(a+)+b` on "aaaaac" is exponential | Simplify pattern, use atomic groups |
| `.` doesn't match `\n` | Multiline content not matched | Use `[\s\S]` or `s` flag |
| Escaping | Backslashes in strings need double-escaping | Use raw strings: `r"pattern"` |
| Unicode | `\w` may not match accented chars | Use Unicode categories: `\p{L}` |
| Anchors in multiline | `^$` match string boundaries, not lines | Use `m` flag for per-line |

## Performance Tips

- **Anchor patterns** with `^` and `$` when possible — prevents scanning entire string
- **Avoid nested quantifiers** like `(a+)+` — causes catastrophic backtracking
- **Compile and reuse** regex objects (don't recompile in loops)
- **Use non-capturing groups** `(?:...)` when you don't need the capture
- **Be specific** — `\d{4}` is faster than `\d+` for a 4-digit year
