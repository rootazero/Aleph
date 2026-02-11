// core/ui/control_plane/src/components/sidebar/types.rs

/// Sidebar 显示模式
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SidebarMode {
    /// 宽模式 (w-64) - 显示图标 + 文字
    Wide,
    /// 窄模式 (w-16) - 仅显示图标
    Narrow,
}

/// 告警级别
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AlertLevel {
    /// 无告警
    None,
    /// 信息提示（蓝色徽章）
    Info,
    /// 警告（黄色徽章）
    Warning,
    /// 严重错误（红色徽章 + 呼吸动画）
    Critical,
}

/// 系统告警
#[derive(Clone, Debug)]
pub struct SystemAlert {
    /// 告警 key（如 "system.health"）
    pub key: String,
    /// 告警级别
    pub level: AlertLevel,
    /// 可选的数字徽章（如 "3 个错误"）
    pub count: Option<u32>,
    /// Tooltip 中显示的详细信息
    pub message: Option<String>,
}
