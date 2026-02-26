---
name: mcp-dev
description: MCP Server development guide — agent-centric design, 4-phase workflow, Aleph extension integration
scope: standalone
---

# MCP Server Development

## When to Use

Invoke this skill when building MCP (Model Context Protocol) servers — the primary mechanism for extending Aleph's capabilities with external tools and integrations.

## Core Design Principles

### Agent-Centric Design

MCP tools should be designed for **agent workflows**, not as thin API wrappers:

```
BAD:  get_user(id) → returns raw JSON blob
GOOD: find_active_users_with_overdue_tasks() → returns actionable summary

BAD:  send_request(method, url, body) → generic HTTP
GOOD: create_github_issue(title, body, labels) → domain-specific
```

### Optimize for Context

LLM context windows are limited. Every tool response should have high signal-to-noise:

- Return **actionable summaries**, not raw data dumps
- Include only fields the agent needs for the next decision
- Truncate large responses with a "more available" indicator
- Use structured format (JSON) for machine consumption

### Clear Errors

Error messages should guide the agent toward correct usage:

```
BAD:  "Error 400"
GOOD: "Authentication failed. Provide API_KEY environment variable. Get one at https://example.com/settings/api"
```

## 4-Phase Development

### Phase 1: Research & Planning

1. **Study the API**: Read documentation, understand auth, rate limits, data models
2. **Identify workflows**: What agent tasks will this server enable?
3. **Design tools**: Each tool = one complete agent action (not one API call)
4. **Define schemas**: Input validation, output format for each tool

Deliverable: Tool specification document listing tools, inputs, outputs, error cases.

### Phase 2: Implementation

**Project structure (Python with FastMCP):**
```
my-mcp-server/
├── src/
│   ├── server.py          # FastMCP server setup + tool registration
│   ├── tools/             # One file per tool
│   ├── auth.py            # Authentication handling
│   └── utils.py           # Shared utilities
├── tests/
├── pyproject.toml
└── README.md
```

**Implementation order:**
1. Server skeleton with health check
2. Authentication and shared utilities
3. One tool at a time, simplest first
4. Error handling for each tool

**Key patterns:**
```python
from mcp.server.fastmcp import FastMCP

mcp = FastMCP("my-server")

@mcp.tool()
async def search_issues(query: str, status: str = "open") -> str:
    """Search GitHub issues by query and status.

    Args:
        query: Search terms
        status: Filter by status (open, closed, all)
    """
    # Validate inputs
    if not query.strip():
        return "Error: query cannot be empty"

    # Call API
    results = await github.search_issues(query, status)

    # Format for agent consumption
    return format_issue_summary(results)
```

### Phase 3: Review & Refine

Quality checklist:
- [ ] Each tool has clear docstring explaining what it does
- [ ] Input validation with helpful error messages
- [ ] Consistent response format across all tools
- [ ] Rate limiting handled gracefully
- [ ] Auth failures return actionable guidance
- [ ] No sensitive data logged
- [ ] Timeouts on all external calls

### Phase 4: Evaluation

Create 5-10 realistic test scenarios:

```
Scenario: "Find all open bugs assigned to me and prioritize by severity"
Expected: Agent uses search_issues → filter_by_assignee → sort_by_severity
Verify: Correct tool sequence, accurate results, no hallucinated data
```

## Aleph Integration

### Configuration

Add MCP server to Aleph's config (`~/.aleph/aleph.jsonc`):

```jsonc
{
  "mcp": {
    "my-server": {
      "command": ["python", "-m", "my_mcp_server"],
      "environment": {
        "API_KEY": "${MY_API_KEY}"
      }
    }
  }
}
```

### As Aleph Plugin

Alternatively, package as an Aleph plugin with MCP capability:

```
my-plugin/
├── .claude-plugin/
│   └── plugin.json          # Manifest
├── skills/
│   └── my-skill/SKILL.md   # Usage instructions
└── src/
    └── server.py            # MCP server
```

## Anti-Patterns

- **Thin API wrappers**: One MCP tool per API endpoint — combine into workflow-level tools
- **Raw data dumps**: Returning entire API responses instead of agent-friendly summaries
- **No error context**: "Request failed" without guidance on how to fix
- **Hardcoded credentials**: Use environment variables, never embed secrets
- **No input validation**: Agents may pass unexpected types or values
- **Synchronous blocking**: Use async for all I/O operations
