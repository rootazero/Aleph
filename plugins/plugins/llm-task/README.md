# llm-task — Aleph Plugin

Batch LLM operations: send multiple prompts in parallel, map templates over items, chain sequential calls, and evaluate prompt quality.

## Tools

- **llm_batch** — Send multiple prompts in parallel with concurrency control
- **llm_map** — Apply a prompt template (with `{{item}}`) to each item in an array
- **llm_chain** — Sequential LLM calls where each step's output feeds into the next
- **llm_evaluate** — Test a prompt against expected outputs using an LLM judge

## Configuration

Set `ANTHROPIC_API_KEY` environment variable for API access.

## File structure

```
llm-task/
  aleph.plugin.toml   # Plugin manifest
  package.json         # Node.js package metadata
  src/
    index.js           # JSON-RPC stdio listener + handler routing
    batch.js           # Core batch execution engine
    templates.js       # Template rendering utilities
  README.md
```

## Testing

```bash
# Test the JSON-RPC interface
echo '{"jsonrpc":"2.0","id":"1","method":"ping"}' | node src/index.js

# Test llm_batch (requires ANTHROPIC_API_KEY)
echo '{"jsonrpc":"2.0","id":"2","method":"plugin.call","params":{"handler":"llmBatch","arguments":{"prompts":["Say hello","Say goodbye"]}}}' | node src/index.js
```
