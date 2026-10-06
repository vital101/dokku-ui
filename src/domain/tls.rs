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
