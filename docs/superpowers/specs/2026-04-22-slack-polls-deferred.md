# Slack polls — deferred (no first-class Web API)

**Status:** Deferred, not abandoned.

**Date:** 2026-04-22

## TL;DR

Slack Web API has **no** first-class method for creating polls as of
2026-04-22. `Channel::create_poll` on `SlackChannel` correctly stays
pinned to `UnsupportedFeature`, and `capabilities().polls` correctly
stays `false`. The next person investigating this should NOT redo the
survey — this document captures what we found and what to look for.

## What we checked

- The Slack Web API method reference at <https://docs.slack.dev/reference/methods.md>
  was searched for `polls.create`, `polls.open`, and the `polls.*`
  family. **No results.** The closest neighbours are `conversations.*`,
  `chat.*`, and `views.*`.
- Slack polls as a UI feature exist (the `/poll` slash command in the
  Slack client), but Slack does not expose a documented Web API method
  for a Slack app to create one on behalf of a user.

## Workarounds that exist but were rejected

| Workaround | Why rejected |
|---|---|
| `chat.command` with `/poll` (unofficial) | Slack does not document `chat.command`; the family it documents is `chat.postMessage` / `chat.update` / `chat.delete` / `chat.postEphemeral` / `chat.meMessage` / `chat.scheduleMessage`. The `/poll` route is brittle to rename and may violate Slack TOS. |
| Third-party Simple Poll app | Requires a separate OAuth install, a third-party API key, and would couple Aleph's poll UX to a vendor. Out of scope for the channel trait. |
| Webhook-triggered Block Kit + emoji-tally UI | Not a poll. Defining a new UX surface is the product team's call, not the channel team's. |

## What to look for when this becomes feasible

If/when Slack ships a public `polls.open` (or similarly-named) method,
the implementation pattern mirrors `chat.postMessage`:

1. Add `pub async fn create_poll(...)` to
   `src/gateway/interfaces/slack/message_ops/api.rs` (the Slack-side
   equivalent of `send_message`).
2. Override `Channel::create_poll` on `SlackChannel` to call it.
3. Flip `capabilities().polls` to `true` and update the cap-test.

Expected LoC: ~80–120. No new dependencies.

## Why this doc lives next to the code

The CLAUDE.md R8 rule ("don't advertise a capability that returns
UnsupportedFeature when called") forces a future investigator to ask
"why is Slack `polls: false`?" before adding the impl. Without this
doc, the natural assumption is "Slack forgot" — and the next person
will spend an hour rediscovering the same answer. This file moves
that answer out of in-conversation memory.