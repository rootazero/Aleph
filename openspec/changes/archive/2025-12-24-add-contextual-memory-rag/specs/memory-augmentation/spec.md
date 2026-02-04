# memory-augmentation Specification

## Purpose

The memory-augmentation capability enables Aleph to inject retrieved past interactions into LLM system prompts, providing context-aware AI responses.

## ADDED Requirements

### Requirement: Prompt Augmentation with Memories
The system SHALL inject retrieved memories into the system prompt before sending to AI provider.

#### Scenario: Augment prompt with memories
- **GIVEN** 3 memories retrieved for current context
- **AND** current user input: "What's the timeline?"
- **WHEN** `augment_prompt()` is called
- **THEN** formats memories as context section
- **AND** prepends to system prompt
- **AND** output includes:
  ```
  Here are relevant past interactions:

  [Previous Interaction 1]
  User: <previous user input>
  Assistant: <previous AI output>

  [Previous Interaction 2]
  ...

  Now respond to: What's the timeline?
  ```

#### Scenario: Handle no memories
- **WHEN** no memories retrieved
- **THEN** returns original prompt without augmentation
- **AND** does not add empty context section

#### Scenario: Truncate long memories
- **GIVEN** memory text exceeds 500 chars
- **WHEN** formatting prompt
- **THEN** truncates each memory to 500 chars with "..." suffix
- **AND** prevents token limit overflow

---

## Cross-References

### Dependencies
- **memory-storage**: Retrieves memories to augment

### Consumers
- **core-library**: AI request pipeline uses augmented prompts

---

## Acceptance Criteria

- [ ] Memories formatted correctly in prompt
- [ ] Handles empty memory list
- [ ] Respects token budget limits
- [ ] Integration test: augmented prompt improves AI response quality
