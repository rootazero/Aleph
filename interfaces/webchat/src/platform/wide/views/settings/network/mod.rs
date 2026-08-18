//! Services & Cluster settings page — combined single page:
//!  · Section 1 Service connection (shell-core separated connection toggle: local / remote)
//!  · Section 2 Aleph cluster (cluster node management)

mod cluster;
mod connection;

use crate::i18n::{t, use_i18n};
use cluster::ClusterSection;
use connection::ConnectionSection;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn NetworkView() -> impl IntoView {
    let i18n = use_i18n();
    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto space-y-10">
            <h1 class="text-2xl font-bold text-text-primary">{t!(i18n, settings.network.page_title)}</h1>
            <ConnectionSection />
            <ClusterSection />
        </div>
    }
}
