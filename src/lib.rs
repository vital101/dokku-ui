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
    let snapshot = Arc::new(dokku::SnapshotStore::new(client.clone(), pool.clone()));
    let capabilities = Arc::new(dokku::CapabilitiesStore::new(client.clone(), pool.clone()));
    let _refresher = dokku::spawn_refresher(
        snapshot.clone(),
        std::time::Duration::from_secs(settings.snapshot_refresh_secs),
    );
    let _capabilities_refresher = dokku::spawn_capabilities_refresher(
        capabilities.clone(),
        std::time::Duration::from_secs(settings.snapshot_refresh_secs),
    );
    let state = web::AppState {
        action_runs: Arc::new(storage::runs::SqliteRunsRepo::with_policy(
            pool.clone(),
            storage::runs::RunPolicy {
                activity_ttl_secs: settings.activity_ttl_secs as i64,
                log_ttl_secs: settings.run_log_ttl_secs as i64,
                orphan_after_secs: storage::runs::ORPHAN_AFTER_SECS,
            },
        )),
        jobs: Arc::new(storage::jobs::SqliteJobsRepo::new(pool.clone())),
        capabilities,
        db: pool.clone(),
        settings: settings.clone(),
        dokku: client,
        snapshot,
        webhooks: Arc::new(storage::webhooks::SqliteWebhooksRepo::new(pool)),
    };
    // The durable job workers: they reclaim expired leases and run any queued
    // jobs whose enqueuing process died. Prompt execution still happens in the
    // handler via the immediate executor; this pool is the safety net.
    let _worker = dokku::spawn_worker(state.clone(), "worker-0");
    let _worker = dokku::spawn_worker(state.clone(), "worker-1");
    actix_web::HttpServer::new(move || web::build_app(state.clone()))
        .bind(("0.0.0.0", settings.port))?
        .run()
        .await
}
