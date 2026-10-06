use crate::domain::email::Email;

/// App-level actions from the `letsencrypt` plugin (dokku-letsencrypt 0.20.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LetsencryptAction {
    /// Issue or renew a certificate.
    Enable,
    /// Stop managing TLS for the app (keeps the certificate).
    Disable,
    /// Revoke the certificate with Let's Encrypt.
    Revoke,
    /// Remove stale certificate directories.
    Cleanup,
}

impl LetsencryptAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Revoke => "revoke",
            Self::Cleanup => "cleanup",
        }
    }
}

/// The registration email travels as SSH argv, so it must satisfy the
/// `Email` rules *and* be free of single quotes: dokku's SSH wrapper re-splits
/// `$SSH_ORIGINAL_COMMAND` with `xargs`, which cannot survive `'\''` escapes.
/// RFC-legal quoted local parts like `o'brien@example.com` are therefore
/// rejected here (unlike the login-path `Email` newtype).
pub fn is_valid_letsencrypt_email(value: &str) -> bool {
    !value.contains('\'') && Email::try_from(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_emails() {
        for email in ["ops@example.com", "a.b+tag@sub.example.co.uk"] {
            assert!(is_valid_letsencrypt_email(email), "{email}");
        }
    }

    #[test]
    fn rejects_apostrophes_and_malformed_addresses() {
        for email in [
            "o'brien@example.com",
            "it's@example.com",
            "not-an-email",
            "",
            "user@example",
        ] {
            assert!(!is_valid_letsencrypt_email(email), "{email}");
        }
    }
}
