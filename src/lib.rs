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
    let client: Arc<dyn dokku::DokkuClient> = Arc::new(dokku::RusshClient::new(&settings));
    let snapshot = Arc::new(dokku::SnapshotStore::new(client.clone()));
    let _refresher = dokku::spawn_refresher(
        snapshot.clone(),
        std::time::Duration::from_secs(settings.snapshot_refresh_secs),
    );
    let state = web::AppState {
        db: pool,
        settings: settings.clone(),
        dokku: client,
        snapshot,
    };
    actix_web::HttpServer::new(move || web::build_app(state.clone()))
        .bind(("0.0.0.0", settings.port))?
        .run()
        .await
}
