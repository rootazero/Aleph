# Automation Skills Design (Skills #21-30)

> **Status**: Approved
> **Date**: 2026-02-26
> **Author**: AI-assisted design
> **Scope**: 10 automation skills extending Aleph's official collection from 20 to 30
> **Prerequisite**: [Official Skills Design (Skills #1-20)](2026-02-25-official-skills-design.md)

---

## 1. Background

The first 20 official skills are pure prompt injection — they guide LLM behavior through
methodology, checklists, and decision trees without executing any external commands. This
phase adds 10 **automation skills** that leverage Aleph's `cli-wrapper` and `allowed-tools`
infrastructure to execute real scripts and CLI commands.

### Design Philosophy

> "前 20 个 skills 是大脑，后 10 个 skills 是手脚。"

The knowledge layer (skills #1-20) tells the AI *what* to do. The automation layer
(skills #21-30) gives it the ability to *actually do it* through scripted execution.

### Sources

Skills were selected from 3,019 curated skills across two registries:
- [ClawHub.ai](https://clawhub.ai) — 2,868 highlighted skills across 32 categories
- [awesome-openclaw-skills](https://github.com/rootazero/awesome-openclaw-skills) — curated fork with quality filtering

Each candidate was evaluated by fetching the actual SKILL.md source code and scoring on:
scripting capability, quality, dependencies, and Aleph alignment.

---

## 2. Expanded Pyramid Architecture

```
           ╱  5 Specialist Skills (prompt)   ╲        ← S1-S5: Advanced scenarios
          ╱   7 Workflow Skills (prompt)      ╲       ← W1-W7: Dev lifecycle
         ╱    8 Foundation Skills (prompt)     ╲     ← F1-F8: Daily essentials
        ╱   10 Automation Skills (scripted)    ╲   ← A1-A10: Script execution (NEW)
```

- **Knowledge Layer** (20 skills): Pure SKILL.md prompt injection, `cli-wrapper: false`
- **Automation Layer** (10 skills): Script-capable, `cli-wrapper: true` or `allowed-tools: [Bash]`

### Three Technical Modes

| Mode | Frontmatter | Security | Use Case |
|------|-------------|----------|----------|
| **CLI-Wrapper** | `cli-wrapper: true` + `requirements.binaries` | `CliWrapperValidator` whitelist | Single CLI tool wrapping |
| **Tool-Scoped** | `allowed-tools: [Bash]` | Aleph exec approval system | Multi-tool composition |
| **Hybrid** | Both fields set | Dual validation | Core CLI + flexible scripting |

---

## 3. Evaluation Results

### Source Skills Evaluated (Top Candidates)

| Source Skill | Author | Quality | Mode | Used For |
|-------------|--------|---------|------|----------|
| `coding-agent` | steipete | High | CLI | A1 (github) |
| `pr-reviewer` | briancolinger | High | CLI+Script | A1 (github) |
| `playwright-npx` | mahone-bot | High | CLI | A2 (playwright) |
| `openclaw-web-scraper` | LiranUdi | High | Python scripts | A4 (web-scraper) |
| `csv-pipeline` | gitgoodordietrying | A | Python+bash | A5 (data-pipeline) |
| `sheetsmith` | crimsondevil333333 | A- | Python script | A5 (data-pipeline) |
| `jq` | gumadeiras | B+ | CLI ref | A5 (data-pipeline) |
| `ffmpeg-cli` | ascendswang | A | 8 shell scripts | A6 (media-tools) |
| `ffmpeg-video-editor` | mahmoudadelbghany | A | Knowledge | A6 (media-tools) |
| `ffmpeg` (expert) | ivangdavila | A | Knowledge | A6 (media-tools) |
| `universal-notify` | josunlp | A | Shell script | A7 (notification) |
| `notify` (design) | ivangdavila | A+ | Design patterns | A7 (notification) |
| `imap-smtp-email` | gzlicanyi | A | Node.js scripts | A8 (email) |
| `ssh-tunnel` | gitgoodordietrying | A+ | Knowledge | A9 (ssh) |
| `ssh-essentials` | arnarsson | A | Knowledge | A9 (ssh) |
| `typetex` | gregm711 | A | HTTP API | A10 (typeset) |
| `tex-render` | thebigoranger | A- | Node.js script | A10 (typeset) |

### Rejected Candidates

| Source Skill | Reason |
|-------------|--------|
| `steipete/github` | Too thin — mere cheatsheet of `gh` commands, no automation value |
| `anki-connect` (gyroninja) | Empty shell — only a link to external docs |
| `cron-writer` | LLM can already do natural language → cron conversion |
| `cron-dashboard` | OpenClaw-specific, not portable to Aleph |
| `clawmail` | Vendor lock-in to ClawMail service |
| `pushover-notify` | Single-channel only, narrow utility |
| `playwright-headless-browser` | Only covers WSL/Linux setup, not general-purpose |

---

## 4. The 10 Automation Skills

### A1. `github` — GitHub CLI Automation

| Field | Value |
|-------|-------|
| **Inspiration** | steipete/coding-agent (High) + briancolinger/pr-reviewer (High) |
| **Score** | 4.7 / 5.0 |
| **Mode** | CLI-Wrapper |
| **Binaries** | `gh` |
| **Install** | `brew install gh` |
| **Triggers** | github, gh, PR, pull request, issue, workflow, CI status |

**Core responsibility**: Intelligent GitHub CLI wrapper covering PR lifecycle,
issue management, CI debugging, and advanced API queries.

**Key design decisions**:
- PR full lifecycle: create draft → review → request changes → approve → merge → cleanup
- CI interaction: view runs, re-trigger failed jobs, download artifacts, view failed logs
- Advanced API: `gh api` + jq pipelines, GraphQL query templates for complex data
- Security checks: secrets scan, dependency review, branch protection audit
- Scene-driven: "I want to check why CI failed" → step-by-step resolution

**Synergy with existing skills**:
- `git` (F4) manages local Git → `github` (A1) manages remote GitHub
- `code-review` (F3) reviews code → `github` submits review comments
- `ci-cd` (W7) writes pipelines → `github` debugs pipeline failures

**Differences from steipete/github**:
- Removed: Thin cheatsheet format
- Added: Scene-driven workflows, CI debugging, security checks, advanced API patterns
- Refined: From reference card to intelligent automation guide

---

### A2. `playwright` — Browser Automation

| Field | Value |
|-------|-------|
| **Inspiration** | mahone-bot/playwright-npx (High) + LiranUdi/web-scraper (High) |
| **Score** | 4.5 / 5.0 |
| **Mode** | Hybrid (CLI-Wrapper + allowed-tools:Bash) |
| **Binaries** | `npx` |
| **Install** | `npm install -g playwright && npx playwright install chromium` |
| **Triggers** | playwright, browser, screenshot, e2e test, scrape, automate browser |

**Core responsibility**: Browser automation for testing, screenshots, form filling,
and structured data extraction from web pages.

**Key design decisions**:
- E2E testing: Generate + execute Playwright tests, trace analysis for failures
- Screenshots: headless capture, fullpage / element / viewport modes
- Form automation: login flows, multi-step forms, file uploads
- Data extraction: structured scraping of tables, lists, product data
- Codegen: `npx playwright codegen` for recording interactions
- Selector priority: getByRole > getByTestId > getByLabel > getByText > CSS
- Wait strategy: Never waitForTimeout — always DOM-based waits
- Decision matrix: When to use Playwright vs WebFetch vs curl

**Differences from playwright-npx**:
- Removed: OpenClaw-specific patterns (sessions_spawn, process monitoring)
- Added: Decision matrix, POE integration, Aleph tool-awareness
- Refined: Unified browser automation + testing + scraping in one skill

---

### A3. `http-client` — API Debugging

| Field | Value |
|-------|-------|
| **Inspiration** | curl best practices + REST API patterns |
| **Score** | 4.3 / 5.0 |
| **Mode** | CLI-Wrapper |
| **Binaries** | `curl` |
| **Install** | Pre-installed on macOS/Linux |
| **Triggers** | curl, API test, HTTP request, REST call, endpoint test, webhook |

**Core responsibility**: API testing, debugging, and request building using curl
with structured response analysis.

**Key design decisions**:
- All HTTP methods: GET/POST/PUT/PATCH/DELETE with complete curl templates
- Authentication: Bearer token, Basic auth, OAuth2 flow, API key patterns
- Debugging: verbose mode (-v), timing breakdown (-w), redirect following (-L)
- Data formats: JSON payload (-d), form data (-F), file upload (multipart)
- Response pipeline: curl | jq for structured parsing, status code validation
- Batch testing: bash loop + curl for simple load testing
- Cookie management: session persistence across requests (-b/-c)

**Synergy with existing skills**:
- `api-design` (W2) designs APIs → `http-client` (A3) tests and validates them
- `security` (W5) audits endpoints → `http-client` probes for vulnerabilities

**Design principle**: Pure curl, zero additional dependencies. Maximum portability.

---

### A4. `web-scraper` — Web Data Extraction

| Field | Value |
|-------|-------|
| **Inspiration** | LiranUdi/openclaw-web-scraper (High) |
| **Score** | 4.2 / 5.0 |
| **Mode** | Tool-Scoped (`allowed-tools: [Bash]`) |
| **Binaries** | `curl`, `python3` |
| **Install** | `pip install requests beautifulsoup4` (optional: `playwright`) |
| **Triggers** | scrape, extract data, crawl, parse HTML, download page |

**Core responsibility**: Structured data extraction from web pages, search engines,
and downloadable documents.

**Key design decisions**:
- Single-page extraction: curl + python/beautifulsoup for structured parsing
- Search engine queries: DuckDuckGo/Brave scraping (no API key needed)
- PDF extraction: download + text extraction pipeline
- Session management: cookie persistence for authenticated scraping
- Anti-detection: User-Agent rotation, request intervals, robots.txt compliance
- Output formats: JSON (default), CSV, Markdown table

**Distinction from `search` (F7)**:
- `search` is "find information" using Aleph's built-in WebSearch/WebFetch
- `web-scraper` is "extract structured data" using external scripts
- They complement, don't overlap

---

### A5. `data-pipeline` — Data Processing

| Field | Value |
|-------|-------|
| **Inspiration** | csv-pipeline (A) + sheetsmith (A-) + jq (B+) |
| **Score** | 4.4 / 5.0 |
| **Mode** | Hybrid (CLI-Wrapper + allowed-tools:Bash) |
| **Binaries** | `jq`, `python3` |
| **Install** | `brew install jq` |
| **Triggers** | jq, JSON process, CSV, data transform, filter data, parse data |

**Core responsibility**: JSON/CSV/TSV data processing combining jq for quick queries
and Python for complex transformations.

**Key design decisions**:
- Dual path: Simple tasks → bash (jq/awk/sort), Complex tasks → Python (pandas/stdlib)
- jq complete reference: filters, recursive descent, @csv/@tsv, env variables, modules
- CSV operations: read, filter, aggregate, join, deduplicate, format conversion
- Data cleaning: null handling, type validation, schema verification
- Format conversion: JSON ↔ CSV ↔ TSV ↔ Excel, encoding conversion
- Report generation: data → Markdown tables / summary statistics
- Large files: streaming mode (don't load everything into memory)

**Design fusion** (three best sources):
- jq's quick queries + csv-pipeline's Python functions + sheetsmith's subcommand UX

---

### A6. `media-tools` — Media Processing

| Field | Value |
|-------|-------|
| **Inspiration** | ffmpeg-cli (A) + ffmpeg-video-editor (A) + ffmpeg expert (A) |
| **Score** | 4.1 / 5.0 |
| **Mode** | CLI-Wrapper |
| **Binaries** | `ffmpeg` |
| **Install** | `brew install ffmpeg` |
| **Triggers** | ffmpeg, video, audio, convert video, compress, GIF, thumbnail |

**Core responsibility**: Video, audio, and image processing via ffmpeg with
natural language → command mapping.

**Key design decisions**:
- Video: cut, merge, transcode, speed change, GIF creation, watermark, subtitles
- Audio: extract track, format conversion, volume adjustment, merge tracks
- Image: resize, crop, format conversion, thumbnail generation
- Metadata: extract resolution, codec, duration, bitrate information
- Each operation: natural language description → ffmpeg command → parameter explanation
- Expert knowledge (from ivangdavila/ffmpeg): seeking semantics (-ss position matters),
  CRF quality control, `-c copy` + `-vf` incompatibility
- Common pitfalls section with solutions

**Design fusion** (three complementary sources):
- ffmpeg-cli's script patterns + ffmpeg-video-editor's NL mapping + ffmpeg expert's gotchas

---

### A7. `notification` — Multi-Channel Notifications

| Field | Value |
|-------|-------|
| **Inspiration** | universal-notify (A) + notify design patterns (A+) |
| **Score** | 4.0 / 5.0 |
| **Mode** | Tool-Scoped (`allowed-tools: [Bash]`) |
| **Binaries** | `curl`, `osascript` |
| **Install** | Pre-installed on macOS |
| **Triggers** | notify, alert, notification, remind me, send message |

**Core responsibility**: Multi-channel notification delivery with intelligent
routing based on urgency level.

**Key design decisions**:
- macOS native: `osascript` system notifications + voice synthesis (say command)
- ntfy.sh: Zero-config push notifications (free, no account needed)
- Telegram Bot: Bot API message delivery
- Webhook: Generic webhook POST for any integration
- Notification routing (from ivangdavila/notify design):
  - Urgent → macOS notification + sound + ntfy
  - Normal → ntfy or Telegram
  - Low → batched, quiet hours respected
- Anti-patterns: notification fatigue, spam protection

**Zero dependency design**: Only curl and osascript, macOS works out of the box.

**Aleph alignment**: Feeds into Aleph's R6 principle ("AI Comes to You") —
notifications are a key mechanism for proactive AI assistance.

---

### A8. `email` — Email Automation

| Field | Value |
|-------|-------|
| **Inspiration** | gzlicanyi/imap-smtp-email (High) |
| **Score** | 3.9 / 5.0 |
| **Mode** | CLI-Wrapper |
| **Binaries** | `curl` |
| **Install** | Pre-installed |
| **Triggers** | email, send email, check mail, SMTP, IMAP |

**Core responsibility**: Email sending (SMTP) and receiving (IMAP) via curl,
supporting 10+ email providers.

**Key design decisions**:
- Send: SMTP via curl with TLS, supporting Gmail/Outlook/163/QQ/Yahoo/iCloud
- Receive: IMAP via curl for search, download attachments, mark as read
- Templates: Code review request, deployment notification, bug report, weekly summary
- Security: TLS mandatory, App Password guidance (never use main password),
  credential storage via environment variables only
- Provider configs: Pre-built SMTP/IMAP settings for major providers

**Design principle**: Pure curl implementation — no Node.js/Python dependency.
Maximally portable across all Unix systems.

---

### A9. `ssh` — SSH Management

| Field | Value |
|-------|-------|
| **Inspiration** | ssh-tunnel (A+) + ssh-essentials (A) |
| **Score** | 4.0 / 5.0 |
| **Mode** | CLI-Wrapper |
| **Binaries** | `ssh`, `scp`, `rsync` |
| **Install** | Pre-installed |
| **Triggers** | ssh, remote, tunnel, port forward, scp, rsync, jump host |

**Core responsibility**: SSH connection management, port forwarding, file transfer,
and jump host configuration.

**Key design decisions**:
- Connection management: SSH config best practices, multi-host management, ControlMaster multiplexing
- Port forwarding: Local/remote/dynamic forwarding with real-world use cases
- File transfer: scp (simple) vs rsync (incremental sync) decision guide
- Jump hosts: ProxyJump config, multi-hop tunnels
- Key management: ed25519 recommended, ssh-agent lifecycle, key rotation
- Troubleshooting: -v debug levels, common error diagnosis, known_hosts management

**Design fusion** (two top-rated sources):
- ssh-tunnel's advanced port forwarding + ssh-essentials' comprehensive coverage
- Scene-driven: "I need to access a remote database" → exact SSH command

---

### A10. `typeset` — Document Rendering

| Field | Value |
|-------|-------|
| **Inspiration** | typetex (A) + tex-render (A-) + LaTeX knowledge (B) |
| **Score** | 3.8 / 5.0 |
| **Mode** | Tool-Scoped (`allowed-tools: [Bash]`) |
| **Binaries** | `curl` (remote) or `typst` (local) |
| **Install** | `brew install typst` or no install needed (API mode) |
| **Triggers** | typeset, LaTeX, Typst, PDF, render document, compile |

**Core responsibility**: Document typesetting and rendering to PDF using Typst
(preferred) or LaTeX, with both local and remote compilation modes.

**Key design decisions**:
- Typst-first: Recommended for new documents (10x simpler than LaTeX)
- LaTeX-compatible: Full LaTeX support for existing/legacy documents
- Math rendering: LaTeX math → PNG/SVG images for inline use
- Dual mode: Local compilation (typst CLI) or remote API (zero dependency, curl only)
- Template library: Resume, paper, report, presentation templates in Typst
- LaTeX gotchas: Special characters, floats, citations, common errors quick-ref

**Design fusion** (three sources):
- typetex's zero-dependency API + tex-render's local rendering + LaTeX knowledge's gotchas

---

## 5. File Structure

```
skills/
├── foundation/         # F1-F8 (existing, prompt-only)
├── workflow/           # W1-W7 (existing, prompt-only)
├── specialist/         # S1-S5 (existing, prompt-only)
└── automation/         # A1-A10 (NEW, script-capable)
    ├── github/SKILL.md
    ├── playwright/SKILL.md
    ├── http-client/SKILL.md
    ├── web-scraper/SKILL.md
    ├── data-pipeline/SKILL.md
    ├── media-tools/SKILL.md
    ├── notification/SKILL.md
    ├── email/SKILL.md
    ├── ssh/SKILL.md
    └── typeset/SKILL.md
```

### SKILL.md Template (Automation Layer)

```markdown
---
name: <skill-name>
description: <one-line description>
emoji: "<emoji>"
category: "automation"
cli-wrapper: true  # or false for Tool-Scoped mode
allowed-tools:
  - Bash           # for Tool-Scoped and Hybrid modes
requirements:
  binaries:
    - <binary-name>
  platforms:
    - macos
    - linux
  install:
    - manager: brew
      package: <package-name>
triggers:
  - <trigger-1>
  - <trigger-2>
---

# <Skill Title>

## When to Use
<2-3 sentences on when this skill is appropriate>

## Prerequisites
<Binary dependencies, installation commands, verification steps>

## Core Operations
<Scene-driven command templates with natural language → command mapping>

## Command Reference
<Quick-lookup table of common operations>

## Gotchas & Troubleshooting
<Common pitfalls with solutions>

## Aleph Integration
<How this skill leverages Aleph's tools and conventions>
```

---

## 6. Implementation Priority

| Priority | Skills | Rationale |
|----------|--------|-----------|
| **P0** (Day 1) | github, http-client, ssh | Zero-install, highest daily frequency |
| **P1** (Day 2) | data-pipeline, media-tools, notification | Common automation scenarios |
| **P2** (Day 3) | playwright, web-scraper, email, typeset | Require additional dependencies |

---

## 7. Quality Criteria

Each automation skill must pass before shipping:

- [ ] **Script-capable**: Uses `cli-wrapper: true` or `allowed-tools: [Bash]` (no pure prompt)
- [ ] **Dependencies declared**: `requirements.binaries` lists all needed tools
- [ ] **Install documented**: Clear installation steps for each platform
- [ ] **Scene-driven**: Operations organized as "I want to X" → here's how
- [ ] **Concise**: 150-400 lines (automation skills may be slightly longer than knowledge skills)
- [ ] **Actionable**: Every command template is copy-paste executable
- [ ] **Safety-aware**: Dangerous operations flagged, credential handling documented
- [ ] **Tested**: Manually verified commands work on macOS
- [ ] **Unique**: No overlap with existing 20 skills or other automation skills
- [ ] **SKILL.md compliant**: Valid frontmatter, correct mode, meaningful triggers

---

## 8. Dependency Matrix

| Skill | Required Binaries | Pre-installed | Brew Install | Other |
|-------|------------------|---------------|-------------|-------|
| A1 github | `gh` | No | `brew install gh` | — |
| A2 playwright | `npx` | No | `npm install -g playwright` | `npx playwright install chromium` |
| A3 http-client | `curl` | **Yes** | — | — |
| A4 web-scraper | `curl`, `python3` | **Yes** | — | `pip install requests beautifulsoup4` |
| A5 data-pipeline | `jq`, `python3` | python3 yes | `brew install jq` | — |
| A6 media-tools | `ffmpeg` | No | `brew install ffmpeg` | — |
| A7 notification | `curl`, `osascript` | **Yes** | — | — |
| A8 email | `curl` | **Yes** | — | — |
| A9 ssh | `ssh`, `scp`, `rsync` | **Yes** | — | — |
| A10 typeset | `curl` or `typst` | curl yes | `brew install typst` | — |

**6 out of 10 skills work with zero installation** (http-client, web-scraper partial,
notification, email, ssh, typeset API mode).
