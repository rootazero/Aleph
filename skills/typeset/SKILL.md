---
name: typeset
description: "Document rendering — compile Typst/LaTeX to PDF, math rendering, professional templates (resume, paper). Use when creating PDFs, rendering documents, compiling Typst or LaTeX, typesetting math equations, or generating professional documents like papers, reports, and resumes."
---

# Document Rendering (Typst/LaTeX)

## Why Typst Over LaTeX

| Aspect | Typst | LaTeX |
|--------|-------|-------|
| Syntax | Simple, markdown-like | Verbose backslashes |
| Compilation | Instant (<1s) | Slow (seconds to minutes) |
| Error messages | Clear, human-readable | Cryptic |
| Package management | Built-in | texlive (GB-sized) |

Use Typst for new documents. Use LaTeX only for existing LaTeX projects or when a specific LaTeX package is required.

## Prerequisites

```bash
# Option A: Local Typst (recommended)
brew install typst

# Option B: Remote API (no install needed, uses TypeTex API)
```

## Typst Quick Start

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
```

### Compile

```bash
typst compile document.typ            # Typst -> PDF
typst watch document.typ              # auto-recompile on save
typst compile --font-path ./fonts document.typ  # custom fonts
```

## Remote Compilation (Zero Install)

```bash
# Compile Typst -> PDF via TypeTex API
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/typst" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat document.typ | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > output.pdf

# Compile LaTeX -> PDF
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/latex" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat document.tex | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > output.pdf
```

## Templates and LaTeX Reference

For resume/paper templates and LaTeX gotchas, see [references/typst-templates.md](references/typst-templates.md).

## Math Rendering to Image

```bash
echo '$ E = m c^2 $' > /tmp/math.typ
curl -s -X POST "https://studio-intrinsic--typetex-compile-app.modal.run/public/compile/typst" \
  -H "Content-Type: application/json" \
  -d "{\"source\": \"$(cat /tmp/math.typ | jq -Rsa .)\"}" \
  | jq -r '.pdf' | base64 -d > /tmp/math.pdf
```

## Aleph Integration

- Synergy with `doc` (F8): co-author content -> render to PDF
- Synergy with `data-pipeline` (A5): process data -> insert into Typst tables
- Synergy with `email` (A8): render report PDF -> email as attachment
