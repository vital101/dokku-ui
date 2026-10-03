use actix_web::HttpResponse;

const FAVICON_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <rect width="32" height="32" rx="7" fill="#0f172a"/>
  <rect x="7" y="7" width="18" height="18" rx="4" fill="#2563eb"/>
  <rect x="12" y="12" width="8" height="8" rx="2" fill="#ffffff" opacity="0.9"/>
</svg>"##;

pub async fn favicon() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("image/svg+xml")
        .body(FAVICON_SVG)
}

#[cfg(test)]
mod tests {
    use actix_web::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn serves_svg_with_icon_content_type() {
        let resp = favicon().await;
        assert_eq!(resp.status(), actix_web::http::StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(actix_web::http::header::CONTENT_TYPE)
                .expect("content type")
                .to_str()
                .expect("utf-8"),
            "image/svg+xml"
        );
        let bytes = to_bytes(resp.into_body()).await.expect("body");
        let svg = String::from_utf8(bytes.to_vec()).expect("utf-8");
        assert!(svg.contains("<svg"));
        assert!(svg.ends_with("</svg>"));
    }
}
