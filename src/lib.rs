use std::io;

pub mod web;

pub async fn run(port: u16) -> io::Result<()> {
    actix_web::HttpServer::new(web::build_app)
        .bind(("0.0.0.0", port))?
        .run()
        .await
}
