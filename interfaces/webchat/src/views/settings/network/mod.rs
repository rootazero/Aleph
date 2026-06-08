//! Network 设置页 — 合并单页:
//!  · Section 1 上游连接(壳核分离连接切换,Feature A)
//!  · Section 2 下游集群(集群节点管理,Feature B 骨架)

mod cluster;
mod connection;

use cluster::ClusterSection;
use connection::ConnectionSection;
use leptos::prelude::*;

#[component]
pub fn NetworkView() -> impl IntoView {
    view! {
        <div class="px-8 pb-8 aleph-content-top max-w-5xl mx-auto space-y-10">
            <h1 class="text-2xl font-bold text-text-primary">"网络与集群"</h1>
            <ConnectionSection />
            <ClusterSection />
        </div>
    }
}
