---
name: search
description: Technical search strategies — code archaeology, documentation retrieval, web research
scope: standalone
---

# Technical Search

## When to Use

Invoke this skill when you need to find information — definitions in code, documentation for a library, answers to technical questions, or understanding how existing code works.

## Search Mode Selection

| I Need To... | Mode | Primary Tools |
|--------------|------|---------------|
| Find where something is defined | Code Archaeology | Grep, Glob |
| Understand how code works | Code Reading | Read, Grep |
| Find library documentation | Documentation | WebFetch, WebSearch |
| Answer a technical question | Web Research | WebSearch |
| Understand commit history | History | `git log`, `git blame` |

## Code Archaeology

### Find Definition
```
Glob: **/<ClassName>.{rs,ts,py}    # find file by name pattern
Grep: "struct ClassName"            # find struct/class definition
Grep: "fn function_name"           # find function definition
Grep: "def function_name"          # Python
Grep: "export.*ClassName"          # TypeScript/JavaScript export
```

### Trace Usage
```
Grep: "ClassName"                   # all references
Grep: "function_name("             # all call sites
Grep: "use.*module::ClassName"     # Rust imports
Grep: "import.*ClassName"          # JS/Python imports
```

### Understand History
```bash
git log --oneline -20 -- <file>    # recent changes to file
git blame <file>                    # who changed each line and when
git log -p -S "search_term"        # commits that added/removed this string
git log --all --grep="keyword"     # commits with keyword in message
```

### Find Patterns
```
Grep: "TODO|FIXME|HACK|XXX"       # find tech debt markers
Grep: "unsafe"                     # find unsafe blocks (Rust)
Grep: "\.unwrap()"                 # find potential panics (Rust)
Grep: "console\.(log|error)"      # find debug logging (JS)
```

## Documentation Retrieval

### Priority Order
1. **Official docs** — Most accurate, most up-to-date
2. **GitHub repository** — README, examples, issues, discussions
3. **Stack Overflow** — Community-verified solutions
4. **Blog posts** — Tutorials (verify date — may be outdated)

### Using WebFetch
```
WebFetch URL with specific prompt:
- "Extract the API for <ClassName>"
- "Find the configuration options for <feature>"
- "Show examples of <usage pattern>"
```

### Using WebSearch
```
WebSearch: "<library> <specific question> site:docs.rs"     # Rust
WebSearch: "<library> <specific question> site:docs.python.org"  # Python
WebSearch: "<library> <specific question> 2026"             # recent results
```

## Research Methodology

### 1. Define the Question
Be specific: "How does Tokio handle task cancellation?" not "Tokio help"

### 2. Search
Start narrow, widen if needed:
- Exact error message first
- Then core concept + language/framework
- Then broader architectural question

### 3. Evaluate Sources
- Official docs > GitHub issues > Stack Overflow > blog posts
- Check dates — anything older than 2 years may be outdated
- Check versions — solutions for v1 may not work for v2

### 4. Synthesize
Combine findings into a coherent answer. Cross-reference multiple sources.

## Anti-Patterns

- **Searching too broadly**: "how to code" — be specific
- **Trusting first result**: Cross-reference at least 2 sources
- **Ignoring version**: Solution for React 17 may break React 19
- **Not reading error messages**: The error message often tells you exactly what's wrong
- **Searching when you should read**: If you have the source code, `Read` it
