# A2A official Python SDK inbound QA

The runner pins two independent environments to the official `a2a-sdk`
distributions.

```sh
./qa/a2a/run.sh check 0.3.26
./qa/a2a/run.sh check 1.0.3
```

`check` installs each SDK in a separate `uv` environment and checks the public
client, discovery, message, streaming configuration, GetTask, and CancelTask
APIs. It performs no Rust build and needs no Aleph credentials.

Pass an already-built `aleph-server` binary to run against the Gateway's
production A2A mount. The runner uses ports 18796 and 18996 by default.
`GATEWAY_PORT` and `MOCK_PORT` override them.

```sh
ALEPH_SERVER_BIN=/absolute/path/to/aleph-server KEEP=1 \
  ./qa/a2a/run.sh gateway 0.3.26
ALEPH_SERVER_BIN=/absolute/path/to/aleph-server KEEP=1 \
  ./qa/a2a/run.sh gateway 1.0.3
```

The runner generates configuration under a temporary `HOME` and `ALEPH_HOME`,
enables the loopback A2A scope for the test server, and uses the controlled
Anthropic fixture as its LLM provider. Each SDK uses ordinary discovery at
`/.well-known/agent-card.json` and sends on the advertised `/a2a/rpc` binding.
The probe reads the first task from SSE, closes the stream, waits for the mock
provider to see the Core request, cancels via the SDK, and confirms the retained
state with GetTask. A separate request makes the model call `ask_user` and
checks that the task pauses with the question, rejects a different credential,
and completes after a follow-up from the original credential. It also cancels
another paused task using the original credential. `KEEP=1` retains the config
and logs under `$QA_ROOT`.

See [Agent-to-Agent](../../docs/reference/A2A.md) for the supported protocol and
configuration.
