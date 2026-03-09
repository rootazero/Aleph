# media-video — Aleph Example Plugin

A minimal Node.js plugin example demonstrating Aleph's plugin system.

## What this demonstrates

1. **TOML manifest** (`aleph.plugin.toml`) — declaring plugin metadata, tools, hooks, and permissions
2. **JSON-RPC stdio transport** — the communication protocol between Aleph host and Node.js plugins
3. **Tool handler** — `video_extract_frames` receives parameters and returns structured results
4. **Hook handler** — a `PreToolUse` interceptor that enriches `media_understand` calls with video metadata

## File structure

```
media-video/
  aleph.plugin.toml   # Plugin manifest (tool + hook declarations)
  package.json         # Node.js package metadata
  src/
    index.js           # Plugin entry point (JSON-RPC listener + handlers)
  README.md            # This file
```

## Running

This is a stub plugin — tool handlers return placeholder results. A real
implementation would require `ffmpeg` and `ffprobe` on the system PATH.

```bash
# Validate the plugin manifest
aleph plugin validate .

# Pack for distribution
aleph plugin pack .

# Test the JSON-RPC interface directly
echo '{"jsonrpc":"2.0","id":"1","method":"plugin.call","params":{"handler":"videoExtractFrames","arguments":{"file_path":"/tmp/test.mp4","count":5}}}' | node src/index.js
```

## Key concepts

- **Tools** are functions the AI can call. Declared in the manifest with a name, description, parameter schema, and handler function name.
- **Hooks** let plugins observe or intercept events in the Aleph lifecycle. This plugin uses a `PreToolUse` interceptor filtered to `media_understand` to inject video metadata before that tool runs.
- **JSON-RPC 2.0 over stdio** is the IPC protocol. The host sends one JSON object per line on stdin; the plugin replies on stdout.
