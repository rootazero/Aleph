//! Internal managed-launch attestation. Never deserialized, never built from
//! tool arguments: the only inputs are the driver-reported service PID, the
//! Gateway's own clock and an [`EndpointProof`] that only
//! `protocol::verify_challenge_response` can mint.

use super::protocol::EndpointProof;

/// Launcher label written into `PreviewProvenance::Managed`. A constant, so a
/// model-supplied name can never pose as the launcher.
pub(crate) const LAUNCHER: &str = "aleph.browser_companion";

/// What `BrowserPresentOperation::from_managed_operation` needs to know about
/// the process behind a managed resource. Private fields + no `Deserialize`.
#[derive(Clone, Debug)]
pub(crate) struct ManagedLaunchRecord {
    service_pid: u32,
    launched_at_ms: i64,
    proof: EndpointProof,
}

impl ManagedLaunchRecord {
    /// `service_pid` must be the PID the sandbox DRIVER vouched for (the child
    /// it spawned, with no wrapper in between). Zero ids / times are refused.
    pub(super) fn new(service_pid: u32, launched_at_ms: i64, proof: EndpointProof) -> Option<Self> {
        (service_pid > 0 && launched_at_ms > 0).then_some(Self {
            service_pid,
            launched_at_ms,
            proof,
        })
    }

    pub(crate) fn service_pid(&self) -> u32 {
        self.service_pid
    }

    pub(crate) fn launched_at_ms(&self) -> i64 {
        self.launched_at_ms
    }

    pub(crate) fn proof(&self) -> &EndpointProof {
        &self.proof
    }
}
