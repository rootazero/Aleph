//! 工具卡片富渲染 —— 把一次工具调用（args/result）按工具类型渲染成
//! diff / shell / 全文 / patch 等富视图。左侧聊天与右侧工作区面板共用。
//!
//! 纯逻辑（ToolKind 分流、diff、截断、汇总）与视图组件分离：逻辑可在
//! 宿主机 `cargo test -p aleph-panel --lib` 下测试。

/// 工具大类 —— 决定卡片体如何渲染。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
    FileEdit,
    FileWrite,
    ApplyPatch,
    FileRead,
    Bash,
    Search,
    Default,
}

impl ToolKind {
    /// 由工具名（大小写不敏感）映射到大类。未知名 → `Default`。
    pub fn from_name(name: &str) -> ToolKind {
        let n = name.to_lowercase();
        match n.as_str() {
            "file_edit" => ToolKind::FileEdit,
            "file_write" => ToolKind::FileWrite,
            "apply_patch" => ToolKind::ApplyPatch,
            "file_read" => ToolKind::FileRead,
            _ => {
                if n.starts_with("bash")
                    || n.starts_with("shell")
                    || n.starts_with("code_exec")
                    || n.contains("_exec")
                {
                    ToolKind::Bash
                } else if n == "search"
                    || n == "web_search"
                    || n == "grep"
                    || n == "find"
                    || n.starts_with("search")
                    || n.ends_with("_search")
                {
                    ToolKind::Search
                } else {
                    ToolKind::Default
                }
            }
        }
    }

    /// 卡片默认是否展开内容：文件改动类默认展开，其余默认折叠。
    pub fn default_open(self) -> bool {
        matches!(
            self,
            ToolKind::FileEdit | ToolKind::FileWrite | ToolKind::ApplyPatch
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_maps_known_and_unknown() {
        assert_eq!(ToolKind::from_name("file_edit"), ToolKind::FileEdit);
        assert_eq!(ToolKind::from_name("FILE_WRITE"), ToolKind::FileWrite);
        assert_eq!(ToolKind::from_name("apply_patch"), ToolKind::ApplyPatch);
        assert_eq!(ToolKind::from_name("file_read"), ToolKind::FileRead);
        assert_eq!(ToolKind::from_name("bash"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("code_exec"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("python_exec"), ToolKind::Bash);
        assert_eq!(ToolKind::from_name("search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("web_search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("hybrid_search"), ToolKind::Search);
        assert_eq!(ToolKind::from_name("memory_recall"), ToolKind::Default);
    }

    #[test]
    fn default_open_only_for_file_mutations() {
        assert!(ToolKind::FileEdit.default_open());
        assert!(ToolKind::FileWrite.default_open());
        assert!(ToolKind::ApplyPatch.default_open());
        assert!(!ToolKind::Bash.default_open());
        assert!(!ToolKind::Search.default_open());
        assert!(!ToolKind::FileRead.default_open());
        assert!(!ToolKind::Default.default_open());
    }
}
