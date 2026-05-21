# Aleph Official Skills Design

> **Status**: Approved
> **Date**: 2026-02-25
> **Author**: AI-assisted design
> **Scope**: 20 official skills for Aleph, inspired by openclaw ecosystem

---

## 1. Background

Aleph's architecture philosophy states: "Architecture is the skeleton, Skills are the flesh."
Skills are the primary extensibility mechanism for Aleph's capabilities. This document
defines the first 20 official skills, selected and redesigned from the 2,868-skill
openclaw ecosystem.

### Design Constraints

- **Target**: Developer-first (software engineers as primary users)
- **Format**: Pure SKILL.md prompt injection (no internal API calls)
- **Depth**: Complete redesign — openclaw skills as inspiration, not copy
- **Style**: POE-aware, Aleph-native, English instructions with Chinese annotations

---

## 2. Evaluation Framework

### 2.1 Scoring Dimensions (5 dimensions, weighted)

| Dimension | Weight | Definition |
|-----------|--------|------------|
| **Developer Frequency** | 30% | How often a developer uses this capability daily/weekly |
| **AI Amplification** | 25% | How much AI assistance improves over manual execution |
| **Universality** | 20% | Applicability across languages, frameworks, and projects |
| **Complementarity** | 15% | Fills a unique gap without overlapping other skills |
| **Aleph Alignment** | 10% | Fits Aleph's POE/DDD philosophy and tool ecosystem |

### 2.2 Funnel Process (4 rounds)

```
2,868 openclaw skills
    ↓ Round 1: Category Filter — remove non-developer categories
~800 candidates
    ↓ Round 2: Quality Filter — remove test/placeholder/duplicate/low-quality
~200 candidates
    ↓ Round 3: Matrix Scoring — 5-dimension weighted scoring
Top 40 finalists
    ↓ Round 4: Pyramid Fit — select 20 for balanced layer coverage
20 official skills
```

### 2.3 Adaptation Principles

1. **Inspiration not imitation**: Original skills define capability scope; content is fully rewritten
2. **Aleph style**: Embed POE thinking (define success → execute → evaluate)
3. **Tool-aware**: Reference Aleph's built-in tools (Shell, Read, Edit, Grep, Glob, WebSearch)
4. **Bilingual**: Frontmatter in English, core instructions in English, annotations in Chinese
5. **Concise**: Each skill 150-300 lines, no information overload (Occam's razor)

---

## 3. Pyramid Architecture

Inspired by Aleph's 5-layer emergence architecture:

```
              ╱ 5 Specialist Skills ╲         ← Advanced scenarios, on-demand
             ╱  7 Workflow Skills     ╲       ← Development lifecycle stages
            ╱   8 Foundation Skills    ╲     ← Daily essentials, universal base
```

- **Foundation (8)**: Developer "muscle memory" — used daily regardless of project
- **Workflow (7)**: Key nodes in the development lifecycle
- **Specialist (5)**: High-value but lower-frequency advanced scenarios

---

## 4. The 20 Official Skills

### 4.1 Foundation Layer (8 skills)

#### F1. `debug` — Systematic Debugging

| Field | Value |
|-------|-------|
| **Inspiration** | debug-pro, systematic-debugging |
| **Score** | 4.6 / 5.0 |
| **Scope** | standalone |
| **Triggers** | debug, investigate, fix bug, troubleshoot |

**Core responsibility**: Guide LLM through a 7-step debugging protocol:
Reproduce → Isolate → Hypothesize → Instrument → Verify → Fix → Regression Test.

**Key design decisions**:
- Focus on **debugging methodology** (thinking framework), not language-specific command dumps
- Include a "Common Error Patterns" quick-reference table (top 20 errors across JS/Python/Rust/Swift)
- POE integration: Step 0 is "Define what 'fixed' looks like" (success contract)
- Encourage `git bisect` as default isolation technique
- Anti-pattern: "Resist the urge to refactor while debugging"

**Differences from debug-pro**:
- Removed: Language-specific debugging command blocks (those belong in docs)
- Added: POE success contract, structured hypothesis tracking
- Refined: Common error patterns table focused on root causes, not symptoms

---

#### F2. `test` — Testing & TDD

| Field | Value |
|-------|-------|
| **Inspiration** | test-runner + tdd-guide |
| **Score** | 4.5 / 5.0 |
| **Scope** | standalone |
| **Triggers** | test, TDD, write tests, coverage, red green refactor |

**Core responsibility**: Unified testing workflow combining framework selection, test execution,
and TDD cycles. Auto-detect project's test framework and guide accordingly.

**Key design decisions**:
- Merge test-runner (execution reference) and tdd-guide (methodology) into one skill
- Remove Python script references (pure prompt, no external tools)
- Framework detection: analyze package.json/Cargo.toml/pyproject.toml to recommend framework
- TDD cycle with POE mapping: Red = P (define contract), Green = O (execute), Refactor = E (evaluate)
- Include "What to test / What NOT to test" decision guide
- Arrange-Act-Assert as universal test structure pattern

**Differences from originals**:
- Removed: Python tool scripts (test_generator.py etc.)
- Added: Auto-detection logic, POE-mapped TDD cycle
- Refined: Focused on patterns not commands (user's test runner handles execution)

---

#### F3. `code-review` — Code Review

| Field | Value |
|-------|-------|
| **Inspiration** | pr-reviewer + code-review patterns |
| **Score** | 4.4 / 5.0 |
| **Scope** | standalone |
| **Triggers** | review, code review, PR review, audit code |

**Core responsibility**: Multi-dimensional code review with confidence-based filtering.
Only report issues with high confidence to minimize noise.

**Key design decisions**:
- 5 review dimensions: Security (P0) → Logic (P1) → Error Handling (P2) → Performance (P3) → Style (P4)
- Confidence threshold: Only report P0-P2 by default; P3-P4 on request
- Output format: Structured findings with file:line, severity, explanation, suggested fix
- Anti-pattern: Don't nitpick style when there are logic bugs
- Include security checklist (hardcoded secrets, SQL injection, XSS, CSRF)

**Differences from pr-reviewer**:
- Removed: gh CLI dependency, external linter integration
- Added: Confidence-based filtering, structured output format
- Refined: Pure LLM analysis rather than tool-dependent scanning

---

#### F4. `git` — Git Workflow

| Field | Value |
|-------|-------|
| **Inspiration** | git-essentials + git-workflows + conventional-commits |
| **Score** | 4.4 / 5.0 |
| **Scope** | standalone |
| **Triggers** | git, commit, branch, merge, rebase, bisect |

**Core responsibility**: One-stop Git skill combining daily operations, advanced techniques,
and Aleph-specific conventions.

**Key design decisions**:
- Scene-driven organization: "I want to X" → here's how
- Include: bisect, reflog, worktree, cherry-pick, interactive rebase, sparse checkout
- Aleph-specific: worktree safety rules (never delete worktree from within it)
- Commit format: `<scope>: <description>` (Aleph convention)
- Include conventional commits reference as subsection
- Git alias recommendations for common compound operations

**Differences from originals**:
- Removed: Beginner tutorial content
- Added: Aleph worktree convention, scene-driven organization
- Merged: git-essentials (basic) + git-workflows (advanced) + conventional-commits (format)

---

#### F5. `refactor` — Code Refactoring

| Field | Value |
|-------|-------|
| **Inspiration** | backend-patterns + general refactoring catalogs |
| **Score** | 4.2 / 5.0 |
| **Scope** | standalone |
| **Triggers** | refactor, clean up, simplify, extract, restructure |

**Core responsibility**: Identify code smells → recommend refactoring techniques →
execute safely with test safety net.

**Key design decisions**:
- Code smell catalog: God class, long method, feature envy, shotgun surgery, primitive obsession
- Refactoring techniques: Extract method/class, inline, rename, replace conditional with polymorphism
- Safety protocol: Must have passing tests before refactoring; run tests after each step
- Integration with F2 (test): If no tests exist, write characterization tests first
- Anti-pattern: "Don't refactor while debugging" (cross-reference F1)

**Unique to Aleph**:
- Aleph CODE_ORGANIZATION.md alignment (file splitting rules, naming conventions)
- Small-step incremental approach with verification at each step

---

#### F6. `shell` — Shell & Containers

| Field | Value |
|-------|-------|
| **Inspiration** | CLI utilities + docker-essentials + docker-sandbox |
| **Score** | 4.1 / 5.0 |
| **Scope** | standalone |
| **Triggers** | shell, docker, container, process, disk, port |

**Core responsibility**: Scene-driven shell command reference combining CLI utilities,
Docker operations, and system diagnostics.

**Key design decisions**:
- Scene-driven sections: "Check port usage", "Clean disk space", "Manage containers", "Debug networking"
- Docker subsection: lifecycle, compose, networking, volumes, system management
- Process management: finding, killing, monitoring processes
- System diagnostics: disk, memory, CPU, network
- Aleph exec safety awareness: dangerous commands need confirmation

**Differences from docker-essentials**:
- Removed: Exhaustive command dictionary format
- Added: Scene-driven organization, system diagnostics, process management
- Refined: Practical "I need to X" approach over reference manual

---

#### F7. `search` — Technical Search

| Field | Value |
|-------|-------|
| **Inspiration** | research skills + deepwiki + exa-search |
| **Score** | 4.0 / 5.0 |
| **Scope** | standalone |
| **Triggers** | search, find, research, investigate, look up |

**Core responsibility**: Efficient search strategies across code, documentation, and web.

**Key design decisions**:
- 3 search modes: Code Archaeology (Grep/Glob/Read), Documentation (WebFetch), Web Research (WebSearch)
- Code archaeology patterns: find definition, trace usage, understand history (git log/blame)
- Documentation search: official docs → GitHub issues → Stack Overflow → blog posts
- Research methodology: define question → search → evaluate sources → synthesize
- Aleph tool mapping: Grep for content, Glob for files, Read for context, WebSearch for external

**Unique to Aleph**:
- Direct integration with Aleph's built-in search tools
- Search strategy decision tree based on information type

---

#### F8. `doc` — Documentation Writing

| Field | Value |
|-------|-------|
| **Inspiration** | doc-coauthoring + essence-distiller + claude-optimised |
| **Score** | 3.9 / 5.0 |
| **Scope** | standalone |
| **Triggers** | document, write doc, RFC, ADR, README, spec |

**Core responsibility**: 3-stage documentation co-authoring with built-in quality checks.

**Key design decisions**:
- 3 stages: Context Gathering → Iterative Refinement → Reader Testing
- Templates: RFC, ADR, API Doc, README, Design Doc, Runbook
- Essence distiller integration: "Rephrasing Test" — if an idea survives rewriting, it's essential
- CLAUDE.md optimization tips (from claude-optimised): less is more, every line must prevent mistakes
- Anti-pattern: Don't write documentation nobody will read

**Differences from doc-coauthoring**:
- Removed: Verbose process description
- Added: Document templates, essence distiller quality check, CLAUDE.md tips
- Refined: Actionable over procedural

---

### 4.2 Workflow Layer (7 skills)

#### W1. `plan` — Technical Design

| Field | Value |
|-------|-------|
| **Inspiration** | senior-architect (decision workflows) |
| **Score** | 4.3 / 5.0 |
| **Scope** | standalone |
| **Triggers** | plan, design, architect, RFC, proposal, ADR |

**Core responsibility**: From ambiguous requirements to clear technical design:
Requirements Analysis → 2-3 Approach Comparison → ADR Documentation.

**Key design decisions**:
- POE P-phase materialized: Must define "what does success look like" before any design work
- Decision workflow: Identify constraints → Generate options → Evaluate trade-offs → Decide → Document
- ADR template: Context, Decision, Status, Consequences
- Architecture pattern selection guide (team size × requirements matrix)
- Output: Mermaid diagrams + structured decision record

---

#### W2. `api-design` — API Design

| Field | Value |
|-------|-------|
| **Inspiration** | backend-patterns (API section) |
| **Score** | 4.1 / 5.0 |
| **Scope** | standalone |
| **Triggers** | API design, REST, GraphQL, endpoint, schema |

**Core responsibility**: Contract-first API design following REST/GraphQL best practices.

**Key design decisions**:
- Contract-first: Define OpenAPI/GraphQL schema before implementation
- REST conventions: HTTP methods, status codes, pagination, filtering, error format
- GraphQL patterns: schema design, resolvers, N+1 prevention, batching
- Versioning strategy: URL path vs header vs query parameter
- Error response standard: `{ error: { code, message, details } }`
- Rate limiting and authentication patterns

---

#### W3. `database` — Database Design

| Field | Value |
|-------|-------|
| **Inspiration** | senior-architect (DB selection) + SQL toolkit |
| **Score** | 4.0 / 5.0 |
| **Scope** | standalone |
| **Triggers** | database, schema, SQL, migration, query optimization |

**Core responsibility**: Database selection, schema design, query optimization, migration strategy.

**Key design decisions**:
- Selection decision tree: data characteristics → scale requirements → recommended DB
- Schema design from DDD perspective: aggregate roots → tables, value objects → embedded
- Query optimization: explain plans, indexing strategies, N+1 prevention
- Migration best practices: reversible migrations, zero-downtime schema changes
- Quick reference: PostgreSQL (default), MongoDB (documents), Redis (cache), SQLite (embedded)

---

#### W4. `deploy` — Deployment & Operations

| Field | Value |
|-------|-------|
| **Inspiration** | docker-essentials + deployment skills + emergency-rescue (rollback section) |
| **Score** | 3.9 / 5.0 |
| **Scope** | standalone |
| **Triggers** | deploy, release, rollback, containerize, infrastructure |

**Core responsibility**: Full deployment lifecycle: Containerize → Orchestrate → Release → Monitor → Rollback.

**Key design decisions**:
- Dockerfile best practices: multi-stage builds, minimal base images, layer caching
- Docker Compose for local development and testing
- Cloud deployment patterns: serverless, containers, VMs
- Release strategies: blue-green, canary, rolling update
- Rollback procedures: git revert, container rollback, database rollback
- Health checks and monitoring setup

---

#### W5. `security` — Security Audit

| Field | Value |
|-------|-------|
| **Inspiration** | security-audit + flaw0 + emergency-rescue (credential leak section) |
| **Score** | 3.8 / 5.0 |
| **Scope** | standalone |
| **Triggers** | security, audit, vulnerability, credentials, OWASP |

**Core responsibility**: Code security audit, credential leak response, dependency scanning.

**Key design decisions**:
- OWASP Top 10 checklist adapted for code review
- Credential leak emergency response: "Credential is compromised the moment it's pushed — revoke FIRST"
- Dependency vulnerability scanning patterns
- Authentication/authorization review checklist
- Input validation and output encoding patterns
- Aleph alignment: exec safety system awareness, approval workflow

---

#### W6. `performance` — Performance Optimization

| Field | Value |
|-------|-------|
| **Inspiration** | perf-profiler + profiling patterns |
| **Score** | 3.7 / 5.0 |
| **Scope** | standalone |
| **Triggers** | performance, optimize, profile, benchmark, slow |

**Core responsibility**: Performance profiling methodology and optimization patterns.

**Key design decisions**:
- Measure first: "If you can't measure it, don't optimize it"
- Profiling tools by language: Rust (flamegraph, criterion), JS (Chrome DevTools, Lighthouse), Python (cProfile, py-spy)
- Common optimization patterns: caching, lazy loading, batch processing, connection pooling
- Database performance: query optimization, indexing, connection management
- Frontend performance: bundle size, code splitting, image optimization
- Anti-pattern: Premature optimization without measurement

---

#### W7. `ci-cd` — CI/CD Pipelines

| Field | Value |
|-------|-------|
| **Inspiration** | gitflow + GitHub Actions patterns + deployment workflows |
| **Score** | 3.6 / 5.0 |
| **Scope** | standalone |
| **Triggers** | CI, CD, pipeline, GitHub Actions, workflow, automation |

**Core responsibility**: CI/CD pipeline design with GitHub Actions as primary platform.

**Key design decisions**:
- GitHub Actions template library: test, lint, build, deploy, release
- Pipeline design patterns: fan-out/fan-in, matrix builds, conditional stages
- Caching strategies: dependency caching, Docker layer caching, build artifact caching
- Secret management in CI
- Notification and status reporting
- Aleph alignment: Aleph's own build commands as reference

---

### 4.3 Specialist Layer (5 skills)

#### S1. `architecture` — System Architecture

| Field | Value |
|-------|-------|
| **Inspiration** | senior-architect (full) + cto-advisor |
| **Score** | 4.2 / 5.0 |
| **Scope** | standalone |
| **Triggers** | architecture, system design, microservices, monolith, scaling |

**Core responsibility**: System architecture design, pattern selection, and visualization.

**Key design decisions**:
- Architecture patterns: Monolith → Modular Monolith → SOA → Microservices decision framework
- Team size × complexity matrix for pattern selection
- Architecture diagram generation in Mermaid format
- Dependency analysis and coupling assessment
- Tech stack evaluation framework
- Aleph alignment: 1-2-3-4 architecture model as case study

---

#### S2. `emergency` — Emergency Rescue

| Field | Value |
|-------|-------|
| **Inspiration** | emergency-rescue |
| **Score** | 3.8 / 5.0 |
| **Scope** | standalone |
| **Triggers** | emergency, disaster, recovery, broken, corrupted, locked |

**Core responsibility**: Disaster recovery handbook for common developer emergencies.

**Key design decisions**:
- Scenarios: Git disasters, disk full, DB locks/deadlocks, deployment failures, credential leaks, process hangs
- Each scenario: Symptoms → Immediate action → Root cause fix → Prevention → Post-mortem template
- Universal diagnostic script for "I don't know what's wrong"
- Aleph alignment: Post-mortem template follows POE E-phase (evaluation)

---

#### S3. `regex` — Regular Expressions

| Field | Value |
|-------|-------|
| **Inspiration** | regex-patterns |
| **Score** | 3.5 / 5.0 |
| **Scope** | standalone |
| **Triggers** | regex, regular expression, pattern matching, validate |

**Core responsibility**: Practical regex cookbook organized by use case.

**Key design decisions**:
- Organized by use case: Validation (email, URL, IP, phone), Parsing (logs, code), Replacement (refactoring)
- Cross-language syntax: JavaScript, Python, Rust, Go, command-line
- Common gotchas: greedy vs lazy, backtracking, Unicode, multiline
- Performance tips: avoid catastrophic backtracking, use anchors
- Compact format: lookup table, not tutorial

---

#### S4. `mcp-dev` — MCP Server Development

| Field | Value |
|-------|-------|
| **Inspiration** | mcp-builder |
| **Score** | 3.4 / 5.0 |
| **Scope** | standalone |
| **Triggers** | MCP, MCP server, protocol, tool integration |

**Core responsibility**: 4-phase MCP server development guide, core to Aleph's extension ecosystem.

**Key design decisions**:
- Phase 1: Research & Planning (study API, design tools)
- Phase 2: Implementation (project setup, core infra, tool implementation)
- Phase 3: Review & Refine (quality, testing, documentation)
- Phase 4: Evaluation (scenario-based testing)
- Key principles: Agent-centric design, optimize for context, clear errors
- Aleph alignment: Direct integration with Aleph's MCP client and Extension System

---

#### S5. `knowledge` — Knowledge Distillation

| Field | Value |
|-------|-------|
| **Inspiration** | essence-distiller + claude-optimised |
| **Score** | 3.3 / 5.0 |
| **Scope** | standalone |
| **Triggers** | summarize, distill, extract key points, TL;DR, compress |

**Core responsibility**: Extract core insights from long content using the "Rephrasing Test".

**Key design decisions**:
- Core method: "Rephrasing Test" — an idea is essential if it survives complete rewording
- Applicable to: code reviews, meeting notes, research papers, documentation
- Output format: Core principles with confidence levels, evidence, and compression ratio
- N-count validation: N=1 (single source), N=2 (corroborated), N=3+ (invariant)
- CLAUDE.md optimization: "Less is more" principle for AI instruction writing
- Aleph alignment: Feeds into Aleph Memory system (distilled facts → MemoryFact)

---

## 5. File Structure

All skills will be placed in the Aleph skills directory:

```
~/.aleph/skills/
├── foundation/
│   ├── debug/SKILL.md
│   ├── test/SKILL.md
│   ├── code-review/SKILL.md
│   ├── git/SKILL.md
│   ├── refactor/SKILL.md
│   ├── shell/SKILL.md
│   ├── search/SKILL.md
│   └── doc/SKILL.md
├── workflow/
│   ├── plan/SKILL.md
│   ├── api-design/SKILL.md
│   ├── database/SKILL.md
│   ├── deploy/SKILL.md
│   ├── security/SKILL.md
│   ├── performance/SKILL.md
│   └── ci-cd/SKILL.md
└── specialist/
    ├── architecture/SKILL.md
    ├── emergency/SKILL.md
    ├── regex/SKILL.md
    ├── mcp-dev/SKILL.md
    └── knowledge/SKILL.md
```

### SKILL.md Template

```markdown
---
name: <skill-name>
description: <one-line description>
scope: standalone
emoji: "<emoji>"
category: "<foundation|workflow|specialist>"
triggers:
  - <trigger-1>
  - <trigger-2>
---

# <Skill Title>

## When to Use
<2-3 sentences on when this skill is appropriate>

## Core Process
<Step-by-step methodology, POE-aligned where applicable>

## Quick Reference
<Tables, checklists, or decision trees for rapid lookup>

## Anti-Patterns
<What NOT to do — common mistakes>

## Aleph Integration
<How this skill leverages Aleph's built-in tools and conventions>
```

---

## 6. Implementation Priority

| Priority | Skills | Rationale |
|----------|--------|-----------|
| **P0** (Week 1) | debug, test, git, code-review | Highest daily frequency |
| **P1** (Week 2) | refactor, shell, search, doc | Complete foundation layer |
| **P2** (Week 3) | plan, api-design, database, deploy | Core workflow coverage |
| **P3** (Week 4) | security, performance, ci-cd, architecture | Complete workflow + start specialist |
| **P4** (Week 5) | emergency, regex, mcp-dev, knowledge | Complete all 20 skills |

---

## 7. Quality Criteria

Each skill must pass before shipping:

- [ ] **Concise**: 150-300 lines, no fluff
- [ ] **Actionable**: Every section guides behavior, not just describes concepts
- [ ] **POE-aligned**: Success criteria defined where applicable
- [ ] **Tool-aware**: References correct Aleph tools (Read, Edit, Grep, Glob, WebSearch, Bash)
- [ ] **Tested**: Manually verified against 3+ real scenarios
- [ ] **Unique**: No significant overlap with other official skills
- [ ] **SKILL.md compliant**: Valid frontmatter, correct scope, meaningful triggers
