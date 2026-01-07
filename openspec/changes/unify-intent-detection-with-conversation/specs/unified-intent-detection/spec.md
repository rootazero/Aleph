# Unified Intent Detection Specification

## MODIFIED Requirements

### Requirement: AI-First Intent Detection

The system SHALL process every user message (initial or continuation) through AI-based intent detection to determine capability needs and missing parameters.

#### Scenario: User asks weather without location

**Given** user inputs "今天天气怎么样"
**When** the message is processed
**Then** AI returns clarification request for location parameter
**And** Halo shows clarification UI with city options
**And** after user selects city, search capability is invoked
**And** AI generates response with search results

#### Scenario: User provides complete information

**Given** user inputs "北京今天天气怎么样"
**When** the message is processed
**Then** AI determines search capability is needed
**And** search capability executes with query "北京天气"
**And** AI generates response with search results
**And** no clarification UI is shown

#### Scenario: General conversation without capability

**Given** user inputs "你好，今天过得怎么样？"
**When** the message is processed
**Then** AI responds directly without invoking any capability
**And** no clarification UI is shown

#### Scenario: Video URL provided

**Given** user inputs "summarize this video https://youtube.com/watch?v=xxx"
**When** the message is processed
**Then** AI determines video capability is needed
**And** video capability extracts transcript
**And** AI generates summary based on transcript

#### Scenario: Video request without URL

**Given** user inputs "summarize this video"
**When** the message is processed
**Then** AI returns clarification request for URL parameter
**And** Halo shows text input for URL
**And** after user provides URL, video capability is invoked

### Requirement: Multi-turn Conversation with Intent Detection

The system SHALL independently evaluate intent for each turn in a multi-turn conversation.

#### Scenario: Follow-up requires different capability

**Given** an active conversation about weather
**And** user inputs "帮我搜一下明天的新闻"
**When** the follow-up is processed
**Then** AI determines search capability is needed for news
**And** search executes independently of previous weather context

#### Scenario: Follow-up needs clarification

**Given** an active conversation
**And** user inputs "翻译成"
**When** the follow-up is processed
**Then** AI returns clarification request for target language
**And** Halo shows clarification UI

#### Scenario: Conversation context is maintained

**Given** previous turn asked about Beijing weather
**And** user inputs "那上海呢？"
**When** the follow-up is processed
**Then** AI understands context refers to weather
**And** search executes for Shanghai weather

## REMOVED Requirements

### Requirement: Regex-based Intent Detection (REMOVED)

The legacy regex-based SmartTriggerDetector is removed. All intent detection is now AI-powered.

### Requirement: Weather-specific Intent Patterns (REMOVED)

Hardcoded weather patterns (`IntentType::Weather`, weather regex) are removed. AI handles weather detection generically along with all other intents.

### Requirement: Legacy Mode Flag (REMOVED)

The `ai_first` configuration flag is removed. AI-first mode is now the only mode.
