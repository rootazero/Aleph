use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::RwLock;

use crate::gateway::channel::{
    ChannelError, ChannelResult, ConversationId, ConversationKind, ConversationPage,
    ConversationRef,
};
use crate::sync_primitives::Arc;

const SLACK_API: &str = "https://slack.com/api";

struct CacheEntry {
    name: String,
    expires_at: tokio::time::Instant,
}

pub struct UserDirectory {
    client: reqwest::Client,
    bot_token: String,
    cache: RwLock<HashMap<String, CacheEntry>>,
    ttl: Duration,
}

impl UserDirectory {
    #[must_use]
    pub fn new(bot_token: String, ttl_secs: u64) -> Self {
        Self {
            client: reqwest::Client::new(),
            bot_token,
            cache: RwLock::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    pub async fn resolve(&self, user_id: &str) -> Option<String> {
        {
            let cache = self.cache.read().await;
            if let Some(entry) = cache.get(user_id) {
                if entry.expires_at > tokio::time::Instant::now() {
                    return Some(entry.name.clone());
                }
            }
        }

        let name = self.fetch_user_name(user_id).await?;

        {
            let mut cache = self.cache.write().await;
            cache.insert(
                user_id.to_string(),
                CacheEntry {
                    name: name.clone(),
                    expires_at: tokio::time::Instant::now() + self.ttl,
                },
            );
        }

        Some(name)
    }

    async fn fetch_user_name(&self, user_id: &str) -> Option<String> {
        let resp: serde_json::Value = self
            .client
            .get(format!("{SLACK_API}/users.info"))
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .query(&[("user", user_id)])
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;

        if resp["ok"].as_bool() != Some(true) {
            tracing::debug!(
                "Slack users.info failed: {}",
                resp["error"].as_str().unwrap_or("unknown")
            );
            return None;
        }

        resp["user"]["profile"]["display_name"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                resp["user"]["profile"]["real_name"]
                    .as_str()
                    .map(String::from)
            })
    }

    pub async fn clear_cache(&self) {
        self.cache.write().await.clear();
    }

    pub async fn cache_size(&self) -> usize {
        self.cache.read().await.len()
    }
}

/// How long a fetched roster stays usable. Rosters drift slowly; the cost of a
/// stale entry is one failed send with an actionable error, while the cost of
/// not caching is a Tier-2 rate-limited sweep on every lookup.
const ROSTER_TTL: Duration = Duration::from_secs(900);

/// Page caps. `conversations.list` accepts up to 1000 per page and the cold
/// sweep is on the model's critical path, so take the big pages; `users.list`
/// is Tier-2 and Slack's own guidance is ≤200.
const CHANNEL_PAGE_LIMIT: u32 = 999;
const USER_PAGE_LIMIT: u32 = 200;

/// Hard stop on pagination. A workspace with more than this many pages is one
/// where "type more letters" is the right answer — and, more importantly, this
/// call is made while `ChannelRegistry` holds the channel's read guard, so an
/// unbounded sweep would block `stop_channel` / `restart_channel`.
const MAX_PAGES: usize = 25;

struct RosterCache {
    /// `Arc` so a lookup shares the roster instead of copying every row out of
    /// the lock — a large workspace is thousands of entries per call.
    rows: Arc<Vec<ConversationRef>>,
    expires_at: tokio::time::Instant,
}

/// One sweep's outcome. A sweep that fails must not take the other one down
/// with it: an app granted `channels:read` but not `users:read` should still be
/// able to find #eng-releases, and should SAY that it could not look at people.
struct Roster {
    rows: Arc<Vec<ConversationRef>>,
    warnings: Vec<String>,
}

/// Workspace roster: `name → conversation id`, for channels and for people.
///
/// The inverse of [`UserDirectory`], which resolves an id the inbound path
/// already has into a display name. This resolves a name a *human said* into an
/// id the outbound path can use — without it, the agent can only ever reply
/// where it was spoken to.
///
/// Both sweeps are pure reads on scopes a Slack app already needs to function
/// (`channels:read`, `groups:read`, `users:read`), and the roster never leaves
/// the process: it is an in-memory cache, not persisted state. Names and ids
/// are routing metadata, not message content.
pub struct ConversationDirectory {
    client: reqwest::Client,
    bot_token: String,
    api_base: Option<String>,
    channels: RwLock<Option<RosterCache>>,
    users: RwLock<Option<RosterCache>>,
}

impl ConversationDirectory {
    #[must_use]
    pub fn new(bot_token: String, api_base: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            bot_token,
            api_base,
            channels: RwLock::new(None),
            users: RwLock::new(None),
        }
    }

    fn base(&self) -> &str {
        self.api_base.as_deref().unwrap_or(SLACK_API)
    }

    /// Matching conversations, best match first, capped at `limit`.
    ///
    /// An empty `query` returns the head of the roster rather than nothing —
    /// "what channels are there?" is a legitimate question.
    ///
    /// Fails only when BOTH sweeps fail. One sweep failing degrades the answer
    /// and says so in `warnings`; it never silently narrows the search.
    pub async fn list(&self, query: &str, limit: usize) -> ChannelResult<ConversationPage> {
        let needle = query.trim().trim_start_matches(['#', '@']).to_lowercase();

        let channels = self.channel_rows().await;
        let users = self.user_rows().await;

        let (channels, users) = match (channels, users) {
            // Both down = there is no answer to give; do not report an empty
            // workspace.
            (Err(e), Err(_)) => return Err(e),
            // One down = a partial answer plus the reason, never a quietly
            // narrowed search.
            pair => pair,
        };

        let mut warnings = Vec::new();
        let mut rows: Vec<Arc<Vec<ConversationRef>>> = Vec::with_capacity(2);
        for (what, roster) in [("channels", channels), ("people", users)] {
            match roster {
                Ok(r) => {
                    rows.push(r.rows);
                    warnings.extend(r.warnings);
                }
                Err(e) => warnings.push(format!("could not list {what}: {e}")),
            }
        }

        let mut scored: Vec<(u8, &ConversationRef)> = rows
            .iter()
            .flat_map(|r| r.iter())
            .filter_map(|r| rank(&r.name, &needle).map(|s| (s, r)))
            .collect();
        // Best rank first, then alphabetical so the order is stable across
        // calls — an unstable list makes the model re-ask.
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));

        Ok(ConversationPage {
            conversations: scored
                .into_iter()
                .take(limit)
                .map(|(_, r)| r.clone())
                .collect(),
            warnings,
        })
    }

    async fn channel_rows(&self) -> ChannelResult<Roster> {
        if let Some(rows) = cached(&self.channels).await {
            return Ok(Roster {
                rows,
                warnings: Vec::new(),
            });
        }
        let (raw, warnings) = self
            .sweep(
                "conversations.list",
                &[
                    ("types", "public_channel,private_channel".to_string()),
                    ("exclude_archived", "true".to_string()),
                    ("limit", CHANNEL_PAGE_LIMIT.to_string()),
                ],
                "channels",
            )
            .await?;

        let rows: Vec<ConversationRef> = raw
            .iter()
            .filter_map(|c| {
                let id = c.get("id")?.as_str()?;
                let name = c.get("name")?.as_str()?;
                Some(ConversationRef {
                    id: ConversationId::new(id),
                    name: name.to_string(),
                    kind: if c.get("is_private").and_then(serde_json::Value::as_bool) == Some(true) {
                        ConversationKind::Group
                    } else {
                        ConversationKind::Channel
                    },
                    // Private channels are only visible at all when the bot is
                    // a member, so an absent flag there means yes.
                    is_member: c
                        .get("is_member")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or_else(|| {
                            c.get("is_private").and_then(serde_json::Value::as_bool) == Some(true)
                        }),
                })
            })
            .collect();

        let rows = store(&self.channels, rows).await;
        Ok(Roster { rows, warnings })
    }

    async fn user_rows(&self) -> ChannelResult<Roster> {
        if let Some(rows) = cached(&self.users).await {
            return Ok(Roster {
                rows,
                warnings: Vec::new(),
            });
        }
        let (raw, warnings) = self
            .sweep(
                "users.list",
                &[("limit", USER_PAGE_LIMIT.to_string())],
                "members",
            )
            .await?;

        let rows: Vec<ConversationRef> = raw
            .iter()
            .filter(|u| {
                // Bots and the workspace's own `USLACKBOT` are addressable but
                // never what "send this to <name>" means; deleted accounts are
                // not addressable at all.
                u.get("deleted").and_then(serde_json::Value::as_bool) != Some(true)
                    && u.get("is_bot").and_then(serde_json::Value::as_bool) != Some(true)
                    && u.get("id").and_then(serde_json::Value::as_str) != Some("USLACKBOT")
            })
            .filter_map(|u| {
                let id = u.get("id")?.as_str()?;
                let profile = u.get("profile");
                let name = profile
                    .and_then(|p| p.get("display_name"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        profile
                            .and_then(|p| p.get("real_name"))
                            .and_then(serde_json::Value::as_str)
                    })
                    .or_else(|| u.get("name").and_then(serde_json::Value::as_str))?;
                Some(ConversationRef {
                    id: ConversationId::new(id),
                    name: name.to_string(),
                    kind: ConversationKind::Direct,
                    // `chat.postMessage` accepts a user id as `channel` and
                    // opens the DM, so a listed human is always postable.
                    is_member: true,
                })
            })
            .collect();

        let rows = store(&self.users, rows).await;
        Ok(Roster { rows, warnings })
    }

    /// Cursor-paginated GET, returning the concatenated `key` arrays plus any
    /// warning about the result being incomplete.
    async fn sweep(
        &self,
        method: &str,
        params: &[(&str, String)],
        key: &str,
    ) -> ChannelResult<(Vec<serde_json::Value>, Vec<String>)> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0usize;

        for _ in 0..MAX_PAGES {
            pages += 1;
            let mut query: Vec<(&str, String)> = params.to_vec();
            if let Some(c) = &cursor {
                query.push(("cursor", c.clone()));
            }

            let resp: serde_json::Value = self
                .client
                .get(format!("{}/{method}", self.base()))
                .header("Authorization", format!("Bearer {}", self.bot_token))
                .query(&query)
                .send()
                .await
                .map_err(|e| ChannelError::ReceiveFailed(format!("{method} request failed: {e}")))?
                .json()
                .await
                .map_err(|e| {
                    ChannelError::ReceiveFailed(format!("{method} response parse failed: {e}"))
                })?;

            if resp["ok"].as_bool() != Some(true) {
                let err = resp["error"].as_str().unwrap_or("unknown error");
                // Slack's error strings are the actionable part — a
                // `missing_scope` here is a workspace-admin fix, not a retry.
                return Err(match err {
                    "ratelimited" => ChannelError::RateLimited {
                        retry_after_secs: 30,
                    },
                    "invalid_auth" | "not_authed" | "account_inactive" => {
                        ChannelError::AuthFailed(format!("Slack {method}: {err}"))
                    }
                    _ => ChannelError::ReceiveFailed(format!("Slack {method} failed: {err}")),
                });
            }

            if let Some(items) = resp.get(key).and_then(serde_json::Value::as_array) {
                out.extend(items.iter().cloned());
            }

            cursor = resp["response_metadata"]["next_cursor"]
                .as_str()
                .filter(|c| !c.is_empty())
                .map(String::from);
            if cursor.is_none() {
                break;
            }
        }

        // Stopping at the cap is a real truncation. Saying nothing here would
        // make "not in the roster" and "we stopped looking" indistinguishable.
        let warnings = if cursor.is_some() && pages >= MAX_PAGES {
            vec![format!(
                "{method} stopped after {MAX_PAGES} pages; the workspace is larger than one \
                 sweep — narrow the query"
            )]
        } else {
            Vec::new()
        };
        Ok((out, warnings))
    }

    /// Drop both rosters — for a reconnect, or an explicit refresh after the
    /// user creates a channel mid-session.
    pub async fn invalidate(&self) {
        *self.channels.write().await = None;
        *self.users.write().await = None;
    }
}

async fn cached(slot: &RwLock<Option<RosterCache>>) -> Option<Arc<Vec<ConversationRef>>> {
    let guard = slot.read().await;
    let entry = guard.as_ref()?;
    (entry.expires_at > tokio::time::Instant::now()).then(|| Arc::clone(&entry.rows))
}

async fn store(
    slot: &RwLock<Option<RosterCache>>,
    rows: Vec<ConversationRef>,
) -> Arc<Vec<ConversationRef>> {
    let rows = Arc::new(rows);
    *slot.write().await = Some(RosterCache {
        rows: Arc::clone(&rows),
        expires_at: tokio::time::Instant::now() + ROSTER_TTL,
    });
    rows
}

/// Rank of `name` against a lowercased `needle`; `None` = no match.
///
/// Lower is better: exact, then prefix, then substring. This is identifier
/// lookup — matching what the user literally typed against a roster of names —
/// not intent classification, so a deterministic comparison is the right tool
/// (P8 forbids regex over natural language, not string equality over ids).
fn rank(name: &str, needle: &str) -> Option<u8> {
    if needle.is_empty() {
        return Some(3);
    }
    let lower = name.to_lowercase();
    if lower == needle {
        Some(0)
    } else if lower.starts_with(needle) {
        Some(1)
    } else if lower.contains(needle) {
        Some(2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_defaults_empty() {
        let dir = UserDirectory::new("xoxb-test".to_string(), 3600);
        assert_eq!(dir.cache_size().await, 0);
    }

    #[test]
    fn rank_prefers_exact_then_prefix_then_substring() {
        assert_eq!(rank("eng-releases", "eng-releases"), Some(0));
        assert_eq!(rank("eng-releases", "eng"), Some(1));
        assert_eq!(rank("team-eng", "eng"), Some(2));
        assert_eq!(rank("design", "eng"), None);
    }

    #[test]
    fn rank_is_case_insensitive_and_empty_query_matches_all() {
        assert_eq!(rank("Eng-Releases", "eng"), Some(1));
        assert_eq!(rank("anything", ""), Some(3));
    }

    /// `#eng` and `eng` are the same request — the sigil is how a human writes
    /// it, not part of the name Slack stores.
    #[tokio::test]
    async fn sigils_are_stripped_before_matching() {
        for raw in ["#eng-releases", "@alice", "  #eng-releases  "] {
            let needle = raw.trim().trim_start_matches(['#', '@']).to_lowercase();
            assert!(!needle.starts_with('#') && !needle.starts_with('@'), "{raw}");
        }
    }

    // ---- HTTP-level behaviour ------------------------------------------

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn ok_channels(channels: serde_json::Value, next_cursor: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "channels": channels,
            "response_metadata": { "next_cursor": next_cursor },
        }))
    }

    async fn dir_for(server: &MockServer) -> ConversationDirectory {
        ConversationDirectory::new("xoxb-test".to_string(), Some(server.uri()))
    }

    /// The whole point of `is_member`: a channel the bot can see but not post
    /// to must come back LISTED, not filtered away. Filtering it would answer
    /// "no such channel" for a channel that plainly exists.
    #[tokio::test]
    async fn lists_channels_and_keeps_non_member_rows() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ok_channels(
                serde_json::json!([
                    {"id": "C1", "name": "eng-releases", "is_private": false, "is_member": true},
                    {"id": "C2", "name": "eng-secret", "is_private": false, "is_member": false},
                ]),
                "",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "members": [], "response_metadata": {"next_cursor": ""}
            })))
            .mount(&server)
            .await;

        let page = dir_for(&server).await.list("eng", 10).await.unwrap();
        let names: Vec<&str> = page
            .conversations
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["eng-releases", "eng-secret"]);
        assert!(page.conversations[0].is_member);
        assert!(!page.conversations[1].is_member);
        assert!(page.warnings.is_empty(), "{:?}", page.warnings);
    }

    /// An app with `channels:read` but not `users:read` must still find
    /// channels — and must SAY it could not look at people. Returning the
    /// channels silently would make "alice is not here" indistinguishable from
    /// "we were never allowed to look".
    #[tokio::test]
    async fn one_failing_sweep_degrades_with_a_warning_instead_of_failing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ok_channels(
                serde_json::json!([
                    {"id": "C1", "name": "eng-releases", "is_private": false, "is_member": true},
                ]),
                "",
            ))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": false, "error": "missing_scope"
            })))
            .mount(&server)
            .await;

        let page = dir_for(&server).await.list("", 10).await.unwrap();
        assert_eq!(page.conversations.len(), 1);
        assert_eq!(page.warnings.len(), 1);
        assert!(page.warnings[0].contains("people"), "{:?}", page.warnings);
        assert!(
            page.warnings[0].contains("missing_scope"),
            "the actionable part is Slack's own error string: {:?}",
            page.warnings
        );
    }

    /// Both sweeps down = no answer exists; do not pretend the workspace is
    /// empty.
    #[tokio::test]
    async fn both_sweeps_failing_is_an_error() {
        let server = MockServer::start().await;
        for p in ["/conversations.list", "/users.list"] {
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": false, "error": "invalid_auth"
                })))
                .mount(&server)
                .await;
        }

        let err = dir_for(&server).await.list("", 10).await.unwrap_err();
        assert!(
            matches!(err, ChannelError::AuthFailed(_)),
            "invalid_auth must map to AuthFailed, got {err:?}"
        );
    }

    /// A second lookup must not re-sweep: the roster is cached for TTL, which
    /// is what keeps a Tier-2 rate-limited endpoint off the model's hot path.
    #[tokio::test]
    async fn a_second_lookup_is_served_from_cache() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ok_channels(
                serde_json::json!([{"id": "C1", "name": "eng", "is_private": false, "is_member": true}]),
                "",
            ))
            .expect(1) // exactly one sweep for two lookups
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "members": [], "response_metadata": {"next_cursor": ""}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let dir = dir_for(&server).await;
        assert_eq!(dir.list("eng", 10).await.unwrap().conversations.len(), 1);
        assert_eq!(dir.list("eng", 10).await.unwrap().conversations.len(), 1);
        // `expect(1)` is asserted on drop.
    }

    /// Bots and deleted accounts are not what "DM this to <name>" means.
    #[tokio::test]
    async fn user_rows_drop_bots_and_deleted_accounts() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/conversations.list"))
            .respond_with(ok_channels(serde_json::json!([]), ""))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users.list"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "members": [
                    {"id": "U1", "name": "alice", "profile": {"display_name": "Alice"}},
                    {"id": "U2", "name": "buildbot", "is_bot": true, "profile": {}},
                    {"id": "U3", "name": "gone", "deleted": true, "profile": {}},
                    {"id": "USLACKBOT", "name": "slackbot", "profile": {}},
                ],
                "response_metadata": {"next_cursor": ""}
            })))
            .mount(&server)
            .await;

        let page = dir_for(&server).await.list("", 10).await.unwrap();
        let names: Vec<&str> = page
            .conversations
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["Alice"]);
        assert_eq!(page.conversations[0].kind, ConversationKind::Direct);
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let dir = UserDirectory::new("xoxb-test".to_string(), 3600);
        dir.clear_cache().await;
        assert_eq!(dir.cache_size().await, 0);
    }
}
