---
name: knowledge
description: Knowledge distillation — extract core insights using the Rephrasing Test, compress for retention
scope: standalone
---

# Knowledge Distillation

## When to Use

Invoke this skill when extracting key insights from long documents, code reviews, meeting notes, research papers, or any content that needs to be compressed to its essentials.

## The Rephrasing Test

An idea is **essential** if you can express it with completely different words and the meaning stays exactly the same.

```
Original: "Functions should do one thing"
Rephrased: "Each function should have a single responsibility"
→ Same meaning survives → This is an essential principle

Original: "Use meaningful variable names"
Rephrased: "Name variables so their purpose is clear without comments"
→ Same meaning survives → Essential

Original: "We should probably refactor this sometime"
Rephrased: [vague — what to refactor? why? when?]
→ Meaning collapses → Not a real insight, just noise
```

## Distillation Process

### Step 1: Read Without Judgment

Read the entire source material. Don't highlight or extract yet. Let the content settle.

### Step 2: Identify Candidates

Find statements, claims, or patterns that seem important. For each, ask:
- Does this survive the Rephrasing Test?
- Is this actionable (can someone act on it)?
- Is this specific (not a platitude)?

### Step 3: Classify Confidence

| Level | Criteria |
|-------|----------|
| **High** | Survives rephrasing, supported by evidence, actionable |
| **Medium** | Survives rephrasing but lacks evidence or specificity |
| **Low** | Partially survives rephrasing, may be context-dependent |

### Step 4: Output

For each extracted principle:

```
**Principle:** [One clear sentence]
**Confidence:** High / Medium / Low
**Evidence:** [Where in the source this comes from]
**Normalized form:** [Canonical version for cross-source comparison]
```

## The N-Count System

Track how many independent sources confirm a principle:

| N | Meaning | Trust Level |
|---|---------|-------------|
| 1 | Single source | Observation — might be opinion |
| 2 | Two independent sources | Corroborated — likely real pattern |
| 3+ | Multiple sources | Invariant — reliable principle |

Cross-reference extractions from different sources. When the same principle appears independently, increase its N-count.

## Applications

### Code Review Distillation

Extract the recurring feedback patterns from code reviews:
- What mistakes keep appearing?
- What standards are implicitly enforced?
- What's the team's actual (not stated) quality bar?

### Meeting Notes

Distill meetings to:
- Decisions made (with owner and deadline)
- Action items (who does what by when)
- Open questions (unresolved, need follow-up)

Drop: Status updates, tangential discussion, restated context.

### Research Papers

Extract:
- Core claim (one sentence)
- Method (how they proved it)
- Key finding (quantified if possible)
- Limitations (what it doesn't show)
- Relevance (how it applies to your problem)

### CLAUDE.md / AI Instructions

Apply the "less is more" principle:
- For each instruction, ask: "Would removing this cause the AI to make a mistake?"
- If no → remove it
- Target: ~15-50 lines for most projects
- Every line should prevent a specific, real error

## Output Format

```
## Distillation: [Source Title]

**Source:** [reference]
**Compression:** [X principles from Y pages/lines]

### Principles

1. **[Principle]** (High confidence, N=2)
   Evidence: [quote/reference]

2. **[Principle]** (Medium confidence, N=1)
   Evidence: [quote/reference]

### Noise Removed
[Brief note on what was discarded and why]

### Open Questions
[Things the source raised but didn't resolve]
```

## Anti-Patterns

- **Extracting everything**: If nothing is cut, nothing is distilled
- **Losing specificity**: "Be good at coding" is not a principle
- **Ignoring context**: A principle valid in one domain may not transfer
- **Confusing frequency with importance**: Something repeated often may just be filler
- **No evidence**: Principles without source references can't be verified
