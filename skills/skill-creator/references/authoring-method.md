# Authoring Method

## Description-First Routing

The `description` is the only thing the router sees before deciding to load a skill. Write
it first and self-test it:

- Name the **trigger**: the verbs and nouns a user actually says ("create a skill",
  "package this workflow", "make this a skill").
- State **when to use**, not just what it does. "Use when X" beats a feature list.
- Keep it one line. If you need two, the skill is probably two skills.
- Self-test: read three plausible user messages. Does this description clearly win for the
  ones it should, and clearly lose for near-neighbors?

## Progressive Disclosure

Keep `SKILL.md` lean — it routes and orients. Push depth down:

- `references/*.md` — the *how*, read on demand when the agent is mid-task.
- `scripts/*` — deterministic logic the agent runs instead of re-deriving.
- `assets/`, `templates/` — fixtures and starting points.

A reader should grasp *what the skill does and when* from SKILL.md alone, and reach for a
reference only when executing that part.

## Intent Dialogue (2-3 questions, not a form)

Before authoring, ask only what changes the package design:

1. What recurring job should this own — and what real inputs/outputs does it have?
2. What near-neighbor requests must stay *out* of scope?
3. Any constraints (privacy, naming, portability) that shape it?

Open human and warm; offer a template only as an optional shortcut. Do not interrogate.

## Two Landing Tiers

Aleph skills land in one of two places — pick the lightest that fits:

- **Personal** — `~/.aleph/skills`, created via `skill_manage(action="create")`. Exploratory,
  user-specific, short-lived. Start here when unsure.
- **Durable / bundled** — shipped in the repo `skills/` tree, read-only at runtime, shared by
  every install. Promote here only when the skill has proven reuse and a stable contract.

(yao-meta-skill's four modes — Scaffold/Production/Library/Governed — collapse to these two
real landing spots in Aleph.)

## Author, Then Check

Tie each meaningful change to a check: did the description route correctly on a sample
message? Did a sample run produce the intended output? Did `skill_status` confirm the new
skill hot-loaded? Author small, verify, iterate — do not ship a large unverified package.
