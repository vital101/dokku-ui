use std::io;
use std::sync::Arc;

pub mod auth;
pub mod dokku;
pub mod domain;
pub mod error;
pub mod settings;
pub mod storage;
pub mod web;

pub async fn run(settings: settings::Settings) -> io::Result<()> {
    storage::ensure_db_parent_dir(&settings.database_url)?;
    let pool = storage::connect(&settings.database_url)
        .await
        .map_err(io::Error::other)?;
    let dokku: Arc<dyn dokku::DokkuClient> = Arc::new(dokku::RusshClient::new(&settings));
    let state = web::AppState {
        db: pool,
        settings: settings.clone(),
        dokku,
    };
    actix_web::HttpServer::new(move || web::build_app(state.clone()))
        .bind(("0.0.0.0", settings.port))?
        .run()
        .await
}
