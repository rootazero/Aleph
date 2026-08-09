pub mod cron;
pub mod heartbeat;
pub mod shared;

// `presence` and `mic_level` — two background reporters that published
// `host.presence.update` / `host.mic_level.update` on the Gateway event bus —
// lived here until 2026-08-09. Neither topic ever had a subscriber: no
// renderer, no RPC, no channel, nothing in `interfaces/`. The capability is
// not lost — `system_tool`'s `user_idle_time` action already answers "is a
// human at this keyboard" on demand, which is the surface R8 endorses, and
// the cluster tracks node liveness with its own ping / idle-watchdog. A timed
// broadcast of the same fact is the harness doing on a cadence what the model
// can ask for in one call (R7), and it broadcast the host's `hostname` and OS
// `username` to do it. Removed under R10's YAGNI-retraction clause rather than
// given a consumer.
