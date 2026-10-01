use actix_web::http::header::LOCATION;
use askama::Template;

use crate::error::AppError;

pub fn render<T: Template>(t: &T) -> Result<actix_web::HttpResponse, AppError> {
    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(t.render()?))
}

/// 307 Temporary Redirect — preserves method (for GET flows).
pub fn redirect(to: &str) -> actix_web::HttpResponse {
    actix_web::HttpResponse::TemporaryRedirect()
        .insert_header((LOCATION, to))
        .finish()
}

/// 303 See Other — PRG pattern for POST redirects.
pub fn see_other(to: &str) -> actix_web::HttpResponse {
    actix_web::HttpResponse::SeeOther()
        .insert_header((LOCATION, to))
        .finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::body::to_bytes;
    use askama::Template;

    #[derive(Template)]
    #[template(path = "dashboard.html")]
    struct DummyPage {
        email: String,
        csrf_token: String,
        flash: Option<crate::web::flash::FlashMessage>,
        stats: crate::domain::types::AppStats,
        rows: Vec<crate::dokku::AppRow>,
    }

    #[tokio::test]
    async fn render_returns_200_with_html_content_type() {
        let page = DummyPage {
            email: "a@b.com".into(),
            csrf_token: String::new(),
            flash: None,
            stats: Default::default(),
            rows: Vec::new(),
        };
        let resp = render(&page).expect("render");
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        let bytes = to_bytes(resp.into_body()).await.expect("body");
        let html = String::from_utf8(bytes.to_vec()).expect("utf-8");
        assert!(html.contains("<!doctype html>"));
    }

    #[test]
    fn redirect_is_307() {
        let resp = redirect("/foo");
        assert_eq!(resp.status(), actix_web::http::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            resp.headers().get(LOCATION).unwrap(),
            "/foo"
        );
    }

    #[test]
    fn see_other_is_303() {
        let resp = see_other("/bar");
        assert_eq!(resp.status(), actix_web::http::StatusCode::SEE_OTHER);
        assert_eq!(
            resp.headers().get(LOCATION).unwrap(),
            "/bar"
        );
    }
}
