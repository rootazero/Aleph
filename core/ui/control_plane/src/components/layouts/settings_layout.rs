// core/ui/control_plane/src/components/layouts/settings_layout.rs
use leptos::prelude::*;
use leptos_router::components::Outlet;
use crate::components::SettingsSidebar;

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

            // 第三栏：内容区（通过 Outlet 渲染子路由）
            <div class="flex-1 overflow-y-auto">
                <Outlet />
            </div>
        </div>
    }
}
