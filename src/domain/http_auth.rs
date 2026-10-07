/// Basic-auth usernames for the `http-auth` plugin: letters, digits, `.`, `_`,
/// `-` (no spaces or shell metacharacters, since they travel in argv).
pub fn is_valid_username(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// Addresses the http-auth plugin's `allowed-ips` list is expected to accept:
/// IPv4/IPv6 with an optional CIDR prefix, the literal `all`, or
/// `unix:<path>`. Validation is deliberately shape-based (slightly looser than
/// nginx's own parser for exotic IPv6 forms); the plugin re-validates at
/// command time, so nothing invalid reaches the nginx config. Values travel as
/// SSH argv, so quotes and whitespace are rejected.
pub fn is_valid_allowed_ip(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || value
            .chars()
            .any(|c| c == '\'' || c.is_whitespace() || c.is_control())
    {
        return false;
    }
    if value == "all" {
        return true;
    }
    if let Some(path) = value.strip_prefix("unix:") {
        return !path.is_empty();
    }
    let (host, prefix) = match value.split_once('/') {
        Some((host, prefix)) => (host, Some(prefix)),
        None => (value, None),
    };
    if host.is_empty() {
        return false;
    }
    if host.contains(':') {
        // IPv6 by shape: hex digits and colons, at most one `::` elision.
        if !host.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
            return false;
        }
        if host.matches("::").count() > 1 {
            return false;
        }
        match prefix {
            None => true,
            Some(prefix) => prefix.parse::<u8>().map(|p| p <= 128) == Ok(true),
        }
    } else {
        let octets: Vec<&str> = host.split('.').collect();
        if octets.len() != 4
            || !octets.iter().all(|octet| {
                !octet.is_empty()
                    && octet.len() <= 3
                    && octet.chars().all(|c| c.is_ascii_digit())
                    && octet.parse::<u8>().is_ok()
            })
        {
            return false;
        }
        match prefix {
            None => true,
            Some(prefix) => prefix.parse::<u8>().map(|p| p <= 32) == Ok(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_usernames() {
        for value in ["alice", "a.b_c-d", "user123", "A"] {
            assert!(is_valid_username(value), "{value}");
        }
    }

    #[test]
    fn rejects_unsafe_usernames() {
        for value in ["", "a b", "it's", "a;rm", "a/b", "a:b", &"a".repeat(65)] {
            assert!(!is_valid_username(value), "{value:?}");
        }
    }

    #[test]
    fn accepts_nginx_addresses() {
        for value in [
            "10.0.0.1",
            "10.0.0.0/8",
            "192.168.1.5/32",
            "all",
            "unix:/var/run/proxy.sock",
            "::1",
            "2001:db8::/32",
        ] {
            assert!(is_valid_allowed_ip(value), "{value}");
        }
    }

    #[test]
    fn rejects_malformed_addresses() {
        for value in [
            "",
            "10.0.0",
            "10.0.0.0/",
            "10.0.0.0/33",
            "10.0.0.999",
            "10.0.0.0 extra",
            "10.0.0.0'an",
            "2001:db8::1::2",
            "2001:db8::/129",
            "unix:",
        ] {
            assert!(!is_valid_allowed_ip(value), "{value}");
        }
    }
}
