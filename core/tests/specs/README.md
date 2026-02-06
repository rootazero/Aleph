# YAML Spec Tests

This directory contains YAML-based behavior specifications for AI capability evaluation.

## Purpose

While Gherkin tests (in `features/`) handle deterministic behavior regression, YAML Specs enable:

1. **Semantic validation** via LlmJudge
2. **AI behavior evaluation** that can't be expressed as simple assertions
3. **Given-When-Then** scenarios with flexible validation criteria

## Directory Structure

```
specs/
├── dispatcher/           # Dispatcher context specs
│   └── dag_scheduling.spec.yaml
├── memory/              # Memory context specs (future)
├── intent/              # Intent context specs (future)
└── README.md
```

## Spec Format

```yaml
name: "Spec Name"
version: "1.0"
context:
  description: "What this spec validates"

scenarios:
  - name: "Scenario name"
    given:
      - condition: value
    when:
      action: "action_name"
    then:
      - assertion_type: "deterministic"  # or "semantic"
        expected: value
      - llm_judge:
          prompt: "Validation prompt"
          criteria: "Success criteria"
```

## Running Specs

```bash
# Run all specs
cargo test --test spec_runner

# Run specific spec
cargo test --test spec_runner -- dispatcher/dag_scheduling
```

## Integration with Gherkin

The `UnifiedTestRunner` coordinates both test types:
- Gherkin: Deterministic behavior regression
- YAML Spec: AI capability evaluation

Both must pass for CI to succeed.
