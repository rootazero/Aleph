//! Server-owned browser-viewer issuance policy. RPCs supply only a resource id
//! and requested permissions; authority always comes from this shared registry.
//!
//! Boot must supply the producer's registry/store and an explicit trusted public
//! HTTP(S) origin. A bind address, caller Origin, or connection id is not one.

use std::sync::Arc;
use std::time::Duration;

use aleph_protocol::browser_live::{
    BusEvent, DecideResourceRequest, IssueViewerGrantRequest, IssueViewerGrantResponse,
    IssuerIdentity, ListResourcesResponse, PreviewResource, RedactionStatus,
    RevokeViewerGrantResponse, ViewerOrigin, ViewerPermission,
};

use super::grant::{GrantDenied, IssuedGrant, ViewerGrantStore};
use super::lifecycle::BrowserLiveLifecycle;
use super::resource::ResourceRegistry;

/// Opaque state shared by the issuer RPCs and the dedicated viewer route.
/// Construction remains in the core composition root, not the I/O builder.
/// Without an installed backend/state, all issuer RPCs deny generically.
/// `decide` additionally requires a lifecycle (see [`Self::new_with_lifecycle`]);
/// a state built by [`Self::new`] issues/revokes as before but denies decisions.
pub struct BrowserViewerIssuerState {
    pub(crate) grants: ViewerGrantStore,
    resources: ResourceRegistry,
    lifecycle: Option<BrowserLiveLifecycle>,
    origin: ViewerOrigin,
    launch_base: String,
    allowed_permissions: Vec<ViewerPermission>,
    max_ttl: Duration,
}

impl BrowserViewerIssuerState {
    /// Only Frames are allowed in this first issuer surface. Input/navigation
    /// require a separately reviewed server policy, never a request flag.
    #[allow(dead_code)] // the production backend/producer composition root is not installed yet
    pub(crate) fn new(
        grants: ViewerGrantStore,
        resources: ResourceRegistry,
        trusted_origin: &str,
        max_ttl: Duration,
    ) -> Result<Arc<Self>, GrantDenied> {
        Self::build(grants, resources, None, trusted_origin, max_ttl)
    }

    /// Issuer state over the lifecycle's own stores. Grants and resources are
    /// taken from the lifecycle itself, so the issuer, the decision path and the
    /// lifecycle's revocation can never act on different stores.
    pub(crate) fn new_with_lifecycle(
        lifecycle: BrowserLiveLifecycle,
        trusted_origin: &str,
        max_ttl: Duration,
    ) -> Result<Arc<Self>, GrantDenied> {
        Self::build(
            lifecycle.grants().clone(),
            lifecycle.resources().clone(),
            Some(lifecycle),
            trusted_origin,
            max_ttl,
        )
    }

    fn build(
        grants: ViewerGrantStore,
        resources: ResourceRegistry,
        lifecycle: Option<BrowserLiveLifecycle>,
        trusted_origin: &str,
        max_ttl: Duration,
    ) -> Result<Arc<Self>, GrantDenied> {
        // Validate before URL normalization can erase credentials or dot paths.
        let uri = trusted_origin
            .parse::<axum::http::Uri>()
            .map_err(|_| GrantDenied)?;
        if trusted_origin.contains(['#', '?', '\\'])
            || uri.authority().is_none_or(|a| a.as_str().contains('@'))
            || uri.path_and_query().is_some_and(|pq| pq.as_str() != "/")
            || max_ttl.as_millis() == 0
        {
            return Err(GrantDenied);
        }
        let url = url::Url::parse(trusted_origin).map_err(|_| GrantDenied)?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(GrantDenied);
        }
        let origin = ViewerOrigin {
            scheme: url.scheme().to_string(),
            // Match the route's http::Uri host representation (including IPv6 brackets).
            host: uri.host().ok_or(GrantDenied)?.to_ascii_lowercase(),
            port: url.port_or_known_default().ok_or(GrantDenied)?,
        };
        let launch_base = format!("{}/viewer/browser", url.origin().ascii_serialization());
        Ok(Arc::new(Self {
            grants,
            resources,
            lifecycle,
            origin,
            launch_base,
            allowed_permissions: vec![ViewerPermission::Frames],
            max_ttl,
        }))
    }

    pub(crate) fn issue(
        &self,
        user: &str,
        request: IssueViewerGrantRequest,
    ) -> Result<IssueViewerGrantResponse, GrantDenied> {
        let (resource, grant) = self.mint(user, request)?;
        self.finish_issue(user, &resource, grant)
    }

    fn mint(
        &self,
        user: &str,
        request: IssueViewerGrantRequest,
    ) -> Result<(PreviewResource, IssuedGrant), GrantDenied> {
        if user.trim().is_empty() {
            return Err(GrantDenied);
        }
        let issuer = IssuerIdentity::User {
            user_id: user.to_string(),
        };
        let resource = self
            .resources
            .get_active_for(&request.resource_id, &issuer)
            .ok_or(GrantDenied)?;
        let binding = resource
            .viewer_binding()
            // Fail closed: only an affirmative Safe verdict is viewable. Pending
            // (undecided) and Refused are the same generic denial as no binding.
            .filter(|_| resource.redaction == RedactionStatus::Safe)
            .ok_or(GrantDenied)?
            .clone();
        let permissions: Vec<_> = request
            .permissions
            .into_iter()
            .filter(|p| self.allowed_permissions.contains(p))
            .collect();
        // Fixed absolute expiry prevents time between registry lookup and store
        // insertion from extending a grant beyond the resource's lifetime.
        let ttl_ms = i64::try_from(self.max_ttl.as_millis()).map_err(|_| GrantDenied)?;
        let policy_expiry = chrono::Utc::now()
            .timestamp_millis()
            .checked_add(ttl_ms)
            .ok_or(GrantDenied)?;
        let expires_at_ms = resource.expires_at_ms.min(policy_expiry);
        let grant = self.grants.issue_until(
            issuer,
            binding,
            &resource.resource_id,
            &permissions,
            self.origin.clone(),
            expires_at_ms,
        )?;
        Ok((resource, grant))
    }

    fn finish_issue(
        &self,
        user: &str,
        resource: &PreviewResource,
        grant: IssuedGrant,
    ) -> Result<IssueViewerGrantResponse, GrantDenied> {
        let issuer = IssuerIdentity::User {
            user_id: user.to_string(),
        };
        let active = self
            .resources
            .get_active_for(&resource.resource_id, &issuer);
        let still_valid = active.is_some_and(|current| {
            current.binding == resource.binding
                && current.revision == resource.revision
                && current.redaction == RedactionStatus::Safe
                && current.viewer_binding().is_some()
                && current.expires_at_ms >= grant.expires_at_ms
        }) && self.grants.grant_info(&grant.grant_id).is_some();
        if !still_valid {
            self.grants.revoke(&grant.grant_id);
            return Err(GrantDenied);
        }
        // Both ids are server-minted base64url. No secret appears in the query,
        // headers or cookies; the browser clears the fragment before consume.
        Ok(IssueViewerGrantResponse {
            launch_url: format!(
                "{}#grant_id={}&nonce={}",
                self.launch_base, grant.grant_id, grant.nonce.0
            ),
            grant_id: grant.grant_id,
            expires_at_ms: grant.expires_at_ms,
        })
    }

    /// Policy-only forwarder for `browser.viewer.resource.decide`: the owner's
    /// explicit decision on one exact revision. Never widens permissions (the
    /// allowed set stays Frames-only; a later `issue` is still required) and
    /// is denied without a lifecycle, so it can never be a reported-success
    /// no-op. All failures are the same generic denial.
    pub(crate) fn decide(
        &self,
        user: &str,
        request: DecideResourceRequest,
    ) -> Result<BusEvent, GrantDenied> {
        if user.trim().is_empty() {
            return Err(GrantDenied);
        }
        self.lifecycle
            .as_ref()
            .ok_or(GrantDenied)?
            .decide(
                user,
                &request.resource_id,
                request.expected_revision,
                request.decision,
            )
            .map_err(|_| GrantDenied)
    }

    /// Read-only `browser.viewer.resource.list`: the caller's own live,
    /// unexpired resources, exactly as the registry holds them. Lets a client
    /// that (re)connected after a resource was published seed its view from
    /// the server instead of inferring it. A blank user is denied.
    pub(crate) fn list(&self, user: &str) -> Result<ListResourcesResponse, GrantDenied> {
        if user.trim().is_empty() {
            return Err(GrantDenied);
        }
        let owner = IssuerIdentity::User {
            user_id: user.to_string(),
        };
        Ok(ListResourcesResponse {
            resources: self.resources.list_active_for(&owner),
        })
    }

    pub(crate) fn revoke(
        &self,
        user: &str,
        grant_id: &str,
    ) -> Result<RevokeViewerGrantResponse, GrantDenied> {
        if user.trim().is_empty() {
            return Err(GrantDenied);
        }
        let issuer = IssuerIdentity::User {
            user_id: user.to_string(),
        };
        let info = self.grants.grant_info(grant_id).ok_or(GrantDenied)?;
        if info.issuer != issuer || !self.grants.revoke_for_issuer(grant_id, &issuer) {
            return Err(GrantDenied);
        }
        Ok(RevokeViewerGrantResponse { revoked: true })
    }
}

#[cfg(test)]
#[path = "issuer_tests.rs"]
pub(crate) mod tests;
