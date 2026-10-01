use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use actix_session::Session;
use actix_web::dev::Payload;
use actix_web::{FromRequest, HttpRequest, web};
use serde::de::DeserializeOwned;

use crate::auth::csrf::tokens_match;
use crate::error::AppError;

pub const CSRF_FIELD: &str = "csrf_token";

/// Ensure the session has a CSRF token, generating one if missing.
pub async fn ensure_csrf(session: &Session) -> Result<String, AppError> {
    if let Ok(Some(token)) = session.get::<String>(CSRF_FIELD) {
        return Ok(token);
    }
    let token = generate_token();
    session
        .insert(CSRF_FIELD, &token)
        .map_err(|err| AppError::Internal(err.to_string()))?;
    Ok(token)
}

pub struct CsrfForm<T>(pub T);

impl<T: DeserializeOwned> FromRequest for CsrfForm<T> {
    type Error = actix_web::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self, Self::Error>>>>;

    fn from_request(req: &HttpRequest, payload: &mut Payload) -> Self::Future {
        let req = req.clone();
        let mut payload = payload.take();
        Box::pin(async move {
            let session = Session::from_request(&req, &mut Payload::None).await?;
            let bytes = web::Bytes::from_request(&req, &mut payload).await?;
            let fields: HashMap<String, String> =
                serde_urlencoded::from_bytes(&bytes).map_err(actix_web::error::Error::from)?;
            let expected = session.get::<String>(CSRF_FIELD).ok().flatten();
            let provided = fields.get(CSRF_FIELD).map(String::as_str);
            if !csrf_valid(expected.as_deref(), provided) {
                return Err(AppError::BadRequest("invalid or missing csrf token".into()).into());
            }
            let inner: T =
                serde_urlencoded::from_bytes(&bytes).map_err(actix_web::error::Error::from)?;
            Ok(CsrfForm(inner))
        })
    }
}

fn csrf_valid(expected: Option<&str>, provided: Option<&str>) -> bool {
    match (expected, provided) {
        (Some(expected), Some(provided)) => tokens_match(expected, provided),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_when_both_tokens_present_and_equal() {
        let token = "a".repeat(64);
        assert!(csrf_valid(Some(&token), Some(&token)));
    }

    #[test]
    fn invalid_when_missing_or_mismatched() {
        assert!(!csrf_valid(None, Some("x")));
        assert!(!csrf_valid(Some("x"), None));
        assert!(!csrf_valid(None, None));
        assert!(!csrf_valid(Some("expected"), Some("provided")));
    }
}
