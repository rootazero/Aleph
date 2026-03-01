## ADDED Requirements

### Requirement: Subshell Substitution Blocking
The exec parser SHALL reject commands containing `$()`, backtick, or `$(command)` subshell substitution patterns before execution.

#### Scenario: Command with $() is blocked
- **WHEN** a command contains `$(whoami)` or similar `$()` patterns
- **THEN** the parser SHALL return an error indicating subshell substitution is not allowed

#### Scenario: Command with backticks is blocked
- **WHEN** a command contains backtick-delimited subshell expressions
- **THEN** the parser SHALL return an error indicating subshell substitution is not allowed

#### Scenario: Legitimate dollar signs are allowed
- **WHEN** a command contains `$VAR` environment variable references (without parentheses)
- **THEN** the parser SHALL allow the command to proceed

### Requirement: Engine Atomic Action Security Gate
The engine's reflex and bash atomic actions SHALL pass through the exec security gate before execution.

#### Scenario: Reflex bash action requires approval
- **WHEN** a reflex layer triggers a bash command execution
- **THEN** the command SHALL be validated against the exec allowlist and approval workflow before running

#### Scenario: Blocked command in reflex context
- **WHEN** a reflex-triggered bash command is not in the allowlist
- **THEN** the execution SHALL be denied and logged
