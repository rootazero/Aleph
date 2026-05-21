# Proposal: Block Streaming & Auth Profiles

## Summary

Implement intelligent block streaming with Markdown fence awareness and API key rotation with exponential backoff, following Moltbot's production-proven patterns.

## Motivation

1. **Block Streaming**: Current `BlockReplyChunker` lacks fence awareness - splitting inside code blocks breaks Markdown rendering in clients
2. **Auth Profiles**: No API key rotation - single key failure blocks all requests instead of rotating to alternatives

## Scope

### In Scope
- Markdown fence parsing and safe-break detection
- Fence split/reopen when forced to break inside code block
- Block coalescing with idle timeout
- Auth profile data model and storage
- Exponential backoff (rate-limit vs billing)
- Round-robin profile rotation

### Out of Scope
- OAuth token refresh (future work)
- Per-channel coalescing UI configuration
- Profile migration tools

## References

- Moltbot `src/agents/pi-embedded-block-chunker.ts`
- Moltbot `src/markdown/fences.ts`
- Moltbot `src/agents/auth-profiles/`
