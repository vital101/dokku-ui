//! Trusted reverse-proxy authentication primitives.
//!
//! The proxy header is only honored when the direct peer address falls inside
//! one of the configured CIDRs — a spoofed header from an untrusted peer is
//! ignored. Parsing and matching are pure so the policy is table-testable.

use std::net::IpAddr;

/// One `address/prefix` (or bare address) entry from `TRUSTED_PROXY_CIDRS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Parses `10.0.0.0/8`, `192.168.1.5` (a /32), or IPv6 equivalents.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let (addr_raw, prefix_raw) = match raw.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (raw, None),
        };
        let addr: IpAddr = addr_raw.parse().ok()?;
        let max_prefix = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix_raw {
            None => max_prefix,
            Some(prefix) => {
                let prefix = prefix.parse::<u8>().ok()?;
                if prefix > max_prefix {
                    return None;
                }
                prefix
            }
        };
        Some(Self { addr, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = v4_mask(self.prefix);
                (u32::from(net) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = v6_mask(self.prefix);
                (u128::from(net) & mask) == (u128::from(ip) & mask)
            }
            // Never match across families (an IPv4-mapped IPv6 peer is the
            // proxy's job to normalize, not ours to guess).
            _ => false,
        }
    }
}

fn v4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn v6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

/// Parses the full `TRUSTED_PROXY_CIDRS` value; an empty value disables
/// proxy auth (returns an empty list). Malformed entries are rejected so a
/// typo can never silently widen trust.
pub fn parse_trusted_cidrs(raw: &str) -> Result<Vec<Cidr>, String> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            Cidr::parse(entry).ok_or_else(|| format!("invalid trusted proxy CIDR `{entry}`"))
        })
        .collect()
}

/// Whether `peer` is one of the trusted proxies. A missing peer address is
/// never trusted.
pub fn peer_is_trusted(peer: Option<IpAddr>, cidrs: &[Cidr]) -> bool {
    match peer {
        Some(ip) => cidrs.iter().any(|cidr| cidr.contains(ip)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().expect("ip")
    }

    #[test]
    fn parses_cidrs_and_bare_addresses() {
        assert_eq!(Cidr::parse("10.0.0.0/8"), Cidr::parse("10.0.0.0/8"));
        assert!(Cidr::parse("192.168.1.5").is_some());
        assert!(Cidr::parse("2001:db8::/32").is_some());
        assert!(Cidr::parse("::1").is_some());
        for raw in ["", "10.0.0.0/33", "300.1.1.1", "10.0.0.0/x"] {
            assert!(Cidr::parse(raw).is_none(), "{raw}");
        }
    }

    #[test]
    fn matches_only_inside_the_range() {
        let cidr = Cidr::parse("10.0.0.0/8").expect("cidr");
        assert!(cidr.contains(ip("10.1.2.3")));
        assert!(!cidr.contains(ip("11.1.2.3")));
        assert!(!cidr.contains(ip("::1")));

        let host = Cidr::parse("192.168.1.5").expect("host");
        assert!(host.contains(ip("192.168.1.5")));
        assert!(!host.contains(ip("192.168.1.6")));

        let v6 = Cidr::parse("2001:db8::/32").expect("v6");
        assert!(v6.contains(ip("2001:db8::1")));
        assert!(!v6.contains(ip("2001:db9::1")));

        let all = Cidr::parse("0.0.0.0/0").expect("default");
        assert!(all.contains(ip("8.8.8.8")));
    }

    #[test]
    fn parses_lists_and_rejects_typos() {
        let cidrs = parse_trusted_cidrs("10.0.0.0/8, 192.168.0.0/16").expect("list");
        assert_eq!(cidrs.len(), 2);
        assert!(parse_trusted_cidrs("").expect("empty").is_empty());
        assert!(parse_trusted_cidrs("10.0.0.0/8,bogus").is_err());
    }

    #[test]
    fn peers_are_trusted_only_when_the_list_matches() {
        let cidrs = parse_trusted_cidrs("10.0.0.0/8").expect("list");
        assert!(peer_is_trusted(Some(ip("10.0.0.1")), &cidrs));
        assert!(!peer_is_trusted(Some(ip("1.2.3.4")), &cidrs));
        assert!(!peer_is_trusted(None, &cidrs));
        assert!(!peer_is_trusted(Some(ip("10.0.0.1")), &[]));
    }
}
