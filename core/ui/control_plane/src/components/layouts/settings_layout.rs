// core/ui/control_plane/src/components/layouts/settings_layout.rs
use leptos::prelude::*;
use leptos_router::components::{Outlet, Route, Routes};
use leptos_router::path;
use crate::components::SettingsSidebar;
use crate::views::settings::*;

/// Settings 域的布局容器
///
/// 管理第二栏（SettingsSidebar）和第三栏（内容区）的渲染。
/// 通过 Outlet 渲染子路由内容。
#[component]
pub fn SettingsLayout() -> impl IntoView {
    view! {
        <div class="flex h-full">
            // 第二栏：Settings 专用侧边栏
            <SettingsSidebar />

            // 第三栏：内容区（通过嵌套路由渲染）
            <div class="flex-1 overflow-y-auto">
                <Routes fallback=|| view! { <div class="p-8">"Settings page not found"</div> }>
                    <Route path=path!("/settings") view=Settings />
                    <Route path=path!("/settings/general") view=GeneralView />
                    <Route path=path!("/settings/shortcuts") view=ShortcutsView />
                    <Route path=path!("/settings/behavior") view=BehaviorView />
                    <Route path=path!("/settings/generation") view=GenerationView />
                    <Route path=path!("/settings/search") view=SearchView />
                    <Route path=path!("/settings/providers") view=ProvidersView />
                    <Route path=path!("/settings/generation-providers") view=GenerationProvidersView />
                    <Route path=path!("/settings/agent") view=AgentView />
                    <Route path=path!("/settings/routing") view=RoutingRulesView />
                    <Route path=path!("/settings/mcp") view=McpView />
                    <Route path=path!("/settings/plugins") view=PluginsView />
                    <Route path=path!("/settings/skills") view=SkillsView />
                    <Route path=path!("/settings/memory") view=MemoryView />
                    <Route path=path!("/settings/security") view=SecurityView />
                    <Route path=path!("/settings/policies") view=PoliciesView />
                </Routes>
            </div>
        </div>
    }
}
