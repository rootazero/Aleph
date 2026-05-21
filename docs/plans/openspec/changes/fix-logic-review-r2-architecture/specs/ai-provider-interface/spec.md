## ADDED Requirements

### Requirement: Rate Limit Retry Backoff
`FailoverProvider` SHALL respect `Retry-After` headers on HTTP 429 responses instead of retrying aggressively.

#### Scenario: 429 with Retry-After header
- **WHEN** a provider returns HTTP 429 with a `Retry-After: 30` header
- **THEN** the failover logic SHALL wait at least 30 seconds before retrying that provider

#### Scenario: 429 without Retry-After header
- **WHEN** a provider returns HTTP 429 without a Retry-After header
- **THEN** the failover logic SHALL use exponential backoff starting at 1 second

### Requirement: Image Data Passthrough
`FailoverProvider::process_with_image` SHALL forward image data to the inner provider.

#### Scenario: Image processing request
- **WHEN** a multimodal request includes image data
- **THEN** the image SHALL be passed through to the selected provider's vision endpoint

#### Scenario: Provider does not support images
- **WHEN** the selected provider does not support image input
- **THEN** the failover logic SHALL try the next provider that does support images
