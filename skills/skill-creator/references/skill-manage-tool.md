# skill_manage Tool Reference

The `skill_manage` builtin authors and curates skills. It vets every write (length +
security scan), protects bundled skills as read-only, and hot-loads new skills so they are
usable the same session.

## Actions

### create
Create a brand-new skill under `~/.aleph/skills/<id>/SKILL.md`.
- Required: `name`, `description`, `content` (the markdown body).
- Optional: `when_to_use` (rendered as the `<when>` routing hint).
- The id is derived from `name` (lowercased, hyphenated). Fails if the id already exists.
- Frontmatter written: `name`, `description`, `when-to-use` only.

### edit
Rewrite an existing skill's **full** SKILL.md (frontmatter included).
- Required: `skill_id`, `content` (the entire file text, `---` frontmatter + body).
- Use this to set advanced frontmatter (`eligibility`, `install`, `emoji`, `bound-tool`,
  `scope`) that `create` does not write.
- Renaming via edit is not supported — create a new skill and delete the old one.

### edit_section
Patch a slice via find/replace.
- Required: `skill_id`, `find`, `replace`. `find` must match a unique span.

### write_file
Add or overwrite a supporting file.
- Required: `skill_id`, `file_name`, `file_content`.
- `file_name` must live under one of: `references/`, `scripts/`, `assets/`, `templates/`
  (e.g. `references/api.md`). No absolute paths, no `..`, no dot-prefixed segments.

### configure
Toggle runtime state: `enabled` (bool) and/or `scope` (string).

## Limits & Vetting

- SKILL.md must be < **100,000** characters.
- `name` and `description` have length caps; keep them tight.
- Every write runs the content security scan. **Trusted-level findings block** the write;
  **Caution-level findings warn** but allow it. Remove flagged constructs and retry.
- **Bundled skills (shipped in the repo) are read-only.** To modify one, `create` a copy
  under `~/.aleph/skills`.

## After create

The new skill is written to disk, its authoring root is registered for future rescans, and
the file is hot-loaded — it can route in the current session. Confirm with `skill_status`.
