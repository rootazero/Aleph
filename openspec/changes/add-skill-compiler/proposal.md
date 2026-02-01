# Change: Add Skill Compiler (Phase 10 - The Hands)

## Why
Repeated user tasks should become durable skills instead of ad-hoc scripts. A Skill Compiler closes the loop by detecting stable patterns, generating skills or tool-backed automations, and registering them safely. This turns short-term execution into long-term capability and strengthens POE by crystallizing only validated success paths.

## What Changes
- Introduce a Skill Compiler pipeline: tracking, solidification detection, suggestion generation, user approval, and skill creation.
- Optionally generate tool-backed skills (script + tool definition) for deterministic transformations.
- Register generated skills/tools in the existing SkillsRegistry and ToolServer with safety gates.
- Add configuration and status tracking for compiler behavior and thresholds.

## Impact
- Affected specs: `skill-compiler` (new)
- Affected code: `core/src/skill_evolution/`, POE crystallization integration, tool registry, skills registry, config
