use std::collections::HashSet;

/// Network filter rule for Windows sandbox.
///
/// Represents a single filter rule that can be applied
/// via WFP (Windows Filtering Platform).
#[derive(Debug, Clone)]
pub enum FilterRule {
    /// Allow connections to specific host (by IP or hostname).
    AllowHost { host: String },
    /// Allow connections to specific IP address.
    AllowIp { ip: String },
    /// Allow connections to specific port.
    AllowPort { port: u16 },
    /// Block all connections (catch-all).
    BlockAll,
    /// Block specific host.
    BlockHost { host: String },
}

/// Collection of filter rules for a sandboxed process.
///
/// Manages the lifecycle of WFP filters:
/// 1. Add rules (AllowHost, BlockAll, etc.)
/// 2. Compile into WFP filters
/// 3. Apply to process
/// 4. Cleanup on drop
#[derive(Debug, Default)]
pub struct FilterSet {
    rules: Vec<FilterRule>,
    allowed_hosts: HashSet<String>,
    blocked_hosts: HashSet<String>,
    allowed_ports: HashSet<u16>,
    block_all: bool,
}

impl FilterSet {
    /// Create an empty filter set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Allow connections to a specific host.
    pub fn allow_host(&mut self, host: impl Into<String>) {
        let host = host.into();
        self.allowed_hosts.insert(host.clone());
        self.rules.push(FilterRule::AllowHost { host });
    }

    /// Allow connections to a specific port.
    pub fn allow_port(&mut self, port: u16) {
        self.allowed_ports.insert(port);
        self.rules.push(FilterRule::AllowPort { port });
    }

    /// Block all connections (catch-all).
    ///
    /// This should be called after all allow rules.
    pub fn block_all(&mut self) {
        self.block_all = true;
        self.rules.push(FilterRule::BlockAll);
    }

    /// Block connections to a specific host.
    pub fn block_host(&mut self, host: impl Into<String>) {
        let host = host.into();
        self.blocked_hosts.insert(host.clone());
        self.rules.push(FilterRule::BlockHost { host });
    }

    /// Get all rules.
    pub fn rules(&self) -> &[FilterRule] {
        &self.rules
    }

    /// Check if a host is explicitly allowed.
    pub fn is_host_allowed(&self, host: &str) -> bool {
        self.allowed_hosts.contains(host)
    }

    /// Check if all connections are blocked.
    pub fn is_block_all(&self) -> bool {
        self.block_all
    }

    /// Get the number of rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Check if no rules are set.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Convert from AllowHosts list.
    ///
    /// Creates a filter set that:
    /// 1. Allows the specified hosts
    /// 2. Blocks everything else
    pub fn from_allow_hosts(hosts: Vec<String>) -> Self {
        let mut set = Self::new();
        for host in hosts {
            set.allow_host(host);
        }
        set.block_all();
        set
    }

    /// Convert from NetworkPolicy.
    ///
    /// Maps Aleph's NetworkPolicy to Windows filter rules.
    pub fn from_network_policy(policy: &crate::sandbox::capabilities::NetworkPolicy) -> Self {
        use crate::sandbox::capabilities::NetworkPolicy;

        match policy {
            NetworkPolicy::None => {
                // Block all network access
                let mut set = Self::new();
                set.block_all();
                set
            }
            NetworkPolicy::AllowAll => {
                // No filters needed - allow all
                Self::new()
            }
            NetworkPolicy::AllowHosts { hosts } => Self::from_allow_hosts(hosts.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::capabilities::NetworkPolicy;

    #[test]
    fn filter_set_basic_operations() {
        let mut set = FilterSet::new();
        assert!(set.is_empty());

        set.allow_host("example.com");
        assert_eq!(set.len(), 1);
        assert!(set.is_host_allowed("example.com"));
        assert!(!set.is_host_allowed("other.com"));

        set.block_all();
        assert!(set.is_block_all());
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn filter_set_from_allow_hosts() {
        let hosts = vec!["example.com".to_string(), "api.example.com".to_string()];
        let set = FilterSet::from_allow_hosts(hosts);

        assert_eq!(set.len(), 3); // 2 allow + 1 block_all
        assert!(set.is_host_allowed("example.com"));
        assert!(set.is_host_allowed("api.example.com"));
        assert!(set.is_block_all());
    }

    #[test]
    fn filter_set_from_network_policy_none() {
        let policy = NetworkPolicy::None;
        let set = FilterSet::from_network_policy(&policy);

        assert_eq!(set.len(), 1); // Just block_all
        assert!(set.is_block_all());
    }

    #[test]
    fn filter_set_from_network_policy_allowall() {
        let policy = NetworkPolicy::AllowAll;
        let set = FilterSet::from_network_policy(&policy);

        assert!(set.is_empty()); // No filters needed
        assert!(!set.is_block_all());
    }

    #[test]
    fn filter_set_from_network_policy_allowhosts() {
        let policy = NetworkPolicy::AllowHosts {
            hosts: vec!["example.com".to_string()],
        };
        let set = FilterSet::from_network_policy(&policy);

        assert_eq!(set.len(), 2); // 1 allow + 1 block_all
        assert!(set.is_host_allowed("example.com"));
        assert!(set.is_block_all());
    }
}
