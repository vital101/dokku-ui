use actix_web::body::MessageBody;
use actix_web::dev::{ServiceFactory, ServiceRequest, ServiceResponse};
use actix_web::{App, Error, HttpResponse, web};

const HEALTHZ_HTML: &str = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Dokku UI</title>
  <link rel="stylesheet" href="/static/css/app.css">
</head>
<body class="flex min-h-screen items-center justify-center bg-slate-950">
  <div class="text-center">
    <h1 class="text-2xl font-semibold text-white">Dokku UI</h1>
    <p class="mt-4 inline-flex items-center gap-2 rounded-full bg-emerald-500/10 px-4 py-1.5 text-sm font-medium text-emerald-400">
      <span class="size-2 rounded-full bg-emerald-400"></span>
      healthy
    </p>
  </div>
</body>
</html>"#;

pub fn build_app() -> App<
    impl ServiceFactory<
        ServiceRequest,
        Config = (),
        Response = ServiceResponse<impl MessageBody>,
        Error = Error,
        InitError = (),
    >,
> {
    App::new()
        .service(actix_files::Files::new("/static", "./static"))
        .route("/healthz", web::get().to(healthz))
}

async fn healthz() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(HEALTHZ_HTML)
}
