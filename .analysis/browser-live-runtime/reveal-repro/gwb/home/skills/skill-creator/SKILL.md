---
name: skill-creator
description: "Create, refactor, and evaluate Aleph skills, and author high-quality memory skill-notes — decide when a skill is warranted, write router-quality descriptions, apply progressive disclosure, and author SKILL.md + supporting files via the skill_manage tool. Use when the user wants to create/author/improve a skill, package a repeated workflow, says 'make this a skill', or when authoring NoteType::Skill memory notes."
---

# Skill Creator

Create and refine Aleph skills, and author high-quality memory skill-notes. The
authoring toolchain already exists (`skill_manage`); this skill is the *method*.

## Critical Rules

1. **Check near-neighbors first** — run `skill_status` to list existing skills. If one
   already covers the job, extend it; do not create a duplicate (R3 core minimalism).
2. **Description-first** — write the `description` and self-test routing *before* expanding
   the body. The `description` is the router; a vague one means the skill never triggers.
3. **Keep SKILL.md lean** — push depth into `references/`, deterministic logic into
   `scripts/`. Progressive disclosure: the entrypoint routes, references teach.
4. **Bundled skills are read-only** — to change a bundled skill, `skill_manage(action="create")`
   a copy under `~/.aleph/skills`; never edit in place.
5. **The security scan vets every write** — Trusted-level findings block the write, Caution
   warns. Remove flagged constructs; don't fight the scanner.
6. **Memory skill-notes follow the same bar** — when authoring a `NoteType::Skill` note via
   `note_manage`, apply [skill-note-authoring.md](references/skill-note-authoring.md).

## When NOT to Create a Skill

Prefer a direct answer, a note (`note_manage`), a script, or an MCP server (`mcp-dev`) when
the job is one-off, explanatory, or already covered by a near-neighbor. Promote to a skill
only when the workflow is recurring, easy to route incorrectly, or needs a reusable boundary.
Full gate: [when-not-to-create.md](references/when-not-to-create.md).

## Core Loop

1. **Qualify** — should this be a skill at all? (see gate above)
2. **Intent** — ask only the 2-3 questions that change the package: the recurring job, the
   real inputs/outputs, and what near-neighbor requests stay out of scope.
3. **Pick the smallest shape** — single SKILL.md, or SKILL.md + a few references; personal
   (`~/.aleph/skills`) vs durable/bundled.
4. **Write + self-test the description** — does it route on the real trigger phrases?
5. **Write the body / split references** — lean entrypoint, depth in `references/`.
6. **Create + verify** — `skill_manage(action="create", ...)` hot-loads it; confirm with
   `skill_status`.

## skill_manage Tool

| action | Use for | Key params |
|--------|---------|-----------|
| `create` | New skill (SKILL.md) | `name`, `description`, `when_to_use`, `content` |
| `edit` | Rewrite full SKILL.md incl. frontmatter | `skill_id`, `content` |
| `edit_section` | Find/replace a slice | `skill_id`, `find`, `replace` |
| `write_file` | Add a support file | `skill_id`, `file_name` (e.g. `references/x.md`), `file_content` |
| `configure` | Toggle enabled / scope | `skill_id`, `enabled`, `scope` |

Full parameter and frontmatter reference: [skill-manage-tool.md](references/skill-manage-tool.md).

## Frontmatter Cheatsheet

`create` writes only `name` / `description` / `when-to-use`. Advanced fields
(`eligibility`, `install`, `emoji`, `bound-tool`, `scope`) require `action="edit"` with the
full SKILL.md text. Keep `name` short and kebab-friendly; keep `description` a single line
that names the trigger.

## Reference Docs

- **[when-not-to-create.md](references/when-not-to-create.md)** — the qualification gate.
- **[authoring-method.md](references/authoring-method.md)** — description-first routing,
  progressive disclosure, intent dialogue, the two landing tiers.
- **[skill-manage-tool.md](references/skill-manage-tool.md)** — every `skill_manage` action,
  full frontmatter fields, limits, security vetting, hot-load behavior.
- **[skill-note-authoring.md](references/skill-note-authoring.md)** — quality bar for
  `NoteType::Skill` memory notes.
