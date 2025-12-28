## ADDED Requirements

### Requirement: Direct Provider Editing Mode
When a user selects a provider from the list, the system SHALL immediately display the editable configuration form without intermediate view states.

#### Scenario: Click unconfigured preset provider
- **GIVEN** the user is on the Providers tab
- **WHEN** the user clicks an unconfigured preset provider (e.g., "OpenAI", "Claude", "Gemini")
- **THEN** the right panel SHALL display an editable configuration form
- **AND** the form SHALL be pre-populated with preset default values (model, base_url, color)
- **AND** all form fields SHALL be immediately editable
- **AND** the "Save" button SHALL be enabled when required fields are filled

#### Scenario: Click configured provider
- **GIVEN** the user is on the Providers tab
- **AND** a provider is already configured with saved settings
- **WHEN** the user clicks that provider in the list
- **THEN** the right panel SHALL display an editable configuration form
- **AND** the form SHALL be pre-populated with the provider's saved configuration
- **AND** all form fields SHALL be immediately editable
- **AND** the user can modify any field and click "Save" to update

#### Scenario: No intermediate "Configure" button
- **GIVEN** the user selects any provider from the list
- **WHEN** the right panel displays the provider details
- **THEN** there SHALL NOT be a "Configure this Provider" button
- **AND** the edit form SHALL be displayed directly
- **AND** the user can immediately edit and save without additional clicks

### Requirement: Provider-Specific Configuration Parameters
The configuration form SHALL display provider-specific parameters based on the selected provider type.

#### Scenario: OpenAI provider parameters
- **GIVEN** the user selects a provider with provider_type="openai"
- **WHEN** the configuration form is displayed
- **THEN** the form SHALL display the following parameters:
- **Required**: API Key, Model, Base URL (optional)
- **Generation Parameters**: temperature (0-2, default 1.0), max_tokens (default 1024), top_p (0-1, default 1.0)
- **Advanced**: frequency_penalty (-2 to 2, default 0), presence_penalty (-2 to 2, default 0), timeout_seconds (default 30)
- **AND** each field SHALL display help text explaining its purpose
- **AND** placeholder text SHALL show recommended default values

#### Scenario: Anthropic Claude provider parameters
- **GIVEN** the user selects a provider with provider_type="claude"
- **WHEN** the configuration form is displayed
- **THEN** the form SHALL display the following parameters:
- **Required**: API Key, Model
- **Generation Parameters**: temperature (0-1, default 1.0), max_tokens (default 1024), top_p (0-1, default 1.0), top_k (default 40)
- **Advanced**: stop_sequences (comma-separated), timeout_seconds (default 30)
- **AND** each field SHALL display help text explaining its purpose
- **AND** the form SHALL note that Claude does not support both temperature and top_p simultaneously for Opus models

#### Scenario: Google Gemini provider parameters
- **GIVEN** the user selects a provider with provider_type="gemini"
- **WHEN** the configuration form is displayed
- **THEN** the form SHALL display the following parameters:
- **Required**: API Key, Model, Base URL (default: https://generativelanguage.googleapis.com/v1beta)
- **Generation Parameters**: temperature (0-2, default 1.0), maxOutputTokens (labeled as "Max Tokens", default 2048), topP (0-1, default 0.95), topK (default 40)
- **Advanced**: thinking_level (dropdown: LOW/HIGH, default HIGH for Gemini 3), media_resolution (LOW/MEDIUM/HIGH), timeout_seconds (default 30)
- **AND** each field SHALL display help text explaining its purpose
- **AND** the form SHALL note that thinking_level only applies to Gemini 3 models

#### Scenario: Ollama provider parameters
- **GIVEN** the user selects a provider with provider_type="ollama"
- **WHEN** the configuration form is displayed
- **THEN** the form SHALL display the following parameters:
- **Required**: Model (no API key required)
- **Base URL**: Default http://localhost:11434
- **Generation Parameters**: temperature (default 0.8), num_predict (labeled as "Max Tokens", default 512), top_k (default 40), top_p (0-1, default 0.9)
- **Advanced**: repeat_penalty (default 1.1), stop (comma-separated sequences), timeout_seconds (default 30)
- **AND** the API Key field SHALL be hidden for Ollama providers
- **AND** each field SHALL display help text explaining its purpose

### Requirement: Parameter Field Organization
Configuration parameters SHALL be organized into logical groups with clear visual hierarchy.

#### Scenario: Form field grouping
- **GIVEN** the provider configuration form is displayed
- **WHEN** the user views the form
- **THEN** fields SHALL be organized into the following groups:
- **Basic Settings**: Provider Name, Provider Type, API Key (if required), Model, Base URL
- **Generation Parameters**: temperature, max_tokens/maxOutputTokens/num_predict, top_p/topP, top_k/topK
- **Advanced Settings** (collapsible): timeout_seconds, provider-specific parameters (frequency_penalty, presence_penalty, stop_sequences, thinking_level, repeat_penalty)
- **AND** each group SHALL have a clear section header
- **AND** "Advanced Settings" SHALL be collapsed by default

#### Scenario: Required vs optional field indicators
- **GIVEN** any parameter field in the form
- **WHEN** the field is displayed
- **THEN** required fields SHALL be marked with an asterisk (*) in the label
- **AND** optional fields SHALL have "(Optional)" suffix in the label
- **AND** missing required fields SHALL prevent the "Save" button from being enabled
- **AND** hovering over a field label SHALL display a tooltip with parameter details

## MODIFIED Requirements

### Requirement: Providers Tab Layout Proportions
The Providers tab SHALL use a balanced two-panel layout with direct editing capability.

#### Scenario: Left panel (provider list) width
- **GIVEN** the Providers tab is selected
- **WHEN** the window is at minimum size (1200x800)
- **THEN** the left panel (provider list) SHALL have:
- Minimum width: 280 points (reduced from 450 to match uisample.png)
- Ideal width: 320 points
- Maximum width: 400 points
- **AND** the panel SHALL contain search bar and provider cards in a compact list format

#### Scenario: Right panel (edit panel) width
- **GIVEN** a provider is selected
- **WHEN** the edit panel is visible
- **THEN** the right panel SHALL have:
- Minimum width: 650 points (increased to accommodate more form fields)
- Ideal width: 880 points (fills remaining space in 1200px window)
- Maximum width: infinity (grows with window)
- **AND** the panel SHALL display the editable configuration form directly
- **AND** the panel SHALL be separated from left panel by a visible divider

#### Scenario: Responsive layout
- **GIVEN** the user resizes the Settings window
- **WHEN** the window width changes
- **THEN** the right panel SHALL consume most of the additional width
- **AND** the left panel SHALL remain at a fixed width (280-400 points)
- **AND** neither panel SHALL shrink below its minimum width
- **AND** content SHALL remain readable without horizontal scrolling (except for long URLs)

### Requirement: ScrollView Behavior
ScrollViews SHALL handle overflow content gracefully with fixed header/footer sections.

#### Scenario: Provider list scrolling
- **GIVEN** more than 8 provider cards exist
- **WHEN** the provider list exceeds the visible area
- **THEN** the list SHALL scroll vertically with native macOS scrollbars
- **AND** the search bar SHALL remain fixed at the top
- **AND** the "Add Custom Provider" button SHALL remain accessible at the top

#### Scenario: Edit panel scrolling
- **GIVEN** the edit form has many fields (e.g., all Generation Parameters and Advanced Settings)
- **WHEN** the form exceeds the visible area
- **THEN** the form content SHALL scroll vertically
- **AND** the form header (provider name and Active toggle) SHALL scroll with content
- **AND** the action buttons (Test Connection, Cancel, Save) SHALL remain fixed at the bottom footer
- **AND** scrolling SHALL be smooth with momentum (native macOS behavior)
- **AND** the fixed footer SHALL have a subtle top divider to separate from scrollable content
