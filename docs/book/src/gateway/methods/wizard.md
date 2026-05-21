# wizard.* Methods

Configuration wizard for guided setup flows.

## Overview

The wizard system provides a multi-step configuration experience with:
- Multiple step types (select, text, confirm, etc.)
- Session-based state management
- Support for multiple concurrent wizards

## Methods

### wizard.start

Start a new wizard session.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "wizard.start",
  "params": {
    "wizard_type": "onboarding",
    "initial_data": {}
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "session_id": "wizard-uuid",
    "step": {
      "id": "step-1",
      "type": "note",
      "title": "Welcome",
      "message": "Welcome to Aleph setup!"
    },
    "status": "running"
  }
}
```

### wizard.answer

Answer the current step and get the next one.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "wizard.answer",
  "params": {
    "session_id": "wizard-uuid",
    "step_id": "step-2",
    "value": "anthropic"
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "done": false,
    "step": {
      "id": "step-3",
      "type": "text",
      "message": "Enter your API key:",
      "placeholder": "sk-...",
      "sensitive": true
    },
    "status": "running"
  }
}
```

### wizard.next

Get the next step without answering (for notes/intros).

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 3,
  "method": "wizard.next",
  "params": {
    "session_id": "wizard-uuid"
  }
}
```

### wizard.cancel

Cancel an active wizard session.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "method": "wizard.cancel",
  "params": {
    "session_id": "wizard-uuid"
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 4,
  "result": {
    "cancelled": true
  }
}
```

### wizard.status

Get the status of a wizard session.

**Request:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "method": "wizard.status",
  "params": {
    "session_id": "wizard-uuid"
  }
}
```

**Response:**
```json
{
  "jsonrpc": "2.0",
  "id": 5,
  "result": {
    "status": "running"
  }
}
```

## Step Types

| Type | Description | Value Type |
|------|-------------|------------|
| `note` | Informational message | None |
| `select` | Single selection | String |
| `multi_select` | Multiple selection | String[] |
| `text` | Text input | String |
| `confirm` | Yes/No question | Boolean |
| `progress` | Progress indicator | None |

## Step Structure

```json
{
  "id": "step-1",
  "type": "select",
  "title": "Provider Selection",
  "message": "Which AI provider would you like to use?",
  "options": [
    {
      "value": "anthropic",
      "label": "Anthropic (Claude)",
      "hint": "Recommended for coding"
    },
    {
      "value": "openai",
      "label": "OpenAI (GPT)",
      "hint": "GPT-4o and o3 series"
    }
  ],
  "initial_value": "anthropic",
  "sensitive": false
}
```

## Available Wizards

| Type | Description |
|------|-------------|
| `onboarding` | Full 10-stage setup |
| `quick-setup` | Minimal configuration |
| `provider-setup` | Add a new provider |

## Wizard Status

| Status | Description |
|--------|-------------|
| `running` | Wizard is active |
| `done` | Completed successfully |
| `cancelled` | User cancelled |
| `error` | An error occurred |

## Onboarding Flow Stages

1. **Welcome** - Introduction
2. **Provider Selection** - Choose AI provider
3. **Credentials** - Enter API key
4. **Primary Model** - Select main model
5. **Secondary Model** - Optional failover
6. **Thinking Level** - Configure AI depth
7. **Messaging Apps** - Optional integrations
8. **Review** - Configuration summary
9. **Finalize** - Apply configuration
10. **Complete** - Success message
