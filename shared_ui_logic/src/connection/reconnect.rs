//! 重连策略实现

/// 重连策略（指数退避）
///
/// 实现了指数退避算法，用于在连接失败时自动重连。
///
/// ## 使用示例
///
/// ```rust
/// use aleph_ui_logic::connection::ReconnectStrategy;
///
/// let mut strategy = ReconnectStrategy::new(5, 1000);
///
/// // 第一次重连：1000ms
/// assert_eq!(strategy.next_delay(), Some(1000));
///
/// // 第二次重连：2000ms
/// assert_eq!(strategy.next_delay(), Some(2000));
///
/// // 第三次重连：4000ms
/// assert_eq!(strategy.next_delay(), Some(4000));
/// ```
#[derive(Debug, Clone)]
pub struct ReconnectStrategy {
    max_attempts: u32,
    current_attempt: u32,
    base_delay_ms: u64,
}

impl ReconnectStrategy {
    /// 创建新的重连策略
    ///
    /// # 参数
    ///
    /// - `max_attempts`: 最大重连次数
    /// - `base_delay_ms`: 基础延迟（毫秒）
    ///
    /// # 示例
    ///
    /// ```rust
    /// use aleph_ui_logic::connection::ReconnectStrategy;
    ///
    /// // 最多重连 5 次，基础延迟 1 秒
    /// let strategy = ReconnectStrategy::new(5, 1000);
    /// ```
    pub fn new(max_attempts: u32, base_delay_ms: u64) -> Self {
        Self {
            max_attempts,
            current_attempt: 0,
            base_delay_ms,
        }
    }

    /// 计算下一次重连延迟（指数退避）
    ///
    /// # 返回
    ///
    /// - `Some(delay)`: 下一次重连的延迟（毫秒）
    /// - `None`: 已达到最大重连次数
    ///
    /// # 算法
    ///
    /// 延迟 = base_delay_ms * 2^current_attempt
    ///
    /// 例如，base_delay_ms = 1000:
    /// - 第 1 次：1000ms (1s)
    /// - 第 2 次：2000ms (2s)
    /// - 第 3 次：4000ms (4s)
    /// - 第 4 次：8000ms (8s)
    /// - 第 5 次：16000ms (16s)
    pub fn next_delay(&mut self) -> Option<u64> {
        if self.current_attempt >= self.max_attempts {
            return None;
        }

        let delay = self.base_delay_ms * 2u64.pow(self.current_attempt);
        self.current_attempt += 1;
        Some(delay)
    }

    /// 重置重连计数
    ///
    /// 在成功连接后调用，重置重连计数器。
    pub fn reset(&mut self) {
        self.current_attempt = 0;
    }

    /// 获取当前重连次数
    pub fn current_attempt(&self) -> u32 {
        self.current_attempt
    }

    /// 获取最大重连次数
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// 检查是否已达到最大重连次数
    pub fn is_exhausted(&self) -> bool {
        self.current_attempt >= self.max_attempts
    }
}

impl Default for ReconnectStrategy {
    /// 默认策略：最多重连 5 次，基础延迟 1 秒
    fn default() -> Self {
        Self::new(5, 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let mut strategy = ReconnectStrategy::new(5, 1000);

        assert_eq!(strategy.next_delay(), Some(1000));
        assert_eq!(strategy.next_delay(), Some(2000));
        assert_eq!(strategy.next_delay(), Some(4000));
        assert_eq!(strategy.next_delay(), Some(8000));
        assert_eq!(strategy.next_delay(), Some(16000));
        assert_eq!(strategy.next_delay(), None);
    }

    #[test]
    fn test_reset() {
        let mut strategy = ReconnectStrategy::new(3, 1000);

        assert_eq!(strategy.next_delay(), Some(1000));
        assert_eq!(strategy.next_delay(), Some(2000));

        strategy.reset();

        assert_eq!(strategy.next_delay(), Some(1000));
        assert_eq!(strategy.next_delay(), Some(2000));
    }

    #[test]
    fn test_is_exhausted() {
        let mut strategy = ReconnectStrategy::new(2, 1000);

        assert!(!strategy.is_exhausted());
        strategy.next_delay();
        assert!(!strategy.is_exhausted());
        strategy.next_delay();
        assert!(strategy.is_exhausted());
    }
}
