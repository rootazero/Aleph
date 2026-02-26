---
name: doc
description: Documentation co-authoring — 3-stage workflow with templates for RFC, ADR, README, and specs
scope: standalone
---

# Documentation Writing

## When to Use

Invoke this skill when writing documentation — READMEs, RFCs, ADRs, design docs, API docs, runbooks, or any structured technical document.

## The 3-Stage Process

### Stage 1: Context Gathering

Before writing anything, collect:

- **Audience**: Who will read this? (developers, users, ops, executives)
- **Purpose**: What decision or action should this document enable?
- **Scope**: What's in and out of scope?
- **Existing material**: Is there prior art to build on?

Ask the user these questions. Don't assume.

### Stage 2: Iterative Refinement

Build the document section by section:

1. Write an outline (section headers only)
2. Get approval on the outline
3. Fill each section, presenting it for review
4. Refine based on feedback

**The Rephrasing Test**: For each core claim, try to express it with completely different words. If the meaning survives, it's a genuine insight. If it falls apart, the original was vague.

### Stage 3: Reader Testing

Before finalizing:
- Read the document as if you know nothing about the project
- List the top 3 questions a new reader would have
- Verify the document answers them
- Check: Could someone act on this document without asking you clarifying questions?

## Document Templates

### README
```
# Project Name
One-line description.

## Quick Start
3-5 steps to get running.

## Usage
Core usage examples.

## Configuration
Required environment variables and options.

## Development
How to build, test, and contribute.
```

### ADR (Architecture Decision Record)
```
# ADR-NNN: Title

**Status:** Proposed | Accepted | Deprecated | Superseded
**Date:** YYYY-MM-DD

## Context
What is the issue that we're seeing that is motivating this decision?

## Decision
What is the change that we're proposing and/or doing?

## Consequences
What becomes easier or harder because of this change?
```

### RFC (Request for Comments)
```
# RFC: Title

**Author:** Name
**Date:** YYYY-MM-DD
**Status:** Draft | In Review | Accepted | Rejected

## Summary
One paragraph.

## Motivation
Why is this needed? What problem does it solve?

## Design
Detailed technical design.

## Alternatives Considered
What other approaches were evaluated and why they were rejected?

## Open Questions
What remains to be decided?
```

### Runbook
```
# Runbook: Operation Name

## Prerequisites
What must be true before starting.

## Steps
Numbered steps with exact commands.

## Verification
How to confirm success.

## Rollback
How to undo if something goes wrong.

## Troubleshooting
Common failure modes and their fixes.
```

## Writing Principles

- **Concise over comprehensive**: If removing a sentence doesn't lose information, remove it
- **Concrete over abstract**: Show examples, not explanations
- **Scannable**: Use headers, tables, and bullet points. Walls of text are not read.
- **Current**: Outdated docs are worse than no docs. Date everything.
- **Honest**: Document limitations and known issues. Don't oversell.

## Anti-Patterns

- **Writing docs nobody reads**: If the audience is hypothetical, don't write it
- **Duplicating code as docs**: If the code is clear, a comment is redundant
- **Never updating**: Docs that drift from reality become traps
- **Over-documenting**: Not everything needs a document. Simple things need simple docs.
- **Burying the lede**: Put the most important information first
