## Context
Phase 10 (The Hands) formalizes the Skill Compiler: a pipeline that converts repeated successful executions into reusable skills or tool-backed automations. The design builds on the existing `skill_evolution` modules and integrates with POE crystallization and the tool registry.

## Goals / Non-Goals
- Goals:
  - Detect repeated successful patterns and emit solidification suggestions.
  - Require explicit user approval before persisting new skills or tools.
  - Generate SKILL.md skills and register them in the skills registry.
  - Support optional tool-backed skills for deterministic transforms.
- Non-Goals:
  - Auto-generation without user approval.
  - Cloud-based compilation or remote execution.
  - Complex multi-language toolchains (beyond the initial supported runtime).

## Decisions
- Use existing `skill_evolution` components (EvolutionTracker, SolidificationDetector, SkillGenerator) as the backbone.
- Store generated skills under `~/.aether/skills/<skill-id>/SKILL.md` and reload registry on success.
- Tool-backed skills are generated as a local package containing:
  - `tool_definition.json` (name, description, input schema)
  - `entrypoint` script (initially Python)
- Tool-backed skills must pass a self-test before registration; failures do not register and leave no side effects.
- All generated tools are gated through confirmation/permission flow before first use.

## Risks / Trade-offs
- False positives could create noisy skills; mitigated by thresholds and user approval.
- Tool-backed skills add security surface; mitigated by sandboxed execution and confirmation.
- Poor instruction quality may reduce usefulness; mitigated by preview and editable suggestion.

## Migration Plan
- Add compiler configuration with conservative defaults (disabled for tool-backed generation).
- Keep existing manual skills workflow unchanged; compiler is additive.

## Open Questions
- Should tool-backed generation support Rust compilation in Phase 10 or be deferred?
- Where should compiled tool packages be stored (`~/.aether/tools/compiled/` vs a registry directory)?
- Should compiler suggestions be exposed in Settings UI or via conversational prompts only?
