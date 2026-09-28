#!/usr/bin/env bash
#
# Run the shared crates' own tests: shared-ui-logic (the transcript reducer and
# its guards), aleph-protocol, aleph-tui and aleph-cli.
#
# Why this script exists: `cargo test -p alephcore` never compiles a
# dependency's `#[cfg(test)]` modules, so these tests run in no other leg.
# shared-ui-logic runs in both feature shapes because the TUI and the CLI build
# it with default features off and the Panel builds it with them on.
#
# The one source for these commands: `just test-shared` and CI's "Run shared
# crate tests" step both call this file.
set -euo pipefail

cargo test -p shared-ui-logic --no-default-features
cargo test -p shared-ui-logic
cargo test -p aleph-protocol -p aleph-tui -p aleph-cli
