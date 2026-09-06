//! Attaching to targets, flat.
//!
//! `flatten: true` is not a preference. Without it the peer wraps every session message in
//! `Target.sendMessageToTarget` / `Target.receivedMessageFromTarget` envelopes, which this crate's
//! read loop does not unwrap — so a non-flat attach would leave every subsequent call timing out
//! with no error anywhere to explain it. Both engines were driven this way in the evidence probes
//! (`…-evidence/probes/m3456.mjs:9`, `m11-geom.mjs:14`).

use serde_json::{json, Value};

use crate::connection::CdpConnection;
use crate::error::{CdpError, Result};
use crate::ids::{SessionId, TargetId};

impl CdpConnection {
    /// Attach to `target` and return the flat session id every later call for that tab carries.
    pub async fn attach(&self, target: &TargetId) -> Result<SessionId> {
        let reply = self
            .call(
                None,
                "Target.attachToTarget",
                json!({ "targetId": target.as_str(), "flatten": true }),
            )
            .await?;
        // A reply with no sessionId — or an empty one — is "we do not know which session", never
        // an empty session: an empty session id would address the browser, so every later call
        // would quietly act on the wrong thing instead of failing (判据 §8). The `filter` rejects
        // `Some("")` the same way `and_then` already rejects a missing key, so both collapse into
        // the one error below rather than one of them silently becoming `Ok(SessionId(""))`.
        let id = reply
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                CdpError::Decode(format!(
                    "Target.attachToTarget({target}): reply carried no sessionId: {reply}"
                ))
            })?;
        Ok(SessionId(id.to_string()))
    }

    /// Detach `session`. The session is a parameter of a browser-level call, not the address of
    /// the request — sending it *inside* the session it is closing is a race the peer answers
    /// inconsistently.
    pub async fn detach(&self, session: &SessionId) -> Result<()> {
        self.call(
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": session.as_str() }),
        )
        .await?;
        Ok(())
    }
}
