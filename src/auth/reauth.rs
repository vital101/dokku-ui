/// Session key under which the re-auth expiry lives.
pub const REAUTH_UNTIL: &str = "reauth_until";

/// Whether a re-auth granted at `until` is still valid at `now` (epoch
/// seconds). Pure, so the expiry policy is trivially testable.
pub fn reauth_valid(now: i64, until: i64) -> bool {
    until > now
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validity_requires_the_expiry_in_the_future() {
        assert!(reauth_valid(100, 101));
        assert!(reauth_valid(100, 400));
        assert!(!reauth_valid(101, 100));
        assert!(!reauth_valid(100, 100), "expiring exactly now is not valid");
        assert!(!reauth_valid(200, 100));
    }
}
