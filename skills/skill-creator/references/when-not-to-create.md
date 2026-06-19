# When NOT to Create a Skill

A skill is a *routed, reusable capability*. Most requests are not that. Default to the
lightest artifact that solves the job — creating a skill for a one-off bloats the index and
makes routing worse for every other skill (R3 core minimalism).

## Use a direct answer when the job is

- explain a concept, summarize a document, translate text
- brainstorm ideas without packaging
- a one-off question with no reuse value

## Use a note (`note_manage`) when the output is

- durable knowledge: a preference, a lesson, a reference document, a project fact
- something to recall later, not a routed capability

## Use a script when

- the task is deterministic and routing is not the hard part
- the user wants a utility, not a capability the agent must discover

## Use an MCP server (`mcp-dev`) when

- the capability wraps an external service/API with its own tools
- it belongs in a plugin, not the skill index

## Promote to a skill only when at least one holds

- the workflow will be reused
- it is easy to route incorrectly without a dedicated entry
- it needs a reusable boundary, checklist, or supporting files

## Near-neighbor check (always)

Run `skill_status` first. If an existing skill covers ≥80% of the job, extend it with
`skill_manage(action="edit"/"edit_section"/"write_file")` instead of creating a new one.

When in doubt, start personal (`~/.aleph/skills`) and promote to durable only after reuse
becomes real.
