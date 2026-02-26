---
name: typeset
description: Document rendering — Typst/LaTeX to PDF compilation, math rendering, templates
emoji: "📄"
category: automation
allowed-tools:
  - Bash
requirements:
  binaries:
    - curl
  install:
    - manager: brew
      package: typst
triggers:
  - typeset
  - LaTeX
  - Typst
  - PDF
  - render document
  - compile document
---

# Document Rendering (Typst/LaTeX)

## When to Use

Invoke this skill when you need to create professional documents (PDF): papers, reports, resumes, presentations, or math-heavy content. Supports Typst (recommended, modern) and LaTeX (legacy compatible).

## Why Typst Over LaTeX

| Aspect | Typst | LaTeX |
|--------|-------|-------|
| Syntax complexity | Simple, markdown-like | Verbose, lots of backslashes |
| Compilation speed | Instant (<1s) | Slow (seconds to minutes) |
| Error messages | Clear, human-readable | Cryptic, hard to debug |
| Package management | Built-in | texlive (GB-sized) |
| Feature parity | 95%+ for most documents | 100% (decades of packages) |

**Recommendation**: Use Typst for new documents. Use LaTeX only for existing LaTeX projects or when a specific LaTeX package is required.

## Prerequisites

```bash
# Option A: Local Typst (recommended)
brew install typst
typst --version

# Option B: Remote API (no install needed)
# Uses TypeTex API — just needs curl (pre-installed)
```

## Typst Quick Start

### Create a Document

```typst
// document.typ
#set page(paper: "a4", margin: 2cm)
#set text(font: "New Computer Modern", size: 11pt)
#set heading(numbering: "1.1")

= My Document Title

== Introduction

This is a paragraph with *bold* and _italic_ text.

=== Math Support

The quadratic formula: $ x = (-b plus.minus sqrt(b^2 - 4a c)) / (2a) $

Display math:
$ integral_0^infinity e^(-x^2) d x = sqrt(pi) / 2 $

=== Tables

#table(
  columns: (auto, auto, auto),
  [*Name*], [*Role*], [*Score*],
  [Alice], [Engineer], [95],
  [Bob], [Designer], [88],
)

=== Code

```rust
fn main() {
    println!("Hello, Typst!");
}
`` `

=== Images

#image("figure.png", width: 80%)
```

### Compile

```bash
# Typst → PDF
typst compile document.typ

# Watch mode (auto-recompile on save)
typst watch document.typ

# Custom output path
typst compile document.typ output.pdf

# With font path
typst compile --font-path ./fonts document.typ
```

## Remote Compilation (Zero Install)

Using TypeTex API (no API key needed):

```bash
# Compile Typst → PDF
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/typst" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat document.typ | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > output.pdf

# Compile LaTeX → PDF
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/latex" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat document.tex | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > output.pdf
```

## Templates

### Resume
```typst
#set page(paper: "a4", margin: (x: 1.5cm, y: 2cm))
#set text(font: "New Computer Modern", size: 10pt)

#align(center)[
  #text(size: 20pt, weight: "bold")[Your Name]
  #linebreak()
  email\@example.com | github.com/you | +1-234-567-8900
]

#line(length: 100%)

== Experience

*Senior Engineer* | Company Name #h(1fr) 2024 -- Present
- Led team of 5 engineers on core infrastructure
- Reduced build times by 60% through caching strategy

== Education

*M.S. Computer Science* | University Name #h(1fr) 2020 -- 2022
```

### Paper
```typst
#set page(paper: "a4", margin: 2.5cm)
#set text(font: "New Computer Modern", size: 11pt)
#set par(justify: true)
#set heading(numbering: "1.1")

#align(center)[
  #text(size: 16pt, weight: "bold")[Paper Title]
  #linebreak()
  #text(size: 12pt)[Author Name]
  #linebreak()
  #text(size: 10pt, style: "italic")[Institution]
]

#set text(size: 10pt)
*Abstract.* #lorem(80)

= Introduction
#lorem(120)

= Related Work
#lorem(100)

= Methodology
#lorem(150)

= Results
#lorem(100)

= Conclusion
#lorem(60)
```

## LaTeX Quick Reference (for existing projects)

```bash
# Compile LaTeX locally (requires texlive)
# brew install --cask mactex  # WARNING: ~4GB download
pdflatex document.tex
bibtex document
pdflatex document.tex
pdflatex document.tex  # yes, run twice for references
```

### LaTeX Common Gotchas

| Issue | Fix |
|-------|-----|
| Special chars: `# $ % & _ { } ~ ^` | Escape with backslash: `\#`, `\$`, etc. |
| Quotes: "wrong" | Use `` `single' `` or ` ``double'' ` |
| Missing package | `\usepackage{packagename}` in preamble |
| Float placement | Use `[htbp]` or `\usepackage{float}` with `[H]` |
| Table too wide | Use `tabularx` with `X` column type |
| Encoding error | Add `\usepackage[utf8]{inputenc}` |

## Math Rendering to Image

For inline use in chat or documentation:

```bash
# Using TypeTex API
echo '$ E = m c^2 $' > /tmp/math.typ
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/typst" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat /tmp/math.typ | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > /tmp/math.pdf
```

## Aleph Integration

- Synergy with `doc` (F8): co-author content → `typeset` renders to PDF
- Synergy with `data-pipeline` (A5): process data → insert into Typst tables
- Synergy with `email` (A8): render report PDF → email as attachment
