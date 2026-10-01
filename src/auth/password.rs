use argon2::password_hash;
use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

use crate::domain::Password;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    #[error("failed to hash password: {0}")]
    Hash(String),
    #[error("failed to parse stored hash: {0}")]
    Parse(String),
}

pub fn hash_password(password: &Password) -> Result<String, PasswordError> {
    let salt = password_hash::generate_salt();
    Argon2::default()
        .hash_password_with_salt(password.as_str().as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|err| PasswordError::Hash(err.to_string()))
}

pub fn verify_password(password: &Password, stored_hash: &str) -> Result<bool, PasswordError> {
    let hash =
        PasswordHash::new(stored_hash).map_err(|err| PasswordError::Parse(err.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_str().as_bytes(), &hash)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password(raw: &str) -> Password {
        Password::new(raw).expect("valid password")
    }

    #[test]
    fn hash_then_verify_roundtrips() {
        let password = password("correct horse battery staple");
        let hash = hash_password(&password).expect("hash");
        assert!(verify_password(&password, &hash).expect("verify"));
    }

    #[test]
    fn wrong_password_fails_verification() {
        let hash = hash_password(&password("correct horse battery staple")).expect("hash");
        assert!(!verify_password(&password("wrong password here"), &hash).expect("verify"));
    }

    #[test]
    fn hashes_are_unique_per_salt() {
        let password = password("correct horse battery staple");
        let first = hash_password(&password).expect("first hash");
        let second = hash_password(&password).expect("second hash");
        assert_ne!(first, second);
    }

    #[test]
    fn malformed_stored_hash_is_an_error() {
        assert!(matches!(
            verify_password(&password("whatever password"), "not-a-phc-hash"),
            Err(PasswordError::Parse(_))
        ));
    }
}
