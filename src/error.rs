use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use askama::Template;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("template render error: {0}")]
    Render(#[from] askama::Error),
    #[error("password error: {0}")]
    Auth(#[from] crate::auth::password::PasswordError),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("the requested resource was not found")]
    NotFound,
    #[error("bad request: {0}")]
    BadRequest(String),
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage<'a> {
    status: u16,
    message: &'a str,
}

impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            AppError::Database(_)
            | AppError::Auth(_)
            | AppError::Render(_)
            | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
        }
    }

    fn error_response(&self) -> HttpResponse {
        let status = self.status_code();
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        }
        let page = ErrorPage {
            status: status.as_u16(),
            message: &self.to_string(),
        };
        match page.render() {
            Ok(html) => HttpResponse::build(status)
                .content_type("text/html; charset=utf-8")
                .body(html),
            Err(err) => {
                tracing::error!(error = %err, "failed to render error page");
                HttpResponse::build(status)
                    .content_type("text/plain; charset=utf-8")
                    .body(self.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use actix_web::body::to_bytes;
    use actix_web::http::StatusCode;

    use super::*;

    #[test]
    fn maps_variants_to_status_codes() {
        assert_eq!(AppError::NotFound.status_code(), StatusCode::NOT_FOUND);
        assert_eq!(
            AppError::BadRequest("x".into()).status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            AppError::Internal("x".into()).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            AppError::Database(sqlx::Error::PoolClosed).status_code(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn renders_html_error_page() {
        let resp = AppError::NotFound.error_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = to_bytes(resp.into_body()).await.expect("body");
        let html = String::from_utf8(bytes.to_vec()).expect("utf-8");
        assert!(html.contains("404"));
        assert!(html.contains("not found"));
    }
}
