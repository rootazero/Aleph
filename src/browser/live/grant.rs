//! Server-side store for r4 browser-viewer grants.
//!
//! One grant binds exactly one [`ViewerBinding`] + resource id + permission
//! set + expiry + viewer origin. Every secret in the flow (fragment nonce,
//! exchange ticket, session secret) is an independent 256-bit CSPRNG value
//! that exists in plaintext exactly once (in the response that issues it),
//! is stored only as a SHA-256 hash, and is compared in constant time.
//!
//! All state transitions happen under one mutex, so the r4 single-consume
//! and rotation-CAS semantics are structural, not best-effort:
//!
//! * nonce `Unused -> Consumed` mints a short-lived ticket in the same
//!   critical section;
//! * ticket exchange atomically consumes the ticket and creates the session
//!   (`rotation_generation` 0, `ws_generation` 0, secret `S0`);
//! * `begin_auth` / `commit_auth` implement the two-phase rotation: the
//!   commit CAS moves `current` into `consumed`, mints `next`, bumps
//!   `generation` and `ws_generation`, and marks every still-registered
//!   attempt on the old secret as a race loser;
//! * presenting a *retained consumed* secret is replay and atomically
//!   revokes the whole grant, closing its live WS via [`ViewerLease`];
//! * abort/timeout drops the attempt registration and never exempts a
//!   future AuthHello.
//!
//! Generic denial: unknown, expired, wrong-origin, and wrong-secret are the
//! same unit error / `Unauthorized`, so an attacker learns nothing about
//! which grants exist.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use aleph_protocol::browser_live::{
    AuthErrorCode, AuthHello, ExchangeTicket, FragmentNonce, InterventionEnd, IssuerIdentity,
    ViewerBinding, ViewerGrantInfo, ViewerOrigin, ViewerPermission, ViewerSecret,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use rand::Rng;
use sha2::{Digest, Sha256};

use super::control::{TargetControl, ViewerLease};
use crate::security::secret_equal_bytes;

/// Raw entropy per secret. 32 bytes -> 43 base64url chars, matching
/// `AuthHello::validate_shape` on the wire.
const SECRET_BYTES: usize = 32;
/// Ticket lifetime after nonce consumption. Short: it only has to survive
/// one page load -> WS exchange hop.
const TICKET_TTL: Duration = Duration::from_secs(60);
/// How long a consumed secret stays recognizable for replay detection
/// (bounded additionally by the grant's own expiry).
const CONSUMED_RETENTION: Duration = Duration::from_secs(600);
/// Bound on retained consumed secrets per session.
const MAX_CONSUMED_SECRETS: usize = 64;
/// Max concurrent in-flight auth attempts per session-generation.
const MAX_INFLIGHT_ATTEMPTS: usize = 8;
/// Hard bound on live grants; on overflow the nearest-expiry grant is
/// evicted (its live viewer is revoked). Pre-auth abuse can therefore
/// DoS a *viewer grant*, never the gateway itself.
#[allow(dead_code)] // enforced by `issue`, wired by the Task 6 issuer RPC surface
const MAX_LIVE_GRANTS: usize = 4096;

#[allow(dead_code)] // consumed by `issue`, wired by the Task 6 issuer RPC surface
fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

const fn os_rng() -> UnwrapErr<SysRng> {
    UnwrapErr(SysRng)
}

pub(super) fn mint_secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    os_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_secret(secret: &str) -> [u8; 32] {
    Sha256::digest(secret.as_bytes()).into()
}

/// Strict origin match: the presented origin (already parsed by the
/// transport) must be present and equal on all three axes. Absent headers
/// are denied here; the fragment flow is browser-only and browsers always
/// send `Origin`.
fn origin_matches(bound: &ViewerOrigin, presented: Option<&ViewerOrigin>) -> bool {
    let Some(presented) = presented else {
        return false;
    };
    bound.scheme == presented.scheme && bound.host == presented.host && bound.port == presented.port
}

/// The only failure visible to credential callers. Carries no reason by
/// design; `Debug` deliberately prints nothing useful either.
pub(crate) struct GrantDenied;

impl fmt::Debug for GrantDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("grant denied")
    }
}

impl fmt::Display for GrantDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("grant denied")
    }
}

/// Result of a successful `issue`: the one-time plaintext nonce.
#[allow(dead_code)] // constructed by `issue`, wired by the Task 6 issuer RPC surface
pub(crate) struct IssuedGrant {
    pub grant_id: String,
    pub nonce: FragmentNonce,
    pub expires_at_ms: i64,
}

/// Result of a successful nonce consumption: the one-time plaintext ticket.
pub(crate) struct ConsumedNonce {
    pub ticket: ExchangeTicket,
    pub expires_at_ms: i64,
}

/// Result of a successful ticket exchange: the session's first secret `S0`
/// plus everything the transport needs to prepare the viewer session.
#[derive(Clone)]
#[allow(dead_code)] // `binding`/`resource_id`/`permissions`/`origin` are read by the Task 6 boot-wired backend
pub(crate) struct ExchangedGrant {
    pub grant_id: String,
    pub viewer_session_id: String,
    pub rotation_generation: u64,
    /// Plaintext `S0`. Emitted exactly once; the store keeps only the hash.
    pub current_secret: ViewerSecret,
    pub binding: ViewerBinding,
    pub resource_id: String,
    pub permissions: Vec<ViewerPermission>,
    pub origin: ViewerOrigin,
    pub expires_at_ms: i64,
}

/// Result of a successful rotation commit.
pub(crate) struct AuthCommit {
    /// `ws_generation` after the bump; equals the presented generation + 1
    /// while generations and ws generations advance together.
    pub ws_generation: u64,
    /// Plaintext `next_secret`. Emitted exactly once, inside `AuthOk`.
    pub next_secret: String,
    /// Fresh identity for the new live WS. `TargetControl::register_viewer`
    /// refuses a re-used viewer session id, so every committed rotation
    /// mints a new one; the client learns it from `State` messages.
    pub viewer_session_id: String,
    #[allow(dead_code)] // exposed for Task 6 surfaces; the route keys off `ws_generation`
    pub generation: u64,
}

struct ConsumedSecret {
    generation: u64,
    hash: [u8; 32],
    /// Attempts still registered against this secret when it rotated out:
    /// the race losers, answered `Conflict` when they reach their decision
    /// point.
    inflight: HashSet<u64>,
    consumed_at: Instant,
}

struct SessionState {
    generation: u64,
    current_hash: [u8; 32],
    inflight: HashSet<u64>,
    consumed: VecDeque<ConsumedSecret>,
    ws_generation: u64,
}

enum Stage {
    #[allow(dead_code)] // constructed by `issue`, wired by the Task 6 issuer RPC surface
    Nonce {
        hash: [u8; 32],
        consumed: bool,
    },
    Ticket {
        hash: [u8; 32],
        deadline: Instant,
    },
    Session(SessionState),
}

/// A live viewer WS bound to this grant. `publish_live` installs it after
/// the AuthOk write commits; `revoke` closes it via the lease.
struct LiveViewer {
    control: Arc<TargetControl>,
    lease: ViewerLease,
}

struct GrantEntry {
    #[allow(dead_code)] // read by the Task 6 issuer RPC surface (revoke authorization)
    issuer: IssuerIdentity,
    binding: ViewerBinding,
    resource_id: String,
    permissions: Vec<ViewerPermission>,
    origin: ViewerOrigin,
    expires_at_ms: i64,
    deadline: Instant,
    stage: Stage,
    live: Option<LiveViewer>,
}

impl GrantEntry {
    fn alive(&self) -> bool {
        Instant::now() < self.deadline
    }
}

type PrincipalValidator = Arc<dyn Fn(&IssuerIdentity) -> bool + Send + Sync>;

#[derive(Default)]
struct Inner {
    // Installed by gateway composition before exposing the issuer. The callback
    // must not re-enter this store; its status read and insertion share the
    // same lock as revoke_for_principal's sweep.
    principal_validator: Option<PrincipalValidator>,
    entries: HashMap<String, GrantEntry>,
    next_attempt: u64,
}

impl Inner {
    #[allow(dead_code)] // called by `issue`, wired by the Task 6 issuer RPC surface
    fn prune_expired(&mut self) {
        let now = Instant::now();
        self.entries.retain(|_, e| now < e.deadline);
    }
}

/// The grant store. Clone is cheap (one shared table); the store never
/// holds plaintext secrets.
#[derive(Clone, Default)]
pub(crate) struct ViewerGrantStore {
    inner: Arc<Mutex<Inner>>,
}

impl ViewerGrantStore {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Install the live authority used at the insertion linearization point.
    /// Low-level stores without gateway composition retain their caller-owned
    /// policy; the gateway always installs a fail-closed validator.
    pub(crate) fn set_principal_validator(&self, validator: PrincipalValidator) {
        self.lock().principal_validator = Some(validator);
    }

    /// Mint a grant over an exact binding. `permissions` are canonicalized
    /// (sorted, deduped); an empty effective set is denied. The wire type
    /// [`ViewerBinding`] is browser-only, so artifact-bound grants are
    /// excluded at the type level, not by a check that could be forgotten.
    #[allow(dead_code)] // consumed by the Task 6 issuer RPC surface
    pub(crate) fn issue(
        &self,
        issuer: IssuerIdentity,
        binding: ViewerBinding,
        resource_id: &str,
        permissions: &[ViewerPermission],
        origin: ViewerOrigin,
        ttl: Duration,
    ) -> Result<IssuedGrant, GrantDenied> {
        let ttl_ms = i64::try_from(ttl.as_millis()).map_err(|_| GrantDenied)?;
        let expires_at_ms = now_ms().checked_add(ttl_ms).ok_or(GrantDenied)?;
        self.issue_until(
            issuer,
            binding,
            resource_id,
            permissions,
            origin,
            expires_at_ms,
        )
    }

    /// Mint with an absolute resource/policy deadline. The monotonic anchor is
    /// captured before reading wall time so store work cannot extend that bound.
    pub(crate) fn issue_until(
        &self,
        issuer: IssuerIdentity,
        binding: ViewerBinding,
        resource_id: &str,
        permissions: &[ViewerPermission],
        origin: ViewerOrigin,
        expires_at_ms: i64,
    ) -> Result<IssuedGrant, GrantDenied> {
        let anchor = Instant::now();
        let remaining_ms = expires_at_ms.checked_sub(now_ms()).ok_or(GrantDenied)?;
        if resource_id.is_empty() || remaining_ms <= 0 {
            return Err(GrantDenied);
        }
        let deadline = anchor
            .checked_add(Duration::from_millis(remaining_ms as u64))
            .ok_or(GrantDenied)?;
        let mut permissions = permissions.to_vec();
        permissions.sort_unstable();
        permissions.dedup();
        if permissions.is_empty() {
            return Err(GrantDenied);
        }
        let grant_id = mint_secret();
        let nonce = mint_secret();
        let entry = GrantEntry {
            issuer,
            binding,
            resource_id: resource_id.to_string(),
            permissions,
            origin,
            expires_at_ms,
            deadline,
            stage: Stage::Nonce {
                hash: hash_secret(&nonce),
                consumed: false,
            },
            live: None,
        };
        let mut inner = self.lock();
        if let Some(validate) = &inner.principal_validator {
            if !validate(&entry.issuer) || !validate(&entry.binding.principal) {
                return Err(GrantDenied);
            }
        }
        inner.prune_expired();
        if inner.entries.len() >= MAX_LIVE_GRANTS {
            let Some(victim) = inner
                .entries
                .iter()
                .min_by_key(|(_, e)| e.deadline)
                .map(|(k, _)| k.clone())
            else {
                return Err(GrantDenied);
            };
            Self::revoke_locked(&mut inner, &victim);
        }
        inner.entries.insert(grant_id.clone(), entry);
        Ok(IssuedGrant {
            grant_id,
            nonce: FragmentNonce(nonce),
            expires_at_ms,
        })
    }

    /// `Unused -> Consumed` CAS on the nonce; mints the ticket atomically.
    /// A wrong or replayed nonce, an expired grant, or a wrong origin all
    /// return the same denial and change no state.
    pub(crate) fn consume_nonce(
        &self,
        grant_id: &str,
        nonce: &FragmentNonce,
        origin: Option<&ViewerOrigin>,
    ) -> Result<ConsumedNonce, GrantDenied> {
        let mut inner = self.lock();
        let entry = inner
            .entries
            .get_mut(grant_id)
            .filter(|e| e.alive())
            .ok_or(GrantDenied)?;
        if !origin_matches(&entry.origin, origin) {
            return Err(GrantDenied);
        }
        let ok = match &entry.stage {
            Stage::Nonce { hash, consumed } => {
                !consumed && secret_equal_bytes(hash, &hash_secret(&nonce.0))
            }
            _ => false,
        };
        if !ok {
            return Err(GrantDenied);
        }
        let ticket = mint_secret();
        entry.stage = Stage::Ticket {
            hash: hash_secret(&ticket),
            deadline: Instant::now() + TICKET_TTL,
        };
        Ok(ConsumedNonce {
            ticket: ExchangeTicket(ticket),
            expires_at_ms: entry.expires_at_ms,
        })
    }

    /// Atomically consume the ticket and create the session (`S0`).
    pub(crate) fn exchange(
        &self,
        ticket: &ExchangeTicket,
        origin: Option<&ViewerOrigin>,
    ) -> Result<ExchangedGrant, GrantDenied> {
        let mut inner = self.lock();
        let now = Instant::now();
        let ticket_hash = hash_secret(&ticket.0);
        let grant_id = inner
            .entries
            .iter()
            .find_map(|(id, e)| match &e.stage {
                Stage::Ticket { hash, deadline }
                    if now < e.deadline
                        && now < *deadline
                        && secret_equal_bytes(hash, &ticket_hash) =>
                {
                    Some(id.clone())
                }
                _ => None,
            })
            .ok_or(GrantDenied)?;
        let entry = inner.entries.get_mut(&grant_id).ok_or(GrantDenied)?;
        // Origin check before the consuming state change: a cross-origin
        // request neither learns nor burns the ticket.
        if !origin_matches(&entry.origin, origin) {
            return Err(GrantDenied);
        }
        let secret = mint_secret();
        let secret_hash = hash_secret(&secret);
        let exchanged = ExchangedGrant {
            grant_id: grant_id.clone(),
            viewer_session_id: format!("vs_{}", mint_secret()),
            rotation_generation: 0,
            current_secret: ViewerSecret(secret),
            binding: entry.binding.clone(),
            resource_id: entry.resource_id.clone(),
            permissions: entry.permissions.clone(),
            origin: entry.origin.clone(),
            expires_at_ms: entry.expires_at_ms,
        };
        entry.stage = Stage::Session(SessionState {
            generation: 0,
            current_hash: secret_hash,
            inflight: HashSet::new(),
            consumed: VecDeque::new(),
            ws_generation: 0,
        });
        Ok(exchanged)
    }

    /// Phase one of the auth CAS: validate the presented secret against the
    /// session's *current* secret and register the attempt. The caller must
    /// drive the attempt to exactly one outcome: [`Self::commit_auth`], or
    /// drop the [`AuthAttempt`] (abort — the registration is removed).
    pub(crate) fn begin_auth(&self, hello: &AuthHello) -> Result<AuthAttempt, AuthErrorCode> {
        let mut inner = self.lock();
        let attempt_id = inner.next_attempt;
        inner.next_attempt += 1;
        let presented = hash_secret(&hello.secret);
        let entry = inner
            .entries
            .get_mut(&hello.grant_id)
            .filter(|e| e.alive())
            .ok_or(AuthErrorCode::Unauthorized)?;
        let Stage::Session(session) = &mut entry.stage else {
            return Err(AuthErrorCode::Unauthorized);
        };
        if hello.rotation_generation == session.generation
            && secret_equal_bytes(&session.current_hash, &presented)
        {
            if session.inflight.len() >= MAX_INFLIGHT_ATTEMPTS {
                return Err(AuthErrorCode::Conflict);
            }
            session.inflight.insert(attempt_id);
            return Ok(AuthAttempt {
                store: self.inner.clone(),
                grant_id: hello.grant_id.clone(),
                attempt_id,
                generation: session.generation,
                hash: presented,
                origin: entry.origin.clone(),
                done: false,
            });
        }
        // Replay: a *retained consumed* secret presented again revokes the
        // whole session and closes its live WS. Generic answer either way.
        let now = Instant::now();
        let replayed = session.consumed.iter().any(|c| {
            now < c.consumed_at + CONSUMED_RETENTION && secret_equal_bytes(&c.hash, &presented)
        });
        if replayed {
            let grant_id = hello.grant_id.clone();
            Self::revoke_locked(&mut inner, &grant_id);
        }
        Err(AuthErrorCode::Unauthorized)
    }

    /// Phase two of the auth CAS. On success the old secret is consumed,
    /// `next` is minted, and generations bump. On failure the error tells
    /// the race losers apart: an attempt still registered on a *consumed*
    /// secret lost the race (`Conflict`); anything else is `Unauthorized`.
    pub(crate) fn commit_auth(
        &self,
        mut attempt: AuthAttempt,
    ) -> Result<AuthCommit, AuthErrorCode> {
        attempt.done = true;
        let mut inner = self.lock();
        let entry = inner
            .entries
            .get_mut(&attempt.grant_id)
            .filter(|e| e.alive())
            .ok_or(AuthErrorCode::Unauthorized)?;
        let Stage::Session(session) = &mut entry.stage else {
            return Err(AuthErrorCode::Unauthorized);
        };
        if attempt.generation != session.generation || attempt.hash != session.current_hash {
            // Something rotated under us. Registered on a consumed record?
            // Then we are a declared race loser.
            let loser = session
                .consumed
                .iter_mut()
                .find(|c| c.generation == attempt.generation && c.hash == attempt.hash)
                .is_some_and(|c| c.inflight.remove(&attempt.attempt_id));
            // Not registered anywhere any more: also scrub the current set.
            session.inflight.remove(&attempt.attempt_id);
            return Err(if loser {
                AuthErrorCode::Conflict
            } else {
                AuthErrorCode::Unauthorized
            });
        }
        session.inflight.remove(&attempt.attempt_id);
        let next = mint_secret();
        session.consumed.push_back(ConsumedSecret {
            generation: session.generation,
            hash: session.current_hash,
            inflight: std::mem::take(&mut session.inflight),
            consumed_at: Instant::now(),
        });
        prune_consumed(session, entry.deadline);
        session.generation += 1;
        session.ws_generation += 1;
        session.current_hash = hash_secret(&next);
        Ok(AuthCommit {
            ws_generation: session.ws_generation,
            next_secret: next,
            viewer_session_id: format!("vs_{}", mint_secret()),
            generation: session.generation,
        })
    }

    /// Install the live viewer WS *after* the AuthOk write committed. Any
    /// prior live WS is superseded: its lease is revoked, which fires its
    /// close signal. Returns false if the grant vanished in between.
    pub(crate) fn publish_live(
        &self,
        grant_id: &str,
        control: Arc<TargetControl>,
        lease: ViewerLease,
    ) -> bool {
        let mut inner = self.lock();
        let Some(entry) = inner.entries.get_mut(grant_id).filter(|e| e.alive()) else {
            return false;
        };
        if let Some(old) = entry.live.replace(LiveViewer { control, lease }) {
            // Lock order is store -> control here, and the control plane
            // never calls into this store, so there is no cycle.
            let _ = old.control.revoke(&old.lease, InterventionEnd::Revoked);
        }
        true
    }

    /// Issuer-side revoke: drop all rotation state and close the live WS.
    /// Afterwards the grant id is unknown, which the generic denial folds
    /// into the same answer as "never existed".
    pub(crate) fn revoke(&self, grant_id: &str) -> bool {
        let mut inner = self.lock();
        Self::revoke_locked(&mut inner, grant_id)
    }

    /// Recheck issuer and expiry under the mutation lock: an issuer RPC's
    /// earlier grant_info lookup must not race into success on an expired grant.
    pub(crate) fn revoke_for_issuer(&self, grant_id: &str, issuer: &IssuerIdentity) -> bool {
        let mut inner = self.lock();
        if !inner
            .entries
            .get(grant_id)
            .is_some_and(|e| e.alive() && &e.issuer == issuer)
        {
            return false;
        }
        Self::revoke_locked(&mut inner, grant_id)
    }

    /// Revoke every outstanding grant of `resource_id` (resource closed or
    /// expired): each live WS is closed through its lease and each grant id
    /// becomes unknown. Returns how many grants were revoked. A bounded scan
    /// (<= `MAX_LIVE_GRANTS`) rather than a second index that could drift
    /// from `entries`.
    #[allow(dead_code)] // consumed by the Task 6 resource close/expiry wiring
    pub(crate) fn revoke_resource(&self, resource_id: &str) -> usize {
        let mut inner = self.lock();
        let doomed: Vec<String> = inner
            .entries
            .iter()
            .filter(|(_, e)| e.resource_id == resource_id)
            .map(|(id, _)| id.clone())
            .collect();
        for grant_id in &doomed {
            Self::revoke_locked(&mut inner, grant_id);
        }
        doomed.len()
    }

    /// Issuer death: revoke every grant `principal` issued or is bound to as
    /// the viewing principal (a deactivated user loses both). Same whole-grant
    /// path as [`Self::revoke`]: each live WS is closed through its lease,
    /// which bumps the target's control epoch and ws generation and hands any
    /// human control back to the Agent with one `Revoked` summary. Returns
    /// how many grants were revoked. Bounded scan, like `revoke_resource`.
    pub(crate) fn revoke_for_principal(&self, principal: &IssuerIdentity) -> usize {
        let mut inner = self.lock();
        let doomed: Vec<String> = inner
            .entries
            .iter()
            .filter(|(_, e)| &e.issuer == principal || &e.binding.principal == principal)
            .map(|(id, _)| id.clone())
            .collect();
        for grant_id in &doomed {
            Self::revoke_locked(&mut inner, grant_id);
        }
        doomed.len()
    }

    fn revoke_locked(inner: &mut Inner, grant_id: &str) -> bool {
        let Some(entry) = inner.entries.remove(grant_id) else {
            return false;
        };
        if let Some(live) = entry.live {
            let _ = live.control.revoke(&live.lease, InterventionEnd::Revoked);
        }
        true
    }

    /// Public, authority-free descriptor. `None` and "denied" are the same
    /// to callers with no credential; this is for the issuer-side paths.
    #[allow(dead_code)] // consumed by the Task 6 issuer RPC surface
    pub(crate) fn grant_info(&self, grant_id: &str) -> Option<ViewerGrantInfo> {
        let inner = self.lock();
        let entry = inner.entries.get(grant_id).filter(|e| e.alive())?;
        Some(ViewerGrantInfo {
            grant_id: grant_id.to_string(),
            issuer: entry.issuer.clone(),
            binding: entry.binding.clone(),
            resource_id: entry.resource_id.clone(),
            permissions: entry.permissions.clone(),
            origin: entry.origin.clone(),
            expires_at_ms: entry.expires_at_ms,
        })
    }

    #[cfg(test)]
    pub(crate) fn expire_grant_for_test(&self, grant_id: &str) {
        let mut inner = self.lock();
        if let Some(entry) = inner.entries.get_mut(grant_id) {
            entry.deadline = Instant::now() - Duration::from_secs(1);
        }
    }

    #[cfg(test)]
    pub(crate) fn expire_ticket_for_test(&self, grant_id: &str) {
        let mut inner = self.lock();
        if let Some(entry) = inner.entries.get_mut(grant_id) {
            if let Stage::Ticket { deadline, .. } = &mut entry.stage {
                *deadline = Instant::now() - Duration::from_secs(1);
            }
        }
    }
}

/// Drop consumed secrets that aged out, bound the list, and never retain
/// past the grant's own expiry.
fn prune_consumed(session: &mut SessionState, grant_deadline: Instant) {
    let now = Instant::now();
    session
        .consumed
        .retain(|c| now < c.consumed_at + CONSUMED_RETENTION && c.consumed_at < grant_deadline);
    while session.consumed.len() > MAX_CONSUMED_SECRETS {
        session.consumed.pop_front();
    }
}

/// A registered in-flight auth attempt. Dropping it without commit is the
/// abort path: the registration is removed and nothing is exempted.
pub(crate) struct AuthAttempt {
    store: Arc<Mutex<Inner>>,
    grant_id: String,
    attempt_id: u64,
    generation: u64,
    hash: [u8; 32],
    origin: ViewerOrigin,
    done: bool,
}

impl AuthAttempt {
    /// The origin this attempt's grant is bound to; the transport compares
    /// it against the socket's own `Origin` before committing.
    pub(crate) fn grant_origin(&self) -> &ViewerOrigin {
        &self.origin
    }
}

impl Drop for AuthAttempt {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let mut inner = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entry) = inner.entries.get_mut(&self.grant_id) else {
            return;
        };
        let Stage::Session(session) = &mut entry.stage else {
            return;
        };
        session.inflight.remove(&self.attempt_id);
        for c in &mut session.consumed {
            c.inflight.remove(&self.attempt_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::browser_live::{
        BrowserLiveState, BrowserTargetBinding, Controller, ConversationBinding, HumanInputEvent,
        HumanInputRequest, ViewportCssPx,
    };

    use super::super::control::InterventionSink;
    use crate::browser::error::BrowserError;

    fn issuer() -> IssuerIdentity {
        IssuerIdentity::User {
            user_id: "user-1".into(),
        }
    }

    fn binding(target_id: &str) -> ViewerBinding {
        ViewerBinding {
            principal: issuer(),
            task: ConversationBinding {
                session_key: "agent:main:demo".into(),
                run_id: "run-1".into(),
            },
            target: BrowserTargetBinding {
                profile: "default".into(),
                browser_session_id: "bs-1".into(),
                target_id: target_id.into(),
            },
        }
    }

    fn origin() -> ViewerOrigin {
        ViewerOrigin {
            scheme: "http".into(),
            host: "127.0.0.1".into(),
            port: 8787,
        }
    }

    fn issue(store: &ViewerGrantStore, resource_id: &str, target_id: &str) -> IssuedGrant {
        store
            .issue(
                issuer(),
                binding(target_id),
                resource_id,
                &[ViewerPermission::Frames, ViewerPermission::HumanInput],
                origin(),
                Duration::from_secs(600),
            )
            .expect("issue")
    }

    /// Drive a grant to a live session; returns (grant_id, plaintext S0).
    fn reach_session(store: &ViewerGrantStore, target_id: &str) -> (String, String) {
        let issued = issue(store, "res-1", target_id);
        let consumed = store
            .consume_nonce(&issued.grant_id, &issued.nonce, Some(&origin()))
            .expect("consume");
        let exchanged = store
            .exchange(&consumed.ticket, Some(&origin()))
            .expect("exchange");
        (issued.grant_id, exchanged.current_secret.0.clone())
    }

    fn hello(grant_id: &str, generation: u64, secret: &str) -> AuthHello {
        AuthHello {
            protocol_version: 1,
            grant_id: grant_id.into(),
            rotation_generation: generation,
            secret: secret.into(),
        }
    }

    #[test]
    fn viewer_credential_cannot_cross_scope() {
        let store = ViewerGrantStore::default();
        let grant_a = issue(&store, "res-a", "target-a");
        let grant_b = issue(&store, "res-b", "target-b");

        // Grant A's nonce against grant B's id: denied, and B stays unused.
        assert!(store
            .consume_nonce(&grant_b.grant_id, &grant_a.nonce, Some(&origin()))
            .is_err());
        // Unknown grant id is indistinguishable.
        assert!(store
            .consume_nonce("no-such-grant", &grant_a.nonce, Some(&origin()))
            .is_err());
        // A wrong origin denies WITHOUT consuming the nonce.
        let foreign = ViewerOrigin {
            scheme: "http".into(),
            host: "localhost".into(), // loopback, but not the bound origin
            port: 8787,
        };
        assert!(store
            .consume_nonce(&grant_a.grant_id, &grant_a.nonce, Some(&foreign))
            .is_err());
        // Both grants still work with their own credential + origin.
        assert!(store
            .consume_nonce(&grant_a.grant_id, &grant_a.nonce, Some(&origin()))
            .is_ok());
        assert!(store
            .consume_nonce(&grant_b.grant_id, &grant_b.nonce, Some(&origin()))
            .is_ok());
    }

    #[test]
    fn nonce_and_ticket_are_single_use_and_expire() {
        let store = ViewerGrantStore::default();
        let issued = issue(&store, "res-1", "target-1");
        let consumed = store
            .consume_nonce(&issued.grant_id, &issued.nonce, Some(&origin()))
            .expect("first consume");
        // Nonce replay: same grant id + same nonce, denied.
        assert!(store
            .consume_nonce(&issued.grant_id, &issued.nonce, Some(&origin()))
            .is_err());

        let exchanged = store
            .exchange(&consumed.ticket, Some(&origin()))
            .expect("first exchange");
        assert_eq!(exchanged.rotation_generation, 0);
        assert_eq!(exchanged.current_secret.0.len(), 43);
        // Ticket replay: denied.
        assert!(store.exchange(&consumed.ticket, Some(&origin())).is_err());

        // Grant expiry closes the whole chain.
        let expired = issue(&store, "res-2", "target-2");
        store.expire_grant_for_test(&expired.grant_id);
        assert!(store
            .consume_nonce(&expired.grant_id, &expired.nonce, Some(&origin()))
            .is_err());

        // Ticket expiry denies the exchange even though the nonce worked.
        let slow = issue(&store, "res-3", "target-3");
        let slow_ticket = store
            .consume_nonce(&slow.grant_id, &slow.nonce, Some(&origin()))
            .expect("consume slow");
        store.expire_ticket_for_test(&slow.grant_id);
        assert!(store
            .exchange(&slow_ticket.ticket, Some(&origin()))
            .is_err());
    }

    #[test]
    fn rotation_commits_once_and_consumed_secret_replay_revokes() {
        let store = ViewerGrantStore::default();
        let (grant_id, s0) = reach_session(&store, "target-1");

        // Two racing attempts on the same generation-0 secret.
        let winner = store.begin_auth(&hello(&grant_id, 0, &s0)).expect("winner");
        let loser = store.begin_auth(&hello(&grant_id, 0, &s0)).expect("loser");
        let commit = store.commit_auth(winner).expect("commit");
        assert_eq!(commit.ws_generation, 1);
        assert_ne!(commit.next_secret, s0, "rotation must mint a fresh secret");
        // The race loser is told Conflict, not Unauthorized.
        assert!(matches!(
            store.commit_auth(loser),
            Err(AuthErrorCode::Conflict)
        ));

        // Presenting the retained CONSUMED secret later is replay: the whole
        // grant is revoked atomically.
        assert!(matches!(
            store.begin_auth(&hello(&grant_id, 0, &s0)),
            Err(AuthErrorCode::Unauthorized)
        ));
        assert!(
            store.grant_info(&grant_id).is_none(),
            "replay must revoke the grant"
        );
        // The new secret dies with it: nothing can authenticate anymore.
        assert!(matches!(
            store.begin_auth(&hello(&grant_id, 1, &commit.next_secret)),
            Err(AuthErrorCode::Unauthorized)
        ));
    }

    #[test]
    fn dropped_attempt_aborts_without_burning_the_secret() {
        let store = ViewerGrantStore::default();
        let (grant_id, s0) = reach_session(&store, "target-1");

        // Abort: register, drop, then a fresh attempt on the SAME secret
        // must still succeed.
        let attempt = store.begin_auth(&hello(&grant_id, 0, &s0)).expect("begin");
        drop(attempt);
        let attempt = store
            .begin_auth(&hello(&grant_id, 0, &s0))
            .expect("begin again");
        store.commit_auth(attempt).expect("commit after abort");
    }

    #[test]
    fn inflight_cap_rejects_the_ninth_attempt() {
        let store = ViewerGrantStore::default();
        let (grant_id, s0) = reach_session(&store, "target-1");
        let mut attempts = Vec::new();
        for _ in 0..8 {
            attempts.push(
                store
                    .begin_auth(&hello(&grant_id, 0, &s0))
                    .expect("inflight"),
            );
        }
        assert!(
            matches!(
                store.begin_auth(&hello(&grant_id, 0, &s0)),
                Err(AuthErrorCode::Conflict)
            ),
            "the cap fails closed as Conflict, never as a silent pass"
        );
        drop(attempts);
    }

    fn live_control(
        resource_id: &str,
        binding: &ViewerBinding,
    ) -> (
        Arc<TargetControl>,
        ViewerLease,
        tokio::sync::watch::Receiver<bool>,
    ) {
        let state = BrowserLiveState {
            resource_id: resource_id.into(),
            binding: binding.clone(),
            connection_epoch: 7,
            stream_epoch: 3,
            viewport_css_px: ViewportCssPx {
                width: 1280,
                height: 800,
            },
            controller: Controller::Agent,
            control_epoch: 0,
        };
        let sink: InterventionSink = Arc::new(|_summary| -> Result<(), BrowserError> { Ok(()) });
        let control = TargetControl::new(state, sink);
        let lease = control
            .register_viewer(
                "viewer-1",
                1,
                chrono::Utc::now().timestamp_millis() + 600_000,
                vec![ViewerPermission::Frames, ViewerPermission::HumanInput],
            )
            .expect("register");
        let close = lease.close_signal();
        (control, lease, close)
    }

    #[tokio::test]
    async fn revocation_closes_frame_and_input() {
        let store = ViewerGrantStore::default();
        let (grant_id, s0) = reach_session(&store, "target-1");
        let attempt = store.begin_auth(&hello(&grant_id, 0, &s0)).expect("begin");
        store.commit_auth(attempt).expect("commit");

        let (control, lease, close) = live_control("res-1", &binding("target-1"));
        assert!(store.publish_live(&grant_id, control.clone(), lease.clone()));

        // Before revocation the viewer can queue input.
        let input = HumanInputRequest {
            connection_epoch: 7,
            stream_epoch: 3,
            control_epoch: 0,
            seq: 1,
            expires_at_ms: chrono::Utc::now().timestamp_millis() + 60_000,
            event: HumanInputEvent::Text {
                text: "hello".into(),
            },
        };
        // Not the controller yet (Agent holds control): take it first.
        let grant_binding = binding("target-1");
        let _ = grant_binding; // binding equality is the store's concern
        let request = aleph_protocol::browser_live::ControlRequest {
            expected_control_epoch: 0,
            action_seq: 1,
            expires_at_ms: chrono::Utc::now().timestamp_millis() + 60_000,
        };
        let taken = control.take(&lease, request).await;
        assert!(taken.accepted, "take should succeed: {:?}", taken.reason);
        let admitted = control.queue_input(
            &lease,
            HumanInputRequest {
                control_epoch: taken.control_epoch,
                ..input.clone()
            },
        );
        assert!(admitted.is_ok(), "input should queue while live");

        // Revoke the grant: the lease's close signal fires and both frames
        // and input die with it.
        assert!(store.revoke(&grant_id));
        assert!(close.has_changed().unwrap_or(true), "close signal fired");
        assert!(lease.closed());
        let rejected = control.queue_input(
            &lease,
            HumanInputRequest {
                seq: 2,
                control_epoch: taken.control_epoch,
                ..input
            },
        );
        assert!(matches!(
            rejected,
            Err(aleph_protocol::browser_live::InputRejectReason::Revoked)
        ));

        // Second revoke is idempotent; unknown grant ids are too.
        assert!(!store.revoke(&grant_id));
        assert!(!store.revoke("never-existed"));
    }

    #[tokio::test]
    async fn revoke_resource_revokes_only_that_resources_grants() {
        let store = ViewerGrantStore::default();
        let a1 = issue(&store, "res-1", "target-1");
        let a2 = issue(&store, "res-1", "target-1");
        let b = issue(&store, "res-2", "target-2");
        // a1 reaches a live WS; a2 stays at the nonce stage.
        let consumed = store
            .consume_nonce(&a1.grant_id, &a1.nonce, Some(&origin()))
            .expect("consume");
        let exchanged = store
            .exchange(&consumed.ticket, Some(&origin()))
            .expect("exchange");
        let attempt = store
            .begin_auth(&hello(&a1.grant_id, 0, &exchanged.current_secret.0))
            .expect("begin");
        store.commit_auth(attempt).expect("commit");
        let (control, lease, close) = live_control("res-1", &binding("target-1"));
        assert!(store.publish_live(&a1.grant_id, control, lease.clone()));

        assert_eq!(store.revoke_resource("res-1"), 2);
        assert!(store.grant_info(&a1.grant_id).is_none());
        assert!(store.grant_info(&a2.grant_id).is_none());
        assert!(close.has_changed().unwrap_or(true), "live WS close fired");
        assert!(lease.closed());
        // Revoked grants are denied generically, like never-existed ones.
        assert!(store
            .consume_nonce(&a2.grant_id, &a2.nonce, Some(&origin()))
            .is_err());
        // The other resource's grant is untouched; repeat is a no-op.
        assert!(store.grant_info(&b.grant_id).is_some());
        assert_eq!(store.revoke_resource("res-1"), 0);
        assert_eq!(store.revoke_resource("never-existed"), 0);
    }

    /// Issuer death (user deactivation) is a whole-grant revoke: the live WS
    /// closes, human control returns to the Agent with a bumped control
    /// epoch, and only that principal's grants are touched.
    #[tokio::test]
    async fn revoke_for_principal_is_whole_grant_revoke_for_that_principal_only() {
        let store = ViewerGrantStore::default();
        let (grant_id, s0) = reach_session(&store, "target-1");
        let attempt = store.begin_auth(&hello(&grant_id, 0, &s0)).expect("begin");
        store.commit_auth(attempt).expect("commit");
        let (control, lease, close) = live_control("res-1", &binding("target-1"));
        assert!(store.publish_live(&grant_id, control.clone(), lease.clone()));
        let taken = control
            .take(
                &lease,
                aleph_protocol::browser_live::ControlRequest {
                    expected_control_epoch: 0,
                    action_seq: 1,
                    expires_at_ms: chrono::Utc::now().timestamp_millis() + 60_000,
                },
            )
            .await;
        assert!(taken.accepted);
        let epoch_before = control.snapshot().control_epoch;

        // A grant whose issuer AND viewing principal is another user.
        let other = IssuerIdentity::User {
            user_id: "user-2".into(),
        };
        let mut other_binding = binding("target-2");
        other_binding.principal = other.clone();
        let foreign = store
            .issue(
                other.clone(),
                other_binding,
                "res-2",
                &[ViewerPermission::Frames],
                origin(),
                Duration::from_secs(600),
            )
            .expect("issue other");

        assert_eq!(store.revoke_for_principal(&issuer()), 1);
        assert!(store.grant_info(&grant_id).is_none());
        assert!(close.has_changed().unwrap_or(true), "live WS close fired");
        assert!(lease.closed());
        let after = control.snapshot();
        assert_eq!(after.controller, Controller::Agent);
        assert!(after.control_epoch > epoch_before, "control epoch bumped");
        // The other principal's grant survives; repeat is a no-op.
        assert!(store.grant_info(&foreign.grant_id).is_some());
        assert_eq!(store.revoke_for_principal(&issuer()), 0);
        assert_eq!(store.revoke_for_principal(&other), 1);
    }
}
