use std::str::FromStr;

use actix_web::Error;
use actix_web::body::BoxBody;
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::header::{HeaderName, HeaderValue};
use actix_web::middleware::Next;

const CONTENT_SECURITY_POLICY: &str =
    "default-src 'self'; style-src 'self'; img-src 'self' data:; frame-ancestors 'none'";

const CACHE_CONTROL_STATIC: &str = "public, max-age=86400";

pub fn headers_for(path: &str) -> Vec<(&'static str, &'static str)> {
    let mut headers = vec![
        ("X-Content-Type-Options", "nosniff"),
        ("X-Frame-Options", "DENY"),
        ("Referrer-Policy", "no-referrer"),
        ("Content-Security-Policy", CONTENT_SECURITY_POLICY),
    ];
    if path.starts_with("/static/") {
        headers.push(("Cache-Control", CACHE_CONTROL_STATIC));
    }
    headers
}

pub async fn security_headers_middleware(
    req: ServiceRequest,
    next: Next<BoxBody>,
) -> Result<ServiceResponse<BoxBody>, Error> {
    let mut res = next.call(req).await?;
    for (name, value) in headers_for(res.request().path()) {
        res.headers_mut().insert(
            HeaderName::from_str(name).expect("valid header name constant"),
            HeaderValue::from_str(value).expect("valid header value constant"),
        );
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_response_gets_the_global_security_headers() {
        for path in ["/", "/login", "/favicon.ico", "/apps/myapp", "/nope"] {
            let headers = headers_for(path);
            assert!(
                headers.contains(&("X-Content-Type-Options", "nosniff")),
                "{path}"
            );
            assert!(headers.contains(&("X-Frame-Options", "DENY")), "{path}");
            assert!(
                headers.contains(&("Referrer-Policy", "no-referrer")),
                "{path}"
            );
            assert!(
                headers.contains(&(
                    "Content-Security-Policy",
                    "default-src 'self'; style-src 'self'; img-src 'self' data:; frame-ancestors 'none'",
                )),
                "{path}"
            );
        }
    }

    #[test]
    fn static_paths_get_cache_control() {
        let headers = headers_for("/static/css/app.css");
        assert!(headers.contains(&("Cache-Control", "public, max-age=86400")));
    }

    #[test]
    fn static_adjacent_paths_do_not_get_cache_control() {
        for path in ["/staticx", "/static.txt", "/static"] {
            let headers = headers_for(path);
            assert!(
                !headers.iter().any(|(name, _)| *name == "Cache-Control"),
                "{path} should not be cached"
            );
        }
    }
}
