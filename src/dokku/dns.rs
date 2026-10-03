use std::collections::HashSet;
use std::time::Duration;

use async_trait::async_trait;

/// Resolves a hostname. Implementations answer `true` only when the host has at
/// least one address; errors (NXDOMAIN, timeout, resolver failure) answer `false`.
#[async_trait]
pub trait DnsResolver: Send + Sync {
    async fn resolves(&self, host: &str) -> bool;
}

/// Production resolver backed by the async std resolver with a per-host timeout.
pub struct TokioResolver {
    timeout: Duration,
}

impl TokioResolver {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Default for TokioResolver {
    fn default() -> Self {
        Self::new(Duration::from_secs(2))
    }
}

#[async_trait]
impl DnsResolver for TokioResolver {
    async fn resolves(&self, host: &str) -> bool {
        match tokio::time::timeout(self.timeout, tokio::net::lookup_host((host, 443))).await {
            Ok(Ok(mut addrs)) => addrs.next().is_some(),
            _ => false,
        }
    }
}

/// Deterministic resolver for tests: `all` resolves anything, `none` resolves
/// nothing, and `hosts` resolves exactly the listed hostnames.
#[derive(Debug, Clone, Default)]
pub struct FakeResolver {
    mode: FakeMode,
}

#[derive(Debug, Clone, Default)]
enum FakeMode {
    #[default]
    None,
    All,
    Hosts(HashSet<String>),
}

impl FakeResolver {
    pub fn all() -> Self {
        Self {
            mode: FakeMode::All,
        }
    }

    pub fn none() -> Self {
        Self {
            mode: FakeMode::None,
        }
    }

    pub fn hosts(hosts: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            mode: FakeMode::Hosts(hosts.into_iter().map(Into::into).collect()),
        }
    }
}

#[async_trait]
impl DnsResolver for FakeResolver {
    async fn resolves(&self, host: &str) -> bool {
        match &self.mode {
            FakeMode::All => true,
            FakeMode::None => false,
            FakeMode::Hosts(hosts) => hosts.contains(host),
        }
    }
}

/// `Some(true)` when every vhost resolves, `Some(false)` when any does not, and
/// `None` when the app has no vhosts to check.
pub fn dns_record_status(vhosts: &[String], resolved: &[bool]) -> Option<bool> {
    if vhosts.is_empty() {
        return None;
    }
    Some(resolved.len() == vhosts.len() && resolved.iter().all(|ok| *ok))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_resolver_modes() {
        assert!(FakeResolver::all().resolves("anything").await);
        assert!(!FakeResolver::none().resolves("anything").await);
        let hosts = FakeResolver::hosts(["a.example", "b.example"]);
        assert!(hosts.resolves("a.example").await);
        assert!(!hosts.resolves("c.example").await);
    }

    #[test]
    fn empty_vhost_list_is_unknown() {
        assert_eq!(dns_record_status(&[], &[]), None);
    }

    #[test]
    fn all_resolved_is_yes() {
        let vhosts = vec!["a.example".to_owned(), "b.example".to_owned()];
        assert_eq!(dns_record_status(&vhosts, &[true, true]), Some(true));
    }

    #[test]
    fn any_unresolved_is_no() {
        let vhosts = vec!["a.example".to_owned(), "b.example".to_owned()];
        assert_eq!(dns_record_status(&vhosts, &[true, false]), Some(false));
        assert_eq!(dns_record_status(&vhosts, &[true]), Some(false));
    }

    #[tokio::test]
    async fn tokio_resolver_resolves_localhost_and_rejects_invalid_tld() {
        let resolver = TokioResolver::new(Duration::from_secs(5));
        assert!(resolver.resolves("localhost").await);
        assert!(!resolver.resolves("does-not-exist.invalid").await);
    }

    #[test]
    fn tokio_resolver_default_timeout_is_set() {
        assert_eq!(TokioResolver::default().timeout, Duration::from_secs(2));
    }
}
