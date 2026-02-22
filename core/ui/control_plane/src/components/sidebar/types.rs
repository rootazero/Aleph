// core/ui/control_plane/src/components/sidebar/types.rs

/// Alert level
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AlertLevel {
    /// No alert
    None,
    /// Info (blue badge)
    Info,
    /// Warning (yellow badge)
    Warning,
    /// Critical (red badge + breathing animation)
    Critical,
}

/// System alert
#[derive(Clone, Debug)]
pub struct SystemAlert {
    /// Alert key (e.g., "system.health")
    pub key: String,
    /// Alert level
    pub level: AlertLevel,
    /// Optional numeric badge (e.g., "3 errors")
    pub count: Option<u32>,
    /// Tooltip detail message
    pub message: Option<String>,
}
