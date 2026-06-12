# Multi-Channel (一核多端) Guide

## Concept

One Aleph core serves many "ends" at once. Each end is pure I/O — it
forwards user input to the core as JSON-RPC and renders the response. All
reasoning happens in the core (R6 一核多端 / R4 I/O-only interfaces).

Ends:
- **Chat channels**: Telegram, Discord, Slack, WhatsApp, iMessage, email, …
- **Panel (WebChat)**: the browser / desktop App dashboard
- **CLI**: the `aleph` terminal client
- **Desktop notifications**: proactive push (R5 AI comes to you)

## Service Connection (本地服务 vs 远程服务)

The Panel and desktop App connect to one core ("服务"):
- **Local service (本地服务)**: the core running on this machine.
- **Remote service (远程服务)**: a core on another host, e.g.
  `https://core.example:18790`.

Switch it in the desktop App: Settings → 服务与集群 → 服务连接. Switching
reloads the Panel against the chosen core. (Browser-only Panels are read-only
here; the switch needs the desktop shell.)

## Configuring ends

### Chat channels
See the `channels` guide: `read_config_guide(topic="channels")`. Each channel
is a `[channels.<name>]` section in `~/.aleph/config.toml`, with secrets in the
vault (`channel:<instance_id>:<field>`). Channel changes need a restart.

### Reaching the core from a browser or another device
There is no authentication step (LAN-trust): the trust boundary is the
network boundary.
- Same machine: open the desktop App "Open in Browser", or visit
  `http://127.0.0.1:18790` directly.
- Another device on your LAN: set `[gateway] host = "0.0.0.0"` in
  `~/.aleph/config.toml` so the core listens on the network, then point a
  browser or the Aleph Panel thin-shell app at the core's IP. **Every
  device on that LAN gets full control** — only do this on a trusted
  network. To reach the core over the internet, front it with your own
  reverse proxy / VPN.

## Caveats

- Ends are stateless I/O — never put business logic, memory, or routing in a
  channel/Panel (R4).
- Secrets always go through the vault, never plaintext in config.
- Each chat channel needs a server restart to connect.
- To extend *execution* to other machines (not I/O), see the `cluster`
  guide — that is a different concept.
