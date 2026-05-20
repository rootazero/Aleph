# Markdown Skill Authoring Guide

> Quick reference for writing SKILL.md files that Aleph can load as runtime CLI tools.

## SKILL.md frontmatter

A Markdown CLI skill lives in a directory under `~/.aleph/skills/<skill-name>/` and must contain a `SKILL.md` with YAML frontmatter followed by the Markdown body that gets injected as LLM context.

```yaml
---
name: gh-pr-create
description: Open a GitHub pull request from the current branch
metadata:
  requires:
    bins: ["gh"]
  aleph:
    security:
      sandbox: host          # host | docker | virtualfs
      confirmation: write    # always | write | never
      network: internet      # internet | local | none
    input_hints:
      title:
        type: string
        optional: false
---

# gh-pr-create

Use `gh pr create` to open a pull request …
```

## Sandbox modes

| Mode | Isolation | When to use | Limitations |
|------|-----------|-------------|-------------|
| `host` | **None** (runs with your full user privileges) | Trusted tools you would run manually anyway (`gh`, `jq`, `git`, etc.) | `network: none` is best-effort only — see below |
| `docker` | Real cross-platform isolation via a container | Untrusted skills, network-restricted workflows, or anything you would not run directly on your laptop | Requires Docker installed and running |
| `virtualfs` | Lightweight filesystem-only isolation (no netns) | Skills that should be unable to write outside a temp directory | No network isolation |

## The host-mode contract

`sandbox: host` runs the skill with your full user privileges. The skill author is choosing to trust the host environment.

`network: none` under `sandbox: host` sets `NO_PROXY=*` and `no_proxy=*` on the executed process **as a partial mitigation**. This stops well-behaved HTTP libraries from honoring an outbound proxy, but **cannot stop the binary from opening sockets directly**. For enforced network isolation, use one of:

- **`sandbox: docker`** — cross-platform real isolation via `--network=none`. Recommended default for any workflow where network restriction matters.
- **`sandbox: bwrap`** *(planned, Linux-only)* — would route through Aleph's existing bubblewrap driver to get real netns isolation without Docker overhead. Tracked as a deferred follow-up; see `docs/superpowers/specs/2026-05-20-host-sandbox-netns-decision-design.md` §6.

This contract is deliberately honest rather than ambitious: a `warn!` fires at execution time when host+network=none is declared, instead of pretending isolation is enforced. The design rationale and the explicit rejection of `unshare(CLONE_NEWNET)` as a host-mode addition is documented in the spec above.

## Confirmation modes

| Mode | Behavior |
|------|----------|
| `always` | Every execution prompts the user via the active channel |
| `write` *(default)* | Prompt only on side-effecting invocations (the executor decides based on the command's first arg) |
| `never` | No prompt — used for read-only / status / list operations |

## Required binaries

`metadata.requires.bins` lists the CLI binaries the skill depends on. Under `sandbox: host`, the loader checks `PATH` at load time and emits a `warn!` if any required binary is missing (`Install it or switch to 'sandbox: docker' mode.`). Under `sandbox: docker`, the binary is expected to be inside the container image, so the check is skipped.

## See also

- `docs/superpowers/specs/2026-05-20-host-sandbox-netns-decision-design.md` — the binding decision for host-mode network isolation
- `docs/reference/SKILL_MODEL_TAXONOMY.md` — the four-layer skill type map
- `docs/reference/SKILL_SANDBOXING.md` — OS-native sandboxing for evolved skills
- `docs/reference/SANDBOX.md` — the underlying `Sandbox` trait and platform drivers (bwrap / seatbelt / Windows AppContainer)
