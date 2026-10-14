# Authoring Memory Skill-Notes (`NoteType::Skill`)

A skill-note is **reusable procedural knowledge** distilled from experience — not a one-off
fact (that is a `lesson` or `project` note). Create one via `note_manage` with note type
`skill`. The dreaming daemon also emits these automatically; the same quality bar applies.

## Quality Bar

- **Transferable rule, not an anecdote.** State the general procedure or invariant that will
  apply again, not what happened once. "Always X before Y because Z" beats "today X failed".
- **Kebab-case title.** Short, specific, searchable (e.g. `async-error-handling`,
  `retry-on-429`). The title is how the note is referenced and deduped.
- **Symptom → cause → fix shape** when the knowledge is corrective: what you observe, why it
  happens, what to do. This makes the note actionable on recall.
- **Calibrated confidence.** High confidence only with repeated or strong evidence. An
  unconvinced note should not claim high severity — low-confidence + high-severity is
  downgraded by the skill gate.
- **Severity reflects real impact**, not enthusiasm: `low` (nice-to-know) → `critical`
  (causes data loss / hard failure if ignored).
- **Link, don't repeat.** Wikilink related notes (`[[other-note]]`) instead of restating them.

## New vs Strengthen vs Supersede

- **New** — no existing candidate covers this rule.
- **Strengthen** — same rule, more evidence: reference the existing note id verbatim, add
  source facts. Do not reword.
- **Supersede** — only when you have a genuinely better or corrected rule; reference the old
  id and provide the improved title/rule.
- **Skip** — transient noise, not a durable rule.

When in doubt between strengthen and supersede, prefer strengthen — superseding churns the
note and loses linkage.
