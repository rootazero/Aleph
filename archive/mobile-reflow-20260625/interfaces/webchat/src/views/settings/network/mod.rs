//! 服务与集群 设置页 — 合并单页:
//!  · Section 1 服务连接(壳核分离连接切换:本地服务 / 远程服务)
//!  · Section 2 Aleph 集群(集群节点管理)

mod cluster;
mod connection;

use cluster::ClusterSection;
use connection::ConnectionSection;
use leptos::prelude::*;

#[component]
#[must_use]
pub fn NetworkView() -> impl IntoView {
    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto space-y-10 max-sm:px-4 max-sm:max-w-none">
            <h1 class="text-2xl font-bold text-text-primary">"服务与集群"</h1>
            <ConnectionSection />
            <ClusterSection />
        </div>
    }
}
