# A2A official Python SDK inbound QA

This fixture belongs to the isolated `a2a-architecture-sdk-qa` worktree. It
pins two independent environments to the official `a2a-sdk` distributions:

```sh
./qa/a2a/run.sh check 0.3.26
./qa/a2a/run.sh check 1.0.3
```

`check` installs each SDK in a separate `uv` environment and checks the public
client, discovery, message, streaming configuration, GetTask, and CancelTask
APIs. It performs no Rust build and needs no Aleph credentials.

When the standard route is mounted, pass an already-built server binary to run
against a real Gateway. This path never compiles Rust:

```sh
ALEPH_SERVER_BIN=/absolute/path/to/aleph-server KEEP=1 \
  ./qa/a2a/run.sh gateway 0.3.26
ALEPH_SERVER_BIN=/absolute/path/to/aleph-server KEEP=1 \
  ./qa/a2a/run.sh gateway 1.0.3
```

The Gateway runner generates configuration beneath a temporary `HOME` and
`ALEPH_HOME`, explicitly enables the local A2A bypass for that scratch server,
uses `qa/busy_input/mock_anthropic.py` as its only LLM provider, and asks the
SDK to use default `/.well-known/agent-card.json` discovery. No user vault,
provider credential, or real user service is used. The provider's `long-run`
plan makes cancellation observable while the SDK consumes the event stream.
`KEEP=1` retains the generated config and logs under `$QA_ROOT`.

## Current evidence boundary

At the time this harness was prepared, the approved standard JSON-RPC/SSE
route and dual-format default card are not mounted in this branch yet. The
`check` mode therefore establishes exact pinned dependency versions and public
SDK API shapes only; it does not claim route interoperability. The `gateway`
mode is prepared for post-integration use and requires a binary built from the
integrated branch. Do not treat the legacy `/a2a` route or a handcrafted local
fixture as proof of standard route behavior.

The route run expects these main-branch integration properties from the
approved A2A plan:

- unauthenticated loopback is explicitly enabled only in the isolated QA config;
- `/.well-known/agent-card.json` is served without nonstandard discovery headers
  and advertises both 0.3 and 1.0 JSON-RPC bindings;
- those bindings select `/a2a/rpc`, which accepts unary and SSE operations;
- an admitted long-running task can be observed from SSE, canceled through the
  SDK, and confirmed as canceled by GetTask;
- the route is mounted before Gateway readiness and remains opt-in by default.

The task's outbound custom-endpoint/`ra2a` comparison and bilingual production
reference are outside this isolated QA harness and remain for the main A2A
integration task.
