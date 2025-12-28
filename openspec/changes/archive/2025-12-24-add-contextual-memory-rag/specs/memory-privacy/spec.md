# memory-privacy Specification

## Purpose

The memory-privacy capability ensures user privacy through PII scrubbing, retention policies, and app exclusion lists for the memory system.

## ADDED Requirements

### Requirement: PII Scrubbing Before Storage
The system SHALL remove personally identifiable information from user input and AI output before storing in memory database.

#### Scenario: Scrub email addresses
- **GIVEN** text: "Contact me at user@example.com"
- **WHEN** PII scrubbing is applied
- **THEN** output: "Contact me at [EMAIL]"

#### Scenario: Scrub phone numbers
- **GIVEN** text: "Call 555-123-4567"
- **WHEN** PII scrubbing applied
- **THEN** output: "Call [PHONE]"

#### Scenario: Scrub multiple PII types
- **GIVEN** text with email + phone + SSN
- **WHEN** scrubbing applied
- **THEN** all PII replaced with placeholders

---

### Requirement: Retention Policy Enforcement
The system SHALL automatically delete memories older than configured retention period.

#### Scenario: Delete expired memories
- **GIVEN** retention_days = 90
- **AND** memories older than 90 days exist
- **WHEN** daily cleanup task runs
- **THEN** old memories are deleted
- **AND** returns count of deleted entries

#### Scenario: Never delete (retention = 0)
- **GIVEN** retention_days = 0
- **WHEN** cleanup runs
- **THEN** no memories are deleted

---

### Requirement: App Exclusion List
The system SHALL not store memories for apps in the exclusion list.

#### Scenario: Exclude password manager
- **GIVEN** excluded_apps = ["com.agilebits.onepassword7"]
- **WHEN** user interacts in 1Password
- **THEN** no memory is stored
- **AND** ingestion is skipped entirely

#### Scenario: Default exclusions
- **WHEN** config is initialized
- **THEN** default exclusion list includes:
  - "com.apple.keychainaccess"
  - Common password managers

---

## Cross-References

### Dependencies
- **memory-storage**: Applies PII scrubbing before insert

---

## Acceptance Criteria

- [ ] PII scrubbed before storage
- [ ] Retention policy enforced automatically
- [ ] App exclusion list respected
- [ ] Default exclusions protect sensitive apps
